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
FONTCONFIG_HERE=$(printf '%s' "$HERE" | sed \
  -e 's/&/\&amp;/g' \
  -e 's/</\&lt;/g' \
  -e 's/>/\&gt;/g' \
  -e 's/"/\&quot;/g' \
  -e "s/'/\&apos;/g")
while IFS= read -r template_line || [ -n "$template_line" ]; do
  template_rest=$template_line
  while [ "${template_rest#*@BUNDLE@}" != "$template_rest" ]; do
    template_prefix=${template_rest%%@BUNDLE@*}
    printf '%s%s' "$template_prefix" "$FONTCONFIG_HERE"
    template_rest=${template_rest#*@BUNDLE@}
  done
  printf '%s\n' "$template_rest"
done < "$HERE/etc/fonts.conf.in" > "$RUNTIME/fonts.conf"

# GTK caches quote module paths. Escape those quotes, then replace the marker
# literally so ampersands in an installation path keep their original value.
BROWSER_HERE=$(printf '%s' "$HERE" | sed 's/[\\"]/\\&/g')
for browser_cache in pixbuf-loaders gtk-immodules; do
  while IFS= read -r template_line || [ -n "$template_line" ]; do
    template_rest=$template_line
    while [ "${template_rest#*@BUNDLE@}" != "$template_rest" ]; do
      template_prefix=${template_rest%%@BUNDLE@*}
      printf '%s%s' "$template_prefix" "$BROWSER_HERE"
      template_rest=${template_rest#*@BUNDLE@}
    done
    printf '%s\n' "$template_rest"
  done < "$HERE/etc/$browser_cache.cache.in" > "$RUNTIME/$browser_cache.cache"
done

# Anything xd launches for the user -- a terminal, an editor -- must run in the
# host's environment, not the bundle's. Remember the values before they are
# overridden so they can be handed back to terminals, agents, and host tools.
export XD_HOST_PATH="${PATH-}"
export XD_HOST_XDG_DATA_DIRS="${XDG_DATA_DIRS-}"
export XD_HOST_LANG="${LANG-}"
export XD_HOST_LC_ALL="${LC_ALL-}"
export XD_HOST_LOCPATH="${LOCPATH-}"
export XD_HOST_XD_BROWSER_BUNDLE_ROOT="${XD_BROWSER_BUNDLE_ROOT-}"
export XD_HOST_XD_BROWSER_RUNTIME_ROOT="${XD_BROWSER_RUNTIME_ROOT-}"
export XD_HOST_WEBKIT_INJECTED_BUNDLE_PATH="${WEBKIT_INJECTED_BUNDLE_PATH-}"
export XD_HOST_GIO_MODULE_DIR="${GIO_MODULE_DIR-}"
export XD_HOST_GSETTINGS_SCHEMA_DIR="${GSETTINGS_SCHEMA_DIR-}"
export XD_HOST_GDK_PIXBUF_MODULE_FILE="${GDK_PIXBUF_MODULE_FILE-}"
export XD_HOST_GTK_IM_MODULE_FILE="${GTK_IM_MODULE_FILE-}"
export XD_HOST_GST_PLUGIN_SYSTEM_PATH_1_0="${GST_PLUGIN_SYSTEM_PATH_1_0-}"
export XD_HOST_GST_PLUGIN_SCANNER_1_0="${GST_PLUGIN_SCANNER_1_0-}"
export XD_HOST_GST_REGISTRY_1_0="${GST_REGISTRY_1_0-}"

# WebKit's helper launcher is relocated by the linked xd-webkit-paths shim.
# Keep the browser sandbox enabled; bubblewrap, its D-Bus proxy, GBM, DRM, and
# Wayland remain host components along with the graphics driver.
export XD_BROWSER_BUNDLE_ROOT="$HERE"
export XD_BROWSER_RUNTIME_ROOT="$RUNTIME"
export WEBKIT_INJECTED_BUNDLE_PATH="$HERE/libexec/webkit2gtk-4.1/injected-bundle"
export GIO_MODULE_DIR="$HERE/lib/gio/modules"
export GSETTINGS_SCHEMA_DIR="$HERE/share/glib-2.0/schemas"
export GDK_PIXBUF_MODULE_FILE="$RUNTIME/pixbuf-loaders.cache"
export GTK_IM_MODULE_FILE="$RUNTIME/gtk-immodules.cache"
export GST_PLUGIN_SYSTEM_PATH_1_0="$HERE/lib/gstreamer-1.0"
export GST_PLUGIN_SCANNER_1_0="$HERE/libexec/gst-plugin-scanner"
export GST_REGISTRY_1_0="$RUNTIME/gstreamer-registry.bin"

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
