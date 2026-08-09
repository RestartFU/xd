#!/usr/bin/env bash
#
# Build and pack the native Rust/GPUI app on the current Mac.
#
#   PROFILE=nightly ./scripts/build-macos.sh
#   PROFILE=release ./scripts/build-macos.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${PROFILE:-release}"
BUILD_JOBS="${XD_BUILD_JOBS:-$("$ROOT/scripts/runner-jobs.sh" --jobs)}"
export CARGO_BUILD_JOBS="$BUILD_JOBS"
export CMAKE_BUILD_PARALLEL_LEVEL="$BUILD_JOBS"
CODEX_VERSION=0.146.0
CLAUDE_VERSION=2.1.220
CLAUDE_PROXY_VERSION=0.1.30
WHISPER_VERSION=1.9.1
WHISPER_SHA256=147267177eef7b22ec3d2476dd514d1b12e160e176230b740e3d1bd600118447

[ "$(uname -s)" = Darwin ] || {
  echo "build-macos: macOS is required" >&2
  exit 1
}

case "$PROFILE" in
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
    echo "build-macos: PROFILE must be nightly or release" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  arm64|aarch64)
    ARCH=arm64
    CODEX_ARCH=aarch64
    CLAUDE_ARCH=darwin-arm64
    PROXY_ARCH=arm64
    CODEX_SHA256=cd961b480f6dfc4703bd244601f1927231fa31a587cb9046ccdffa6c4c29e7d5
    CLAUDE_SHA256=8addc857f3fe64d5a0368af9ee50321b50afb4a6918ba3ef018ab84f5dbbe081
    PROXY_SHA256=23732b1a189db57ce24fd80e0f181a1c199fbba17542cd02008f02fb09630d32
    ;;
  x86_64|amd64)
    ARCH=x86_64
    CODEX_ARCH=x86_64
    CLAUDE_ARCH=darwin-x64
    PROXY_ARCH=amd64
    CODEX_SHA256=f72f5ab71729e90b8e86343e9199c0f7a7eebbca5d6b1fc4cfcdaf35a3e5b641
    CLAUDE_SHA256=dca7be0aa7d3d924836d440e0c6d8e3d47ef3c8e61fa5809b54b9017170ce2f3
    PROXY_SHA256=f01d686da6ae899adf9887744ad8d72fc231d0b17b789d2ded3ac52f4a192059
    ;;
  *)
    echo "build-macos: unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

for command in cargo cmake codesign curl iconutil rsvg-convert shasum sips tar; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "build-macos: $command is required" >&2
    exit 1
  }
done

VERSION=$("$ROOT/scripts/bump-version.sh" --current)
COMMIT=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)
BUILD_VERSION=$VERSION
if [ "$PROFILE" = nightly ]; then
  BUILD_VERSION=$(git -C "$ROOT" show -s --format=%ct HEAD 2>/dev/null || true)
  case "$BUILD_VERSION" in
    ''|*[!0-9]*) BUILD_VERSION=$VERSION ;;
  esac
fi
OUT="$ROOT/dist/macos"
CACHE="$ROOT/.build-cache/macos-assets/$ARCH"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/xd-macos-build.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

mkdir -p "$OUT" "$CACHE"

fetch () {
  url=$1
  destination=$2
  checksum=$3
  if [ -f "$destination" ] \
    && printf '%s  %s\n' "$checksum" "$destination" | shasum -a 256 -c - >/dev/null 2>&1; then
    return
  fi
  partial="$destination.partial.$$"
  curl --fail --location --retry 3 --silent --show-error \
    "$url" --output "$partial"
  printf '%s  %s\n' "$checksum" "$partial" | shasum -a 256 -c -
  mv "$partial" "$destination"
}

CODEX_ARCHIVE="$CACHE/codex-package-$CODEX_VERSION.tar.gz"
CLAUDE_BINARY="$CACHE/claude-$CLAUDE_VERSION"
PROXY_ARCHIVE="$CACHE/claude-code-proxy-$CLAUDE_PROXY_VERSION.tar.gz"
WHISPER_ARCHIVE="$CACHE/whisper.cpp-$WHISPER_VERSION.tar.gz"
WHISPER_CACHE="$ROOT/.build-cache/macos-whisper/$ARCH/$WHISPER_VERSION"

fetch \
  "https://releases.openai.com/codex/releases/$CODEX_VERSION/codex-package-$CODEX_ARCH-apple-darwin.tar.gz" \
  "$CODEX_ARCHIVE" "$CODEX_SHA256"
fetch \
  "https://downloads.claude.ai/claude-code-releases/$CLAUDE_VERSION/$CLAUDE_ARCH/claude" \
  "$CLAUDE_BINARY" "$CLAUDE_SHA256"
