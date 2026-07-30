#!/usr/bin/env bash
#
# Assemble a relocatable bundle from an install staging tree.
#
#   bundle.sh <staging-dir> <out-dir> <launcher-template>
#
# The result is a directory that can be copied to any x86_64 Linux host and run
# via ./xd.sh, with zero dependency on the host's own GTK/glib stack.
#
# Paths that must be absolute at runtime are written as @BUNDLE@ placeholders
# and substituted by the launcher, which is what makes the tree relocatable.

set -euo pipefail

STAGE="${1:?staging dir}"
OUT="${2:?output dir}"
LAUNCHER="${3:?launcher template}"

ARCH_DIR=/usr/lib/x86_64-linux-gnu
PIXBUF_LOADERS="$ARCH_DIR/gdk-pixbuf-2.0/2.10.0/loaders"

mkdir -p "$OUT"/{bin,lib,libexec,share,etc}

# Deliberately empty: GIO_MODULE_DIR points here so the app cannot pick up the
# host's GIO modules. See scripts/xd.sh.
mkdir -p "$OUT/lib/gio/modules"
mkdir -p "$OUT/lib/ossl-modules"
cp -a "$ARCH_DIR"/ossl-modules/*.so "$OUT/lib/ossl-modules/" 2>/dev/null || true

install -Dm755 "$STAGE/usr/bin/xd" "$OUT/bin/xd"
install -Dm755 "$STAGE/usr/bin/git" "$OUT/bin/git"
cp -a "$STAGE/usr/libexec/." "$OUT/libexec/"
if [ -d "$STAGE/usr/lib" ]; then
  cp -a "$STAGE/usr/lib/." "$OUT/lib/"
fi
if [ -d "$STAGE/usr/share/git-core" ]; then
  mkdir -p "$OUT/share/git-core"
  cp -a "$STAGE/usr/share/git-core/." "$OUT/share/git-core/"
fi

# Git's compiled exec path points into Debian's /usr/lib. Rebuild that path
# inside the bundle. Scripts stay scripts; ELF helpers become symlinks to one
# loader wrapper, which preserves each helper's argv[0] before dispatch.
mkdir -p "$OUT/libexec/git-core"
for helper in "$OUT/libexec/git-core-real/"*; do
  name=$(basename "$helper")
  if [ -d "$helper" ]; then
    cp -a "$helper" "$OUT/libexec/git-core/$name"
  elif file -Lb "$(readlink -f "$helper")" | grep -q '^ELF '; then
    ln -s ../git-helper "$OUT/libexec/git-core/$name"
  else
    cp -aL "$helper" "$OUT/libexec/git-core/$name"
  fi
done

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
  "$OUT/bin/xd" \
  "$OUT/libexec/claude-bin" \
  "$OUT/libexec/git-bin" \
  "$OUT/libexec/git-core-real"/* \
  "$OUT/libexec/openssl-bin" \
  "$OUT/libexec/whisper-bin" \
  "$OUT/lib/ossl-modules"/*.so \
  "$OUT/lib/gdk-pixbuf-2.0/loaders"/*.so \
  "$ARCH_DIR"/libnss_files.so.2 \
  "$ARCH_DIR"/libnss_dns.so.2)

for root in "${roots[@]}"; do
  [ -e "$root" ] || continue
  file -Lb "$root" | grep -q '^ELF ' || continue
  LD_LIBRARY_PATH="$OUT/lib" \
    ldd "$root" 2>/dev/null | awk '/=> \//{print $3}'
done | sort -u | while read -r lib; do
  cp -Ln "$lib" "$OUT/lib/" 2>/dev/null || true
done

for extra in "$ARCH_DIR"/libnss_files.so.2 "$ARCH_DIR"/libnss_dns.so.2; do
  [ -e "$extra" ] && cp -Ln "$extra" "$OUT/lib/" || true
done

# The dynamic loader itself: the host may not have one at the usual path.
cp -L "$ARCH_DIR/ld-linux-x86-64.so.2" "$OUT/lib/ld-linux-x86-64.so.2"

# --- GSettings schemas (ours + GTK's) --------------------------------------
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

# --- MIME database ---------------------------------------------------------
# GdkPixbuf finds the SVG loader through its cache, but GIO must first classify
# the icon as image/svg+xml. Minimal hosts do not carry a shared MIME database.
cp -a /usr/share/mime "$OUT/share/mime"

# --- keyboard data ----------------------------------------------------------
# libxkbcommon compiles the keymap the compositor hands over against these
# files. Without them GDK ends up with no keymap at all and crashes on the
# first input event, so they are not optional.
mkdir -p "$OUT/share/X11"
cp -a /usr/share/X11/xkb "$OUT/share/X11/xkb"
[ -d /usr/share/X11/locale ] && cp -a /usr/share/X11/locale "$OUT/share/X11/locale"

# --- locale -----------------------------------------------------------------
# This glibc looks C.UTF-8 up as locale data rather than answering from
# within itself; without the file, setlocale falls back to plain C and UTF-8
# handling goes with it.
mkdir -p "$OUT/share/locale-data"
cp -a /usr/lib/locale/C.utf8 "$OUT/share/locale-data/"

# --- TLS --------------------------------------------------------------------
# GIO loads TLS from a module, not from itself; without this the daemon's
# sockets exist but cannot speak TLS on any machine.
mkdir -p "$OUT/lib/gio/modules"
cp -a "$ARCH_DIR"/gio/modules/libgiognutls.so "$OUT/lib/gio/modules/" 2>/dev/null || \
  cp -a /usr/lib/x86_64-linux-gnu/gio/modules/libgiognutls.so "$OUT/lib/gio/modules/"
for extra in $(ldd /usr/lib/x86_64-linux-gnu/gio/modules/libgiognutls.so \
                 | awk '/=> \//{print $3}'); do
  base=$(basename "$extra")
  [ -e "$OUT/lib/$base" ] || cp -a "$extra" "$OUT/lib/"
done

# --- software GL ------------------------------------------------------------
# Mesa's own llvmpipe, carried whole: rendering is then identical on every
# machine, and GTK's GL renderer never depends on what the host has. The
# vendor file needs an absolute path, so it is a template rewritten per
# launch like the caches above.
ARCH_LIB=/usr/lib/x86_64-linux-gnu
for lib in "$ARCH_LIB"/libEGL.so.1* "$ARCH_LIB"/libEGL_mesa.so.0* \
           "$ARCH_LIB"/libGLdispatch.so.0* "$ARCH_LIB"/libgbm.so.1* \
           "$ARCH_LIB"/libglapi.so.0* "$ARCH_LIB"/libGLESv2.so.2* \
           "$ARCH_LIB"/libGL.so.1* "$ARCH_LIB"/libGLX.so.0*; do
  [ -e "$lib" ] && cp -a "$lib" "$OUT/lib/"
done
mkdir -p "$OUT/lib/dri"
cp -aL "$ARCH_LIB"/dri/*.so "$OUT/lib/dri/" 2>/dev/null || true
for extra in $(ldd "$ARCH_LIB"/libEGL_mesa.so.0 "$OUT"/lib/dri/*.so 2>/dev/null \
                 | awk '/=> \//{print $3}' | sort -u); do
  base=$(basename "$extra")
  [ -e "$OUT/lib/$base" ] || cp -a "$extra" "$OUT/lib/"
done
cat > "$OUT/etc/egl_vendor.json.in" <<'JSON'
{
    "file_format_version" : "1.0.0",
    "ICD" : { "library_path" : "@BUNDLE@/lib/libEGL_mesa.so.0" }
}
JSON

# --- fonts + fontconfig -----------------------------------------------------
# FONTCONFIG_SYSROOT would isolate the config more thoroughly, but fontconfig
# reports FC_FILE without the sysroot prefix, so cairo then fails to open every
# font. Pointing FONTCONFIG_FILE/PATH at the bundle is the workable option.
mkdir -p "$OUT/share/fonts"
for dir in /usr/share/fonts/opentype/cantarell /usr/share/fonts/truetype/dejavu \
           /usr/share/fonts/truetype/inter /usr/share/fonts/opentype/inter \
           /usr/share/fonts/truetype/jetbrains-mono \
           /usr/share/fonts/truetype/noto; do
  [ -d "$dir" ] && cp -a "$dir" "$OUT/share/fonts/"
done
cp -a "$STAGE/usr/share/fonts/." "$OUT/share/fonts/"

# conf.d entries are symlinks into conf.avail; -L flattens them so the bundle
# stays self-contained.
mkdir -p "$OUT/etc/fonts"
cp -rL /etc/fonts/conf.d "$OUT/etc/fonts/conf.d"

cat > "$OUT/etc/fonts.conf.in" <<'EOF'
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <dir>@BUNDLE@/share/fonts</dir>
  <cachedir prefix="xdg">xd/fontconfig</cachedir>
  <include ignore_missing="yes">@BUNDLE@/etc/fonts/conf.d</include>
</fontconfig>
EOF

# --- certificate authorities -----------------------------------------------
mkdir -p "$OUT/etc/ssl/certs"
cp /etc/ssl/certs/ca-certificates.crt "$OUT/etc/ssl/certs/"
cp /etc/ssl/openssl.cnf "$OUT/etc/ssl/openssl.cnf"

# --- desktop metadata + launcher -------------------------------------------
mkdir -p "$OUT/share/applications"
cp -a "$STAGE/usr/share/applications/." "$OUT/share/applications/" 2>/dev/null || true

install -Dm755 "$LAUNCHER" "$OUT/xd.sh"

printf 'bundle: %s libs, %s\n' \
  "$(find "$OUT/lib" -maxdepth 1 -name '*.so*' | wc -l)" \
  "$(du -sh "$OUT" | cut -f1)"
