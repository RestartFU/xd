#!/usr/bin/env bash
#
# Run the headless test suite in Docker.

set -euo pipefail

cd "$(dirname "$0")/.."

./scripts/runner-docker-build.sh --target test --progress plain "$@" .