fetch \
  "https://github.com/raine/claude-code-proxy/releases/download/v$CLAUDE_PROXY_VERSION/claude-code-proxy-darwin-$PROXY_ARCH.tar.gz" \
  "$PROXY_ARCHIVE" "$PROXY_SHA256"
fetch \
  "https://github.com/ggml-org/whisper.cpp/archive/refs/tags/v$WHISPER_VERSION.tar.gz" \
  "$WHISPER_ARCHIVE" "$WHISPER_SHA256"

if [ ! -x "$WHISPER_CACHE/whisper-server-bin" ]; then
  mkdir -p "$WORK/whisper-source" "$WHISPER_CACHE"
  tar -xzf "$WHISPER_ARCHIVE" \
    -C "$WORK/whisper-source" --strip-components=1
  cmake \
    -S "$WORK/whisper-source" \
    -B "$WORK/whisper-build" \
    -DCMAKE_BUILD_TYPE=Release \
    -DBUILD_SHARED_LIBS=OFF \
    -DWHISPER_BUILD_TESTS=OFF \
    -DWHISPER_BUILD_EXAMPLES=ON \
    -DWHISPER_BUILD_SERVER=ON \
    -DGGML_NATIVE=OFF \
    -DGGML_BACKEND_DL=OFF \
    -DGGML_OPENMP=OFF \
    -DGGML_CCACHE=OFF
  cmake --build "$WORK/whisper-build" \
    --target whisper-server --parallel "$BUILD_JOBS"
  install -m0755 "$WORK/whisper-build/bin/whisper-server" \
    "$WHISPER_CACHE/whisper-server-bin"
fi
"$WHISPER_CACHE/whisper-server-bin" --help >/dev/null

cd "$ROOT"
XD_COMMIT="$COMMIT" cargo build --locked --release --manifest-path desktop/Cargo.toml
XD_COMMIT="$COMMIT" cargo build --locked --release --manifest-path daemon-rs/Cargo.toml
cargo build --release --manifest-path tls-proxy-rs/Cargo.toml

APP="$OUT/$BUNDLE_NAME.app"
rm -rf "$APP"
mkdir -p \
  "$APP/Contents/MacOS" \
  "$APP/Contents/Resources/libexec/codex-package" \
  "$APP/Contents/Resources/fonts" \
  "$APP/Contents/Resources/licenses"

install -m0755 desktop/target/release/xd-desktop \
  "$APP/Contents/MacOS/xd-desktop"
install -m0755 installer/macos/xd-rust-launcher.sh \
  "$APP/Contents/MacOS/xd"
install -m0755 daemon-rs/target/release/xd-daemon \
  "$APP/Contents/Resources/libexec/xd-daemon"
install -m0755 tls-proxy-rs/target/release/xd-tls-proxy \
  "$APP/Contents/Resources/libexec/xd-tls-proxy"
install -m0755 scripts/install-macos.sh \
  "$APP/Contents/Resources/libexec/install.sh"
install -m0755 "$CLAUDE_BINARY" \
  "$APP/Contents/Resources/libexec/claude"
tar -xzf "$CODEX_ARCHIVE" \
  -C "$APP/Contents/Resources/libexec/codex-package"
tar -xzf "$PROXY_ARCHIVE" -C "$WORK"
install -m0755 "$WORK/claude-code-proxy" \
  "$APP/Contents/Resources/libexec/claude-code-proxy"
install -m0755 "$WHISPER_CACHE/whisper-server-bin" \
  "$APP/Contents/Resources/libexec/whisper-server-bin"
install -m0644 data/fonts/DMSans-Variable.ttf \
  "$APP/Contents/Resources/fonts/DMSans-Variable.ttf"
tar -xOf "$WHISPER_ARCHIVE" "whisper.cpp-$WHISPER_VERSION/LICENSE" \
  > "$APP/Contents/Resources/licenses/whisper.cpp-LICENSE"

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
"$APP/Contents/Resources/libexec/codex-package/bin/codex" --version \
  | grep -F "$CODEX_VERSION"
"$APP/Contents/Resources/libexec/claude" --version \
  | grep -F "$CLAUDE_VERSION"
"$APP/Contents/Resources/libexec/claude-code-proxy" --version \
  | grep -F "$CLAUDE_PROXY_VERSION"
"$APP/Contents/Resources/libexec/whisper-server-bin" --help >/dev/null

ASSET="$BUNDLE_NAME-macos-$ARCH.zip"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$OUT/$ASSET"
(
  cd "$OUT"
  shasum -a 256 "$ASSET" > "$ASSET.sha256"
)

printf 'macOS artifact: %s\n' "$OUT/$ASSET"
