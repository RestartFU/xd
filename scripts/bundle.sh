#!/usr/bin/env bash
#
# Assemble a relocatable bundle from an install staging tree.
#
#   bundle.sh <staging-dir> <out-dir> <launcher-template>
#
# The result is a directory that can be copied to an x86_64 Linux desktop and
# run via ./xd.sh. The Vulkan loader is included, while glibc and the GPU driver
# come from the host because both are part of the host platform ABI.
#
# Paths that must be absolute at runtime are written as @BUNDLE@ placeholders
# and substituted by the launcher, which is what makes the tree relocatable.

set -euo pipefail

STAGE="${1:?staging dir}"
OUT="${2:?output dir}"
LAUNCHER="${3:?launcher template}"

ARCH_DIR=/usr/lib/x86_64-linux-gnu
mkdir -p "$OUT"/{bin,lib,libexec,share,etc}
mkdir -p "$OUT/lib/ossl-modules"
cp -a "$ARCH_DIR"/ossl-modules/*.so "$OUT/lib/ossl-modules/" 2>/dev/null || true

install -Dm755 "$STAGE/usr/bin/xd" "$OUT/bin/xd"
install -Dm755 "$STAGE/usr/bin/git" "$OUT/bin/git"
cp -a "$STAGE/usr/libexec/." "$OUT/libexec/"
# Git dispatches from argv[0]. Calling the stored `git-bin` name makes it look
# for a nonexistent `bin` builtin, so expose the payload through its real name.
ln -s git-bin "$OUT/libexec/git"
if [ -d "$STAGE/usr/lib" ]; then
  cp -a "$STAGE/usr/lib/." "$OUT/lib/"
fi
if [ -d "$STAGE/usr/share/git-core" ]; then
  mkdir -p "$OUT/share/git-core"
  cp -a "$STAGE/usr/share/git-core/." "$OUT/share/git-core/"
fi

# Git's compiled exec path points into Debian's /usr/lib. Rebuild that path
# inside the bundle. Scripts stay scripts; ELF helpers link directly to their
# bundled counterparts so the kernel naturally preserves each helper's name.
mkdir -p "$OUT/libexec/git-core"
for helper in "$OUT/libexec/git-core-real/"*; do
  name=$(basename "$helper")
  if [ -d "$helper" ]; then
    cp -a "$helper" "$OUT/libexec/git-core/$name"
  elif file -Lb "$(readlink -f "$helper")" | grep -q '^ELF '; then
    ln -s "../git-core-real/$name" "$OUT/libexec/git-core/$name"
  else
    cp -aL "$helper" "$OUT/libexec/git-core/$name"
  fi
done

# --- browser auxiliary processes and dynamically loaded modules ------------
# WebKit's Web/Network/GPU processes are separate ELF executables. Including
# only libwebkit leaves the first navigation broken on non-Debian desktops.
WEBKIT_DIR="$ARCH_DIR/webkit2gtk-4.1"
mkdir -p "$OUT/libexec/webkit2gtk-4.1"
for process in WebKitWebProcess WebKitNetworkProcess WebKitGPUProcess; do
  install -m755 "$WEBKIT_DIR/$process" "$OUT/libexec/webkit2gtk-4.1/$process"
done
cp -a "$WEBKIT_DIR/injected-bundle" "$OUT/libexec/webkit2gtk-4.1/"
patchelf --add-needed xd-webkit-paths.so "$OUT/bin/xd"

# HTTPS uses GIO's dynamically loaded TLS backend. GTK image/input modules and
# GStreamer plugins are also absent from ldd's main-executable dependency list.
mkdir -p "$OUT/lib/gio" "$OUT/lib/gdk-pixbuf-2.0" "$OUT/lib/gtk-3.0"
cp -a "$ARCH_DIR/gio/modules" "$OUT/lib/gio/"
cp -a "$ARCH_DIR/gdk-pixbuf-2.0/2.10.0/loaders" "$OUT/lib/gdk-pixbuf-2.0/"
cp -a "$ARCH_DIR/gtk-3.0/3.0.0/immodules" "$OUT/lib/gtk-3.0/"
sed "s|$ARCH_DIR/gdk-pixbuf-2.0/2.10.0/loaders|@BUNDLE@/lib/gdk-pixbuf-2.0/loaders|g" \
  "$ARCH_DIR/gdk-pixbuf-2.0/2.10.0/loaders.cache" > "$OUT/etc/pixbuf-loaders.cache.in"
sed "s|$ARCH_DIR/gtk-3.0/3.0.0/immodules|@BUNDLE@/lib/gtk-3.0/immodules|g" \
  "$ARCH_DIR/gtk-3.0/3.0.0/immodules.cache" > "$OUT/etc/gtk-immodules.cache.in"
cp -a "$ARCH_DIR/gstreamer-1.0" "$OUT/lib/"
install -m755 "$ARCH_DIR/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner" \
  "$OUT/libexec/gst-plugin-scanner"
mkdir -p "$OUT/share/glib-2.0"
cp -a /usr/share/glib-2.0/schemas "$OUT/share/glib-2.0/"
cp -a /usr/share/mime "$OUT/share/"

# --- shared library closure -------------------------------------------------
# ldd already resolves transitively, so one pass over every dlopen root is
# enough. glibc, its loader, and compiler runtimes deliberately stay on the
# host. GBM, DRM, and Wayland also stay with the graphics driver: newer Mesa
# drivers can require Wayland symbols absent from a bundled older library.
mapfile -t roots < <(printf '%s\n' \
  "$OUT/bin/xd" \
  "$OUT/libexec/xd-host" \
  "$OUT/libexec/tmux" \
  "$OUT/libexec/curl-bin" \
  "$OUT/libexec/git-bin" \
  "$OUT/libexec/git-core-real"/* \
  "$OUT/libexec/openssl-bin" \
  "$OUT/libexec/webkit2gtk-4.1"/WebKit*Process \
  "$OUT/libexec/webkit2gtk-4.1/injected-bundle"/*.so \
  "$OUT/libexec/gst-plugin-scanner" \
  "$OUT/lib/xd-webkit-paths.so" \
  "$OUT/lib/gio/modules"/*.so \
  "$OUT/lib/gdk-pixbuf-2.0/loaders"/*.so \
  "$OUT/lib/gtk-3.0/immodules"/*.so \
  "$OUT/lib/gstreamer-1.0"/*.so \
  "$OUT/lib/ossl-modules"/*.so)

for root in "${roots[@]}"; do
  [ -e "$root" ] || continue
  file -Lb "$root" | grep -q '^ELF ' || continue
  LD_LIBRARY_PATH="$OUT/lib" \
    ldd "$root" 2>/dev/null | awk '/=> \//{print $3}'
done | sort -u | while read -r lib; do
  case "$(basename "$lib")" in
    ld-linux-*.so.*|libc.so.*|libm.so.*|libdl.so.*|libpthread.so.*|\
      libresolv.so.*|librt.so.*|libanl.so.*|libutil.so.*|libnss_*.so.*|\
      libthread_db.so.*|libgcc_s.so.*|libstdc++.so.*|libgbm.so.*|libdrm*.so.*|\
      libwayland-*.so.*)
      continue
      ;;
  esac
  cp -Ln "$lib" "$OUT/lib/" 2>/dev/null || true
done

# Give every ELF object a path back to the private library directory. This is
# local to the object, unlike LD_LIBRARY_PATH, so terminals and agent CLIs
# launched by xd remain entirely in the host environment.
while IFS= read -r -d '' elf; do
  file -Lb "$elf" | grep -q '^ELF ' || continue
  relative_lib=$(realpath --relative-to="$(dirname "$elf")" "$OUT/lib")
  patchelf --set-rpath "\$ORIGIN/$relative_lib" "$elf"
done < <(find "$OUT/bin" "$OUT/lib" "$OUT/libexec" -type f -print0)

# --- application icon and cursor theme -------------------------------------
mkdir -p "$OUT/share/icons"
mkdir -p "$OUT/share/icons/Adwaita"
cp -a /usr/share/icons/Adwaita/index.theme "$OUT/share/icons/Adwaita/"
cp -a /usr/share/icons/Adwaita/cursors "$OUT/share/icons/Adwaita/"
mkdir -p "$OUT/share/icons/hicolor"
cp -a "$STAGE/usr/share/icons/hicolor/." "$OUT/share/icons/hicolor/"

# --- keyboard data ----------------------------------------------------------
# libxkbcommon compiles the keymap the compositor hands over against these
# files. Without them the window backend has no keymap, so they are not
# optional.
mkdir -p "$OUT/share/X11"
cp -a /usr/share/X11/xkb "$OUT/share/X11/xkb"
[ -d /usr/share/X11/locale ] && cp -a /usr/share/X11/locale "$OUT/share/X11/locale"

ARCH_LIB=/usr/lib/x86_64-linux-gnu
# --- Vulkan loader ----------------------------------------------------------
# GPUI opens the loader dynamically, so ldd cannot see it. Drivers stay on the
# host: bundling every Mesa hardware driver also pulled in LLVM and Z3, adding
# over 200 MiB of machine-specific graphics code to the application.
cp -aL "$ARCH_LIB"/libvulkan.so.1 "$OUT/lib/"

# --- fonts + fontconfig -----------------------------------------------------
# FONTCONFIG_SYSROOT would isolate the config more thoroughly, but fontconfig
# reports FC_FILE without the sysroot prefix, so cairo then fails to open every
# font. Pointing FONTCONFIG_FILE/PATH at the bundle is the workable option.
mkdir -p "$OUT/share/fonts"
for dir in /usr/share/fonts/truetype/dejavu \
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
mkdir -p "$OUT/share/licenses"
cp -a "$STAGE/usr/share/licenses/." "$OUT/share/licenses/"
for package in libwebkit2gtk-4.1-0 libjavascriptcoregtk-4.1-0 \
               libgtk-3-0t64 glib-networking \
               gstreamer1.0-plugins-base gstreamer1.0-plugins-good; do
  install -m644 "/usr/share/doc/$package/copyright" \
    "$OUT/share/licenses/$package-copyright"
done

install -Dm755 "$LAUNCHER" "$OUT/xd.sh"

printf 'bundle: %s libs, %s\n' \
  "$(find "$OUT/lib" -maxdepth 1 -name '*.so*' | wc -l)" \
  "$(du -sh "$OUT" | cut -f1)"
