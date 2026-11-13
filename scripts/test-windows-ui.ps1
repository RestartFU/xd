#!/usr/bin/env pwsh
# Drive the real xd window on a Windows desktop the way a person does: with
# the pointer, on the title bar, the window edges and the caption buttons.
#
# Nothing else exercises what only exists once Windows is drawing the window:
# which part of the frame it takes each pixel for, whether dragging moves it,
# whether the browser pane survives being created. Those broke unnoticed, so
# they are checked here against the installed app, with real input.
#
# The app runs on throwaway settings and data, so this never touches the
# user's own xd. Screenshots and results.json are written to -Output.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Executable,
    [string] $Output = 'dist/windows-ui',
    # Record what happens without failing, to capture a build known to be broken.
    [switch] $Baseline
)

$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'test-windows-ui.ps1 requires Windows.' }
$executablePath = (Resolve-Path -LiteralPath $Executable).Path
$outputPath = [IO.Path]::GetFullPath($Output)
New-Item -ItemType Directory -Force -Path $outputPath | Out-Null

Add-Type -AssemblyName System.Drawing
Add-Type -Namespace Xd -Name Desktop -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)]
public struct RECT { public int Left, Top, Right, Bottom; }

[StructLayout(LayoutKind.Sequential)]
struct MOUSEINPUT {
    public int dx, dy;
    public uint mouseData, dwFlags, time;
    public IntPtr dwExtraInfo;
}

[StructLayout(LayoutKind.Sequential)]
struct INPUT { public uint type; public MOUSEINPUT mi; }

delegate bool EnumWindowsProc(IntPtr window, IntPtr parameter);

[DllImport("user32.dll")] static extern bool SetProcessDPIAware();
[DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint count, INPUT[] inputs, int size);
[DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
[DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);
[DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
[DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr window, System.Text.StringBuilder name, int capacity);
[DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr window);
[DllImport("user32.dll")] public static extern bool IsIconic(IntPtr window);
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr window, int command);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr window, out RECT bounds);
[DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr window, uint message, IntPtr wparam, IntPtr lparam, uint flags, uint timeout, out IntPtr result);
[DllImport("user32.dll", EntryPoint = "GetClassLongPtrW")] static extern IntPtr GetClassLongPtr(IntPtr window, int index);
[DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr window, int attribute, out RECT value, int size);

public static void Initialize() { SetProcessDPIAware(); }

public static int ScreenWidth { get { return GetSystemMetrics(0); } }
public static int ScreenHeight { get { return GetSystemMetrics(1); } }

// The app's window: GPUI registers every window under this class.
public static IntPtr FindWindow(int process) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr window, IntPtr parameter) {
        uint owner;
        GetWindowThreadProcessId(window, out owner);
        if (owner != (uint)process || !IsWindowVisible(window)) return true;
        var name = new System.Text.StringBuilder(256);
        GetClassName(window, name, name.Capacity);
        if (name.ToString() != "Zed::Window") return true;
        found = window;
        return false;
    }, IntPtr.Zero);
    return found;
}

// What is drawn, without the invisible border Windows resizes a window by.
public static RECT Frame(IntPtr window) {
    RECT frame;
    if (DwmGetWindowAttribute(window, 9, out frame, Marshal.SizeOf(typeof(RECT))) != 0) {
        GetWindowRect(window, out frame);
    }
    return frame;
}

// Which part of the window Windows is told a screen point is. GPUI answers
// for where it last saw the pointer, so move there first.
public static int HitTest(IntPtr window, int x, int y) {
    IntPtr result;
    IntPtr point = (IntPtr)(((y & 0xffff) << 16) | (x & 0xffff));
    SendMessageTimeout(window, 0x0084, IntPtr.Zero, point, 2, 2000, out result);
    return (int)result.ToInt64();
}

public static bool HasIcon(IntPtr window) {
    IntPtr result;
    SendMessageTimeout(window, 0x007f, (IntPtr)1, IntPtr.Zero, 2, 2000, out result);
    return result != IntPtr.Zero || GetClassLongPtr(window, -14) != IntPtr.Zero;
}

static void Send(int x, int y, uint flags) {
    var input = new INPUT[1];
    input[0].type = 0;
    input[0].mi.dx = x;
    input[0].mi.dy = y;
    input[0].mi.dwFlags = flags;
    if (SendInput(1, input, Marshal.SizeOf(typeof(INPUT))) != 1) {
        throw new InvalidOperationException("Windows refused the input: this session has no interactive desktop.");
    }
}

