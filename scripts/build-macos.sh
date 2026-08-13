#!/usr/bin/env bash
#
# Build and pack the native Rust/GPUI app on the current Mac.
#
#   PROFILE=dev ./scripts/build-macos.sh
#   PROFILE=nightly ./scripts/build-macos.sh
#   PROFILE=release ./scripts/build-macos.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${PROFILE:-release}"
BUILD_JOBS="${XD_BUILD_JOBS:-$("$ROOT/scripts/runner-jobs.sh" --jobs)}"
export CARGO_BUILD_JOBS="$BUILD_JOBS"
export CMAKE_BUILD_PARALLEL_LEVEL="$BUILD_JOBS"

[ "$(uname -s)" = Darwin ] || {
  echo "build-macos: macOS is required" >&2
  exit 1
}

case "$PROFILE" in
  dev)
    BUNDLE_NAME=xd-dev
    DISPLAY_NAME='xd (Dev)'
    APP_ID=com.restartfu.Xd.Dev
    ;;
  nightly)
    BUNDLE_NAME=xd-nightly
    DISPLAY_NAME='xd (Nightly)'
    APP_ID=com.restartfu.Xd.Nightly
    ;;
  release|default)
    PROFILE=release
    BUNDLE_NAME=xd
    DISPLAY_NAME=xd
    APP_ID=com.restartfu.Xd
    ;;
  *)
    echo "build-macos: PROFILE must be dev, nightly, or release" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  arm64|aarch64)
    ARCH=arm64
    ;;
  x86_64|amd64)
    ARCH=x86_64
    ;;
  *)
    echo "build-macos: unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

for command in cargo codesign iconutil rsvg-convert shasum sips; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "build-macos: $command is required" >&2
    exit 1
  }
done

VERSION=$("$ROOT/scripts/bump-version.sh" --current)
COMMIT=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)
BUILD_VERSION=$VERSION
if [ "$PROFILE" != release ]; then
  BUILD_VERSION=$(git -C "$ROOT" show -s --format=%ct HEAD 2>/dev/null || true)
  case "$BUILD_VERSION" in
    ''|*[!0-9]*) BUILD_VERSION=$VERSION ;;
  esac
fi
OUT="$ROOT/dist/macos"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/xd-macos-build.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

mkdir -p "$OUT"

cd "$ROOT"
DESKTOP_PROFILE=$PROFILE
[ "$DESKTOP_PROFILE" != dev ] || DESKTOP_PROFILE=nightly
XD_BUILD_PROFILE="$DESKTOP_PROFILE" XD_COMMIT="$COMMIT" \
  cargo build --locked --release --manifest-path desktop/Cargo.toml
XD_COMMIT="$COMMIT" cargo build --locked --release --manifest-path host/Cargo.toml

APP="$OUT/$BUNDLE_NAME.app"
rm -rf "$APP"
mkdir -p \
  "$APP/Contents/MacOS" \
  "$APP/Contents/Resources/libexec" \
  "$APP/Contents/Resources/fonts" \
  "$APP/Contents/Resources/licenses"

install -m0755 desktop/target/release/xd-desktop \
  "$APP/Contents/MacOS/xd-desktop"
install -m0755 installer/macos/xd-rust-launcher.sh \
  "$APP/Contents/MacOS/xd"
install -m0755 host/target/release/xd-host \
  "$APP/Contents/Resources/libexec/xd-host"
install -m0755 scripts/install-macos.sh \
  "$APP/Contents/Resources/libexec/install.sh"
install -m0644 data/fonts/DMSans-Variable.ttf \
  "$APP/Contents/Resources/fonts/DMSans-Variable.ttf"
install -m0644 data/licenses/alacritty-terminal-LICENSE-APACHE \
  "$APP/Contents/Resources/licenses/alacritty-terminal-LICENSE-APACHE"

sed \
  -e "s|@BUNDLE_NAME@|$BUNDLE_NAME|g" \
  -e "s|@DISPLAY_NAME@|$DISPLAY_NAME|g" \
  -e "s|@APP_ID@|$APP_ID|g" \
  -e "s|@VERSION@|$VERSION|g" \
  -e "s|@BUILD_VERSION@|$BUILD_VERSION|g" \
  installer/macos/Info.plist.in > "$APP/Contents/Info.plist"

# Finder requires an icns file. Generate it from the same vector artwork used
# by Linux so both platforms keep one source of truth.
ICONSET="$WORK/xd.iconset"
mkdir -p "$ICONSET"
rsvg-convert -w 1024 -h 1024 \
  data/icons/hicolor/scalable/apps/com.restartfu.Xd.svg \
  > "$WORK/icon-1024.png"
for specification in \
  "16 icon_16x16.png" "32 icon_16x16@2x.png" \
  "32 icon_32x32.png" "64 icon_32x32@2x.png" \
  "128 icon_128x128.png" "256 icon_128x128@2x.png" \
  "256 icon_256x256.png" "512 icon_256x256@2x.png" \
  "512 icon_512x512.png" "1024 icon_512x512@2x.png"; do
  size=${specification%% *}
  name=${specification#* }
  sips -z "$size" "$size" "$WORK/icon-1024.png" \
    --out "$ICONSET/$name" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/xd.icns"

codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

"$APP/Contents/MacOS/xd" --version

ASSET="$BUNDLE_NAME-macos-$ARCH.zip"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$OUT/$ASSET"
(
  cd "$OUT"
  shasum -a 256 "$ASSET" > "$ASSET.sha256"
)

printf 'macOS artifact: %s\n' "$OUT/$ASSET"
