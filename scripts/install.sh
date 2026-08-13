#!/bin/sh
#
# Installs xd on Linux, from a prebuilt bundle:
#
#   curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install.sh | sh
#   curl -fsSL https://github.com/RestartFU/xd/releases/download/dev/install.sh | sh -s -- --dev
#
# "sh -s -- --release" installs the newest tagged release and "--dev" installs
# the rolling development build. All three live side by side. "--from DIR"
# installs a bundle already built on this machine; the remaining paths, icon,
# menu entry stay identical to a downloaded install. It
# takes itself away again with:
#
#   curl -fsSL .../install.sh | sh -s -- --uninstall
#
# Published beside the bundle it installs, from the same commit, so the two can
# never be from different builds. The dev and nightly tags are rolling: their
# links always select the most recent build on that channel.
#
# Nothing is compiled and nothing is needed on the machine: the bundle carries
# its own native runtime, Git, and graphics fallback, so it runs on any glibc
# x86_64 system. It goes in the home directory -- no root, no package manager,
# and nothing outside these paths:
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
# selects the newest tagged release and --dev selects the rolling development
# build; none touches another channel's chats.
CHANNEL=nightly

# A bundle already on this machine, from --from.
SOURCE=
UNINSTALL=no

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dev) CHANNEL=dev ;;
    --release|--stable) CHANNEL=release ;;
    --from) [ "$#" -ge 2 ] || die "--from needs a directory."
            SOURCE=$2; shift ;;
    --from=*) SOURCE=${1#--from=} ;;
    --uninstall) UNINSTALL=yes ;;
    --no-service) : ;; # accepted for compatibility; there is no service now
  esac
  shift
done

case "$CHANNEL" in
  dev)
    NAME=xd-dev
    DISPLAY_NAME='xd (Dev)'
    APP_ID=com.restartfu.Xd.Dev
    ASSET=xd-dev-linux-x86_64.tar.gz
    BASE="https://github.com/$REPO/releases/download/dev"
    ;;
  nightly)
    NAME=xd-nightly
    DISPLAY_NAME='xd (Nightly)'
    APP_ID=com.restartfu.Xd.Nightly
    ASSET=xd-nightly-linux-x86_64.tar.gz
    BASE="https://github.com/$REPO/releases/download/nightly"
    ;;
  release)
    NAME=xd
    DISPLAY_NAME=xd
    APP_ID=com.restartfu.Xd
    ASSET=xd-linux-x86_64.tar.gz
    BASE="https://github.com/$REPO/releases/latest/download"
    ;;
esac

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
OPT="$HOME/.local/opt/$NAME"
BIN="$HOME/.local/bin/$NAME"
DESKTOP="$DATA_HOME/applications/$APP_ID.desktop"
SERVICE_NAME="$NAME.service"
SERVICE="$CONFIG_HOME/systemd/user/$SERVICE_NAME"

validate_bundle () {
  bundle=$1
  [ -x "$bundle/xd.sh" ] || die "$bundle is not a built bundle."

  if [ "$CHANNEL" = dev ]; then
    [ "$(sed -n '1p' "$bundle/share/xd/profile" 2>/dev/null || true)" = dev ] \
      || die "the build in $bundle is not an $NAME build; build it with PROFILE=dev."
  else
    [ -f "$bundle/share/applications/$APP_ID.desktop" ] \
      || die "the build in $bundle is not a $NAME build; build it with PROFILE=$CHANNEL."
  fi
}

# --- systemd, if this machine uses it ---------------------------------------

# Two separate questions. systemd may be the init system while this script has
# no user manager to talk to -- a container, a chroot, an ssh session with no
# lingering -- and then the unit can still be written for the next login even
# though it cannot be started now.
have_systemd () {
  [ -d /run/systemd/system ] || return 1
  command -v systemctl >/dev/null 2>&1
}

have_user_manager () {
  have_systemd || return 1
  systemctl --user show-environment >/dev/null 2>&1
}

