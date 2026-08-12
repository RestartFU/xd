#!/bin/sh
#
# Install a native macOS dev, nightly, or stable release without root access.
#
#   curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install-macos.sh | sh
#   curl -fsSL https://github.com/RestartFU/xd/releases/download/dev/install-macos.sh | sh -s -- --dev
#   curl -fsSL https://github.com/RestartFU/xd/releases/latest/download/install-macos.sh | sh -s -- --release

set -eu

REPO=RestartFU/xd
CHANNEL=nightly
SOURCE=
UNINSTALL=no

say () { printf '%s\n' "$*"; }
die () { printf 'install-macos: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dev) CHANNEL=dev ;;
    --release|--stable) CHANNEL=release ;;
    --from) [ "$#" -ge 2 ] || die "--from needs an app bundle."
            SOURCE=$2; shift ;;
    --from=*) SOURCE=${1#--from=} ;;
    --uninstall) UNINSTALL=yes ;;
  esac
  shift
done

[ "$(uname -s)" = Darwin ] || die "this installer requires macOS."
case "$(uname -m)" in
  arm64|aarch64) ARCH=arm64 ;;
  x86_64|amd64) ARCH=x86_64 ;;
  *) die "unsupported architecture: $(uname -m)." ;;
esac

case "$CHANNEL" in
  dev)
    NAME=xd-dev
    APP_ID=com.restartfu.Xd.Dev
    ASSET=xd-dev-macos-$ARCH.zip
    BASE="https://github.com/$REPO/releases/download/dev"
    ;;
  nightly)
    NAME=xd-nightly
    APP_ID=com.restartfu.Xd.Nightly
    ASSET=xd-nightly-macos-$ARCH.zip
    BASE="https://github.com/$REPO/releases/download/nightly"
    ;;
  release)
    NAME=xd
    APP_ID=com.restartfu.Xd
    ASSET=xd-macos-$ARCH.zip
    BASE="https://github.com/$REPO/releases/latest/download"
    ;;
esac

APP="$HOME/Applications/$NAME.app"
BIN="$HOME/.local/bin/$NAME"

if [ "$UNINSTALL" = yes ]; then
  rm -rf "$APP"
  rm -f "$BIN"
  say "Removed $NAME."
  say "Its chats and workspaces remain in $HOME/Library/Application Support/$NAME."
  exit 0
fi

for command in curl ditto file plutil shasum; do
  command -v "$command" >/dev/null 2>&1 || die "$command is required."
done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/xd-install.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

if [ -n "$SOURCE" ]; then
  [ -d "$SOURCE" ] || die "$SOURCE is not an app bundle."
  actual_id=$(plutil -extract CFBundleIdentifier raw "$SOURCE/Contents/Info.plist" 2>/dev/null || true)
  [ "$actual_id" = "$APP_ID" ] || die "$SOURCE is not a $NAME build."
  ditto "$SOURCE" "$WORK/$NAME.app"
else
  say "Downloading the $CHANNEL build…"
  curl -fsSL --proto '=https' --tlsv1.2 \
    -o "$WORK/$ASSET" "$BASE/$ASSET" \
    || die "cannot download $BASE/$ASSET"
  curl -fsSL --proto '=https' --tlsv1.2 \
    -o "$WORK/$ASSET.sha256" "$BASE/$ASSET.sha256" \
    || die "cannot download the checksum."
  (cd "$WORK" && shasum -a 256 -c "$ASSET.sha256" >/dev/null) \
    || die "the download does not match its checksum."
  ditto -x -k "$WORK/$ASSET" "$WORK"
fi

[ -x "$WORK/$NAME.app/Contents/MacOS/xd" ] \
  || die "the app bundle is incomplete."
file "$WORK/$NAME.app/Contents/MacOS/xd-desktop" | grep -F "$(uname -m)" >/dev/null \
  || die "the app bundle is for a different Mac architecture."
actual_id=$(plutil -extract CFBundleIdentifier raw \
  "$WORK/$NAME.app/Contents/Info.plist" 2>/dev/null || true)
[ "$actual_id" = "$APP_ID" ] || die "the app bundle has the wrong identity."

mkdir -p "$HOME/Applications" "$(dirname "$BIN")"
OLD="$APP.previous.$$"
rm -rf "$OLD"
if [ -e "$APP" ]; then
  mv "$APP" "$OLD"
fi
if mv "$WORK/$NAME.app" "$APP"; then
  rm -rf "$OLD"
else
  [ ! -e "$OLD" ] || mv "$OLD" "$APP"
  die "cannot replace $APP."
fi
ln -sfn "$APP/Contents/MacOS/xd" "$BIN"

VERSION=$("$BIN" --version 2>/dev/null || echo "$NAME")
say ""
say "Installed $VERSION."
say "  app       $APP"
say "  command   $BIN"
say "  data      $HOME/Library/Application Support/$NAME"
say ""
say "Open it from Finder or Spotlight, or run: $NAME"
