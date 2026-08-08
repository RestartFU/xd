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
# its own GTK, glib, Git and everything under them, so it runs on any glibc
# x86_64 system. It goes in the home directory -- no root, no package manager,
# and nothing outside these paths:
#
#   ~/.local/opt/xd-nightly           the program
#   ~/.local/bin/xd-nightly           the command
#   ~/.local/share/applications/…     the entry in the app menu
#   ~/.local/share/icons/…            its icon
#   ~/.config/systemd/user/…          the daemon's unit, where systemd runs
#
# The unit is a *user* unit, like everything else here: no root, and it is the
# person's daemon rather than the machine's. It is written but not enabled --
# what runs at login is the machine owner's call, not an installer's:
#
#   systemctl --user enable --now xd-nightly   hands the daemon to systemd
#   sh -s -- --no-service                      skips the unit altogether
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

# Whether to write the unit at all. It is never enabled from here, so this only
# decides between a unit waiting to be turned on and no unit on the machine.
SERVICE_WANTED=yes

while [ "$#" -gt 0 ]; do
  case "$1" in
    --release|--stable) CHANNEL=release ;;
    --from) [ "$#" -ge 2 ] || die "--from needs a directory."
            SOURCE=$2; shift ;;
    --from=*) SOURCE=${1#--from=} ;;
    --uninstall) UNINSTALL=yes ;;
    --no-service) SERVICE_WANTED=no ;;
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
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
OPT="$HOME/.local/opt/$NAME"
BIN="$HOME/.local/bin/$NAME"
DESKTOP="$DATA_HOME/applications/$APP_ID.desktop"
SERVICE_NAME="$NAME.service"
SERVICE="$CONFIG_HOME/systemd/user/$SERVICE_NAME"

# The socket the window looks for, resolved here rather than left to the
# daemon. A login shell's XDG_DATA_HOME is not part of the user manager's
# environment, so a unit that worked it out for itself could end up serving a
# different directory than the one this script just reported.
SOCKET="$DATA_HOME/$NAME/daemon.sock"

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
  # Before the files it runs: a unit left enabled would keep restarting a
  # daemon whose bundle is being deleted out from under it.
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

# A daemon under the unit is a process out of this bundle, so the check below
# would refuse every upgrade on a machine where it is turned on. Stopping it
# first is this script's own business -- it wrote the unit -- and unlike a
# window, a daemon holds no unsent input.
#
# Whether it comes back at the end is decided by whether it was running:
# restoring an upgrade is this script's business too, but switching a service
# off, or on, is not.
SERVICE_WAS_ACTIVE=no
if have_user_manager && [ -f "$SERVICE" ]; then
  if systemctl --user is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
    SERVICE_WAS_ACTIVE=yes
  fi
  systemctl --user stop "$SERVICE_NAME" >/dev/null 2>&1 || true
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
  [ -x "$SOURCE/xd.sh" ] || die "$SOURCE is not a built bundle."

  # And it is a build of what is being installed. A bundle built as the
  # default profile carries the release's application id, and installing it to
  # the nightly's paths would leave a copy that answers to neither.
  [ -f "$SOURCE/share/applications/$APP_ID.desktop" ] \
    || die "the build in $SOURCE is not a $NAME build; build it with PROFILE=$CHANNEL."

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
  [ -x "$WORK/$NAME/xd.sh" ] || die "the archive is not what was expected."
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

# --- the daemon's unit ------------------------------------------------------

# Written, never enabled. What runs at a person's login is their decision, and
# an installer that quietly adds one has taken it from them; the window goes on
# starting the daemon itself until somebody turns the unit on.
#
# Only where systemd is actually the init system -- everywhere else there is
# nothing that would ever read this file.
SERVICE_STATE=none
if [ "$SERVICE_WANTED" = yes ] && have_systemd; then
  mkdir -p "$(dirname "$SERVICE")"

  # ExecStart is rewritten rather than templated at build time for the same
  # reason Exec= is in the entry above: the bundle does not know where it will
  # be installed, and --socket pins both ends to one daemon.
  if [ -f "$OPT/share/systemd/user/$SERVICE_NAME" ]; then
    sed -e "s|^ExecStart=.*|ExecStart=$BIN serve --socket $SOCKET|" \
        "$OPT/share/systemd/user/$SERVICE_NAME" > "$SERVICE"
  else
    cat > "$SERVICE" <<EOF
[Unit]
Description=$NAME daemon
Documentation=https://github.com/$REPO
StartLimitIntervalSec=60
StartLimitBurst=3

[Service]
ExecStart=$BIN serve --socket $SOCKET
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
EOF
  fi
  chmod 644 "$SERVICE"

  # The daemon keeps its database in there and would create it anyway; making
  # it here means the unit never starts against a directory that is not the
  # one this script just reported.
  mkdir -p "$DATA_HOME/$NAME"

  # systemd may be the init system while this script still has no user manager
  # to talk to -- an ssh session without lingering, a chroot. The unit is worth
  # writing either way: the next graphical login has a manager that reads it.
  SERVICE_STATE=written
  if have_user_manager; then
    systemctl --user daemon-reload >/dev/null 2>&1 || true
    if [ "$SERVICE_WAS_ACTIVE" = yes ]; then
      # Back on because it was on. This restores an upgrade; it does not enable
      # anything, and a machine installing for the first time stays untouched.
      if systemctl --user start "$SERVICE_NAME" >/dev/null 2>&1; then
        SERVICE_STATE=running
      else
        SERVICE_STATE=failed
      fi
    fi
  fi
elif [ "$SERVICE_WAS_ACTIVE" = yes ]; then
  # --no-service asks this script to leave the unit alone, not to leave a
  # running daemon down: it was stopped above only so the bundle underneath it
  # could be replaced.
  systemctl --user start "$SERVICE_NAME" >/dev/null 2>&1 || true
fi

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
case "$SERVICE_STATE" in
  written|running|failed) say "  service   $SERVICE" ;;
esac
say ""
say "Run it from the app menu, or with: $NAME"

case "$SERVICE_STATE" in
  written)
    say ""
    say "A systemd unit for its daemon is installed, and switched off: the"
    say "window starts the daemon itself, the way it always has. Turn the unit"
    say "on to keep paired devices reachable and turns running with the window"
    say "closed:"
    say "  systemctl --user enable --now $SERVICE_NAME"
    ;;
  running)
    say ""
    say "Its daemon is back under systemd, where this upgrade found it:"
    say "  systemctl --user status $SERVICE_NAME"
    ;;
  failed)
    say ""
    say "Its daemon was running under systemd before this upgrade and did not"
    say "come back; the window will start one itself in the meantime. To see"
    say "why:"
    say "  systemctl --user status $SERVICE_NAME"
    ;;
esac

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) say ""
     say "$HOME/.local/bin is not on your PATH; add it to use the command." ;;
esac
