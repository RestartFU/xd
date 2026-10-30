#!/usr/bin/env bash

set -euo pipefail

apk=${1:?APK path}
apksigner=${2:?apksigner path}
aapt2=${3:?aapt2 path}

certificates=$("$apksigner" verify --verbose --print-certs "$apk")
printf '%s\n' "$certificates"

debug_digest=fac77d3eb6b167cd2334a1497f9b5606af120af178e5d90a7e429586e6a7fc20
release_digest=$(printf '%s\n' "$certificates" |
  sed -n 's/^Signer #1 certificate SHA-256 digest: //p' |
  tr '[:upper:]' '[:lower:]' |
  head -n 1)
test -n "$release_digest" || {
  echo "Android release has no signer certificate digest" >&2
  exit 1
}
test "$release_digest" != "$debug_digest" || {
  echo "Android release is signed by the public debug key" >&2
  exit 1
}

manifest=$("$aapt2" dump xmltree "$apk" --file AndroidManifest.xml)
if printf '%s\n' "$manifest" |
    grep -Eq 'android:debuggable.*(0xffffffff|true)'; then
  echo "Android release is debuggable" >&2
  exit 1
fi

echo "Android release signing and debuggability: ok"
