#!/usr/bin/env bash
#
# Check the formatting of every Rust crate and run clippy over it, in Docker.

set -euo pipefail

cd "$(dirname "$0")/.."

./scripts/runner-docker-build.sh --target clippy --progress plain "$@" .
