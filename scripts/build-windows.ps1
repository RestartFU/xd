#!/usr/bin/env pwsh
#
# Build and validate the complete native Rust/GPUI Windows x86_64 payload.
#
#   ./scripts/build-windows.ps1 -OutputDirectory windows-dist
#       [-Profile dev|nightly|release]

# Run from a native x86_64 Windows PowerShell with Rust and 7-Zip.
# The resulting tree is self-contained and is consumed by package-windows.ps1.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $OutputDirectory,

    [ValidateSet('dev', 'nightly', 'release')]
    [string] $Profile = 'nightly'
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
$workDirectory = Join-Path ([IO.Path]::GetTempPath()) (
    'xd-windows-build-' + [guid]::NewGuid().ToString('N')
)
$cacheDirectory = Join-Path $repositoryRoot '.build-cache\windows-assets'
$buildJobs = if ($env:XD_BUILD_JOBS -match '^[1-9][0-9]*$') {
    [Math]::Min([int] $env:XD_BUILD_JOBS, [Environment]::ProcessorCount)
} else {
    [Math]::Max(1, [Math]::Floor([Environment]::ProcessorCount * 0.75))
}
$env:CARGO_BUILD_JOBS = $buildJobs.ToString()

$gitVersion = '2.55.0.3'
$gitAsset = 'PortableGit-2.55.0.3-64-bit.7z.exe'
$gitSha256 = 'ab00566336b5472120f9a52d34f2e79c5406535792acb0548001ffd0bd090e5d'

function Invoke-CheckedDownload {
    param(
        [Parameter(Mandatory = $true)][string] $Uri,
        [Parameter(Mandatory = $true)][string] $Destination,
        [Parameter(Mandatory = $true)][string] $Sha256
    )

    if (Test-Path -LiteralPath $Destination) {
        $cachedHash = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash
        if ($cachedHash.ToLowerInvariant() -eq $Sha256) {
            return
        }
        Remove-Item -LiteralPath $Destination -Force
    }

    $partial = "$Destination.partial"
    Remove-Item -LiteralPath $partial -Force -ErrorAction SilentlyContinue
    Invoke-WebRequest -UseBasicParsing -Uri $Uri -OutFile $partial
    $actual = (Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash
    if ($actual.ToLowerInvariant() -ne $Sha256) {
        Remove-Item -LiteralPath $partial -Force
        throw "Checksum mismatch for $Uri."
    }
    Move-Item -LiteralPath $partial -Destination $Destination
}

function Assert-LastExitCode {
    param([Parameter(Mandatory = $true)][string] $Operation)
    if ($LASTEXITCODE -ne 0) {
        throw "$Operation failed with exit code $LASTEXITCODE."
    }
}

if ($env:OS -ne 'Windows_NT' -or -not [Environment]::Is64BitOperatingSystem) {
    throw 'build-windows.ps1 requires Windows x86_64.'
}
if ((Test-Path -LiteralPath $outputPath) -and
    (Get-ChildItem -LiteralPath $outputPath -Force | Select-Object -First 1)) {
    throw "Output directory must be empty: $outputPath"
}
foreach ($command in @('cargo', 'git', 'rustc')) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "$command is required."
    }
}
$sevenZip = (Get-Command 7z -ErrorAction SilentlyContinue).Source
if ([string]::IsNullOrWhiteSpace($sevenZip)) {
    $sevenZip = Join-Path $env:ProgramFiles '7-Zip\7z.exe'
}
if (-not (Test-Path -LiteralPath $sevenZip)) {
    throw '7-Zip is required.'
}

