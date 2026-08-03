#!/usr/bin/env sh
#
# Create the platform-neutral install tree consumed by native bundlers.
#
#   stage-native.sh <crystal-xd-binary> <staging-directory> [nightly|release]
#
# The destination must be empty. Native bundle scripts add shared libraries,
# platform tools, and installer metadata from one Crystal build output.

set -eu

BINARY=${1:?Crystal xd binary}
STAGE=${2:?empty staging directory}
PROFILE=${3:-nightly}
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)

case "$PROFILE" in
  nightly)
    APP_ID=com.restartfu.Xd.Nightly
    APP_NAME='xd (Nightly)'
    SETTINGS_PATH=/com/restartfu/XdNightly/
    ;;
  release)
    APP_ID=com.restartfu.Xd
    APP_NAME=xd
    SETTINGS_PATH=/com/restartfu/Hy/
    ;;
  *)
    echo "stage-native: profile must be nightly or release" >&2
    exit 1
    ;;
esac

[ -f "$BINARY" ] || {
  echo "stage-native: binary not found: $BINARY" >&2
  exit 1
}

[ "$("$BINARY" --bundle-runtime)" = crystal ] || {
  echo "stage-native: input is not the Crystal rewrite" >&2
  exit 1
}

if [ -d "$STAGE" ] && [ -n "$(find "$STAGE" -mindepth 1 -print -quit)" ]; then
  echo "stage-native: destination must be empty: $STAGE" >&2
  exit 1
fi

case "$BINARY" in
  *.exe) EXECUTABLE=xd.exe ;;
  *) EXECUTABLE=xd ;;
esac

mkdir -p \
  "$STAGE/bin" \
  "$STAGE/share/applications" \
  "$STAGE/share/fonts/xd" \
  "$STAGE/share/glib-2.0/schemas" \
  "$STAGE/share/icons/hicolor/scalable/apps" \
  "$STAGE/share/icons/hicolor/symbolic/apps" \
  "$STAGE/share/licenses/xd"

install -m0755 "$BINARY" "$STAGE/bin/$EXECUTABLE"
if [ "$EXECUTABLE" = xd.exe ]; then
  for library in "$(dirname "$BINARY")"/*.dll; do
    [ -e "$library" ] || continue
    install -m0755 "$library" "$STAGE/bin/"
  done
fi
install -m0644 \
  "$ROOT/data/fonts/DMSans-Variable.ttf" \
  "$STAGE/share/fonts/xd/DMSans-Variable.ttf"
install -m0644 \
  "$ROOT/data/fonts/OFL.txt" \
  "$STAGE/share/fonts/xd/OFL.txt"
install -m0644 \
  "$ROOT/data/licenses/claude-code-proxy-LICENSE" \
  "$STAGE/share/licenses/xd/claude-code-proxy-LICENSE"

sed \
  -e "s|@APP_ID@|$APP_ID|g" \
  -e "s|@APP_NAME@|$APP_NAME|g" \
  "$ROOT/data/com.restartfu.Xd.desktop.in" \
  > "$STAGE/share/applications/$APP_ID.desktop"

sed \
  -e "s|@APP_ID@|$APP_ID|g" \
  -e "s|@SETTINGS_PATH@|$SETTINGS_PATH|g" \
  "$ROOT/data/com.restartfu.Xd.gschema.xml.in" \
  > "$STAGE/share/glib-2.0/schemas/$APP_ID.gschema.xml"

install -m0644 \
  "$ROOT/data/icons/hicolor/scalable/apps/com.restartfu.Xd.svg" \
  "$STAGE/share/icons/hicolor/scalable/apps/$APP_ID.svg"
install -m0644 \
  "$ROOT/data/icons/hicolor/scalable/apps/xd-backend-claude.svg" \
  "$STAGE/share/icons/hicolor/scalable/apps/xd-backend-claude.svg"
install -m0644 \
  "$ROOT/data/icons/hicolor/scalable/apps/xd-backend-claude-mode.svg" \
  "$STAGE/share/icons/hicolor/scalable/apps/xd-backend-claude-mode.svg"
install -m0644 \
  "$ROOT/data/icons/hicolor/symbolic/apps/xd-backend-codex-symbolic.svg" \
  "$STAGE/share/icons/hicolor/symbolic/apps/xd-backend-codex-symbolic.svg"
install -m0644 \
  "$ROOT/data/icons/hicolor/symbolic/apps/xd-download-symbolic.svg" \
  "$STAGE/share/icons/hicolor/symbolic/apps/xd-download-symbolic.svg"

glib-compile-schemas "$STAGE/share/glib-2.0/schemas"

printf 'native Crystal stage: %s\n' "$STAGE"
