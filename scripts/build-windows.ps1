#!/usr/bin/env pwsh
# Build the native Windows frontend with its matching static Linux WSL host.
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
$name = if ($Profile -eq 'nightly') { 'xd-nightly' } else { 'xd' }
$payload = Join-Path $outputPath $name

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

# MSVC's static CRT keeps the extracted app independent of redistributable DLLs.
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

New-Item -ItemType Directory -Force -Path $outputPath | Out-Null
if (Test-Path -LiteralPath $payload) { Remove-Item -LiteralPath $payload -Recurse -Force }
New-Item -ItemType Directory -Path $payload, (Join-Path $payload 'libexec'), `
    (Join-Path $payload 'licenses') | Out-Null
Copy-Item -LiteralPath (Join-Path $repositoryRoot `
    'desktop/target/x86_64-pc-windows-msvc/release/xd-desktop.exe') `
    -Destination (Join-Path $payload 'xd.exe')
Copy-Item -LiteralPath $hostPath -Destination (Join-Path $payload 'libexec/xd-host-linux')
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'installer/windows/README.txt') `
    -Destination (Join-Path $payload 'README.txt')
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'installer/windows/install-webview2.ps1') `
    -Destination (Join-Path $payload 'install-webview2.ps1')
Copy-Item -LiteralPath (Join-Path $repositoryRoot `
    'data/licenses/alacritty-terminal-LICENSE-APACHE') `
    -Destination (Join-Path $payload 'licenses/alacritty-terminal-LICENSE-APACHE')

$asset = "$name-windows-x86_64.zip"
$archive = Join-Path $outputPath $asset
Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
Compress-Archive -LiteralPath $payload -DestinationPath $archive
$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText("$archive.sha256", "$hash  $asset`n", [Text.UTF8Encoding]::new($false))
Write-Host "Windows artifact: $archive"
