#!/usr/bin/env pwsh
#
# Turn a validated native Rust/GPUI Windows payload into a small downloading
# setup executable backed by MSI metadata and an external cabinet.
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
    [string] $Version = '0.1.0',

    [string] $BundleVersion
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
    $setupAsset = 'xd-nightly-windows-x86_64-setup.exe'
    $downloadBase = 'https://github.com/RestartFU/xd/releases/download/nightly'
    $upgradeCode = '5A04BCF5-0C4A-43DE-B8F4-B5D64E8E4F93'
    $bundleUpgradeCode = '6F7420C1-080C-47A6-92A6-7FB5C3BEBCF2'
    $shortcutGuid = 'E7E776D9-E605-4AE2-BAD8-6350E845A1DE'
} else {
    $productName = 'xd'
    $installName = 'xd'
    $asset = 'xd-windows-x86_64.msi'
    $setupAsset = 'xd-windows-x86_64-setup.exe'
    $downloadBase = 'https://github.com/RestartFU/xd/releases/latest/download'
    $upgradeCode = 'CDE5479B-9093-4E3C-8144-FCE8C86BEA18'
    $bundleUpgradeCode = '94B90C1E-173A-4628-9DC3-188E411429D0'
    $shortcutGuid = '4E2C6D60-BBC8-4E6F-B230-C9824E0D1715'
}
$cabAsset = 'xd1.cab'
if ([string]::IsNullOrWhiteSpace($BundleVersion)) {
    if ($Profile -eq 'nightly') {
        $epoch = [DateTime]::new(2020, 1, 1, 0, 0, 0, [DateTimeKind]::Utc)
        $now = [DateTime]::UtcNow
        $day = [Math]::Floor(($now - $epoch).TotalDays)
        $minute = $now.Hour * 60 + $now.Minute
        $BundleVersion = "0.1.$day.$minute"
    } else {
        $BundleVersion = $Version
    }
}
if ($BundleVersion -notmatch '^\d+\.\d+\.\d+(\.\d+)?$') {
    throw "Invalid bundle version: $BundleVersion"
}

New-Item -ItemType Directory -Force -Path $outputPath | Out-Null

$wix = Join-Path $toolDirectory 'wix.exe'
if (-not (Test-Path -LiteralPath $wix -PathType Leaf)) {
    New-Item -ItemType Directory -Force -Path $toolDirectory | Out-Null
    & dotnet tool install --tool-path $toolDirectory wix --version $wixVersion
    if ($LASTEXITCODE -ne 0) { throw 'Installing WiX failed.' }
}

$bootstrapperExtension = 'WixToolset.BootstrapperApplications.wixext'
& $wix extension add --global "$bootstrapperExtension/$wixVersion"
if ($LASTEXITCODE -ne 0) { throw 'Installing the WiX bootstrapper extension failed.' }

$output = Join-Path $outputPath $asset
$cabOutput = Join-Path $outputPath $cabAsset
$setupOutput = Join-Path $outputPath $setupAsset
$iconPath = Join-Path $repositoryRoot 'desktop\assets\xd.ico'
if (-not (Test-Path -LiteralPath $iconPath -PathType Leaf)) {
    throw "The Windows application icon is missing: $iconPath"
}
& $wix build (Join-Path $repositoryRoot 'installer\windows\xd.wxs') `
    -arch x64 `
    -d "Payload=$payloadPath" `
    -d "Version=$Version" `
    -d "ProductName=$productName" `
    -d "InstallName=$installName" `
    -d "UpgradeCode=$upgradeCode" `
    -d "ShortcutGuid=$shortcutGuid" `
    -d "IconPath=$iconPath" `
    -o $output
if ($LASTEXITCODE -ne 0) {
    throw "WiX failed with exit code $LASTEXITCODE."
}
if (-not (Test-Path -LiteralPath $cabOutput -PathType Leaf)) {
    throw "WiX did not produce the external payload cabinet: $cabOutput"
}

& $wix build (Join-Path $repositoryRoot 'installer\windows\bundle.wxs') `
    -arch x64 `
    -ext $bootstrapperExtension `
    -d "MsiPath=$output" `
    -d "Version=$BundleVersion" `
    -d "ProductName=$productName" `
    -d "BundleUpgradeCode=$bundleUpgradeCode" `
    -d "DownloadBase=$downloadBase" `
    -d "IconPath=$iconPath" `
    -o $setupOutput
if ($LASTEXITCODE -ne 0) {
    throw "Building the downloading installer failed with exit code $LASTEXITCODE."
}
foreach ($smallArtifact in @($output, $setupOutput)) {
    if ((Get-Item -LiteralPath $smallArtifact).Length -ge 32MB) {
        throw "The downloading installer unexpectedly contains the application payload: $smallArtifact"
    }
}

function Write-Checksum([string] $Path) {
    $name = Split-Path -Leaf $Path
    $hash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    "$($hash.ToLowerInvariant())  $name" |
        Set-Content -LiteralPath "$Path.sha256" -Encoding ascii
}

Write-Checksum $output
Write-Checksum $cabOutput
Write-Checksum $setupOutput

Write-Host "Windows web installer: $setupOutput"
Write-Host "Windows MSI metadata: $output"
Write-Host "Windows payload cabinet: $cabOutput"
