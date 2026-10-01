#!/usr/bin/env bash
#
# Run shared mobile tests inside the Android build image.

set -euo pipefail

cd "$(dirname "$0")/.."

./scripts/test-mobile-session-routing.sh
./scripts/runner-docker-build.sh \
  --target test \
  --progress plain \
  --file mobile/Dockerfile \
  "$@" \
  mobile
