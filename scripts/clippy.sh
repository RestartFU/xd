#!/usr/bin/env bash
#
# Run clippy over every Rust crate in Docker.

set -euo pipefail

cd "$(dirname "$0")/.."

./scripts/runner-docker-build.sh --target clippy --progress plain "$@" .
