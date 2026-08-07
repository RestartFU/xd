# Installs xd on Windows from its release checksum and MSI.
#
# Latest nightly:
#
#   irm https://github.com/RestartFU/xd/releases/download/nightly/install.ps1 | iex
#
# Pass -Release when invoking a downloaded script file to install the newest
# tagged release. -MsiPath and -ChecksumPath let CI exercise this exact path.

[CmdletBinding()]
param(
    [switch] $Release,
    [string] $MsiPath,
    [string] $ChecksumPath,
    [switch] $Quiet,
    [switch] $InApp,
    [string] $StageDirectory,
    [switch] $StageOnly,
    [switch] $WaitForInstalledExit,
    [string] $InstallRoot,
    [string] $RelaunchPath,
    [string] $CleanupDirectory
)

$ErrorActionPreference = 'Stop'
$repository = 'RestartFU/xd'
$channel = if ($Release) { 'release' } else { 'nightly' }
$installName = if ($Release) { 'xd' } else { 'xd-nightly' }
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

function Get-InstallRoot {
    if (-not [string]::IsNullOrWhiteSpace($InstallRoot)) {
        return ([IO.Path]::GetFullPath($InstallRoot)).TrimEnd('\', '/')
    }

    if (-not [string]::IsNullOrWhiteSpace($PSCommandPath)) {
        $binDirectory = Split-Path -Parent $PSCommandPath
        $scriptRoot = Split-Path -Parent $binDirectory
        if ((Split-Path -Leaf $scriptRoot) -ieq $installName -and
            (Split-Path -Leaf (Split-Path -Parent $scriptRoot)) -ieq 'RestartFU') {
            return ([IO.Path]::GetFullPath($scriptRoot)).TrimEnd('\', '/')
        }
    }

    $programFiles = @($env:ProgramW6432, $env:ProgramFiles) |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    foreach ($base in $programFiles) {
        $candidate = Join-Path $base "RestartFU\$installName"
        if (Test-Path -LiteralPath $candidate -PathType Container) {
            return ([IO.Path]::GetFullPath($candidate)).TrimEnd('\', '/')
        }
    }
    return $null
}

function Find-RunningInstalledProcess {
    $installDirectory = Get-InstallRoot
    if ([string]::IsNullOrWhiteSpace($installDirectory)) {
        return $null
    }
    $prefix = "$installDirectory\"
    foreach ($process in Get-Process -ErrorAction SilentlyContinue) {
        try {
            $path = $process.Path
        } catch {
            continue
        }
        if (-not [string]::IsNullOrWhiteSpace($path) -and
            $path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
            return $path
        }
    }
    return $null
}

# Refuse an external install while either the desktop or daemon still has an
# installed executable mapped. The handoff path waits for both processes to
# exit before invoking MSI, so it does not need this bypass.
if (-not $StageOnly -and $env:XD_ALLOW_RUNNING_INSTALL -ne '1') {
    $running = Find-RunningInstalledProcess
    if ($null -ne $running) {
        throw "xd is running from $running. Quit it completely, then rerun this installer."
    }
}

try {
    if ($StageOnly) {
        if ([string]::IsNullOrWhiteSpace($StageDirectory)) {
            throw '-StageDirectory is required with -StageOnly.'
        }
        $StageDirectory = [IO.Path]::GetFullPath($StageDirectory)
        New-Item -ItemType Directory -Force -Path $StageDirectory | Out-Null
        $MsiPath = Join-Path $StageDirectory $asset
        $ChecksumPath = "$MsiPath.sha256"
        Write-Host "Downloading xd $channel..."
        Invoke-WebRequest -UseBasicParsing -Uri "$baseUri/$asset" -OutFile $MsiPath
        Invoke-WebRequest -UseBasicParsing -Uri "$baseUri/$asset.sha256" `
            -OutFile $ChecksumPath
    } elseif ([string]::IsNullOrWhiteSpace($MsiPath)) {
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
        $actual = (Get-FileHash -LiteralPath $MsiPath -Algorithm SHA256).Hash
        if ($actual.ToLowerInvariant() -ne $expected) {
            throw 'Downloaded MSI does not match its release checksum.'
        }
    }

    if ($StageOnly) {
        Write-Host "Staged xd $channel update in $StageDirectory."
        return
    }

    if ($WaitForInstalledExit) {
        $deadline = [DateTime]::UtcNow.AddSeconds(60)
        do {
            $running = Find-RunningInstalledProcess
            if ($null -eq $running) {
                break
            }
            if ([DateTime]::UtcNow -ge $deadline) {
                throw "Timed out waiting for the installed xd process ($running) to exit."
            }
            Start-Sleep -Milliseconds 100
        } while ($true)
    }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $isAdministrator = $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
    $displayMode = if ($Quiet) { '/qn' } else { '/passive' }
    $start = @{
        FilePath = "$env:SystemRoot\System32\msiexec.exe"
        ArgumentList = @(
            '/i', "`"$MsiPath`"", $displayMode, '/norestart', 'REBOOT=ReallySuppress'
        )
        Wait = $true
        PassThru = $true
    }
    if (-not $isAdministrator) { $start['Verb'] = 'RunAs' }

    Write-Host 'Installing xd...'
    $process = Start-Process @start
    if (@(0, 1641, 3010) -notcontains $process.ExitCode) {
        throw "Windows Installer failed with exit code $($process.ExitCode)."
    }
    if ($InApp -and @(1641, 3010) -contains $process.ExitCode) {
        throw 'Windows Installer needs a Windows restart before the in-app update can be used.'
    }
    if (-not [string]::IsNullOrWhiteSpace($RelaunchPath)) {
        $workingDirectory = Split-Path -Parent $RelaunchPath
        Start-Process -FilePath $RelaunchPath -WorkingDirectory $workingDirectory | Out-Null
    }
    Write-Host "Installed xd $channel. Open it from the Start menu."
} finally {
    if ($null -ne $downloadDirectory) {
        Remove-Item -LiteralPath $downloadDirectory -Recurse -Force `
            -ErrorAction SilentlyContinue
    }
    if (-not [string]::IsNullOrWhiteSpace($CleanupDirectory) -and
        (Test-Path -LiteralPath $CleanupDirectory)) {
        Remove-Item -LiteralPath $CleanupDirectory -Recurse -Force `
            -ErrorAction SilentlyContinue
    }
}
