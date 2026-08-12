#!/usr/bin/env bash
#
# Build xd in Docker and export the runnable bundle to ./dist.
# The only host requirement is Docker.
#
# PROFILE=dev builds from the prerelease (nightly) Docker profile, then marks
# the bundle for the isolated dev installer. The public launcher selects its
# final identity from the directory it is unpacked into (`xd-dev`).

set -euo pipefail

cd "$(dirname "$0")/.."

# So the bundle can say which commit it is, which is the only way to tell one
# nightly from the next. Empty outside a checkout, which is fine.
COMMIT=$(git rev-parse HEAD 2>/dev/null || true)

PROFILE="${PROFILE:-}"
PROFILE_ARGS=()
case "$PROFILE" in
  '') ;;
  dev)
    # The Docker image has release/nightly build flavours. Dev uses the same
    # prerelease bits but gets a third runtime identity from the launcher and
    # installer rather than changing either established image profile.
    PROFILE_ARGS=(--build-arg PROFILE=nightly)
    ;;
  nightly)
    PROFILE_ARGS=(--build-arg PROFILE=nightly)
    ;;
  release|default)
    PROFILE_ARGS=(--build-arg PROFILE=default)
    ;;
  *)
    echo "build: PROFILE must be dev, nightly, or release" >&2
    exit 1
    ;;
esac

rm -rf dist
./scripts/runner-docker-build.sh \
  --target bundle \
  --build-arg COMMIT="$COMMIT" \
  "${PROFILE_ARGS[@]}" \
  --output "type=local,dest=dist" \
  "$@" \
  .

if [ "$PROFILE" = dev ]; then
  install -d dist/share/xd
  printf '%s\n' dev > dist/share/xd/profile
fi

echo
echo "Bundle ready. Run it with:"
echo "  ./dist/xd.sh"
