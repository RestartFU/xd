#!/bin/sh
#
# Launcher for the relocatable xd bundle.
#
# Private dependencies use relative runtime paths embedded during assembly.
# glibc and graphics drivers remain host-owned, and no LD_LIBRARY_PATH leaks
# into terminals or agent CLIs launched by xd.

set -e

# A shell can stay attached to a directory after that directory is deleted.
# Its cached $PWD still looks right, but every child then inherits a cwd that
# getcwd(3) cannot resolve. Recover before the bundle starts helper processes.
if ! pwd -P >/dev/null 2>&1; then
  cd "${HOME:-/}" 2>/dev/null || cd /
fi

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)

case "$(basename "$HERE")" in
  xd-dev)
    export XD_APP_ID=com.restartfu.Xd.Dev
    export XD_DATA_NAME=xd-dev
    export XD_UPDATE_CHANNEL=dev
    export XD_SETTINGS_PATH="${XDG_CONFIG_HOME:-${HOME}/.config}/xd-dev/settings.json"
    ;;
  xd-nightly)
    export XD_APP_ID=com.restartfu.Xd.Nightly
    export XD_DATA_NAME=xd-nightly
    export XD_UPDATE_CHANNEL=nightly
    export XD_SETTINGS_PATH="${XDG_CONFIG_HOME:-${HOME}/.config}/xd-nightly/settings.json"
    ;;
  *)
    export XD_APP_ID=com.restartfu.Xd
    export XD_DATA_NAME=xd
    export XD_UPDATE_CHANNEL=release
    export XD_SETTINGS_PATH="${XDG_CONFIG_HOME:-${HOME}/.config}/xd/settings.json"
    ;;
esac

# Per bundle, not just per user: dev, nightly, and release installs side by
# side would otherwise rewrite each other's caches while they are running.
RUNTIME="${XDG_RUNTIME_DIR:-/tmp}/xd-$(id -u)/$(basename "$HERE")"
mkdir -p "$RUNTIME"

# This cache stores an absolute path, so it is rewritten per launch from an
# @BUNDLE@ template; that is what keeps the bundle relocatable.
sed "s|@BUNDLE@|$HERE|g" "$HERE/etc/fonts.conf.in"    > "$RUNTIME/fonts.conf"

# Anything xd launches for the user -- a terminal, an editor -- must run in the
# host's environment, not the bundle's. Remember the values before they are
# overridden so they can be handed back to terminals, agents, and host tools.
export XD_HOST_PATH="${PATH-}"
export XD_HOST_XDG_DATA_DIRS="${XDG_DATA_DIRS-}"
export XD_HOST_LANG="${LANG-}"
export XD_HOST_LC_ALL="${LC_ALL-}"
export XD_HOST_LOCPATH="${LOCPATH-}"

# Both matter: without FONTCONFIG_PATH, fontconfig also reads the host's
# /etc/fonts. It still scans the conf.avail template dir compiled into the
# library (/usr/share/fontconfig), which on a non-Debian host may hold
# newer-format files -- harmless parse warnings on stderr, fonts still resolve.
export FONTCONFIG_PATH="$HERE/etc/fonts"
export FONTCONFIG_FILE="$RUNTIME/fonts.conf"

# Keymap data. A host without these (they are not standard outside X11
# installs) leaves the window backend with no keymap.
export XKB_CONFIG_ROOT="$HERE/share/X11/xkb"
export XLOCALEDIR="$HERE/share/X11/locale"

# Use the host glibc's portable UTF-8 locale. This avoids locale-specific
# number and date formatting while retaining correct terminal text handling.
export LC_ALL=C.UTF-8
export LANG=C.UTF-8

export XDG_DATA_DIRS="$HERE/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export XCURSOR_PATH="$HERE/share/icons:${XCURSOR_PATH:-$HOME/.icons:/usr/share/icons}"
export PATH="$HERE/bin:${PATH:-/usr/local/bin:/usr/bin:/bin}"

# Project state is served by a short-lived stdio host owned by the window.
export XD_HOST_EXECUTABLE="$HERE/libexec/xd-host"
export XD_TMUX_EXECUTABLE="$HERE/libexec/tmux"
export XD_SESSION_RUNTIME="$RUNTIME/sessions"
# The in-app updater must remain self-contained too. Its installer honors
# these paths instead of selecting unrelated host network or crypto tools.
export XD_CURL="$HERE/libexec/curl"
export XD_OPENSSL="$HERE/libexec/openssl"

export SSL_CERT_FILE="$HERE/etc/ssl/certs/ca-certificates.crt"
export OPENSSL_CONF="$HERE/etc/ssl/openssl.cnf"
export OPENSSL_MODULES="$HERE/lib/ossl-modules"
export GIT_EXEC_PATH="$HERE/libexec/git-core"
export GIT_TEMPLATE_DIR="$HERE/share/git-core/templates"
export GIT_SSL_CAINFO="$HERE/etc/ssl/certs/ca-certificates.crt"

exec "$HERE/bin/xd" "$@"
