#!/usr/bin/env pwsh
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string] $Archive)

$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'smoke-windows.ps1 requires Windows.' }
$archivePath = (Resolve-Path -LiteralPath $Archive).Path
$expected = (Get-Content -LiteralPath "$archivePath.sha256" -Raw).Split(' ')[0].Trim()
$actual = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw 'Windows archive checksum mismatch.' }
$work = Join-Path ([IO.Path]::GetTempPath()) ('xd-windows-smoke-' + [guid]::NewGuid().ToString('N'))
try {
    Expand-Archive -LiteralPath $archivePath -DestinationPath $work
    $payload = @(Get-ChildItem -LiteralPath $work -Directory)
    if ($payload.Count -ne 1) { throw 'Archive must contain one application directory.' }
    $payload = $payload[0].FullName
    foreach ($file in @('xd.exe', 'libexec/xd-host-linux', 'README.txt',
                       'install-webview2.ps1', 'licenses/alacritty-terminal-LICENSE-APACHE')) {
        if (-not (Test-Path -LiteralPath (Join-Path $payload $file) -PathType Leaf)) {
            throw "Windows package is missing $file."
        }
    }
    $executable = Join-Path $payload 'xd.exe'
    $pe = [IO.File]::ReadAllBytes($executable)
    $peOffset = [BitConverter]::ToInt32($pe, 0x3c)
    if ($pe[0] -ne 0x4d -or $pe[1] -ne 0x5a -or
        [BitConverter]::ToUInt32($pe, $peOffset) -ne 0x00004550 -or
        [BitConverter]::ToUInt16($pe, $peOffset + 4) -ne 0x8664) {
        throw 'Windows frontend must be an x86_64 PE executable.'
    }
    $stdout = Join-Path $work 'version.stdout'
    $stderr = Join-Path $work 'version.stderr'
    $process = Start-Process -FilePath $executable -ArgumentList '--version' `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
    if (-not $process.WaitForExit(30000)) {
        $process.Kill()
        throw 'Windows version smoke timed out.'
    }
    if ($process.ExitCode -ne 0) {
        throw "Windows version smoke failed: $(Get-Content -LiteralPath $stderr -Raw)"
    }
    $version = (Get-Content -LiteralPath $stdout -Raw).Trim()
    if ($version -notmatch '^xd \d+\.\d+\.\d+') { throw "Unexpected version: $version" }
    Write-Host $version
    Write-Host 'Windows archive: checksum, layout, architecture, and executable smoke passed'
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
