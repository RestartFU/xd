#!/usr/bin/env pwsh
# Install the Windows MSI for real, check what it put on this machine, and
# remove it again.
#
# This changes the signed-in user's own installation, so it refuses to run
# where that build of xd is already installed unless -Force is given. CI
# installs with -Keep, drives the installed app, then calls again with -Remove.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Installer,
    # An installer of another build of the same channel. It is installed
    # first, so that installing -Installer over it is a real upgrade.
    [string] $Previous,
    # Leave xd installed once the checks pass.
    [switch] $Keep,
    # Only remove an installation a previous -Keep left, and check it is gone.
    [switch] $Remove,
    # Replace an installation that is already there.
    [switch] $Force
)

$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'smoke-windows.ps1 requires Windows.' }
$installerPath = (Resolve-Path -LiteralPath $Installer).Path
$nightly = [IO.Path]::GetFileName($installerPath) -like 'xd-nightly-*'
$name = if ($nightly) { 'xd-nightly' } else { 'xd' }
$productName = if ($nightly) { 'xd Nightly' } else { 'xd' }
$installFolder = Join-Path $env:LOCALAPPDATA "Programs\$name"
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) "$productName.lnk"
$registryKey = "HKCU:\Software\RestartFU\$name"

function Invoke-Installer([string[]] $Arguments, [string] $What) {
    $log = Join-Path ([IO.Path]::GetTempPath()) ('xd-msi-' + [guid]::NewGuid().ToString('N') + '.log')
    $process = Start-Process -FilePath msiexec.exe -Wait -PassThru `
        -ArgumentList ($Arguments + @('/qn', '/norestart', '/l*v', "`"$log`""))
    if ($process.ExitCode -ne 0) {
        Get-Content -LiteralPath $log -Tail 60 -ErrorAction SilentlyContinue | Write-Host
        throw "$What failed with exit code $($process.ExitCode)."
    }
    Remove-Item -LiteralPath $log -Force -ErrorAction SilentlyContinue
}

# What Windows lists under installed apps, asked of Windows Installer itself:
# where it keeps that in the registry differs by who installed.
function Get-InstalledProduct {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    foreach ($product in $installer.ProductsEx('', '', 7)) {
        if ($product.InstallProperty('InstalledProductName') -eq $productName -and
            $product.InstallProperty('Publisher') -eq 'RestartFU') {
            [pscustomobject]@{
                Version = $product.InstallProperty('VersionString')
                # Unique to each installer file ever built.
                Package = $product.InstallProperty('PackageCode')
                # 0 is installed for one user, 1 for the whole machine.
                Assignment = $product.InstallProperty('AssignmentType')
            }
        }
    }
}

function Remove-Installation {
    Invoke-Installer @('/x', "`"$installerPath`"") 'Removing xd'
    if (Test-Path -LiteralPath $installFolder) { throw "Removing xd left $installFolder behind." }
    if (Test-Path -LiteralPath $shortcut) { throw 'Removing xd left its Start menu shortcut behind.' }
    if (Test-Path -LiteralPath $registryKey) { throw 'Removing xd left its registry key behind.' }
    if (@(Get-InstalledProduct).Count -ne 0) { throw 'Removing xd left it listed as installed.' }
    Write-Host 'Windows installer: removal left nothing behind'
}

if ($Remove) {
    Remove-Installation
    return
}

$expected = (Get-Content -LiteralPath "$installerPath.sha256" -Raw).Split(' ')[0].Trim()
$actual = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw 'Windows installer checksum mismatch.' }

if ((Test-Path -LiteralPath $installFolder) -and -not $Force) {
    throw "$productName is already installed in $installFolder. Pass -Force to replace it."
}

# An update is a different build installed over the one that is there. Two
# builds of one version, which every nightly is, must still replace each other
# rather than end up installed side by side.
$earlier = $null
if ($Previous) {
    $previousPath = (Resolve-Path -LiteralPath $Previous).Path
    Invoke-Installer @('/i', "`"$previousPath`"") 'Installing the earlier build'
    $earlier = @(Get-InstalledProduct)
    if ($earlier.Count -ne 1) { throw "The earlier build is listed as installed $($earlier.Count) times." }
}

Invoke-Installer @('/i', "`"$installerPath`"") 'Installing xd'

if ($null -ne $earlier) {
    $current = @(Get-InstalledProduct)
    if ($current.Count -ne 1) {
        throw "Installing over an earlier build left $($current.Count) copies of $productName installed."
    }
    if ($current[0].Package -eq $earlier[0].Package) {
        throw 'Installing over an earlier build left the earlier build in place.'
    }
    Write-Host 'Windows installer: a newer build replaced the one installed'
}

# Only what the app needs: no readme or helper scripts beside the executable.
$expectedFiles = @(
    'xd.exe', 'libexec\xd-host-linux', 'licenses\alacritty-terminal-LICENSE-APACHE',
    'licenses\JetBrainsMono-OFL.txt', 'licenses\t3code-LICENSE'
)
$installed = @(Get-ChildItem -LiteralPath $installFolder -Recurse -File |
    ForEach-Object { $_.FullName.Substring($installFolder.Length + 1) } | Sort-Object)
$difference = Compare-Object ($expectedFiles | Sort-Object) $installed
if ($null -ne $difference) {
    throw "Unexpected installed files: $($installed -join ', ')"
}

$executable = Join-Path $installFolder 'xd.exe'
$pe = [IO.File]::ReadAllBytes($executable)
$peOffset = [BitConverter]::ToInt32($pe, 0x3c)
if ($pe[0] -ne 0x4d -or $pe[1] -ne 0x5a -or
    [BitConverter]::ToUInt32($pe, $peOffset) -ne 0x00004550 -or
    [BitConverter]::ToUInt16($pe, $peOffset + 4) -ne 0x8664) {
    throw 'Windows frontend must be an x86_64 PE executable.'
}

# The icon GPUI gives the window, and Explorer the file, is icon resource 1.
Add-Type -Namespace Xd -Name Resources -MemberDefinition @'
[DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern IntPtr LoadLibraryEx(string path, IntPtr file, uint flags);
[DllImport("kernel32.dll", SetLastError = true)]
public static extern IntPtr FindResource(IntPtr module, IntPtr name, IntPtr type);
[DllImport("kernel32.dll")]
public static extern bool FreeLibrary(IntPtr module);
'@
$module = [Xd.Resources]::LoadLibraryEx($executable, [IntPtr]::Zero, 2)
if ($module -eq [IntPtr]::Zero) { throw 'Cannot read the resources of xd.exe.' }
try {
    $groupIcon = [IntPtr]14
    if ([Xd.Resources]::FindResource($module, [IntPtr]1, $groupIcon) -eq [IntPtr]::Zero) {
        throw 'xd.exe has no application icon.'
    }
} finally {
    [void][Xd.Resources]::FreeLibrary($module)
}
$description = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable).FileDescription
if ($description -ne $productName) {
    throw "xd.exe describes itself as '$description', not '$productName'."
}

$stdout = Join-Path ([IO.Path]::GetTempPath()) ('xd-version-' + [guid]::NewGuid().ToString('N') + '.out')
$stderr = "$stdout.err"
try {
    $process = Start-Process -FilePath $executable -ArgumentList '--version' `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
    if (-not $process.WaitForExit(30000)) {
        $process.Kill()
        throw 'Windows version smoke timed out.'
    }
    if ($process.ExitCode -ne 0) {
        throw "Windows version smoke failed: $(Get-Content -LiteralPath $stderr -Raw)"
    }
    $version = (Get-Content -LiteralPath $stdout -Raw).Trim()
} finally {
    Remove-Item -LiteralPath $stdout, $stderr -Force -ErrorAction SilentlyContinue
}
if ($version -notmatch '^xd (\d+\.\d+\.\d+)') { throw "Unexpected version: $version" }
$packageVersion = $Matches[1]
Write-Host $version

if (-not (Test-Path -LiteralPath $shortcut -PathType Leaf)) {
    throw "The Start menu has no $productName shortcut."
}
$target = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut).TargetPath
if ($target -ne $executable) { throw "The Start menu shortcut opens $target." }