public static void MoveTo(int x, int y) {
    int dx = (int)Math.Round(x * 65535.0 / (ScreenWidth - 1));
    int dy = (int)Math.Round(y * 65535.0 / (ScreenHeight - 1));
    Send(dx, dy, 0x8001);
}

public static void Press() { Send(0, 0, 0x0002); }
public static void Release() { Send(0, 0, 0x0004); }
'@
[Xd.Desktop]::Initialize()

# A hosted runner's screen is 1024x768, too small to move a window around on.
if ([Xd.Desktop]::ScreenWidth -lt 1280 -and (Get-Command Set-DisplayResolution -ErrorAction SilentlyContinue)) {
    try { Set-DisplayResolution -Width 1920 -Height 1080 -Force | Out-Null } catch { }
    Start-Sleep -Seconds 2
}
Write-Host "Screen: $([Xd.Desktop]::ScreenWidth)x$([Xd.Desktop]::ScreenHeight)"

$results = [Collections.Generic.List[object]]::new()
function Add-Result([string] $Name, [bool] $Passed, [string] $Detail) {
    $results.Add([pscustomobject]@{ name = $Name; passed = $Passed; detail = $Detail })
    Write-Host ("{0} {1}: {2}" -f $(if ($Passed) { 'PASS' } else { 'FAIL' }), $Name, $Detail)
}

