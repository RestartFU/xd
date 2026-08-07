#!/usr/bin/env pwsh
#
# Turn a validated native Rust/GPUI Windows payload into the MSI consumed by
# install.ps1.
#
#   package-windows.ps1 -Payload windows-dist -OutputDirectory artifacts
#                       [-Profile nightly|release] [-Version 0.1.0]

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Payload,

    [Parameter(Mandatory = $true)]
    [string] $OutputDirectory,

    [ValidateSet('nightly', 'release')]
    [string] $Profile = 'nightly',

    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string] $Version = '0.1.0'
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$payloadPath = (Resolve-Path -LiteralPath $Payload).Path
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
# Beside the other build caches rather than in a fresh temp directory: the
# tool is pinned, so installing it again on every run buys nothing.
$wixVersion = '6.0.2'
$toolDirectory = Join-Path $repositoryRoot ".build-cache\wix-$wixVersion"

foreach ($relativePath in @(
    'bin\xd.exe',
    'bin\xd-daemon.exe',
    'bin\xd-tls-proxy.exe',
    'bin\install.ps1',
    'bin\codex-package\bin\codex.exe',
    'bin\claude.exe',
    'bin\claude-code-proxy.exe',
    'bin\whisper-server-bin.exe',
    'git\cmd\git.exe'
)) {
    if (-not (Test-Path -LiteralPath (Join-Path $payloadPath $relativePath))) {
        throw "Windows payload is missing $relativePath`: $payloadPath"
    }
}

if ($Profile -eq 'nightly') {
    $productName = 'xd (Nightly)'
    $installName = 'xd-nightly'
    $asset = 'xd-nightly-windows-x86_64.msi'
    $upgradeCode = '5A04BCF5-0C4A-43DE-B8F4-B5D64E8E4F93'
    $shortcutGuid = 'E7E776D9-E605-4AE2-BAD8-6350E845A1DE'
} else {
    $productName = 'xd'
    $installName = 'xd'
    $asset = 'xd-windows-x86_64.msi'
    $upgradeCode = 'CDE5479B-9093-4E3C-8144-FCE8C86BEA18'
    $shortcutGuid = '4E2C6D60-BBC8-4E6F-B230-C9824E0D1715'
}

New-Item -ItemType Directory -Force -Path $outputPath | Out-Null

$wix = Join-Path $toolDirectory 'wix.exe'
if (-not (Test-Path -LiteralPath $wix -PathType Leaf)) {
    New-Item -ItemType Directory -Force -Path $toolDirectory | Out-Null
    & dotnet tool install --tool-path $toolDirectory wix --version $wixVersion
    if ($LASTEXITCODE -ne 0) { throw 'Installing WiX failed.' }
}

$output = Join-Path $outputPath $asset
& $wix build (Join-Path $repositoryRoot 'installer\windows\xd.wxs') `
    -arch x64 `
    -d "Payload=$payloadPath" `
    -d "Version=$Version" `
    -d "ProductName=$productName" `
    -d "InstallName=$installName" `
    -d "UpgradeCode=$upgradeCode" `
    -d "ShortcutGuid=$shortcutGuid" `
    -o $output
if ($LASTEXITCODE -ne 0) {
    throw "WiX failed with exit code $LASTEXITCODE."
}

$hash = (Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash
$checksum = Join-Path $outputPath "$asset.sha256"
"$($hash.ToLowerInvariant())  $asset" |
    Set-Content -LiteralPath $checksum -Encoding ascii

Write-Host "Windows artifact: $output"
