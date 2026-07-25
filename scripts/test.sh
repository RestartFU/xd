#!/usr/bin/env bash
#
# Run the headless test suite in Docker.

set -euo pipefail

cd "$(dirname "$0")/.."

docker build --target test --progress plain "$@" .