try {
    New-Item -ItemType Directory -Force -Path @(
        $outputPath,
        (Join-Path $outputPath 'bin'),
        (Join-Path $outputPath 'share\fonts\xd'),
        (Join-Path $outputPath 'share\licenses\xd'),
        $workDirectory,
        $cacheDirectory
    ) | Out-Null

    $gitArchive = Join-Path $cacheDirectory "$gitVersion-$gitAsset"

    Invoke-CheckedDownload `
        -Uri "https://github.com/git-for-windows/git/releases/download/v2.55.0.windows.3/$gitAsset" `
        -Destination $gitArchive -Sha256 $gitSha256

    Push-Location $repositoryRoot
    try {
        $commit = (& git rev-parse HEAD 2>$null)
        if ($LASTEXITCODE -ne 0) { $commit = '' }
        # Dev is prerelease code like nightly. A tiny launcher below supplies
        # its third runtime identity without changing desktop source files.
        $env:XD_BUILD_PROFILE = if ($Profile -eq 'dev') { 'nightly' } else { $Profile }
        $env:XD_COMMIT = $commit
        & cargo build --locked --release --manifest-path desktop/Cargo.toml
        Assert-LastExitCode 'desktop build'
        & cargo build --locked --release --manifest-path daemon-rs/Cargo.toml
        Assert-LastExitCode 'host build'
    } finally {
        Pop-Location
    }

    $desktopSource = Join-Path $repositoryRoot 'desktop\target\release\xd-desktop.exe'
    if ($Profile -eq 'dev') {
        Copy-Item -LiteralPath $desktopSource `
            -Destination (Join-Path $outputPath 'bin\xd-desktop.exe')

        # Windows has no shell launcher around the native executable. Build a
        # no-console shim that gives dev its own app id and data directory,
        # then forwards every argument and exit status to the actual desktop.
        $launcherSource = Join-Path $workDirectory 'xd-dev-launcher.rs'
        @'
#![windows_subsystem = "windows"]

use std::{env, process::{self, Command}};

fn main() {
    env::set_var("XD_APP_ID", "com.restartfu.Xd.Dev");
    env::set_var("XD_DATA_NAME", "xd-dev");
    env::set_var("XD_UPDATE_CHANNEL", "dev");

    let executable = env::current_exe().unwrap_or_else(|error| {
        eprintln!("xd-dev: cannot locate its launcher: {error}");
        process::exit(1);
    });
    let desktop = executable.with_file_name("xd-desktop.exe");
    match Command::new(desktop).args(env::args_os().skip(1)).status() {
        Ok(status) => process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("xd-dev: cannot start the desktop: {error}");
            process::exit(1);
        }
    }
}
'@ | Set-Content -LiteralPath $launcherSource -Encoding utf8
        & rustc --edition 2021 -C opt-level=s $launcherSource `
            -o (Join-Path $outputPath 'bin\xd.exe')
        Assert-LastExitCode 'dev launcher build'
    } else {
        Copy-Item -LiteralPath $desktopSource `
            -Destination (Join-Path $outputPath 'bin\xd.exe')
    }
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'daemon-rs\target\release\xd-host.exe') `
        -Destination (Join-Path $outputPath 'bin\xd-host.exe')
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'data\fonts\DMSans-Variable.ttf') `
        -Destination (Join-Path $outputPath 'share\fonts\xd\DMSans-Variable.ttf')
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'scripts\install.ps1') `
        -Destination (Join-Path $outputPath 'bin\install.ps1')

    $gitDirectory = Join-Path $outputPath 'git'
    & $sevenZip x -y "-o$gitDirectory" $gitArchive | Out-Null
    Assert-LastExitCode 'PortableGit extraction'
    $postInstall = Join-Path $gitDirectory 'post-install.bat'
    if (Test-Path -LiteralPath $postInstall) {
        & (Join-Path $gitDirectory 'git-bash.exe') `
            --no-needs-console --hide --no-cd --command=post-install.bat
        Assert-LastExitCode 'PortableGit post-install'
    }

    $required = @(
        'bin\xd.exe',
        'bin\xd-host.exe',
        'bin\install.ps1',
        'git\cmd\git.exe',
        'git\mingw64\libexec\git-core\git-remote-https.exe',
        'git\mingw64\etc\ssl\certs\ca-bundle.crt'
    )
    if ($Profile -eq 'dev') {
        $required += 'bin\xd-desktop.exe'
    }
    foreach ($relativePath in $required) {
        if (-not (Test-Path -LiteralPath (Join-Path $outputPath $relativePath))) {
            throw "Windows payload is missing $relativePath."
        }
    }

    & (Join-Path $outputPath 'bin\xd.exe') --version
    Assert-LastExitCode 'xd smoke test'
    & (Join-Path $outputPath 'git\cmd\git.exe') --version |
        Select-String -SimpleMatch 'git version 2.55.0.windows.3' | Out-Null
    Assert-LastExitCode 'PortableGit smoke test'

    Write-Host "Windows payload: $outputPath"
} finally {
    Remove-Item -LiteralPath $workDirectory -Recurse -Force `
        -ErrorAction SilentlyContinue
}
