#!/usr/bin/env pwsh
# Build the native Windows frontend and package it, with its matching static
# Linux WSL host, as a per-user MSI installer.
[CmdletBinding()]
param(
    [ValidateSet('nightly', 'release')][string] $Profile = 'release',
    [Parameter(Mandatory = $true)][string] $HostPayload,
    [string] $OutputDirectory = 'dist/windows'
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
if ($env:OS -ne 'Windows_NT' -or
    [Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
    throw 'build-windows.ps1 requires Windows x86_64.'
}
$hostPath = (Resolve-Path -LiteralPath $HostPayload).Path
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)

# Stable and nightly are separate products: each has its own folder, Start
# menu entry and settings, and its own upgrade code so that installing one
# never replaces the other. The upgrade codes must never change, or an update
# would install beside the old version instead of over it.
$product = if ($Profile -eq 'nightly') {
    @{
        Name = 'xd-nightly'
        ProductName = 'xd Nightly'
        UpgradeCode = 'EEFC7CAA-3EAF-41D3-998B-083422A38BE3'
        AppUserModelId = 'com.restartfu.Xd.Nightly'
    }
} else {
    @{
        Name = 'xd'
        ProductName = 'xd'
        UpgradeCode = '0C3865A7-0B00-4FA9-B5A8-B11F0C62B9D6'
        AppUserModelId = 'com.restartfu.Xd'
    }
}
$name = $product.Name
$payload = Join-Path $outputPath $name

# Find WiX 3 before the long build, so a machine without it fails early.
# GitHub's Windows runners have it installed; anywhere else the pinned
# binaries are downloaded once into the build cache.
function Get-WixDirectory {
    $candidates = @()
    if ($env:WIX) { $candidates += (Join-Path $env:WIX 'bin') }
    $candidates += Get-ChildItem -Path "${env:ProgramFiles(x86)}/WiX Toolset v3*/bin" `
        -Directory -ErrorAction SilentlyContinue | Sort-Object FullName -Descending |
        ForEach-Object { $_.FullName }
    $cached = Join-Path $repositoryRoot '.build-cache/wix-3.14.1'
    $candidates += $cached
    foreach ($candidate in $candidates) {
        if ((Test-Path -LiteralPath (Join-Path $candidate 'candle.exe') -PathType Leaf) -and
            (Test-Path -LiteralPath (Join-Path $candidate 'light.exe') -PathType Leaf)) {
            return $candidate
        }
    }
    $archive = Join-Path ([IO.Path]::GetTempPath()) ('xd-wix-' + [guid]::NewGuid().ToString('N') + '.zip')
    try {
        Invoke-WebRequest -UseBasicParsing -OutFile $archive `
            -Uri 'https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip'
        $expected = '6ac824e1642d6f7277d0ed7ea09411a508f6116ba6fae0aa5f2c7daa2ff43d31'
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
            throw 'The WiX download does not match its pinned checksum.'
        }
        New-Item -ItemType Directory -Force -Path $cached | Out-Null
        Expand-Archive -LiteralPath $archive -DestinationPath $cached -Force
    } finally {
        Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
    }
    return $cached
}
$wix = Get-WixDirectory

