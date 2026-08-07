#!/usr/bin/env pwsh
#
# Build and validate the complete native Rust/GPUI Windows x86_64 payload.
#
#   ./scripts/build-windows.ps1 -OutputDirectory windows-dist
#       [-Profile nightly|release]

# Run from a native x86_64 Windows PowerShell with Rust, CMake, and 7-Zip.
# The resulting tree is self-contained and is consumed by package-windows.ps1.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $OutputDirectory,

    [ValidateSet('nightly', 'release')]
    [string] $Profile = 'nightly'
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
$workDirectory = Join-Path ([IO.Path]::GetTempPath()) (
    'xd-windows-build-' + [guid]::NewGuid().ToString('N')
)
$cacheDirectory = Join-Path $repositoryRoot 'desktop\target\windows-assets'
$buildJobs = if ($env:XD_BUILD_JOBS -match '^[1-9][0-9]*$') {
    [Math]::Min([int] $env:XD_BUILD_JOBS, [Environment]::ProcessorCount)
} else {
    [Math]::Max(1, [Math]::Floor([Environment]::ProcessorCount * 0.75))
}
$env:CARGO_BUILD_JOBS = $buildJobs.ToString()
$env:CMAKE_BUILD_PARALLEL_LEVEL = $buildJobs.ToString()

