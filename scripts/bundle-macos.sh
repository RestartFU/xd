#!/usr/bin/env bash
#
# Assemble a relocatable Apple Silicon .app from a Meson install prefix.
#
#   bundle-macos.sh <installed-prefix> <output-directory> [nightly|release]

set -euo pipefail

PREFIX="${1:?installed prefix}"
OUT="${2:?output directory}"
PROFILE="${3:-nightly}"
BUNDLER_COMMIT=fbc6ffd1590cec6fef6e17ec19b6aa00ce01ca6d

[ "$(uname -s)" = "Darwin" ] || {
  echo "bundle-macos: macOS is required" >&2
  exit 1
}

case "$PROFILE" in
  nightly)
    BUNDLE_NAME=xd-nightly
    DISPLAY_NAME="xd (Nightly)"
    APP_ID=com.restartfu.Xd.Nightly
    ;;
  release)
    BUNDLE_NAME=xd
    DISPLAY_NAME=xd
    APP_ID=com.restartfu.Xd
    ;;
  *)
    echo "bundle-macos: profile must be nightly or release" >&2
    exit 1
    ;;
esac

PREFIX="$(cd "$PREFIX" && pwd)"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
HOMEBREW_PREFIX="$(brew --prefix)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/xd-macos.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT INT TERM

git clone --quiet https://gitlab.gnome.org/GNOME/gtk-mac-bundler.git \
  "$WORK/gtk-mac-bundler"
git -C "$WORK/gtk-mac-bundler" checkout --quiet "$BUNDLER_COMMIT"
sed "s|@PATH@|$WORK/gtk-mac-bundler|g" \
  "$WORK/gtk-mac-bundler/gtk-mac-bundler.in" \
  > "$WORK/gtk-mac-bundler-command"
chmod +x "$WORK/gtk-mac-bundler-command"

sed \
  -e "s|@BUNDLE_NAME@|$BUNDLE_NAME|g" \
  -e "s|@DISPLAY_NAME@|$DISPLAY_NAME|g" \
  -e "s|@APP_ID@|$APP_ID|g" \
  installer/macos/Info.plist.in > "$WORK/Info.plist"

# Finder needs a real .icns, not the SVG used by Linux desktops.
ICONSET="$WORK/xd.iconset"
mkdir -p "$ICONSET"
rsvg-convert -w 1024 -h 1024 \
  data/icons/hicolor/scalable/apps/com.restartfu.Xd.svg \
  > "$WORK/icon-1024.png"
for specification in \
  "16 icon_16x16.png" "32 icon_16x16@2x.png" \
  "32 icon_32x32.png" "64 icon_32x32@2x.png" \
  "128 icon_128x128.png" "256 icon_128x128@2x.png" \
  "256 icon_256x256.png" "512 icon_256x256@2x.png" \
  "512 icon_512x512.png" "1024 icon_512x512@2x.png"; do
  size="${specification%% *}"
  name="${specification#* }"
  sips -z "$size" "$size" "$WORK/icon-1024.png" \
    --out "$ICONSET/$name" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$WORK/xd.icns"

export HOMEBREW_PREFIX
export XD_MACOS_PREFIX="$PREFIX"
export XD_MACOS_DEST="$OUT"
export XD_MACOS_PLIST="$WORK/Info.plist"
export XD_MACOS_ICON="$WORK/xd.icns"
"$WORK/gtk-mac-bundler-command" installer/macos/xd.bundle

APP="$OUT/$BUNDLE_NAME.app"
RESOURCES="$APP/Contents/Resources"
[ -x "$APP/Contents/MacOS/xd" ] || {
  echo "bundle-macos: bundler did not produce $APP" >&2
  exit 1
}

# Data not modeled by gtk-mac-bundler itself.
for directory in gtk-4.0 libadwaita-1 themes; do
  if [ -d "$HOMEBREW_PREFIX/share/$directory" ]; then
    mkdir -p "$RESOURCES/share/$directory"
    cp -a "$HOMEBREW_PREFIX/share/$directory/." \
      "$RESOURCES/share/$directory/"
  fi
done

mkdir -p "$RESOURCES/share/fonts/xd" "$RESOURCES/etc/fonts"
cp -a data/fonts/. "$RESOURCES/share/fonts/xd/"
cp -RL "$HOMEBREW_PREFIX/etc/fonts/conf.d" "$RESOURCES/etc/fonts/"
cp installer/macos/fonts.conf.in "$RESOURCES/etc/fonts.conf.in"

glib-compile-schemas "$RESOURCES/share/glib-2.0/schemas"
gio-querymodules "$RESOURCES/lib/gio/modules"

# Cache paths must follow the .app after installation. main.c expands this
# template against Contents/Resources before GTK starts.
PIXBUF_LOADERS="$RESOURCES/lib/gdk-pixbuf-2.0/2.10.0/loaders"
QUERY_LOADERS="$(pkg-config --variable=gdk_pixbuf_query_loaders gdk-pixbuf-2.0)"
GDK_PIXBUF_MODULEDIR="$PIXBUF_LOADERS" "$QUERY_LOADERS" |
  sed "s|$RESOURCES|@BUNDLE@|g" \
  > "$RESOURCES/etc/gdk-pixbuf-loaders.cache.in"

# Ad-hoc signing catches malformed bundles and keeps every nested Mach-O under
# one consistent identity. Release notarization can replace this later.
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
"$APP/Contents/MacOS/xd" --version

printf 'macOS bundle: %s\n' "$(du -sh "$APP" | cut -f1)"
