#!/usr/bin/env bash
#
# Assemble a relocatable bundle from a `meson install` staging tree.
#
#   bundle.sh <staging-dir> <out-dir> <launcher-template>
#
# The result is a directory that can be copied to any x86_64 Linux host and run
# via ./hy.sh, with zero dependency on the host's own GTK/glib stack.
#
# Paths that must be absolute at runtime are written as @BUNDLE@ placeholders
# and substituted by the launcher, which is what makes the tree relocatable.

set -euo pipefail

STAGE="${1:?staging dir}"
OUT="${2:?output dir}"
LAUNCHER="${3:?launcher template}"

ARCH_DIR=/usr/lib/x86_64-linux-gnu
PIXBUF_LOADERS="$ARCH_DIR/gdk-pixbuf-2.0/2.10.0/loaders"

mkdir -p "$OUT"/{bin,lib,share,etc}

# Deliberately empty: GIO_MODULE_DIR points here so the app cannot pick up the
# host's GIO modules. See scripts/hy.sh.
mkdir -p "$OUT/lib/gio/modules"

install -Dm755 "$STAGE/usr/bin/hy" "$OUT/bin/hy"

# --- gdk-pixbuf loaders (dlopened, so they are extra closure roots) ---------
mkdir -p "$OUT/lib/gdk-pixbuf-2.0/loaders"
cp -a "$PIXBUF_LOADERS"/*.so "$OUT/lib/gdk-pixbuf-2.0/loaders/"

# Debian keeps the query tool out of $PATH, under the gdk-pixbuf libdir.
QUERY_LOADERS=$(command -v gdk-pixbuf-query-loaders \
  || echo "$ARCH_DIR/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders")

"$QUERY_LOADERS" \
  | sed "s|$PIXBUF_LOADERS|@BUNDLE@/lib/gdk-pixbuf-2.0/loaders|g" \
  > "$OUT/etc/loaders.cache.in"

# --- shared library closure -------------------------------------------------
# ldd already resolves transitively, so one pass over every dlopen root is
# enough. NSS modules are added by hand: they are opened by name, never linked.
mapfile -t roots < <(printf '%s\n' \
  "$OUT/bin/hy" \
  "$OUT/lib/gdk-pixbuf-2.0/loaders"/*.so \
  "$ARCH_DIR"/libnss_files.so.2 \
  "$ARCH_DIR"/libnss_dns.so.2)

for root in "${roots[@]}"; do
  [ -e "$root" ] || continue
  ldd "$root" 2>/dev/null | awk '/=> \//{print $3}'
done | sort -u | while read -r lib; do
  cp -Ln "$lib" "$OUT/lib/" 2>/dev/null || true
done

for extra in "$ARCH_DIR"/libnss_files.so.2 "$ARCH_DIR"/libnss_dns.so.2; do
  [ -e "$extra" ] && cp -Ln "$extra" "$OUT/lib/" || true
done

# The dynamic loader itself: the host may not have one at the usual path.
cp -L "$ARCH_DIR/ld-linux-x86-64.so.2" "$OUT/lib/ld-linux-x86-64.so.2"

# --- GSettings schemas (ours + GTK's + libadwaita's) ------------------------
mkdir -p "$OUT/share/glib-2.0/schemas"
cp -a /usr/share/glib-2.0/schemas/*.xml "$OUT/share/glib-2.0/schemas/" 2>/dev/null || true
cp -a "$STAGE/usr/share/glib-2.0/schemas/"*.xml "$OUT/share/glib-2.0/schemas/"
glib-compile-schemas "$OUT/share/glib-2.0/schemas"

# --- icon themes ------------------------------------------------------------
mkdir -p "$OUT/share/icons"
cp -a /usr/share/icons/Adwaita "$OUT/share/icons/"
cp -a /usr/share/icons/hicolor "$OUT/share/icons/"
cp -a "$STAGE/usr/share/icons/hicolor/." "$OUT/share/icons/hicolor/"
gtk4-update-icon-cache -q -t -f "$OUT/share/icons/hicolor" 2>/dev/null || true

# --- keyboard data ----------------------------------------------------------
# libxkbcommon compiles the keymap the compositor hands over against these
# files. Without them GDK ends up with no keymap at all and crashes on the
# first input event, so they are not optional.
mkdir -p "$OUT/share/X11"
cp -a /usr/share/X11/xkb "$OUT/share/X11/xkb"
[ -d /usr/share/X11/locale ] && cp -a /usr/share/X11/locale "$OUT/share/X11/locale"

# --- fonts + fontconfig -----------------------------------------------------
# FONTCONFIG_SYSROOT would isolate the config more thoroughly, but fontconfig
# reports FC_FILE without the sysroot prefix, so cairo then fails to open every
# font. Pointing FONTCONFIG_FILE/PATH at the bundle is the workable option.
mkdir -p "$OUT/share/fonts"
for dir in /usr/share/fonts/opentype/cantarell /usr/share/fonts/truetype/dejavu \
           /usr/share/fonts/truetype/inter /usr/share/fonts/opentype/inter; do
  [ -d "$dir" ] && cp -a "$dir" "$OUT/share/fonts/"
done

# conf.d entries are symlinks into conf.avail; -L flattens them so the bundle
# stays self-contained.
mkdir -p "$OUT/etc/fonts"
cp -rL /etc/fonts/conf.d "$OUT/etc/fonts/conf.d"

cat > "$OUT/etc/fonts.conf.in" <<'EOF'
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <dir>@BUNDLE@/share/fonts</dir>
  <cachedir prefix="xdg">hy/fontconfig</cachedir>
  <include ignore_missing="yes">@BUNDLE@/etc/fonts/conf.d</include>
</fontconfig>
EOF

# --- desktop metadata + launcher -------------------------------------------
mkdir -p "$OUT/share/applications"
cp -a "$STAGE/usr/share/applications/." "$OUT/share/applications/" 2>/dev/null || true

install -Dm755 "$LAUNCHER" "$OUT/hy.sh"

printf 'bundle: %s libs, %s\n' \
  "$(find "$OUT/lib" -maxdepth 1 -name '*.so*' | wc -l)" \
  "$(du -sh "$OUT" | cut -f1)"
