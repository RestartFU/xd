#!/bin/sh
#
# Installs the native Apple Silicon app bundle:
#
#   curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install-macos.sh | sh

set -eu

REPO=RestartFU/xd
CHANNEL=nightly

for argument in "$@"; do
  case "$argument" in
    --release|--stable) CHANNEL=release ;;
  esac
done

if [ "$CHANNEL" = release ]; then
  NAME=xd
  ASSET=xd-macos-arm64.zip
  BASE="https://github.com/$REPO/releases/latest/download"
else
  NAME=xd-nightly
  ASSET=xd-nightly-macos-arm64.zip
  BASE="https://github.com/$REPO/releases/download/nightly"
fi

APP="$HOME/Applications/$NAME.app"

say () { printf '%s\n' "$*"; }
die () { printf 'install: %s\n' "$*" >&2; exit 1; }

uninstall () {
  rm -rf "$APP"
  say "Removed $NAME. Chats and workspaces were left in place."
  exit 0
}

for argument in "$@"; do
  [ "$argument" = "--uninstall" ] && uninstall
done

[ "$(uname -s)" = "Darwin" ] || die "this installer requires macOS."
case "$(uname -m)" in
  arm64|aarch64) ;;
  *) die "only Apple Silicon is published so far; found $(uname -m)." ;;
esac

command -v curl >/dev/null 2>&1 || die "curl is needed."
command -v ditto >/dev/null 2>&1 || die "ditto is needed."
command -v shasum >/dev/null 2>&1 || die "shasum is needed."

WORK=$(mktemp -d "${TMPDIR:-/tmp}/xd-install.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

say "Downloading the $CHANNEL build…"
curl -fsSL --proto '=https' --tlsv1.2 -o "$WORK/$ASSET" "$BASE/$ASSET" \
  || die "cannot download $BASE/$ASSET"
curl -fsSL --proto '=https' --tlsv1.2 -o "$WORK/$ASSET.sha256" \
  "$BASE/$ASSET.sha256" \
  || die "cannot download the checksum."

( cd "$WORK" && shasum -a 256 -c "$ASSET.sha256" >/dev/null ) \
  || die "the download does not match its checksum."

ditto -x -k "$WORK/$ASSET" "$WORK"
[ -d "$WORK/$NAME.app" ] || die "the archive is not what was expected."

mkdir -p "$HOME/Applications"
rm -rf "$APP"
mv "$WORK/$NAME.app" "$APP"

say ""
say "Installed $NAME in $APP."
say "Open it from Finder, Spotlight, or with: open \"$APP\""
