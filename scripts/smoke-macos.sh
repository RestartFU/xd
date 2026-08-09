#!/usr/bin/env bash

set -euo pipefail

APP="${1:?macOS app bundle}"

[ "$(uname -s)" = Darwin ] || {
  echo "smoke-macos: macOS is required" >&2
  exit 1
}
[ -x "$APP/Contents/MacOS/xd" ]
[ -x "$APP/Contents/MacOS/xd-desktop" ]
[ -x "$APP/Contents/Resources/libexec/xd-daemon" ]
[ -x "$APP/Contents/Resources/libexec/xd-tls-proxy" ]
[ -x "$APP/Contents/Resources/libexec/install.sh" ]
[ -x "$APP/Contents/Resources/libexec/codex-package/bin/codex" ]
[ -x "$APP/Contents/Resources/libexec/claude" ]
[ -x "$APP/Contents/Resources/libexec/claude-code-proxy" ]
[ -x "$APP/Contents/Resources/libexec/whisper-server-bin" ]
[ -f "$APP/Contents/Resources/xd.icns" ]

plutil -lint "$APP/Contents/Info.plist"
[ "$(plutil -extract CFBundleIconFile raw "$APP/Contents/Info.plist")" = xd.icns ]
case "$(plutil -extract CFBundleVersion raw "$APP/Contents/Info.plist")" in
  ''|*[!0-9.]*) exit 1 ;;
esac
ICONSET=$(mktemp -d "${TMPDIR:-/tmp}/xd-icon-smoke.XXXXXX")
trap 'rm -rf "$ICONSET"' EXIT INT TERM
iconutil -c iconset "$APP/Contents/Resources/xd.icns" -o "$ICONSET"
[ -f "$ICONSET/icon_16x16.png" ]
[ -f "$ICONSET/icon_512x512@2x.png" ]
codesign --verify --deep --strict "$APP"
"$APP/Contents/MacOS/xd" --version

expected=$(uname -m)
for binary in \
  "$APP/Contents/MacOS/xd-desktop" \
  "$APP/Contents/Resources/libexec/xd-daemon" \
  "$APP/Contents/Resources/libexec/xd-tls-proxy" \
  "$APP/Contents/Resources/libexec/claude" \
  "$APP/Contents/Resources/libexec/claude-code-proxy" \
  "$APP/Contents/Resources/libexec/whisper-server-bin"; do
  file "$binary" | grep -F "$expected"
done
