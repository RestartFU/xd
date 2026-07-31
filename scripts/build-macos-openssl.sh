#!/usr/bin/env sh
#
# Build a static OpenSSL CLI for Apple Silicon certificate generation.
#
#   build-macos-openssl.sh <staging-directory>

set -eu

STAGE=${1:?staging directory}
OPENSSL_VERSION=3.5.7
OPENSSL_SHA256=a8c0d28a529ca480f9f36cf5792e2cd21984552a3c8e4aa11a24aa31aeac98e8
OPENSSL_URL="https://www.openssl.org/source/openssl-$OPENSSL_VERSION.tar.gz"

[ "$(uname -s)" = Darwin ] || {
  echo "build-macos-openssl: macOS is required" >&2
  exit 1
}
[ "$(uname -m)" = arm64 ] || {
  echo "build-macos-openssl: Apple Silicon is required" >&2
  exit 1
}
[ ! -e "$STAGE/libexec/openssl" ] || {
  echo "build-macos-openssl: destination already exists" >&2
  exit 1
}

for command in cc curl make perl shasum tar; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "build-macos-openssl: $command is required" >&2
    exit 1
  }
done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/xd-macos-openssl.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

curl --fail --location --silent --show-error \
  "$OPENSSL_URL" --output "$WORK/openssl.tar.gz"
printf '%s  %s\n' "$OPENSSL_SHA256" "$WORK/openssl.tar.gz" |
  shasum -a 256 --check
mkdir "$WORK/source"
tar -xzf "$WORK/openssl.tar.gz" \
  -C "$WORK/source" --strip-components=1

(
  cd "$WORK/source"
  ./Configure \
    darwin64-arm64-cc \
    no-shared \
    no-module \
    no-tests \
    --prefix=/ \
    --openssldir=/etc/ssl
  jobs=$(sysctl -n hw.logicalcpu 2>/dev/null || printf '4')
  make -j"$jobs" build_sw
  make DESTDIR="$WORK/install" install_sw
  make DESTDIR="$WORK/install" install_ssldirs
)

mkdir -p "$STAGE/libexec" "$STAGE/etc/ssl"
install -m0755 "$WORK/install/bin/openssl" "$STAGE/libexec/openssl"
install -m0644 \
  "$WORK/install/etc/ssl/openssl.cnf" \
  "$STAGE/etc/ssl/openssl.cnf"

OPENSSL_CONF="$STAGE/etc/ssl/openssl.cnf" \
  "$STAGE/libexec/openssl" version |
  grep -F "OpenSSL $OPENSSL_VERSION"

printf 'macOS static OpenSSL: %s\n' "$OPENSSL_VERSION"
