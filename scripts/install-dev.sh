#!/bin/sh
#
# Install the rolling Linux GPUI/Rust build as `xd-dev`:
#
#   curl -fsSL https://github.com/RestartFU/xd/releases/download/dev/install-dev.sh | sh
#
# This channel is intentionally separate from xd and xd-nightly. Its archive
# contains the GPUI desktop, Rust daemon, and Codex CLI and replaces neither
# production client.

set -eu

REPO=RestartFU/xd
NAME=xd-dev
ASSET=xd-dev-linux-x86_64.tar.gz
BASE="https://github.com/$REPO/releases/download/dev"
SOURCE=
UNINSTALL=no

say () { printf '%s\n' "$*"; }
die () { printf 'install-dev: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --from)
      [ "$#" -ge 2 ] || die "--from needs a directory."
      SOURCE=$2
      shift
      ;;
    --from=*) SOURCE=${1#--from=} ;;
    --uninstall) UNINSTALL=yes ;;
    *) die "unknown option: $1" ;;
  esac
  shift
done

OPT="$HOME/.local/opt/$NAME"
BIN="$HOME/.local/bin/$NAME"

if [ "$UNINSTALL" = yes ]; then
  rm -rf "$OPT"
  rm -f "$BIN"
  say "Removed $NAME."
  exit 0
fi

[ "$(uname -s)" = Linux ] \
  || die "this installs the Linux build; found $(uname -s)."
case "$(uname -m)" in
  x86_64|amd64) ;;
  *) die "only x86_64 is published so far; found $(uname -m)." ;;
esac

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

if [ -n "$SOURCE" ]; then
  [ -x "$SOURCE/$NAME" ] || die "$SOURCE does not contain an xd-dev build."
  cp -a "$SOURCE" "$WORK/$NAME"
else
  command -v curl >/dev/null 2>&1 || die "curl is needed."
  command -v sha256sum >/dev/null 2>&1 || die "sha256sum is needed."
  command -v tar >/dev/null 2>&1 || die "tar is needed."

  say "Downloading $NAME…"
  curl -fsSL --proto '=https' --tlsv1.2 \
    -o "$WORK/$ASSET" "$BASE/$ASSET" \
    || die "cannot download $BASE/$ASSET"
  curl -fsSL --proto '=https' --tlsv1.2 \
    -o "$WORK/$ASSET.sha256" "$BASE/$ASSET.sha256" \
    || die "cannot download the checksum"

  (cd "$WORK" && sha256sum -c "$ASSET.sha256" >/dev/null) \
    || die "the download does not match its checksum."

  tar -xzf "$WORK/$ASSET" -C "$WORK"
  [ -x "$WORK/$NAME/$NAME" ] || die "the archive is not what was expected."
fi

mkdir -p "$(dirname "$OPT")" "$(dirname "$BIN")"

OLD="$OPT.previous.$$"
rm -rf "$OLD"
if [ -e "$OPT" ]; then
  mv "$OPT" "$OLD"
fi
if mv "$WORK/$NAME" "$OPT"; then
  rm -rf "$OLD"
else
  [ ! -e "$OLD" ] || mv "$OLD" "$OPT"
  die "cannot replace $OPT."
fi

ln -sfn "$OPT/$NAME" "$BIN"

say ""
say "Installed $NAME."
say "  app       $OPT"
say "  command   $BIN"
say ""
say "Run it with: $NAME"

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) say "$HOME/.local/bin is not on your PATH; add it to use the command." ;;
esac
