#!/bin/sh
#
# Launcher for the relocatable xd bundle.
#
# Everything the app needs is loaded out of this directory. The bundled loader
# is invoked with --library-path rather than exporting LD_LIBRARY_PATH on
# purpose: host tools spawned by terminal sessions must keep using host
# libraries. Bundled Claude and OpenSSL use their own small loader wrappers.

set -e

# A shell can stay attached to a directory after that directory is deleted.
# Its cached $PWD still looks right, but every child then inherits a cwd that
# getcwd(3) cannot resolve. Recover before the bundle starts helper processes.
if ! pwd -P >/dev/null 2>&1; then
  cd "${HOME:-/}" 2>/dev/null || cd /
fi

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)

case "$(basename "$HERE")" in
  xd-nightly)
    export XD_APP_ID=com.restartfu.Xd.Nightly
    export XD_DATA_NAME=xd-nightly
    export XD_SETTINGS_PATH="${XDG_CONFIG_HOME:-${HOME}/.config}/xd-nightly/settings.json"
    ;;
  *)
    export XD_APP_ID=com.restartfu.Xd
    export XD_DATA_NAME=xd
    export XD_SETTINGS_PATH="${XDG_CONFIG_HOME:-${HOME}/.config}/xd/settings.json"
    ;;
esac

# Per bundle, not just per user: a nightly and a release installed side by side
# would otherwise rewrite each other's caches while both are running.
RUNTIME="${XDG_RUNTIME_DIR:-/tmp}/xd-$(id -u)/$(basename "$HERE")"
mkdir -p "$RUNTIME"

# These caches store absolute paths, so they are rewritten per launch from
# @BUNDLE@ templates; that is what keeps the bundle relocatable.
sed "s|@BUNDLE@|$HERE|g" "$HERE/etc/loaders.cache.in" > "$RUNTIME/loaders.cache"
sed "s|@BUNDLE@|$HERE|g" "$HERE/etc/fonts.conf.in"    > "$RUNTIME/fonts.conf"
sed "s|@BUNDLE@|$HERE|g" "$HERE/etc/egl_vendor.json.in" > "$RUNTIME/egl_vendor.json"

# Anything xd launches for the user -- a terminal, an editor -- must run in the
# host's environment, not the bundle's. Remember the values before they are
# overridden so they can be handed back to terminals, agents, and host tools.
export XD_HOST_PATH="${PATH-}"
export XD_HOST_XDG_DATA_DIRS="${XDG_DATA_DIRS-}"
export XD_HOST_LANG="${LANG-}"
export XD_HOST_LC_ALL="${LC_ALL-}"
export XD_HOST_LOCPATH="${LOCPATH-}"
export XD_HOST_LOCALE_ARCHIVE="${LOCALE_ARCHIVE-}"
export XD_HOST_GIO_EXTRA_MODULES="${GIO_EXTRA_MODULES-}"
export XD_HOST_GIO_MODULE_DIR="${GIO_MODULE_DIR-}"
export XD_HOST_GSETTINGS_SCHEMA_DIR="${GSETTINGS_SCHEMA_DIR-}"
export XD_HOST_GSETTINGS_BACKEND="${GSETTINGS_BACKEND-}"
export XD_HOST_GDK_PIXBUF_MODULE_FILE="${GDK_PIXBUF_MODULE_FILE-}"
export XD_HOST_GTK_IM_MODULE="${GTK_IM_MODULE-}"
export XD_HOST_GTK_IM_MODULE_FILE="${GTK_IM_MODULE_FILE-}"
export XD_HOST_GTK_MODULES="${GTK_MODULES-}"
export XD_HOST_GTK_PATH="${GTK_PATH-}"
export XD_HOST_GTK_THEME="${GTK_THEME-}"
export XD_HOST_GTK_DATA_PREFIX="${GTK_DATA_PREFIX-}"
export XD_HOST_GTK_EXE_PREFIX="${GTK_EXE_PREFIX-}"
export XD_HOST_GSK_RENDERER="${GSK_RENDERER-}"
export XD_HOST_XCURSOR_PATH="${XCURSOR_PATH-}"
export XD_HOST_FONTCONFIG_FILE="${FONTCONFIG_FILE-}"
export XD_HOST_FONTCONFIG_PATH="${FONTCONFIG_PATH-}"
export XD_HOST_XKB_CONFIG_ROOT="${XKB_CONFIG_ROOT-}"
export XD_HOST_XLOCALEDIR="${XLOCALEDIR-}"
export XD_HOST_SSL_CERT_FILE="${SSL_CERT_FILE-}"
export XD_HOST_OPENSSL_CONF="${OPENSSL_CONF-}"
export XD_HOST_OPENSSL_MODULES="${OPENSSL_MODULES-}"
export XD_HOST_GIT_EXEC_PATH="${GIT_EXEC_PATH-}"
export XD_HOST_GIT_TEMPLATE_DIR="${GIT_TEMPLATE_DIR-}"
export XD_HOST_GIT_SSL_CAINFO="${GIT_SSL_CAINFO-}"
export XD_HOST___EGL_VENDOR_LIBRARY_FILENAMES="${__EGL_VENDOR_LIBRARY_FILENAMES-}"
export XD_HOST_LIBGL_DRIVERS_PATH="${LIBGL_DRIVERS_PATH-}"
export XD_HOST_LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE-}"

