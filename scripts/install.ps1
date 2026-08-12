# Installs xd on Windows through its small downloading setup executable, or
# directly from a local MSI and external cabinet for CI/source builds.
#
# Latest nightly:
#
#   irm https://github.com/RestartFU/xd/releases/download/nightly/install.ps1 | iex
#
# Pass -Dev for the isolated rolling development build, or -Release for the
# newest tagged release. The path parameters let CI exercise the offline MSI
# path.

[CmdletBinding()]
param(
    [switch] $Release,
    [switch] $Dev,
    [string] $SetupPath,
    [string] $SetupChecksumPath,
    [string] $MsiPath,
    [string] $ChecksumPath,
    [string] $CabPath,
    [string] $CabChecksumPath,
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
if ($Release -and $Dev) {
    throw '-Release and -Dev cannot be used together.'
}
$channel = if ($Release) { 'release' } elseif ($Dev) { 'dev' } else { 'nightly' }
$installName = if ($Release) { 'xd' } elseif ($Dev) { 'xd-dev' } else { 'xd-nightly' }
$setupAsset = if ($Release) {
    'xd-windows-x86_64-setup.exe'
} elseif ($Dev) {
    'xd-dev-windows-x86_64-setup.exe'
} else {
    'xd-nightly-windows-x86_64-setup.exe'
}
$cabAsset = 'xd1.cab'
$baseUri = if ($Release) {
    "https://github.com/$repository/releases/latest/download"
} elseif ($Dev) {
    "https://github.com/$repository/releases/download/dev"
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

function Confirm-Checksum(
    [string] $Path,
    [string] $Checksum,
    [string] $Description
) {
    $Path = (Resolve-Path -LiteralPath $Path).Path
    $Checksum = (Resolve-Path -LiteralPath $Checksum).Path
    $expected = ((Get-Content -LiteralPath $Checksum -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
    if ($expected -notmatch '^[0-9a-f]{64}$') {
        throw "$Description checksum has an invalid format."
    }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    if ($actual.ToLowerInvariant() -ne $expected) {
        throw "$Description does not match its release checksum."
    }
    return $Path
}

# Refuse an external install while either the desktop or daemon still has an
# installed executable mapped. The handoff path waits for both processes to
# exit before invoking the staged setup, so it does not need this bypass.
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
        $SetupPath = Join-Path $StageDirectory $setupAsset
        $SetupChecksumPath = "$SetupPath.sha256"
        Write-Host "Downloading xd $channel..."
        Invoke-WebRequest -UseBasicParsing -Uri "$baseUri/$setupAsset" `
            -OutFile $SetupPath
        Invoke-WebRequest -UseBasicParsing -Uri "$baseUri/$setupAsset.sha256" `
            -OutFile $SetupChecksumPath
    } elseif ([string]::IsNullOrWhiteSpace($MsiPath) -and
        [string]::IsNullOrWhiteSpace($SetupPath)) {
        $downloadDirectory = Join-Path ([IO.Path]::GetTempPath()) (
            'xd-install-' + [guid]::NewGuid().ToString('N')
        )
        New-Item -ItemType Directory -Path $downloadDirectory | Out-Null
        $SetupPath = Join-Path $downloadDirectory $setupAsset
        $SetupChecksumPath = "$SetupPath.sha256"

        Write-Host "Downloading xd $channel..."
        Invoke-WebRequest -UseBasicParsing -Uri "$baseUri/$setupAsset" `
            -OutFile $SetupPath
        Invoke-WebRequest -UseBasicParsing -Uri "$baseUri/$setupAsset.sha256" `
            -OutFile $SetupChecksumPath
    }

    $usingSetup = -not [string]::IsNullOrWhiteSpace($SetupPath)
    if ($usingSetup) {
        if ([string]::IsNullOrWhiteSpace($SetupChecksumPath)) {
            throw 'A setup checksum is required.'
        }
        $SetupPath = Confirm-Checksum $SetupPath $SetupChecksumPath 'Downloaded setup'
    } else {
        $MsiPath = (Resolve-Path -LiteralPath $MsiPath).Path
        if (-not [string]::IsNullOrWhiteSpace($ChecksumPath)) {
            $MsiPath = Confirm-Checksum $MsiPath $ChecksumPath 'MSI'
        }
        if ([string]::IsNullOrWhiteSpace($CabPath)) {
            $CabPath = Join-Path (Split-Path -Parent $MsiPath) $cabAsset
        }
        $CabPath = (Resolve-Path -LiteralPath $CabPath).Path
        if ([string]::IsNullOrWhiteSpace($CabChecksumPath)) {
            $candidate = "$CabPath.sha256"
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                $CabChecksumPath = $candidate
            }
        }
        if (-not [string]::IsNullOrWhiteSpace($CabChecksumPath)) {
            $CabPath = Confirm-Checksum $CabPath $CabChecksumPath 'MSI payload cabinet'
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
    if ($usingSetup) {
        $arguments = @('/norestart')
        if ($Quiet) { $arguments += '/quiet' }
        $start = @{
            FilePath = $SetupPath
            ArgumentList = $arguments
            Wait = $true
            PassThru = $true
        }
    } else {
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
    }

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
