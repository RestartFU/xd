#!/usr/bin/env bash
#
# Build the Android debug APK in Docker and export it to ./dist/mobile.

set -euo pipefail

cd "$(dirname "$0")/.."

mkdir -p dist/mobile
./scripts/runner-docker-build.sh \
  --target apk \
  --output type=local,dest=dist/mobile \
  --file mobile/Dockerfile \
  "$@" \
  mobile

echo
echo "Android APK ready:"
echo "  ./dist/mobile/xd-mobile-debug.apk"
