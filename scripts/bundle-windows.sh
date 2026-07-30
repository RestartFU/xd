#!/usr/bin/env bash
#
# Assemble a relocatable Windows payload from a Crystal native stage.
#
#   bundle-windows.sh <staging-dir> <out-dir>
#
# Run inside an MSYS2 UCRT64 shell. The MSI harvests the resulting directory.

set -euo pipefail

STAGE="${1:?staging dir}"
OUT="${2:?output dir}"
PREFIX="${MINGW_PREFIX:?MINGW_PREFIX is not set}"
STAGED_EXE="$(find "$STAGE" -type f -path '*/bin/xd.exe' -print -quit)"

if [ -z "$STAGED_EXE" ]; then
  echo "bundle-windows: staged xd.exe not found under $STAGE" >&2
  exit 1
fi

# Meson canonicalizes /ucrt64 to its native drive path before DESTDIR is
# applied, so the staged prefix is not necessarily "$STAGE$MINGW_PREFIX".
STAGED_PREFIX="${STAGED_EXE%/bin/xd.exe}"

[ "$("$STAGED_EXE" --bundle-runtime)" = crystal ] || {
  echo "bundle-windows: refusing legacy C binary" >&2
  exit 1
}

mkdir -p "$OUT"/{bin,etc,lib,libexec,share}

install -Dm755 "$STAGED_EXE" "$OUT/bin/xd.exe"
cp -a "$STAGED_PREFIX/share/." "$OUT/share/"
cp -a "$STAGED_PREFIX/libexec/." "$OUT/libexec/"
cp -a "$STAGED_PREFIX/git" "$OUT/git"

# GSettings schemas used by GTK/libadwaita plus xd's installed schema.
mkdir -p "$OUT/share/glib-2.0/schemas"
cp -a "$PREFIX/share/glib-2.0/schemas/"*.xml \
  "$OUT/share/glib-2.0/schemas/" 2>/dev/null || true
cp -a "$STAGED_PREFIX/share/glib-2.0/schemas/"*.xml \
  "$OUT/share/glib-2.0/schemas/"
glib-compile-schemas "$OUT/share/glib-2.0/schemas"

# Symbolic icons are runtime data, not resources compiled into GTK.
mkdir -p "$OUT/share/icons"
for theme in Adwaita hicolor; do
  if [ -d "$PREFIX/share/icons/$theme" ]; then
    cp -a "$PREFIX/share/icons/$theme" "$OUT/share/icons/"
  fi
done
cp -a "$STAGED_PREFIX/share/icons/hicolor/." \
  "$OUT/share/icons/hicolor/" 2>/dev/null || true

# Keep GTK/libadwaita support data that their Windows builds install outside
# DLL resources.
for data_dir in gtk-4.0 libadwaita-1 themes; do
  if [ -d "$PREFIX/share/$data_dir" ]; then
    cp -a "$PREFIX/share/$data_dir" "$OUT/share/"
  fi
done

mkdir -p "$OUT/share/fonts/dmsans"
cp -a data/fonts/. "$OUT/share/fonts/dmsans/"

# GIO loads TLS and proxy support dynamically.
mkdir -p "$OUT/lib/gio/modules"
cp -a "$PREFIX/lib/gio/modules/"*.dll "$OUT/lib/gio/modules/" \
  2>/dev/null || true

# Image loaders are also dynamic. Store a relocatable cache template; main.c
# expands @BUNDLE@ to the actual MSI installation directory before GTK starts.
PIXBUF_LOADERS="$(pkg-config --variable=gdk_pixbuf_moduledir gdk-pixbuf-2.0)"
QUERY_LOADERS="$(pkg-config --variable=gdk_pixbuf_query_loaders gdk-pixbuf-2.0)"
OUT_LOADERS="$OUT/lib/gdk-pixbuf-2.0/2.10.0/loaders"

mkdir -p "$OUT_LOADERS"
cp -a "$PIXBUF_LOADERS/"*.dll "$OUT_LOADERS/"

GDK_PIXBUF_MODULEDIR="$OUT_LOADERS" "$QUERY_LOADERS" |
  sed -E \
    's|^".*[/\\](libpixbufloader-[^/\\"]+\.dll)"$|"@BUNDLE@/lib/gdk-pixbuf-2.0/2.10.0/loaders/\1"|' \
  > "$OUT/etc/gdk-pixbuf-loaders.cache.in"

# Linked DLL closure. ldd resolves transitively; dynamic module roots are added
# because they do not appear in xd.exe's own import table.
{
  ldd "$OUT/bin/xd.exe" 2>/dev/null || true
  for module in "$OUT_LOADERS/"*.dll "$OUT/lib/gio/modules/"*.dll; do
    [ -e "$module" ] && ldd "$module" 2>/dev/null || true
  done
  for tool in "$OUT/libexec/"*.exe \
    "$OUT/libexec/codex-package/bin/"*.exe; do
    [ -e "$tool" ] && ldd "$tool" 2>/dev/null || true
  done
} |
  awk -v prefix="$PREFIX/bin/" '$3 ~ ("^" prefix) { print $3 }' |
  sort -u |
  while read -r dll; do
    cp -a "$dll" "$OUT/bin/"
  done

"$OUT/bin/xd.exe" --validate-native-bundle windows "$OUT"

printf 'windows bundle: %s DLLs, %s\n' \
  "$(find "$OUT/bin" -maxdepth 1 -name '*.dll' | wc -l)" \
  "$(du -sh "$OUT" | cut -f1)"
