#!/bin/sh
#
# Installs xd on Linux, from the prebuilt bundle:
#
#   curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install.sh | sh
#
# "sh -s -- --release" installs the newest tagged release instead of the
# nightly; the two live side by side. "sh -s -- --from DIR" installs a bundle
# that is already built, which is how the nightly installs a branch it built
# from source: nothing is downloaded, and the rest of this -- the paths, the
# command, the icon, the menu entry -- is the same as for a download, because
# it is the same script. It takes itself away again with:
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
#   ~/.local/share/icons/…            its icon
#
# Chats and workspaces live in ~/.local/share/xd-nightly and are never touched
# by installing, upgrading or uninstalling.

set -eu

REPO=RestartFU/xd

say () { printf '%s\n' "$*"; }
die () { printf 'install: %s\n' "$*" >&2; exit 1; }

# The nightly by default, since it is the one that is always there. --release
# takes the newest tagged release instead; the two install side by side and
# neither touches the other's chats.
CHANNEL=nightly

# A bundle already on this machine, from --from: a build of a branch, which is
# a nightly and installs to the nightly's paths.
SOURCE=
UNINSTALL=no

while [ "$#" -gt 0 ]; do
  case "$1" in
    --release|--stable) CHANNEL=release ;;
    --from) [ "$#" -ge 2 ] || die "--from needs a directory."
            SOURCE=$2; shift ;;
    --from=*) SOURCE=${1#--from=} ;;
    --uninstall) UNINSTALL=yes ;;
  esac
  shift
done

if [ "$CHANNEL" = release ]; then
  NAME=xd
  APP_ID=com.restartfu.Xd
  ASSET=xd-linux-x86_64.tar.gz
  BASE="https://github.com/$REPO/releases/latest/download"
else
  NAME=xd-nightly
  APP_ID=com.restartfu.Xd.Nightly
  ASSET=xd-nightly-linux-x86_64.tar.gz
  BASE="https://github.com/$REPO/releases/download/$CHANNEL"
fi

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
OPT="$HOME/.local/opt/$NAME"
BIN="$HOME/.local/bin/$NAME"
DESKTOP="$DATA_HOME/applications/$APP_ID.desktop"

uninstall () {
  rm -rf "$OPT"
  rm -f "$BIN" "$DESKTOP" \
        "$DATA_HOME/icons/hicolor/scalable/apps/$APP_ID.svg"

  say "Removed $NAME."
  say "Its chats and workspaces are still in $DATA_HOME/$NAME."
  exit 0
}

[ "$UNINSTALL" = yes ] && uninstall

# --- what this machine is ---------------------------------------------------

[ "$(uname -s)" = "Linux" ] || die "this installs the Linux build; found $(uname -s)."

case "$(uname -m)" in
  x86_64|amd64) ;;
  *) die "only x86_64 is published so far; found $(uname -m)." ;;
esac

# --- the bundle -------------------------------------------------------------

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

if [ -n "$SOURCE" ]; then
  # Already built, here on this machine. Copied into the staging directory
  # rather than moved, so a failure later leaves the build where it was and
  # the same one line below installs either kind.
  [ -x "$SOURCE/xd.sh" ] || die "$SOURCE is not a built bundle."

  # And it is a build of what is being installed. A bundle built as the
  # default profile carries the release's application id, and installing it to
  # the nightly's paths would leave a copy that answers to neither.
  [ -f "$SOURCE/share/applications/$APP_ID.desktop" ] \
    || die "the build in $SOURCE is not a $NAME build; build it with PROFILE=$CHANNEL."

  say "Installing the build in $SOURCE…"
  cp -a "$SOURCE" "$WORK/$NAME"
else
  command -v curl >/dev/null 2>&1 || die "curl is needed."
  command -v tar  >/dev/null 2>&1 || die "tar is needed."

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
fi

# --- install ----------------------------------------------------------------

mkdir -p "$(dirname "$OPT")" "$(dirname "$BIN")" "$(dirname "$DESKTOP")"

# Replaced whole rather than merged: an upgrade that left a stale library
# behind would be a bundle that no longer matches itself.
rm -rf "$OPT"
mv "$WORK/$NAME" "$OPT"

ln -sfn "$OPT/xd.sh" "$BIN"

#
# The icon goes in the icon theme, and the entry names it rather than pointing
# at it.
#
# An Icon= that is a path is loaded as a file, outside the theme, and the
# desktop caches what it drew from that path -- so a new picture at the same
# path goes on looking like the old one until the session restarts. An icon in
# the theme is watched: replacing it is a change the desktop notices, which is
# the whole reason the theme directories exist.
#
ICON_THEME="$DATA_HOME/icons/hicolor"
ICON_DIR="$ICON_THEME/scalable/apps"

mkdir -p "$ICON_DIR"
if [ -f "$OPT/share/icons/hicolor/scalable/apps/$APP_ID.svg" ]; then
  cp -f "$OPT/share/icons/hicolor/scalable/apps/$APP_ID.svg" "$ICON_DIR/$APP_ID.svg"
fi

if [ -f "$OPT/share/applications/$APP_ID.desktop" ]; then
  sed -e "s|^Exec=.*|Exec=$BIN|" \
      -e "s|^Icon=.*|Icon=$APP_ID|" \
      "$OPT/share/applications/$APP_ID.desktop" > "$DESKTOP"
else
  cat > "$DESKTOP" <<EOF
[Desktop Entry]
Name=xd (Nightly)
Comment=Workspace-organized AI conversations
Exec=$BIN
Icon=$APP_ID
Terminal=false
Type=Application
Categories=Development;Utility;
StartupNotify=true
EOF
fi

chmod 644 "$DESKTOP"

# Both caches are only hints, and both are watched: touching the directories is
# what tells a running desktop to look again.
for cache in gtk4-update-icon-cache gtk-update-icon-cache; do
  if command -v "$cache" >/dev/null 2>&1; then
    "$cache" -q -f -t "$ICON_THEME" 2>/dev/null || true
    break
  fi
done

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$DATA_HOME/applications" 2>/dev/null || true
fi

touch "$ICON_DIR" "$ICON_THEME" "$DATA_HOME/applications" 2>/dev/null || true

# --- say what happened ------------------------------------------------------

# Asked of the thing that was just installed rather than of the download, so
# what is printed is what will actually run -- and so "is the fix in this one?"
# is answered here instead of being a second thing to go and check.
VERSION=$("$BIN" --version 2>/dev/null || echo "$NAME")

say ""
say "Installed $VERSION."
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
