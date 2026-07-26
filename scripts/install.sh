#!/bin/sh
#
# Installs xd on Linux, from the prebuilt bundle:
#
#   curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install.sh | sh
#
# and takes it away again with:
#
#   curl -fsSL .../install.sh | sh -s -- --uninstall
#
# Published beside the bundle it installs, from the same commit, so the two can
# never be from different builds. The tag is rolling: that link is always the
# most recent nightly.
#
# Nothing is compiled and nothing is needed on the machine: the bundle carries
# its own GTK, glib and everything under them, so it runs on any glibc x86_64
# system. It goes in the home directory -- no root, no package manager, and
# nothing outside these three paths:
#
#   ~/.local/opt/xd-nightly           the program
#   ~/.local/bin/xd-nightly           the command
#   ~/.local/share/applications/…     the entry in the app menu
#
# Chats and workspaces live in ~/.local/share/xd-nightly and are never touched
# by installing, upgrading or uninstalling.

set -eu

REPO=RestartFU/xd

# Only the nightly is published so far, so there is nothing to choose between.
CHANNEL=nightly
NAME=xd-nightly
APP_ID=com.restartfu.Xd.Nightly
ASSET=xd-nightly-linux-x86_64.tar.gz

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
OPT="$HOME/.local/opt/$NAME"
BIN="$HOME/.local/bin/$NAME"
DESKTOP="$DATA_HOME/applications/$APP_ID.desktop"

say () { printf '%s\n' "$*"; }
die () { printf 'install: %s\n' "$*" >&2; exit 1; }

uninstall () {
  rm -rf "$OPT"
  rm -f "$BIN" "$DESKTOP"

  say "Removed $NAME."
  say "Its chats and workspaces are still in $DATA_HOME/$NAME."
  exit 0
}

[ "${1:-}" = "--uninstall" ] && uninstall

# --- what this machine is ---------------------------------------------------

[ "$(uname -s)" = "Linux" ] || die "this installs the Linux build; found $(uname -s)."

case "$(uname -m)" in
  x86_64|amd64) ;;
  *) die "only x86_64 is published so far; found $(uname -m)." ;;
esac

command -v curl >/dev/null 2>&1 || die "curl is needed."
command -v tar  >/dev/null 2>&1 || die "tar is needed."

# --- fetch ------------------------------------------------------------------

BASE="https://github.com/$REPO/releases/download/$CHANNEL"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

say "Downloading the $CHANNEL build…"
curl -fsSL --proto '=https' --tlsv1.2 -o "$WORK/$ASSET" "$BASE/$ASSET" \
  || die "cannot download $BASE/$ASSET"

# Published beside the tarball, so a truncated or tampered download is caught
# before anything is unpacked.
if curl -fsSL --proto '=https' --tlsv1.2 -o "$WORK/$ASSET.sha256" \
     "$BASE/$ASSET.sha256" 2>/dev/null; then
  if command -v sha256sum >/dev/null 2>&1; then
    ( cd "$WORK" && sha256sum -c "$ASSET.sha256" >/dev/null ) \
      || die "the download does not match its checksum."
  fi
fi

say "Unpacking…"
tar -xzf "$WORK/$ASSET" -C "$WORK"
[ -x "$WORK/$NAME/xd.sh" ] || die "the archive is not what was expected."

# --- install ----------------------------------------------------------------

mkdir -p "$(dirname "$OPT")" "$(dirname "$BIN")" "$(dirname "$DESKTOP")"

# Replaced whole rather than merged: an upgrade that left a stale library
# behind would be a bundle that no longer matches itself.
rm -rf "$OPT"
mv "$WORK/$NAME" "$OPT"

ln -sfn "$OPT/xd.sh" "$BIN"

# The entry the app menu shows. Written from the one in the bundle so it keeps
# the name and categories the build gave it, with the paths made absolute --
# nothing here is on PATH or in an icon theme.
ICON="$OPT/share/icons/hicolor/scalable/apps/$APP_ID.svg"

if [ -f "$OPT/share/applications/$APP_ID.desktop" ]; then
  sed -e "s|^Exec=.*|Exec=$BIN|" \
      -e "s|^Icon=.*|Icon=$ICON|" \
      "$OPT/share/applications/$APP_ID.desktop" > "$DESKTOP"
else
  cat > "$DESKTOP" <<EOF
[Desktop Entry]
Name=xd (Nightly)
Comment=Workspace-organized AI conversations
Exec=$BIN
Icon=$ICON
Terminal=false
Type=Application
Categories=Development;Utility;
StartupNotify=true
EOF
fi

chmod 644 "$DESKTOP"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$DATA_HOME/applications" 2>/dev/null || true
fi

# --- say what happened ------------------------------------------------------

say ""
say "Installed $NAME."
say "  app       $OPT"
say "  command   $BIN"
say "  data      $DATA_HOME/$NAME"
say ""
say "Run it from the app menu, or with: $NAME"

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) say ""
     say "$HOME/.local/bin is not on your PATH; add it to use the command." ;;
esac