$entry = @(Get-InstalledProduct)
if ($entry.Count -ne 1) { throw "$productName is listed as installed $($entry.Count) times." }
if ($entry[0].Version -ne $packageVersion) {
    throw "Windows lists version $($entry[0].Version), but xd.exe is $packageVersion."
}
if ($entry[0].Assignment -ne '0') {
    throw "$productName installed for the whole machine, which needs an administrator."
}
if ((Get-ItemProperty -LiteralPath $registryKey).InstallFolder.TrimEnd('\') -ne $installFolder) {
    throw 'The installer did not record where it installed xd.'
}

# Running the installer over an installed copy is how an update arrives.
Invoke-Installer @('/i', "`"$installerPath`"") 'Installing xd over itself'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw 'Installing xd over itself removed it.'
}
if (@(Get-InstalledProduct).Count -ne 1) { throw 'Installing xd over itself listed it twice.' }

# Someone who double-clicks the installer sees a progress bar and then xd
# itself. The installer opens it from a system service, so it must still come
# up as the signed-in user's xd, with its files in their profile, and close
# like one.
$running = Join-Path $env:LOCALAPPDATA "$name\running"
$passive = Start-Process -FilePath msiexec.exe -Wait -PassThru `
    -ArgumentList '/i', "`"$installerPath`"", '/passive', '/norestart'
if ($passive.ExitCode -ne 0) { throw "Installing xd with a progress bar failed with exit code $($passive.ExitCode)." }
$app = $null
for ($attempt = 0; $attempt -lt 150 -and $null -eq $app; $attempt++) {
    Start-Sleep -Milliseconds 200
    $app = Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $executable } | Select-Object -First 1
}
if ($null -eq $app) { throw 'The installer did not open xd when it finished.' }
try {
    for ($attempt = 0; $attempt -lt 150; $attempt++) {
        $app.Refresh()
        if ($app.HasExited -or ((Test-Path -LiteralPath $running) -and
                $app.MainWindowHandle -ne [IntPtr]::Zero)) { break }
        Start-Sleep -Milliseconds 200
    }
    if ($app.HasExited) { throw "xd opened by the installer ended with code $($app.ExitCode)." }
    if (-not (Test-Path -LiteralPath $running)) {
        throw "xd opened by the installer does not keep its files under $env:LOCALAPPDATA."
    }
    if ($app.MainWindowHandle -eq [IntPtr]::Zero) { throw 'xd opened by the installer showed no window.' }
} finally {
    if (-not $app.HasExited) {
        [void]$app.CloseMainWindow()
        if (-not $app.WaitForExit(15000)) { $app.Kill() }
    }
}
if (Test-Path -LiteralPath $running) { throw 'Closing xd left the mark of a run that never finished.' }

Write-Host 'Windows installer: checksum, files, icon, version, shortcut, reinstall, and first launch passed'
if (-not $Keep) { Remove-Installation }
