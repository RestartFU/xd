#!/usr/bin/env bash
#
# Build and validate the complete Windows x86_64 Crystal payload.
#
#   build-windows.sh <output-directory> [nightly|release]
#
# Run inside an MSYS2 UCRT64 shell. Build dependencies come from the runner;
# every runtime DLL, Git, assistant, and speech tool is copied into the result.

set -euo pipefail

OUT="${1:?empty output directory}"
PROFILE="${2:-nightly}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CRYSTAL_VERSION=1.21.0
CRYSTAL_ASSET=crystal-1.21.0-windows-x86_64-gnu-unsupported.zip
CRYSTAL_SHA256=d0f6f50b16720b240a4c49563c7670ddf1a790fd84c04b29d09818cd9b20608f
CRYSTAL_URL="https://github.com/crystal-lang/crystal/releases/download/$CRYSTAL_VERSION/$CRYSTAL_ASSET"

case "$(uname -s):$(uname -m):${MSYSTEM:-}" in
  MINGW*:x86_64:UCRT64|MSYS*:x86_64:UCRT64) ;;
  *)
    echo "build-windows: MSYS2 UCRT64 on Windows x86_64 is required" >&2
    exit 1
    ;;
esac
case "$PROFILE" in
  nightly) CRYSTAL_PROFILE=nightly ;;
  release) CRYSTAL_PROFILE=default ;;
  *)
    echo "build-windows: profile must be nightly or release" >&2
    exit 1
    ;;
esac

if [ -d "$OUT" ] && [ -n "$(find "$OUT" -mindepth 1 -print -quit)" ]; then
  echo "build-windows: output directory must be empty" >&2
  exit 1
fi

for command in cmake curl glib-compile-schemas pkg-config unzip; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "build-windows: $command is required" >&2
    exit 1
  }
done
for package in bdw-gc gtk4 libadwaita-1 portaudio-2.0 sqlite3; do
  pkg-config --exists "$package" || {
    echo "build-windows: pkg-config package missing: $package" >&2
    exit 1
  }
done

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/xd-windows-build.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT INT TERM

curl --fail --location --silent --show-error \
  "$CRYSTAL_URL" --output "$WORK/$CRYSTAL_ASSET"
printf '%s  %s\n' "$CRYSTAL_SHA256" "$WORK/$CRYSTAL_ASSET" |
  sha256sum --check
unzip -q "$WORK/$CRYSTAL_ASSET" -d "$WORK/crystal"
export PATH="$WORK/crystal/bin:$PATH"
crystal --version | grep -F "Crystal $CRYSTAL_VERSION"

cd "$ROOT"
shards install --production --frozen
./bin/gi-crystal

COMMIT="$(git rev-parse --short HEAD 2>/dev/null || true)"
XD_BUILD_PROFILE="$CRYSTAL_PROFILE" XD_BUILD_COMMIT="$COMMIT" \
  crystal spec \
    spec/xd/git_path_spec.cr \
    spec/xd/native_bundle_spec.cr \
    --error-trace
XD_BUILD_PROFILE="$CRYSTAL_PROFILE" XD_BUILD_COMMIT="$COMMIT" \
  crystal build src/xd.cr \
    --release \
    --no-debug \
    --link-flags "-Wl,--subsystem,windows" \
    -o "$WORK/xd.exe"
"$WORK/xd.exe" --bundle-runtime | grep -Fx crystal

scripts/stage-native.sh "$WORK/xd.exe" "$WORK/stage" "$PROFILE"
scripts/fetch-native-agents.sh windows-x86_64 "$WORK/stage"
scripts/fetch-windows-git.sh "$WORK/stage"
scripts/build-native-whisper.sh windows-x86_64 "$WORK/stage"
scripts/bundle-windows.sh "$WORK/stage" "$OUT"

"$OUT/bin/xd.exe" --version
printf 'Windows payload: %s\n' "$OUT"
