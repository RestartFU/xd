#!/bin/sh
# Run against the assembled library closure, without a display or network.
set -eu
bundle=$(CDPATH='' cd -- "${1:?bundle directory}" && pwd -P)
probe=${2:?TLS probe executable}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
mkdir -p "$work/relocated bundle/etc/ssl/certs"
openssl="$bundle/libexec/openssl"
"$openssl" req -x509 -newkey rsa:2048 -nodes -days 1 \
  -subj /CN=Bundle-Test-CA -keyout "$work/ca.key" -out "$work/ca.pem" 2>/dev/null
"$openssl" req -newkey rsa:2048 -nodes -subj /CN=browser.example \
  -keyout "$work/server.key" -out "$work/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:browser.example\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' > "$work/extensions"
"$openssl" x509 -req -in "$work/server.csr" -CA "$work/ca.pem" \
  -CAkey "$work/ca.key" -CAcreateserial -days 1 -extfile "$work/extensions" \
  -out "$work/server.pem" 2>/dev/null
run_probe() {
  XD_BROWSER_BUNDLE_ROOT="$work/relocated bundle" \
    GIO_MODULE_DIR="$bundle/lib/gio/modules" "$probe" "$@"
}
cp "$work/ca.pem" "$work/relocated bundle/etc/ssl/certs/ca-certificates.crt"
run_probe "$work/server.pem" browser.example 0
# G_TLS_CERTIFICATE_BAD_IDENTITY = 2.
run_probe "$work/server.pem" other.example 2
cp "$bundle/etc/ssl/certs/ca-certificates.crt" \
  "$work/relocated bundle/etc/ssl/certs/ca-certificates.crt"
# G_TLS_CERTIFICATE_UNKNOWN_CA = 1.
run_probe "$work/server.pem" browser.example 1
echo 'browser TLS trust, hostname rejection, and unknown CA rejection: ok'
