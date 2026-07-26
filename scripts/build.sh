#!/usr/bin/env bash
#
# Build xd in Docker and export the runnable bundle to ./dist.
# The only host requirement is Docker.

set -euo pipefail

cd "$(dirname "$0")/.."

# So the bundle can say which commit it is, which is the only way to tell one
# nightly from the next. Empty outside a checkout, which is fine.
COMMIT=$(git rev-parse --short HEAD 2>/dev/null || true)

rm -rf dist
docker buildx build \
  --target bundle \
  --build-arg COMMIT="$COMMIT" \
  --output "type=local,dest=dist" \
  "$@" \
  .

echo
echo "Bundle ready. Run it with:"
echo "  ./dist/xd.sh"
