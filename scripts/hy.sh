#!/bin/sh
#
# Launcher for the relocatable hy bundle.
#
# Everything the app needs is loaded out of this directory. The bundled loader
# is invoked with --library-path rather than exporting LD_LIBRARY_PATH on
# purpose: hy spawns the host's `claude` and `codex` binaries, and those must
# keep using the host's own libraries.

set -e

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)

RUNTIME="${XDG_RUNTIME_DIR:-/tmp}/hy-$(id -u)"
mkdir -p "$RUNTIME"

# These caches store absolute paths, so they are rewritten per launch from
# @BUNDLE@ templates; that is what keeps the bundle relocatable.
sed "s|@BUNDLE@|$HERE|g" "$HERE/etc/loaders.cache.in" > "$RUNTIME/loaders.cache"
sed "s|@BUNDLE@|$HERE|g" "$HERE/etc/fonts.conf.in"    > "$RUNTIME/fonts.conf"

# GNOME sessions export these to point GTK/GIO at host plugins (ibus, dconf,
# gvfs). Those .so files are built against the host's glib and GTK; dlopening
# them into the bundled stack mixes ABIs. hy needs none of them: it only
# touches local files and stores settings in the keyfile backend.
unset GIO_EXTRA_MODULES GTK_PATH GTK_MODULES GTK_IM_MODULE_FILE
unset GTK_EXE_PREFIX GTK_DATA_PREFIX LOCALE_ARCHIVE
export GIO_MODULE_DIR="$HERE/lib/gio/modules"
export GTK_IM_MODULE=gtk-im-context-simple

# Both matter: without FONTCONFIG_PATH, fontconfig also reads the host's
# /etc/fonts. It still scans the conf.avail template dir compiled into the
# library (/usr/share/fontconfig), which on a non-Debian host may hold
# newer-format files -- harmless parse warnings on stderr, fonts still resolve.
export FONTCONFIG_PATH="$HERE/etc/fonts"
export FONTCONFIG_FILE="$RUNTIME/fonts.conf"

export GDK_PIXBUF_MODULE_FILE="$RUNTIME/loaders.cache"
export GSETTINGS_SCHEMA_DIR="$HERE/share/glib-2.0/schemas"
export XDG_DATA_DIRS="$HERE/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export XCURSOR_PATH="$HERE/share/icons:${XCURSOR_PATH:-$HOME/.icons:/usr/share/icons}"

# The keyfile backend keeps settings in $XDG_CONFIG_HOME/glib-2.0/settings and
# is built into GIO, so the bundle needs no dconf module or D-Bus round trip.
export GSETTINGS_BACKEND="${GSETTINGS_BACKEND:-keyfile}"

# Software rendering: the bundled Mesa/GL stack would otherwise have to match
# the host's kernel drivers.
export GSK_RENDERER="${GSK_RENDERER:-cairo}"

exec "$HERE/lib/ld-linux-x86-64.so.2" \
     --library-path "$HERE/lib" \
     "$HERE/bin/hy" "$@"