$codexVersion = '0.146.0'
$codexAsset = 'codex-package-x86_64-pc-windows-msvc.tar.gz'
$codexSha256 = 'a945559cc0da3437c022d53e5f601f9e8c95980d717c9aad82997e4582ecd55e'
$claudeVersion = '2.1.220'
$claudeSha256 = 'af5bf1f1b2aadffc768eccd787084c6fdf9ba81624cbe96c1c6d9ac1a1550231'
$proxyVersion = '0.1.30'
$proxyAsset = 'claude-code-proxy-windows-amd64.zip'
$proxySha256 = '7ee1e9c275de326e97ea7914f9eafa74ed7fb6bfa60223e3fafc0e0daf02e233'
$gitVersion = '2.55.0.3'
$gitAsset = 'PortableGit-2.55.0.3-64-bit.7z.exe'
$gitSha256 = 'ab00566336b5472120f9a52d34f2e79c5406535792acb0548001ffd0bd090e5d'
$whisperVersion = '1.9.1'
$whisperSha256 = '147267177eef7b22ec3d2476dd514d1b12e160e176230b740e3d1bd600118447'

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
foreach ($command in @('cargo', 'cmake', 'git', 'tar')) {
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

    $codexArchive = Join-Path $cacheDirectory "$codexVersion-$codexAsset"
    $claudeBinary = Join-Path $cacheDirectory "claude-$claudeVersion.exe"
    $proxyArchive = Join-Path $cacheDirectory "$proxyVersion-$proxyAsset"
    $gitArchive = Join-Path $cacheDirectory "$gitVersion-$gitAsset"
    $whisperArchive = Join-Path $cacheDirectory "whisper.cpp-$whisperVersion.tar.gz"

    Invoke-CheckedDownload `
        -Uri "https://releases.openai.com/codex/releases/$codexVersion/$codexAsset" `
        -Destination $codexArchive -Sha256 $codexSha256
    Invoke-CheckedDownload `
        -Uri "https://downloads.claude.ai/claude-code-releases/$claudeVersion/win32-x64/claude.exe" `
        -Destination $claudeBinary -Sha256 $claudeSha256
    Invoke-CheckedDownload `
        -Uri "https://github.com/raine/claude-code-proxy/releases/download/v$proxyVersion/$proxyAsset" `
        -Destination $proxyArchive -Sha256 $proxySha256
    Invoke-CheckedDownload `
        -Uri "https://github.com/git-for-windows/git/releases/download/v2.55.0.windows.3/$gitAsset" `
        -Destination $gitArchive -Sha256 $gitSha256
    Invoke-CheckedDownload `
        -Uri "https://github.com/ggml-org/whisper.cpp/archive/refs/tags/v$whisperVersion.tar.gz" `
        -Destination $whisperArchive -Sha256 $whisperSha256

    Push-Location $repositoryRoot
    try {
        $commit = (& git rev-parse HEAD 2>$null)
        if ($LASTEXITCODE -ne 0) { $commit = '' }
        $env:XD_BUILD_PROFILE = $Profile
        $env:XD_COMMIT = $commit
        & cargo build --locked --release --manifest-path desktop/Cargo.toml
        Assert-LastExitCode 'desktop build'
        & cargo build --locked --release --manifest-path daemon-rs/Cargo.toml
        Assert-LastExitCode 'daemon build'
        & cargo build --release --manifest-path tls-proxy-rs/Cargo.toml
        Assert-LastExitCode 'TLS helper build'
    } finally {
        Pop-Location
    }

    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'desktop\target\release\xd-desktop.exe') `
        -Destination (Join-Path $outputPath 'bin\xd.exe')
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'daemon-rs\target\release\xd-daemon.exe') `
        -Destination (Join-Path $outputPath 'bin\xd-daemon.exe')
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'tls-proxy-rs\target\release\xd-tls-proxy.exe') `
        -Destination (Join-Path $outputPath 'bin\xd-tls-proxy.exe')
    Copy-Item -LiteralPath $claudeBinary `
        -Destination (Join-Path $outputPath 'bin\claude.exe')
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'data\fonts\DMSans-Variable.ttf') `
        -Destination (Join-Path $outputPath 'share\fonts\xd\DMSans-Variable.ttf')
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'data\licenses\claude-code-proxy-LICENSE') `
        -Destination (Join-Path $outputPath 'share\licenses\xd\claude-code-proxy-LICENSE')
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'scripts\install.ps1') `
        -Destination (Join-Path $outputPath 'bin\install.ps1')

    $codexDirectory = Join-Path $outputPath 'bin\codex-package'
    New-Item -ItemType Directory -Path $codexDirectory | Out-Null
    & tar -xzf $codexArchive -C $codexDirectory
    Assert-LastExitCode 'Codex extraction'

    $proxyDirectory = Join-Path $workDirectory 'proxy'
    Expand-Archive -LiteralPath $proxyArchive -DestinationPath $proxyDirectory
    $proxyBinary = Get-ChildItem -LiteralPath $proxyDirectory `
        -Filter 'claude-code-proxy.exe' -File -Recurse | Select-Object -First 1
    if ($null -eq $proxyBinary) { throw 'Claude proxy archive has no executable.' }
    Copy-Item -LiteralPath $proxyBinary.FullName `
        -Destination (Join-Path $outputPath 'bin\claude-code-proxy.exe')

    $gitDirectory = Join-Path $outputPath 'git'
    & $sevenZip x -y "-o$gitDirectory" $gitArchive | Out-Null
    Assert-LastExitCode 'PortableGit extraction'
    $postInstall = Join-Path $gitDirectory 'post-install.bat'
    if (Test-Path -LiteralPath $postInstall) {
        & (Join-Path $gitDirectory 'git-bash.exe') `
            --no-needs-console --hide --no-cd --command=post-install.bat
        Assert-LastExitCode 'PortableGit post-install'
    }

    $whisperSource = Join-Path $workDirectory 'whisper-source'
    $whisperBuild = Join-Path $workDirectory 'whisper-build'
    New-Item -ItemType Directory -Path $whisperSource | Out-Null
    & tar -xzf $whisperArchive -C $whisperSource --strip-components=1
    Assert-LastExitCode 'whisper.cpp extraction'
    & cmake -S $whisperSource -B $whisperBuild `
        -A x64 `
        -DCMAKE_BUILD_TYPE=Release `
        -DBUILD_SHARED_LIBS=OFF `
        -DWHISPER_BUILD_TESTS=OFF `
        -DWHISPER_BUILD_EXAMPLES=ON `
        -DWHISPER_BUILD_SERVER=ON `
        -DGGML_NATIVE=OFF `
        -DGGML_BACKEND_DL=OFF `
        -DGGML_OPENMP=OFF `
        -DGGML_CCACHE=OFF
    Assert-LastExitCode 'whisper.cpp configuration'
    & cmake --build $whisperBuild --config Release --target whisper-server `
        --parallel $buildJobs
    Assert-LastExitCode 'whisper.cpp build'
    $whisperServer = Get-ChildItem -LiteralPath $whisperBuild `
        -Filter 'whisper-server.exe' -File -Recurse | Select-Object -First 1
    if ($null -eq $whisperServer) { throw 'whisper-server.exe was not built.' }
    Copy-Item -LiteralPath $whisperServer.FullName `
        -Destination (Join-Path $outputPath 'bin\whisper-server-bin.exe')
    Copy-Item -LiteralPath (Join-Path $whisperSource 'LICENSE') `
        -Destination (Join-Path $outputPath 'share\licenses\xd\whisper.cpp-LICENSE')

    $required = @(
        'bin\xd.exe',
        'bin\xd-daemon.exe',
        'bin\xd-tls-proxy.exe',
        'bin\install.ps1',
        'bin\codex-package\bin\codex.exe',
        'bin\claude.exe',
        'bin\claude-code-proxy.exe',
        'bin\whisper-server-bin.exe',
        'git\cmd\git.exe',
        'git\mingw64\libexec\git-core\git-remote-https.exe',
        'git\mingw64\etc\ssl\certs\ca-bundle.crt'
    )
    foreach ($relativePath in $required) {
        if (-not (Test-Path -LiteralPath (Join-Path $outputPath $relativePath))) {
            throw "Windows payload is missing $relativePath."
        }
    }

    & (Join-Path $outputPath 'bin\xd.exe') --version
    Assert-LastExitCode 'xd smoke test'
    & (Join-Path $outputPath 'bin\codex-package\bin\codex.exe') --version |
        Select-String -SimpleMatch $codexVersion | Out-Null
    Assert-LastExitCode 'Codex smoke test'
    & (Join-Path $outputPath 'bin\claude.exe') --version |
        Select-String -SimpleMatch $claudeVersion | Out-Null
    Assert-LastExitCode 'Claude smoke test'
    & (Join-Path $outputPath 'bin\claude-code-proxy.exe') --version |
        Select-String -SimpleMatch $proxyVersion | Out-Null
    Assert-LastExitCode 'Claude proxy smoke test'
    & (Join-Path $outputPath 'bin\whisper-server-bin.exe') --help *> $null
    Assert-LastExitCode 'whisper.cpp smoke test'
    & (Join-Path $outputPath 'git\cmd\git.exe') --version |
        Select-String -SimpleMatch 'git version 2.55.0.windows.3' | Out-Null
    Assert-LastExitCode 'PortableGit smoke test'

    Write-Host "Windows payload: $outputPath"
} finally {
    Remove-Item -LiteralPath $workDirectory -Recurse -Force `
        -ErrorAction SilentlyContinue
}
