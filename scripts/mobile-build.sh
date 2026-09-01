#!/usr/bin/env bash
#
# Build the Android APK in Docker and export it to ./dist/mobile. Local builds
# remain debug builds; release automation opts into signing explicitly.

set -euo pipefail

cd "$(dirname "$0")/.."

./scripts/test-mobile-native-chat.sh
target=apk
artifact=xd-mobile-debug.apk
cache_options=()
if [ "${XD_MOBILE_RELEASE:-0}" = 1 ]; then
  target=release-apk
  artifact=xd-mobile-release.apk
  # BuildKit deliberately excludes secret contents from cache keys. Force the
  # signing stage to rerun so a corrected or rotated key cannot export an APK
  # cached under an earlier signer.
  cache_options+=(--no-cache-filter release)
fi
mkdir -p dist/mobile
./scripts/runner-docker-build.sh \
  --target "$target" \
  "${cache_options[@]}" \
  --build-arg "XD_MOBILE_CHANNEL=${XD_MOBILE_CHANNEL:-release}" \
  --output type=local,dest=dist/mobile \
  --file mobile/Dockerfile \
  "$@" \
  mobile

echo
echo "Android APK ready:"
echo "  ./dist/mobile/$artifact"