# GNOME sessions export these to point GTK/GIO at host plugins (ibus, dconf,
# gvfs). Those .so files are built against the host's glib and GTK; dlopening
# them into the bundled stack mixes ABIs. xd needs none of them: it only
# touches local files and stores settings in the keyfile backend.
unset GIO_EXTRA_MODULES GTK_PATH GTK_MODULES GTK_IM_MODULE_FILE
unset GTK_EXE_PREFIX GTK_DATA_PREFIX LOCALE_ARCHIVE

# xd owns its GTK styling. Preserve host theme for child tools, but do not let
# it override the app stylesheet.
unset GTK_THEME
export GIO_MODULE_DIR="$HERE/lib/gio/modules"
export GTK_IM_MODULE=gtk-im-context-simple

# Both matter: without FONTCONFIG_PATH, fontconfig also reads the host's
# /etc/fonts. It still scans the conf.avail template dir compiled into the
# library (/usr/share/fontconfig), which on a non-Debian host may hold
# newer-format files -- harmless parse warnings on stderr, fonts still resolve.
export FONTCONFIG_PATH="$HERE/etc/fonts"
export FONTCONFIG_FILE="$RUNTIME/fonts.conf"

# Keymap data. A host without these (they are not standard outside X11
# installs) leaves GDK with no keymap, which crashes on the first key event.
export XKB_CONFIG_ROOT="$HERE/share/X11/xkb"
export XLOCALEDIR="$HERE/share/X11/locale"

# The bundle carries no compiled locale data, so anything but C.UTF-8 would
# fail in setlocale() and fall back to plain C -- losing UTF-8 handling along
# with it. C.UTF-8 is built into glibc and behaves correctly, at the cost of
# locale-specific number and date formatting.
export LC_ALL=C.UTF-8
export LANG=C.UTF-8
export LOCPATH="$HERE/share/locale-data"

export GDK_PIXBUF_MODULE_FILE="$RUNTIME/loaders.cache"
export GSETTINGS_SCHEMA_DIR="$HERE/share/glib-2.0/schemas"
export XDG_DATA_DIRS="$HERE/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export XCURSOR_PATH="$HERE/share/icons:${XCURSOR_PATH:-$HOME/.icons:/usr/share/icons}"
export PATH="$HERE/bin:${PATH:-/usr/local/bin:/usr/bin:/bin}"

# The Rust desktop and daemon are separate processes. Resolve every bundled
# helper here so neither process can accidentally select a stale host install.
export XD_DAEMON_EXECUTABLE="$HERE/libexec/xd-daemon"
export XD_TLS_PROXY_EXECUTABLE="$HERE/libexec/xd-tls-proxy"
export XD_CODEX_EXECUTABLE="$HERE/libexec/codex-package/bin/codex"
export XD_CLAUDE_EXECUTABLE="$HERE/libexec/claude"
export XD_CLAUDE_PROXY_EXECUTABLE="$HERE/libexec/claude-code-proxy"
export XD_WHISPER_SERVER="$HERE/libexec/whisper-server-bin"

# The keyfile backend keeps settings in $XDG_CONFIG_HOME/glib-2.0/settings and
# is built into GIO, so the bundle needs no dconf module or D-Bus round trip.
export GSETTINGS_BACKEND="${GSETTINGS_BACKEND:-keyfile}"
export SSL_CERT_FILE="$HERE/etc/ssl/certs/ca-certificates.crt"
export OPENSSL_CONF="$HERE/etc/ssl/openssl.cnf"
export OPENSSL_MODULES="$HERE/lib/ossl-modules"
export GIT_EXEC_PATH="$HERE/libexec/git-core"
export GIT_TEMPLATE_DIR="$HERE/share/git-core/templates"
export GIT_SSL_CAINFO="$HERE/etc/ssl/certs/ca-certificates.crt"

# Framework AMD laptops can hit kernel DMCUB/flip_done hangs under sustained
# GTK GL redraws. Use GTK's bounded cairo fallback on AMD unless the user made
# an explicit renderer choice. Other GPUs keep GTK's native default. This is
# an app mitigation; it does not rewrite host kernel or boot settings.
if [ -z "${GSK_RENDERER-}" ] && [ -z "${XD_HARDWARE_RENDERING-}" ]; then
  for vendor_file in /sys/class/drm/card*/device/vendor; do
    [ -r "$vendor_file" ] || continue
    vendor=
    IFS= read -r vendor < "$vendor_file" || true
    if [ "$vendor" = "0x1002" ]; then
      export GSK_RENDERER=cairo
      export XD_RENDER_SAFE_MODE=1
      break
    fi
  done
fi

# The bundle's own Mesa, driving the machine's GPU through the kernel's
# stable DRM interface -- the same arrangement Flatpak uses, so no host GL
# userland is ever consulted. Where there is no GPU (or an NVIDIA card that
# only the proprietary driver speaks for), Mesa falls back to its own
# software rasterizer by itself, and the picture stays identical either way.
# XD_SOFTWARE_GL=1 forces both GTK and Mesa software paths for comparison.
export __EGL_VENDOR_LIBRARY_FILENAMES="$RUNTIME/egl_vendor.json"
export LIBGL_DRIVERS_PATH="$HERE/lib/dri"
if [ -n "${XD_SOFTWARE_GL-}" ]; then
  export GSK_RENDERER=cairo
  export XD_RENDER_SAFE_MODE=1
  export LIBGL_ALWAYS_SOFTWARE=1
fi

exec "$HERE/lib/ld-linux-x86-64.so.2" \
     --library-path "$HERE/lib" \
     "$HERE/bin/xd" "$@"
