#!/usr/bin/env bash
#
# Run shared mobile tests inside the Android build image.

set -euo pipefail

cd "$(dirname "$0")/.."

docker build \
  --target test \
  --progress plain \
  --file mobile/Dockerfile \
  "$@" \
  mobile
