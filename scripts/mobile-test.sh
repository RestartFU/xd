#!/usr/bin/env bash
#
# Run shared mobile tests inside the Android build image.

set -euo pipefail

cd "$(dirname "$0")/.."

./scripts/test-mobile-native-chat.sh
./scripts/runner-docker-build.sh \
  --target test \
  --progress plain \
  --file mobile/Dockerfile \
  "$@" \
  mobile