function Save-Screenshot([string] $Name, $Frame) {
    $bounds = if ($null -ne $Frame) {
        [Drawing.Rectangle]::FromLTRB($Frame.Left, $Frame.Top, $Frame.Right, $Frame.Bottom)
    } else {
        [Drawing.Rectangle]::new(0, 0, [Xd.Desktop]::ScreenWidth, [Xd.Desktop]::ScreenHeight)
    }
    $screen = [Drawing.Rectangle]::new(0, 0, [Xd.Desktop]::ScreenWidth, [Xd.Desktop]::ScreenHeight)
    $bounds.Intersect($screen)
    if ($bounds.Width -le 0 -or $bounds.Height -le 0) { $bounds = $screen }
    $bitmap = [Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($bounds.Location, [Drawing.Point]::Empty, $bounds.Size)
        $bitmap.Save((Join-Path $outputPath "$Name.png"), [Drawing.Imaging.ImageFormat]::Png)
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

# The share of a screen rectangle that is one colour, give or take.
function Measure-Colour($Left, $Top, $Right, $Bottom, [Drawing.Color] $Colour) {
    $width = $Right - $Left
    $height = $Bottom - $Top
    if ($width -le 0 -or $height -le 0) { return 0.0 }
    $bitmap = [Drawing.Bitmap]::new($width, $height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($Left, $Top, 0, 0, $bitmap.Size)
        $matching = 0
        $sampled = 0
        for ($x = 0; $x -lt $width; $x += 8) {
            for ($y = 0; $y -lt $height; $y += 8) {
                $pixel = $bitmap.GetPixel($x, $y)
                $sampled++
                if ([Math]::Abs($pixel.R - $Colour.R) -le 12 -and
                    [Math]::Abs($pixel.G - $Colour.G) -le 12 -and
                    [Math]::Abs($pixel.B - $Colour.B) -le 12) { $matching++ }
            }
        }
        return $matching / [Math]::Max(1, $sampled)
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

function Move-Pointer([int] $X, [int] $Y) {
    # Two steps: Windows reports a pointer that arrives, not one that appears.
    [Xd.Desktop]::MoveTo($X - 3, $Y)
    Start-Sleep -Milliseconds 40
    [Xd.Desktop]::MoveTo($X, $Y)
    Start-Sleep -Milliseconds 160
}

function Invoke-Click([int] $X, [int] $Y) {
    Move-Pointer $X $Y
    [Xd.Desktop]::Press()
    Start-Sleep -Milliseconds 90
    [Xd.Desktop]::Release()
    Start-Sleep -Milliseconds 700
}

function Invoke-Drag([int] $FromX, [int] $FromY, [int] $ByX, [int] $ByY) {
    Move-Pointer $FromX $FromY
    [Xd.Desktop]::Press()
    Start-Sleep -Milliseconds 200
    $steps = 12
    for ($step = 1; $step -le $steps; $step++) {
        [Xd.Desktop]::MoveTo($FromX + [int]($ByX * $step / $steps), $FromY + [int]($ByY * $step / $steps))
        Start-Sleep -Milliseconds 35
    }
    Start-Sleep -Milliseconds 200
    [Xd.Desktop]::Release()
    Start-Sleep -Milliseconds 700
}

function Get-HitTest($Window, [int] $X, [int] $Y) {
    Move-Pointer $X $Y
    return [Xd.Desktop]::HitTest($Window, $X, $Y)
}

$hitNames = @{ 1 = 'client'; 2 = 'caption'; 8 = 'minimize'; 9 = 'maximize'; 10 = 'left edge'
    11 = 'right edge'; 12 = 'top edge'; 15 = 'bottom edge'; 20 = 'close' }
function Test-HitTest([string] $Name, $Window, [int] $X, [int] $Y, [int] $Expected) {
    $actual = Get-HitTest $Window $X $Y
    $shown = if ($hitNames.ContainsKey($actual)) { $hitNames[$actual] } else { "code $actual" }
    Add-Result $Name ($actual -eq $Expected) "Windows takes it for the $shown"
}

function Test-Near([int] $Actual, [int] $Expected) { [Math]::Abs($Actual - $Expected) -le 4 }

# Throwaway settings, data and WSL-side state, so each launch starts the same.
$work = Join-Path ([IO.Path]::GetTempPath()) ('xd-ui-test-' + [guid]::NewGuid().ToString('N'))
$dataName = 'xd-ui-test-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
$configHome = Join-Path $work 'config'
$settingsPath = Join-Path $configHome "$dataName\settings.json"
New-Item -ItemType Directory -Force -Path (Split-Path $settingsPath), (Join-Path $work 'data'),
    (Join-Path $work 'cache'), (Join-Path $work 'site') | Out-Null
$saved = @{}
foreach ($variable in 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XD_DATA_NAME') {
    $saved[$variable] = [Environment]::GetEnvironmentVariable($variable)
}
$env:XDG_CONFIG_HOME = $configHome
$env:XDG_DATA_HOME = Join-Path $work 'data'
$env:XDG_CACHE_HOME = Join-Path $work 'cache'
$env:XD_DATA_NAME = $dataName

function Write-Settings([hashtable] $Settings) {
    [IO.File]::WriteAllText($settingsPath, ($Settings | ConvertTo-Json -Depth 8),
        [Text.UTF8Encoding]::new($false))
}

function Start-App {
    $process = Start-Process -FilePath $executablePath -PassThru
    $window = [IntPtr]::Zero
    for ($attempt = 0; $attempt -lt 150 -and $window -eq [IntPtr]::Zero; $attempt++) {
        Start-Sleep -Milliseconds 200
        if ($process.HasExited) { break }
        $window = [Xd.Desktop]::FindWindow($process.Id)
    }
    if ($window -ne [IntPtr]::Zero) {
        [void][Xd.Desktop]::SetForegroundWindow($window)
        # Fonts, the first frame and the host connection settle.
        Start-Sleep -Seconds 4
    }
    return [pscustomobject]@{ Process = $process; Window = $window }
}

function Stop-App($App) {
    if ($null -eq $App -or $App.Process.HasExited) { return }
    [void]$App.Process.CloseMainWindow()
    if (-not $App.Process.WaitForExit(8000)) { $App.Process.Kill() }
}

$app = $null
$server = $null
try {
    # ---- The window frame ---------------------------------------------------
    Write-Settings @{ window_width = 900; window_height = 640 }
    $app = Start-App
    $opened = $app.Window -ne [IntPtr]::Zero
    Add-Result 'the window opens' $opened $(if ($opened) { 'found' } else { 'no window appeared' })
    if ($opened) {
        $window = $app.Window
        $frame = [Xd.Desktop]::Frame($window)
        Save-Screenshot '01-window' $frame
        $onScreen = $frame.Top -ge 0 -and $frame.Left -ge 0 -and
            $frame.Bottom -le [Xd.Desktop]::ScreenHeight -and $frame.Right -le [Xd.Desktop]::ScreenWidth
        Add-Result 'the window opens on the screen' $onScreen `
            "at $($frame.Left),$($frame.Top) to $($frame.Right),$($frame.Bottom)"
        Add-Result 'the window has an icon' ([Xd.Desktop]::HasIcon($window)) 'asked the window for its icon'

        $middle = [int](($frame.Left + $frame.Right) / 2)
        $titleY = $frame.Top + 19
        Test-HitTest 'the title bar is the caption' $window $middle $titleY 2
        Test-HitTest 'the gap beside the brand is the caption' $window ($frame.Left + 8) $titleY 2
        Test-HitTest 'the close button is close' $window ($frame.Right - 23) $titleY 20
        Test-HitTest 'the maximize button is maximize' $window ($frame.Right - 69) $titleY 9
        Test-HitTest 'the minimize button is minimize' $window ($frame.Right - 115) $titleY 8
        Save-Screenshot '02-hover-minimize' $frame
        Test-HitTest 'the top edge resizes' $window $middle ($frame.Top + 1) 12
        Test-HitTest 'the top edge over the buttons resizes' $window ($frame.Right - 69) ($frame.Top + 1) 12
        Test-HitTest 'the border right of the window resizes' $window ($frame.Right + 3) `
            ([int](($frame.Top + $frame.Bottom) / 2)) 11
        Test-HitTest 'the border under the window resizes' $window $middle ($frame.Bottom + 3) 15
        Test-HitTest 'inside the right edge is the window, not a border' $window ($frame.Right - 3) `
            ([int](($frame.Top + $frame.Bottom) / 2)) 1

        Invoke-Drag $middle $titleY 70 50
        $moved = [Xd.Desktop]::Frame($window)
        Add-Result 'dragging the title bar moves the window' `
            ((Test-Near ($moved.Left - $frame.Left) 70) -and (Test-Near ($moved.Top - $frame.Top) 50)) `
            "moved by $($moved.Left - $frame.Left),$($moved.Top - $frame.Top), asked for 70,50"

        $frame = [Xd.Desktop]::Frame($window)
        Invoke-Drag ($frame.Right + 3) ([int](($frame.Top + $frame.Bottom) / 2)) 60 0
        $resized = [Xd.Desktop]::Frame($window)
        Add-Result 'dragging the right border widens the window' `
            (Test-Near (($resized.Right - $resized.Left) - ($frame.Right - $frame.Left)) 60) `
            "width went from $($frame.Right - $frame.Left) to $($resized.Right - $resized.Left)"

        $frame = [Xd.Desktop]::Frame($window)
        Invoke-Drag ([int](($frame.Left + $frame.Right) / 2)) ($frame.Bottom + 3) 0 40
        $resized = [Xd.Desktop]::Frame($window)
        Add-Result 'dragging the bottom border heightens the window' `
            (Test-Near (($resized.Bottom - $resized.Top) - ($frame.Bottom - $frame.Top)) 40) `
            "height went from $($frame.Bottom - $frame.Top) to $($resized.Bottom - $resized.Top)"

        $frame = [Xd.Desktop]::Frame($window)
        Invoke-Drag ([int](($frame.Left + $frame.Right) / 2)) ($frame.Top + 1) 0 -30
        $resized = [Xd.Desktop]::Frame($window)
        Add-Result 'dragging the top edge heightens the window' `
            ((Test-Near ($frame.Top - $resized.Top) 30) -and (Test-Near ($resized.Bottom - $frame.Bottom) 0)) `
            "top went from $($frame.Top) to $($resized.Top)"
        Save-Screenshot '03-moved-and-resized' ([Xd.Desktop]::Frame($window))

        $frame = [Xd.Desktop]::Frame($window)
        Invoke-Click ($frame.Right - 69) ($frame.Top + 19)
        $maximized = [Xd.Desktop]::IsZoomed($window)
        Add-Result 'the maximize button maximizes' $maximized $(if ($maximized) { 'maximized' } else { 'unchanged' })
        Save-Screenshot '04-maximized' $null
        if ($maximized) {
            # The corner of the screen is the close button, so it cannot be overshot.
            Test-HitTest 'the corner of a maximized window is close' $window `
                ([Xd.Desktop]::ScreenWidth - 1) 0 20
            Invoke-Click ([Xd.Desktop]::ScreenWidth - 69) 19
            $restored = -not [Xd.Desktop]::IsZoomed($window)
            Add-Result 'the same button restores' $restored $(if ($restored) { 'restored' } else { 'still maximized' })
            if (-not $restored) { [void][Xd.Desktop]::ShowWindow($window, 9) }
        }

        $frame = [Xd.Desktop]::Frame($window)
        $middle = [int](($frame.Left + $frame.Right) / 2)
        Move-Pointer $middle ($frame.Top + 19)
        [Xd.Desktop]::Press(); [Xd.Desktop]::Release()
        Start-Sleep -Milliseconds 80
        [Xd.Desktop]::Press(); [Xd.Desktop]::Release()
        Start-Sleep -Milliseconds 900
        $maximized = [Xd.Desktop]::IsZoomed($window)
        Add-Result 'double-clicking the title bar maximizes' $maximized `
            $(if ($maximized) { 'maximized' } else { 'unchanged' })
        if ($maximized) { [void][Xd.Desktop]::ShowWindow($window, 9); Start-Sleep -Milliseconds 700 }

        $frame = [Xd.Desktop]::Frame($window)
        Invoke-Click ($frame.Right - 115) ($frame.Top + 19)
        $minimized = [Xd.Desktop]::IsIconic($window)
        Add-Result 'the minimize button minimizes' $minimized $(if ($minimized) { 'minimized' } else { 'unchanged' })
        [void][Xd.Desktop]::ShowWindow($window, 9)
        [void][Xd.Desktop]::SetForegroundWindow($window)
        Start-Sleep -Milliseconds 900

        $frame = [Xd.Desktop]::Frame($window)
        Invoke-Click ($frame.Right - 23) ($frame.Top + 19)
        $closed = $app.Process.WaitForExit(10000)
        Add-Result 'the close button closes xd' $closed $(if ($closed) { 'the process exited' } else { 'still running' })
    }
    Stop-App $app

    # ---- The browser pane, and starting again with it open ---------------------
    # A page that is one flat colour, so a screenshot can tell it is showing.
    $page = [Drawing.Color]::FromArgb(0, 166, 81)
    [IO.File]::WriteAllText((Join-Path $work 'site\index.html'),
        '<!doctype html><title>xd browser test</title><body style="margin:0;background:rgb(0,166,81)">',
        [Text.UTF8Encoding]::new($false))
    $port = 47000 + (Get-Random -Maximum 900)
    $server = Start-Process -FilePath python -PassThru -WindowStyle Hidden -ArgumentList @(
        '-m', 'http.server', "$port", '--bind', '127.0.0.1', '--directory', "`"$(Join-Path $work 'site')`"")
    Start-Sleep -Seconds 2
    Write-Settings @{
        window_width = 1000
        window_height = 640
        browser_width = 460
        browser_sessions = @{
            'local/browser' = @{
                open = $true
                session = @{ tabs = @("http://127.0.0.1:$port/"); active_tab = 0 }
            }
        }
    }

    # Twice: the pane is saved as open, so whatever happens on the first
    # launch happens again on every one after it.
    foreach ($launch in 'first', 'second') {
        $app = Start-App
        $shown = 0.0
        if ($app.Window -ne [IntPtr]::Zero) {
            for ($attempt = 0; $attempt -lt 40 -and -not $app.Process.HasExited; $attempt++) {
                $frame = [Xd.Desktop]::Frame($app.Window)
                $shown = Measure-Colour ($frame.Right - 440) ($frame.Top + 120) ($frame.Right - 20) `
                    ($frame.Bottom - 20) $page
                if ($shown -gt 0.9) { break }
                Start-Sleep -Milliseconds 500
            }
            # Timers and host traffic keep arriving while the page settles.
            Start-Sleep -Seconds 3
        }
        $alive = $app.Window -ne [IntPtr]::Zero -and -not $app.Process.HasExited
        Add-Result "xd stays open with the browser pane open ($launch launch)" $alive `
            $(if ($alive) { 'running' } else { "the process ended with code $($app.Process.ExitCode)" })
        if ($alive) {
            Save-Screenshot "05-browser-$launch" ([Xd.Desktop]::Frame($app.Window))
            Add-Result "the page shows in the browser pane ($launch launch)" ($shown -gt 0.9) `
                ("{0:P0} of the pane is the page" -f $shown)
        } else {
            Save-Screenshot "05-browser-$launch" $null
        }
        if ($launch -eq 'second' -and $alive) {
            # A second xd hands over to the first and leaves.
            $second = Start-Process -FilePath $executablePath -PassThru
            $left = $second.WaitForExit(15000)
            if (-not $left) { $second.Kill() }
            Add-Result 'a second xd leaves the first one running' ($left -and -not $app.Process.HasExited) `
                $(if ($left) { 'the second one exited' } else { 'a second window opened' })
        }
        Stop-App $app
    }
} finally {
    Stop-App $app
    if ($null -ne $server -and -not $server.HasExited) { $server.Kill() }
    foreach ($variable in $saved.Keys) {
        [Environment]::SetEnvironmentVariable($variable, $saved[$variable])
    }
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    [IO.File]::WriteAllText((Join-Path $outputPath 'results.json'), ($results | ConvertTo-Json -Depth 4),
        [Text.UTF8Encoding]::new($false))
}

$failed = @($results | Where-Object { -not $_.passed })
if ($env:GITHUB_STEP_SUMMARY) {
    $lines = @('| Check | Result | What happened |', '|---|---|---|')
    foreach ($result in $results) {
        $lines += "| $($result.name) | $(if ($result.passed) { 'pass' } else { 'FAIL' }) | $($result.detail) |"
    }
    Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value ($lines -join "`n")
}
Write-Host "$($results.Count - $failed.Count) of $($results.Count) window checks passed"
if ($failed.Count -ne 0 -and -not $Baseline) {
    throw "The Windows window failed: $(($failed | ForEach-Object name) -join '; ')"
}