uninstall () {
  # Clean up the user service left by pre-SSH releases before deleting files.
  if have_user_manager; then
    systemctl --user disable --now "$SERVICE_NAME" >/dev/null 2>&1 || true
  fi
  rm -f "$SERVICE"
  if have_user_manager; then
    systemctl --user daemon-reload >/dev/null 2>&1 || true
  fi

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

# Remove the service installed by older builds. Current xd has no daemon: its
# local host belongs to the window and remote mode is an SSH stdio process.
if have_user_manager; then
  systemctl --user disable --now "$SERVICE_NAME" >/dev/null 2>&1 || true
fi
rm -f "$SERVICE"
if have_user_manager; then
  systemctl --user daemon-reload >/dev/null 2>&1 || true
fi

# Replacing a running bundle leaves its old GtkApplication registered with the
# desktop. A later launch then activates stale code instead of the new binary.
# Refusing is safer than killing a session that may contain unsent input.
#
if [ "${XD_ALLOW_RUNNING_INSTALL:-}" != 1 ]; then
  for process_exe in /proc/[0-9]*/exe; do
    executable=$(readlink "$process_exe" 2>/dev/null || true)
    case "$executable" in
      "$OPT"/*)
        die "$NAME is running. Quit it completely, then rerun this installer."
        ;;
    esac
  done
fi

# --- the bundle -------------------------------------------------------------

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

if [ -n "$SOURCE" ]; then
  # Already built, here on this machine. Copied into the staging directory
  # rather than moved, so a failure later leaves the build where it was and
  # the same one line below installs either kind.
  # And it is a build of what is being installed. A bundle built for another
  # profile would otherwise be placed at paths where its launcher cannot give
  # it the identity the installer promises.
  validate_bundle "$SOURCE"

  say "Installing the build in $SOURCE…"
  cp -a "$SOURCE" "$WORK/$NAME"
else
  CURL=${XD_CURL:-curl}
  command -v "$CURL" >/dev/null 2>&1 || [ -x "$CURL" ] || die "curl is needed."
  command -v tar  >/dev/null 2>&1 || die "tar is needed."

  # XD_PROGRESS=1 asks for curl's meter, which the in-app updater reads off
  # stderr to show how far the download is. Off by default: piped into a
  # terminal the meter is noise, and there the bundle's size is already known.
  fetch () {
    if [ "${XD_PROGRESS:-}" = 1 ]; then
      "$CURL" -fL --progress-bar --proto '=https' --tlsv1.2 -o "$1" "$2"
    else
      "$CURL" -fsSL --proto '=https' --tlsv1.2 -o "$1" "$2"
    fi
  }

  say "Downloading the $CHANNEL build…"
  fetch "$WORK/$ASSET" "$BASE/$ASSET" \
    || die "cannot download $BASE/$ASSET"

  # Published beside the tarball, so a truncated or tampered download is caught
  # before anything is unpacked. Never silently install an unchecked archive.
  "$CURL" -fsSL --proto '=https' --tlsv1.2 -o "$WORK/$ASSET.sha256" \
    "$BASE/$ASSET.sha256" 2>/dev/null \
    || die "cannot download the checksum."
  if command -v sha256sum >/dev/null 2>&1; then
    ( cd "$WORK" && sha256sum -c "$ASSET.sha256" >/dev/null ) \
      || die "the download does not match its checksum."
  else
    OPENSSL=${XD_OPENSSL:-openssl}
    command -v "$OPENSSL" >/dev/null 2>&1 || [ -x "$OPENSSL" ] \
      || die "sha256sum or openssl is needed to verify the download."
    expected=$(sed -n '1{s/[[:space:]].*//;p;}' "$WORK/$ASSET.sha256")
    case "$expected" in
      *[!0123456789abcdefABCDEF]*|'') die "the checksum is malformed." ;;
    esac
    [ "${#expected}" -eq 64 ] || die "the checksum is malformed."
    actual=$("$OPENSSL" dgst -sha256 "$WORK/$ASSET") \
      || die "cannot calculate the download checksum."
    actual=${actual##* }
    [ "$actual" = "$expected" ] \
      || die "the download does not match its checksum."
  fi

  say "Unpacking…"
  tar -xzf "$WORK/$ASSET" -C "$WORK"
  validate_bundle "$WORK/$NAME"
fi

# --- install ----------------------------------------------------------------

mkdir -p "$(dirname "$OPT")" "$(dirname "$BIN")" "$(dirname "$DESKTOP")"

# Replaced whole rather than merged: an upgrade that left a stale library
# behind would be a bundle that no longer matches itself. Rename first so a
# running updater never watches its own files disappear one by one, and restore
# the old bundle if placing the new one fails.
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
ICON_SOURCE="$OPT/share/icons/hicolor/scalable/apps/$APP_ID.svg"
if [ "$CHANNEL" = dev ] && [ ! -f "$ICON_SOURCE" ]; then
  # The Linux dev bundle reuses the nightly Docker payload, but publishes and
  # installs it under a separate identity. Re-home the common artwork without
  # exposing or replacing the nightly icon in the user's icon theme.
  ICON_SOURCE="$OPT/share/icons/hicolor/scalable/apps/com.restartfu.Xd.Nightly.svg"
fi
if [ -f "$ICON_SOURCE" ]; then
  cp -f "$ICON_SOURCE" "$ICON_DIR/$APP_ID.svg"
fi

if [ -f "$OPT/share/applications/$APP_ID.desktop" ]; then
  sed -e "s|^Exec=.*|Exec=$BIN|" \
      -e "s|^Icon=.*|Icon=$APP_ID|" \
      "$OPT/share/applications/$APP_ID.desktop" > "$DESKTOP"
else
  cat > "$DESKTOP" <<EOF
[Desktop Entry]
Name=$DISPLAY_NAME
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
