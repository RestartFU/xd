#!/usr/bin/env bash
#
# Build the signed nightly Android APK in Docker and export it to ./dist/mobile.
#
# Signing material enters the build only through BuildKit secret mounts, so it
# never lands in an image layer. Required environment:
#
#   XD_ANDROID_KEYSTORE           path to a PKCS#12 keystore
#   XD_ANDROID_KEYSTORE_PASSWORD  its password
#
# Optional, for a keystore whose key differs from its store:
#
#   XD_ANDROID_KEY_ALIAS          defaults to xd-nightly
#   XD_ANDROID_KEY_PASSWORD       defaults to the keystore password
#
# XD_ANDROID_VERSION_CODE must increase on every published build or Android
# will refuse the update.

set -euo pipefail

cd "$(dirname "$0")/.."

: "${XD_ANDROID_KEYSTORE:?a PKCS#12 keystore path is required}"
: "${XD_ANDROID_KEYSTORE_PASSWORD:?the keystore password is required}"

if [ ! -f "$XD_ANDROID_KEYSTORE" ]; then
  echo "xd: no keystore at $XD_ANDROID_KEYSTORE" >&2
  exit 1
fi

version_code="${XD_ANDROID_VERSION_CODE:-1}"
key_alias="${XD_ANDROID_KEY_ALIAS:-xd-nightly}"

secrets=(
  --secret "id=android_keystore,src=${XD_ANDROID_KEYSTORE}"
  --secret "id=android_keystore_password,env=XD_ANDROID_KEYSTORE_PASSWORD"
)
if [ -n "${XD_ANDROID_KEY_PASSWORD:-}" ]; then
  secrets+=(--secret "id=android_key_password,env=XD_ANDROID_KEY_PASSWORD")
fi

mkdir -p dist/mobile
docker buildx build \
  --target nightly-apk \
  --build-arg "ANDROID_VERSION_CODE=${version_code}" \
  --build-arg "ANDROID_SIGNING_ALIAS=${key_alias}" \
  "${secrets[@]}" \
  --output type=local,dest=dist/mobile \
  --file mobile/Dockerfile \
  "$@" \
  mobile

cd dist/mobile
sha256sum xd-nightly-android.apk > xd-nightly-android.apk.sha256

echo
echo "Signed nightly APK ready:"
echo "  ./dist/mobile/xd-nightly-android.apk"
