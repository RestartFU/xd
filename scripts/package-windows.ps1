#!/usr/bin/env pwsh
#
# Turn a validated native Rust/GPUI Windows payload into a small downloading
# setup executable backed by MSI metadata and an external cabinet.
#
#   package-windows.ps1 -Payload windows-dist -OutputDirectory artifacts
#                       [-Profile dev|nightly|release] [-Version 0.1.0]

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Payload,

    [Parameter(Mandatory = $true)]
    [string] $OutputDirectory,

    [ValidateSet('dev', 'nightly', 'release')]
    [string] $Profile = 'nightly',

    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string] $Version = '0.1.0',

    [string] $BundleVersion,

    [string] $TestDownloadBase
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
    'bin\xd-host.exe',
    'bin\install.ps1',
    'git\cmd\git.exe'
)) {
    if (-not (Test-Path -LiteralPath (Join-Path $payloadPath $relativePath))) {
        throw "Windows payload is missing $relativePath`: $payloadPath"
    }
}
if ($Profile -eq 'dev' -and
    -not (Test-Path -LiteralPath (Join-Path $payloadPath 'bin\xd-desktop.exe'))) {
    throw "Windows dev payload is missing bin\xd-desktop.exe`: $payloadPath"
}

if ($Profile -eq 'dev') {
    $productName = 'xd (Dev)'
    $installName = 'xd-dev'
    $asset = 'xd-dev-windows-x86_64.msi'
    $setupAsset = 'xd-dev-windows-x86_64-setup.exe'
    $msiPayloadAsset = 'xd-dev-windows-x86_64-msi.payload'
    $cabPayloadAsset = 'xd-dev-windows-x86_64-cab.payload'
    $downloadBase = 'https://github.com/RestartFU/xd/releases/download/dev'
    $upgradeCode = 'EEA6BB82-95FA-4166-B9C1-CD11D42541C6'
    $bundleUpgradeCode = 'B3942064-156A-4522-AFB9-A20D28E29A54'
    $shortcutGuid = '10D7B5B7-B89C-4D6F-8AE9-98B78CBCB30D'
} elseif ($Profile -eq 'nightly') {
    $productName = 'xd (Nightly)'
    $installName = 'xd-nightly'
    $asset = 'xd-nightly-windows-x86_64.msi'
    $setupAsset = 'xd-nightly-windows-x86_64-setup.exe'
    $msiPayloadAsset = 'xd-nightly-windows-x86_64-msi.payload'
    $cabPayloadAsset = 'xd-nightly-windows-x86_64-cab.payload'
    $downloadBase = 'https://github.com/RestartFU/xd/releases/download/nightly'
    $upgradeCode = '5A04BCF5-0C4A-43DE-B8F4-B5D64E8E4F93'
    $bundleUpgradeCode = '6F7420C1-080C-47A6-92A6-7FB5C3BEBCF2'
    $shortcutGuid = 'E7E776D9-E605-4AE2-BAD8-6350E845A1DE'
} else {
    $productName = 'xd'
    $installName = 'xd'
    $asset = 'xd-windows-x86_64.msi'
    $setupAsset = 'xd-windows-x86_64-setup.exe'
    $msiPayloadAsset = 'xd-windows-x86_64-msi.payload'
    $cabPayloadAsset = 'xd-windows-x86_64-cab.payload'
    $downloadBase = 'https://github.com/RestartFU/xd/releases/latest/download'
    $upgradeCode = 'CDE5479B-9093-4E3C-8144-FCE8C86BEA18'
    $bundleUpgradeCode = '94B90C1E-173A-4628-9DC3-188E411429D0'
    $shortcutGuid = '4E2C6D60-BBC8-4E6F-B230-C9824E0D1715'
}
$cabAsset = 'xd1.cab'
if ([string]::IsNullOrWhiteSpace($BundleVersion)) {
    if ($Profile -ne 'release') {
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
$msiPayloadOutput = Join-Path $outputPath $msiPayloadAsset
$cabPayloadOutput = Join-Path $outputPath $cabPayloadAsset
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
Copy-Item -LiteralPath $output -Destination $msiPayloadOutput -Force
Copy-Item -LiteralPath $cabOutput -Destination $cabPayloadOutput -Force

function Build-Bundle([string] $Target, [string] $BaseUri) {
    $base = $BaseUri.TrimEnd('/')
    & $wix build (Join-Path $repositoryRoot 'installer\windows\bundle.wxs') `
        -arch x64 `
        -ext $bootstrapperExtension `
        -d "MsiPath=$output" `
        -d "CabPath=$cabOutput" `
        -d "Version=$BundleVersion" `
        -d "ProductName=$productName" `
        -d "BundleUpgradeCode=$bundleUpgradeCode" `
        -d "MsiDownloadUrl=$base/$msiPayloadAsset" `
        -d "CabDownloadUrl=$base/$cabPayloadAsset" `
        -d "IconPath=$iconPath" `
        -o $Target
    if ($LASTEXITCODE -ne 0) {
        throw "Building the downloading installer failed with exit code $LASTEXITCODE."
    }
}

Build-Bundle $setupOutput $downloadBase
$smallArtifacts = @($output, $setupOutput)
if (-not [string]::IsNullOrWhiteSpace($TestDownloadBase)) {
    $testSetupOutput = Join-Path $outputPath (
        $setupAsset -replace '\.exe$', '-download-test.exe'
    )
    Build-Bundle $testSetupOutput $TestDownloadBase
    $smallArtifacts += $testSetupOutput
}
foreach ($smallArtifact in $smallArtifacts) {
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
if (-not [string]::IsNullOrWhiteSpace($TestDownloadBase)) {
    Write-Checksum $testSetupOutput
}

Write-Host "Windows web installer: $setupOutput"
Write-Host "Windows remote MSI payload: $msiPayloadOutput"
Write-Host "Windows remote cabinet payload: $cabPayloadOutput"
Write-Host "Windows MSI metadata: $output"
Write-Host "Windows payload cabinet: $cabOutput"
