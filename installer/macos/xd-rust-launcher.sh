#!/bin/sh

set -eu

CONTENTS=$(cd "$(dirname "$0")/.." && pwd)
RESOURCES="$CONTENTS/Resources"

case "$(basename "$(dirname "$CONTENTS")")" in
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
# terminal sessions, then put the app's native agent helpers first for xd.
export XD_HOST_PATH="${PATH-}"
export PATH="$RESOURCES/libexec/codex-package/bin:$RESOURCES/libexec:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

# Use the native macOS data locations. The channel name keeps a release and a
# nightly completely independent when both applications are installed.
export XDG_DATA_HOME="${HOME}/Library/Application Support"
export XDG_CONFIG_HOME="${HOME}/Library/Application Support"
export XDG_CACHE_HOME="${HOME}/Library/Caches"
export XD_SETTINGS_PATH="$XDG_CONFIG_HOME/$XD_DATA_NAME/settings.json"

export XD_DAEMON_EXECUTABLE="$RESOURCES/libexec/xd-daemon"
export XD_TLS_PROXY_EXECUTABLE="$RESOURCES/libexec/xd-tls-proxy"
export XD_CODEX_EXECUTABLE="$RESOURCES/libexec/codex-package/bin/codex"
export XD_CLAUDE_EXECUTABLE="$RESOURCES/libexec/claude"
export XD_CLAUDE_PROXY_EXECUTABLE="$RESOURCES/libexec/claude-code-proxy"
export XD_WHISPER_SERVER="$RESOURCES/libexec/whisper-server-bin"

exec "$CONTENTS/MacOS/xd-desktop" "$@"
