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

function Get-UninstallEntry {
    Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue |
        ForEach-Object { Get-ItemProperty -LiteralPath $_.PSPath } |
        Where-Object { $_.DisplayName -eq $productName -and $_.Publisher -eq 'RestartFU' }
}

function Remove-Installation {
    Invoke-Installer @('/x', "`"$installerPath`"") 'Removing xd'
    if (Test-Path -LiteralPath $installFolder) { throw "Removing xd left $installFolder behind." }
    if (Test-Path -LiteralPath $shortcut) { throw 'Removing xd left its Start menu shortcut behind.' }
    if (Test-Path -LiteralPath $registryKey) { throw 'Removing xd left its registry key behind.' }
    if (@(Get-UninstallEntry).Count -ne 0) { throw 'Removing xd left it listed as installed.' }
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

Invoke-Installer @('/i', "`"$installerPath`"") 'Installing xd'

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

$entry = @(Get-UninstallEntry)
if ($entry.Count -ne 1) { throw "$productName is listed as installed $($entry.Count) times." }
if ($entry[0].DisplayVersion -ne $packageVersion) {
    throw "Windows lists version $($entry[0].DisplayVersion), but xd.exe is $packageVersion."
}
if ((Get-ItemProperty -LiteralPath $registryKey).InstallFolder.TrimEnd('\') -ne $installFolder) {
    throw 'The installer did not record where it installed xd.'
}

# Running the installer over an installed copy is how an update arrives.
Invoke-Installer @('/i', "`"$installerPath`"") 'Installing xd over itself'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw 'Installing xd over itself removed it.'
}
if (@(Get-UninstallEntry).Count -ne 1) { throw 'Installing xd over itself listed it twice.' }

Write-Host 'Windows installer: checksum, files, icon, version, shortcut, and reinstall passed'
if (-not $Keep) { Remove-Installation }