if (-not $env:GPUI_FXC_PATH) {
    $sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
    $fxc = Get-ChildItem -Path "$sdkRoot/*/x64/fxc.exe" -File |
        Sort-Object FullName -Descending | Select-Object -First 1
    if ($null -eq $fxc) { throw 'The Windows SDK HLSL compiler (fxc.exe) is required.' }
    $env:GPUI_FXC_PATH = $fxc.FullName
}
if (-not $env:CARGO_BUILD_JOBS) {
    $env:CARGO_BUILD_JOBS = [string][Math]::Max(1, [Math]::Min(4,
        [Math]::Floor([Environment]::ProcessorCount * 0.75)))
}
$env:XD_BUILD_PROFILE = $Profile
$env:XD_COMMIT = (& git -C $repositoryRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot determine the build commit.' }

# MSVC's static CRT keeps the installed app independent of redistributable DLLs.
$savedRustFlags = $env:RUSTFLAGS
try {
    if ($savedRustFlags -notmatch 'target-feature=\+crt-static') {
        $env:RUSTFLAGS = "$savedRustFlags -C target-feature=+crt-static".Trim()
    }
    & cargo build --locked --release --target x86_64-pc-windows-msvc `
        --manifest-path (Join-Path $repositoryRoot 'desktop/Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'Native Windows desktop build failed.' }
} finally {
    $env:RUSTFLAGS = $savedRustFlags
}

# Exactly what the installer puts on disk.
New-Item -ItemType Directory -Force -Path $outputPath | Out-Null
if (Test-Path -LiteralPath $payload) { Remove-Item -LiteralPath $payload -Recurse -Force }
New-Item -ItemType Directory -Path $payload, (Join-Path $payload 'libexec'), `
    (Join-Path $payload 'licenses') | Out-Null
Copy-Item -LiteralPath (Join-Path $repositoryRoot `
    'desktop/target/x86_64-pc-windows-msvc/release/xd-desktop.exe') `
    -Destination (Join-Path $payload 'xd.exe')
Copy-Item -LiteralPath $hostPath -Destination (Join-Path $payload 'libexec/xd-host-linux')
Copy-Item -LiteralPath (Join-Path $repositoryRoot `
    'data/licenses/alacritty-terminal-LICENSE-APACHE') `
    -Destination (Join-Path $payload 'licenses/alacritty-terminal-LICENSE-APACHE')
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'data/fonts/jetbrains-mono/OFL.txt') `
    -Destination (Join-Path $payload 'licenses/JetBrainsMono-OFL.txt')
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'data/licenses/t3code-LICENSE') `
    -Destination (Join-Path $payload 'licenses/t3code-LICENSE')

# Windows Installer compares three numeric fields, which is all a version has.
$manifest = Get-Content -LiteralPath (Join-Path $repositoryRoot 'desktop/Cargo.toml') -Raw
if ($manifest -notmatch '(?m)^version\s*=\s*"(\d+\.\d+\.\d+)"') {
    throw 'Cannot read the version from desktop/Cargo.toml.'
}
$version = $Matches[1]

$asset = "$name-windows-x86_64.msi"
$installer = Join-Path $outputPath $asset
$work = Join-Path $outputPath 'wix'
if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
New-Item -ItemType Directory -Path $work | Out-Null
Remove-Item -LiteralPath $installer -Force -ErrorAction SilentlyContinue

$object = Join-Path $work 'xd.wixobj'
& (Join-Path $wix 'candle.exe') -nologo -arch x64 `
    "-dProductName=$($product.ProductName)" `
    "-dVersion=$version" `
    "-dUpgradeCode=$($product.UpgradeCode)" `
    "-dInstallFolder=$name" `
    "-dRegistryKey=Software\RestartFU\$name" `
    "-dAppUserModelId=$($product.AppUserModelId)" `
    "-dPayload=$payload" `
    "-dIcon=$(Join-Path $repositoryRoot 'installer/windows/xd.ico')" `
    -out $object (Join-Path $repositoryRoot 'installer/windows/xd.wxs')
if ($LASTEXITCODE -ne 0) { throw 'Compiling the installer failed.' }

# ICE91 warns that the files go to a per-user folder, which is the point.
# ICE61 warns that the same version may upgrade, which nightly builds rely on.
& (Join-Path $wix 'light.exe') -nologo -sice:ICE91 -sice:ICE61 `
    -spdb -out $installer $object
if ($LASTEXITCODE -ne 0) { throw 'Linking the installer failed.' }
Remove-Item -LiteralPath $work -Recurse -Force

$hash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText("$installer.sha256", "$hash  $asset`n", [Text.UTF8Encoding]::new($false))
Write-Host "Windows installer: $installer"
