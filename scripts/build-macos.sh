#!/usr/bin/env bash
#
# Build, validate, and pack the complete Apple Silicon Crystal app.
#
#   build-macos.sh <output-directory> [nightly|release]
#
# Build dependencies are supplied by the native runner. Runtime dependencies,
# Git, agents, whisper.cpp, and OpenSSL are all copied into the resulting app.

set -euo pipefail

OUT="${1:?empty output directory}"
PROFILE="${2:-nightly}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CRYSTAL_VERSION=1.21.0
CRYSTAL_ASSET=crystal-1.21.0-1-darwin-universal.tar.gz
CRYSTAL_SHA256=7fc4af56b0cb5c7ea5703f744c6629bb19ff36ba3abbf232d50e40c39a20ee16
CRYSTAL_URL="https://github.com/crystal-lang/crystal/releases/download/$CRYSTAL_VERSION/$CRYSTAL_ASSET"

[ "$(uname -s)" = Darwin ] || {
  echo "build-macos: macOS is required" >&2
  exit 1
}
[ "$(uname -m)" = arm64 ] || {
  echo "build-macos: Apple Silicon is required" >&2
  exit 1
}
case "$PROFILE" in
  nightly)
    BUNDLE_NAME=xd-nightly
    ASSET=xd-nightly-macos-arm64.zip
    CRYSTAL_PROFILE=nightly
    ;;
  release)
    BUNDLE_NAME=xd
    ASSET=xd-macos-arm64.zip
    CRYSTAL_PROFILE=default
    ;;
  *)
    echo "build-macos: profile must be nightly or release" >&2
    exit 1
    ;;
esac

if [ -d "$OUT" ] && [ -n "$(find "$OUT" -mindepth 1 -print -quit)" ]; then
  echo "build-macos: output directory must be empty" >&2
  exit 1
fi

for command in curl ditto pkg-config shards shasum tar; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "build-macos: $command is required" >&2
    exit 1
  }
done
BUILD_LIBRARY_PATH=
for package in \
  gobject-introspection-1.0 \
  gtk4 \
  libadwaita-1 \
  vte-2.91-gtk4 \
  portaudio-2.0 \
  sqlite3; do
  pkg-config --exists "$package" || {
    echo "build-macos: pkg-config package missing: $package" >&2
    exit 1
  }
  package_libdir="$(pkg-config --variable=libdir "$package")"
  if [ -n "$package_libdir" ]; then
    case ":$BUILD_LIBRARY_PATH:" in
      *":$package_libdir:"*) ;;
      *)
        BUILD_LIBRARY_PATH="${BUILD_LIBRARY_PATH:+$BUILD_LIBRARY_PATH:}$package_libdir"
        ;;
    esac
  fi
done

# Crystal link annotations name native libraries directly. Homebrew keeps
# several of them in versioned Cellar paths outside clang's default search.
export LIBRARY_PATH="$BUILD_LIBRARY_PATH${LIBRARY_PATH:+:$LIBRARY_PATH}"

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/xd-macos-build.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT INT TERM

curl --fail --location --silent --show-error \
  "$CRYSTAL_URL" --output "$WORK/$CRYSTAL_ASSET"
printf '%s  %s\n' "$CRYSTAL_SHA256" "$WORK/$CRYSTAL_ASSET" |
  shasum -a 256 --check
tar -xzf "$WORK/$CRYSTAL_ASSET" -C "$WORK"
export PATH="$WORK/crystal-1.21.0-1/bin:$PATH"
crystal --version | grep -F "Crystal $CRYSTAL_VERSION"

cd "$ROOT"
shards install --production --frozen
./bin/gi-crystal bindings/vte/binding-unix.yml

COMMIT="$(git rev-parse --short HEAD 2>/dev/null || true)"
XD_BUILD_PROFILE="$CRYSTAL_PROFILE" XD_BUILD_COMMIT="$COMMIT" \
  crystal spec --error-trace
XD_BUILD_PROFILE="$CRYSTAL_PROFILE" XD_BUILD_COMMIT="$COMMIT" \
  crystal build src/xd.cr --release --no-debug -o "$WORK/xd"
"$WORK/xd" --bundle-runtime | grep -Fx crystal

scripts/stage-native.sh "$WORK/xd" "$WORK/stage" "$PROFILE"
scripts/fetch-native-agents.sh macos-arm64 "$WORK/stage"
scripts/build-macos-git.sh "$WORK/stage"
scripts/build-native-whisper.sh macos-arm64 "$WORK/stage"
scripts/build-macos-openssl.sh "$WORK/stage"
scripts/bundle-macos.sh "$WORK/stage" "$OUT" "$PROFILE"

ditto -c -k --keepParent \
  "$OUT/$BUNDLE_NAME.app" \
  "$OUT/$ASSET"
(
  cd "$OUT"
  shasum -a 256 "$ASSET" > "$ASSET.sha256"
)

printf 'macOS artifact: %s\n' "$OUT/$ASSET"
