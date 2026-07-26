# Installs xd on Windows from its release checksum and MSI.
#
# Latest nightly:
#
#   irm https://github.com/RestartFU/xd/releases/download/nightly/install.ps1 | iex
#
# Pass -Release when invoking the downloaded script block to install the newest
# tagged release instead. -MsiPath and -ChecksumPath exist so CI and local
# builds can exercise this exact installer without downloading an older build.

[CmdletBinding()]
param(
    [switch] $Release,
    [string] $MsiPath,
    [string] $ChecksumPath,
    [switch] $Quiet
)

$ErrorActionPreference = 'Stop'
$repository = 'RestartFU/xd'
$channel = if ($Release) { 'release' } else { 'nightly' }
$asset = if ($Release) {
    'xd-windows-x86_64.msi'
} else {
    'xd-nightly-windows-x86_64.msi'
}
$baseUri = if ($Release) {
    "https://github.com/$repository/releases/latest/download"
} else {
    "https://github.com/$repository/releases/download/nightly"
}
$downloadDirectory = $null

if ($env:OS -ne 'Windows_NT') {
    throw 'This installer requires Windows.'
}

if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'Only Windows x86_64 is published so far.'
}

try {
    if ([string]::IsNullOrWhiteSpace($MsiPath)) {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        $downloadDirectory = Join-Path ([IO.Path]::GetTempPath()) (
            'xd-install-' + [guid]::NewGuid().ToString('N')
        )
        New-Item -ItemType Directory -Path $downloadDirectory | Out-Null

        $MsiPath = Join-Path $downloadDirectory $asset
        $ChecksumPath = "$MsiPath.sha256"

        Write-Host "Downloading xd $channel..."
        Invoke-WebRequest -UseBasicParsing -Uri "$baseUri/$asset" -OutFile $MsiPath
        Invoke-WebRequest -UseBasicParsing -Uri "$baseUri/$asset.sha256" `
            -OutFile $ChecksumPath
    }

    $MsiPath = (Resolve-Path -LiteralPath $MsiPath).Path

    if (-not [string]::IsNullOrWhiteSpace($ChecksumPath)) {
        $ChecksumPath = (Resolve-Path -LiteralPath $ChecksumPath).Path
        $expected = (
            (Get-Content -LiteralPath $ChecksumPath -Raw).Trim() -split '\s+'
        )[0].ToLowerInvariant()

        if ($expected -notmatch '^[0-9a-f]{64}$') {
            throw 'Release checksum has an invalid format.'
        }

        $actualHash = (Get-FileHash -LiteralPath $MsiPath -Algorithm SHA256).Hash
        $actual = $actualHash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw 'Downloaded MSI does not match its release checksum.'
        }
    }

    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $isAdministrator = $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
    $displayMode = if ($Quiet) { '/qn' } else { '/passive' }
    $arguments = @('/i', "`"$MsiPath`"", $displayMode, '/norestart')
    $start = @{
        FilePath = "$env:SystemRoot\System32\msiexec.exe"
        ArgumentList = $arguments
        Wait = $true
        PassThru = $true
    }

    if (-not $isAdministrator) {
        $start['Verb'] = 'RunAs'
    }

    Write-Host 'Installing xd...'
    $process = Start-Process @start
    if (@(0, 1641, 3010) -notcontains $process.ExitCode) {
        throw "Windows Installer failed with exit code $($process.ExitCode)."
    }

    Write-Host "Installed xd $channel. Open it from the Start menu."
} finally {
    if ($null -ne $downloadDirectory) {
        Remove-Item -LiteralPath $downloadDirectory -Recurse -Force `
            -ErrorAction SilentlyContinue
    }
}
