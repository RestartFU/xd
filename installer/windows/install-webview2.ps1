# Install Microsoft's Evergreen WebView2 Runtime only when explicitly invoked.
# Distribution guidance: https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution
[CmdletBinding()]
param([switch] $CheckOnly)

$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'WebView2 installation requires Windows.' }

function Get-WebView2Version {
    $client = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    foreach ($key in @("HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$client",
                       "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$client",
                       "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$client")) {
        $value = Get-ItemProperty -LiteralPath $key -Name pv -ErrorAction SilentlyContinue
        $version = $null
        if ($null -ne $value -and [Version]::TryParse([string]$value.pv, [ref]$version) -and
            $version -gt [Version]'0.0.0.0') { return $version }
    }
    return $null
}

$installed = Get-WebView2Version
if ($null -ne $installed) {
    Write-Host "WebView2 Runtime $installed is installed."
    return
}
if ($CheckOnly) { throw 'WebView2 Runtime is missing. Run install-webview2.ps1 to install it.' }
$bootstrapper = Join-Path ([IO.Path]::GetTempPath()) ('xd-webview2-' + [guid]::NewGuid().ToString('N') + '.exe')
try {
    Invoke-WebRequest -UseBasicParsing -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' `
        -OutFile $bootstrapper
    $signature = Get-AuthenticodeSignature -LiteralPath $bootstrapper
    if ($signature.Status -ne 'Valid' -or
        $signature.SignerCertificate.Subject -notmatch '(^|,\s*)O=Microsoft Corporation(,|$)') {
        throw 'The WebView2 bootstrapper does not have a valid Microsoft signature.'
    }
    $process = Start-Process -FilePath $bootstrapper -ArgumentList '/silent', '/install' -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "WebView2 installation failed with exit code $($process.ExitCode)." }
    $installed = Get-WebView2Version
    if ($null -eq $installed) { throw 'WebView2 installation finished but no Runtime was found.' }
    Write-Host "WebView2 Runtime $installed is installed."
} finally {
    Remove-Item -LiteralPath $bootstrapper -Force -ErrorAction SilentlyContinue
}
