#!/bin/sh

set -eu

# Finder invokes this file directly, while the command-line installer exposes
# it through ~/.local/bin. Resolve that relative symlink without readlink -f,
# which macOS does not provide, so both entries find the same app bundle.
LAUNCHER=$0
while [ -L "$LAUNCHER" ]; do
  LAUNCHER_DIR=$(CDPATH='' cd -P "$(dirname "$LAUNCHER")" && pwd)
  LINK=$(readlink "$LAUNCHER")
  case "$LINK" in
    /*) LAUNCHER=$LINK ;;
    *) LAUNCHER=$LAUNCHER_DIR/$LINK ;;
  esac
done

CONTENTS=$(CDPATH='' cd -P "$(dirname "$LAUNCHER")/.." && pwd)
RESOURCES="$CONTENTS/Resources"

case "$(basename "$(dirname "$CONTENTS")")" in
  xd-dev.app)
    export XD_APP_ID=com.restartfu.Xd.Dev
    export XD_DATA_NAME=xd-dev
    export XD_UPDATE_CHANNEL=dev
    ;;
  xd-nightly.app)
    export XD_APP_ID=com.restartfu.Xd.Nightly
    export XD_DATA_NAME=xd-nightly
    export XD_UPDATE_CHANNEL=nightly
    ;;
  *)
    export XD_APP_ID=com.restartfu.Xd
    export XD_DATA_NAME=xd
    export XD_UPDATE_CHANNEL=release
    ;;
esac

# Finder launches applications with a small PATH. Keep the original value for
# terminal sessions, then add common locations for user-installed assistants.
export XD_HOST_PATH="${PATH-}"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

# Use the native macOS data locations. The channel name keeps dev, nightly,
# and release applications independent when they are installed together.
export XDG_DATA_HOME="${HOME}/Library/Application Support"
export XDG_CONFIG_HOME="${HOME}/Library/Application Support"
export XDG_CACHE_HOME="${HOME}/Library/Caches"
export XD_SETTINGS_PATH="$XDG_CONFIG_HOME/$XD_DATA_NAME/settings.json"

export XD_HOST_EXECUTABLE="$RESOURCES/libexec/xd-host"
exec "$CONTENTS/MacOS/xd-desktop" "$@"
