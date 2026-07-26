#!/usr/bin/env bash
#
# Build xd in Docker and export the runnable bundle to ./dist.
# The only host requirement is Docker.

set -euo pipefail

cd "$(dirname "$0")/.."

rm -rf dist
docker buildx build \
  --target bundle \
  --output "type=local,dest=dist" \
  "$@" \
  .

echo
echo "Bundle ready. Run it with:"
echo "  ./dist/xd.sh"
