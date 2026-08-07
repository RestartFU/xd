#!/usr/bin/env bash
# Run a BuildKit build within the repository's runner CPU budget.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
jobs=$("$ROOT/scripts/runner-jobs.sh" --jobs)
cpuset=$("$ROOT/scripts/runner-jobs.sh" --cpuset)
arguments=(buildx build --build-arg "BUILD_JOBS=$jobs")

# Keep the job limit on older Buildx installations and add a hard shared CPU
# set wherever the installed BuildKit frontend supports resource controls.
buildx_help=$(docker buildx build --help)
if grep -q -- '--resource' <<< "$buildx_help"; then
  arguments+=(--resource "cpuset-cpus=$cpuset")
fi

exec docker "${arguments[@]}" "$@"
