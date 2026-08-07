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
[ -x "$APP/Contents/Resources/libexec/codex-package/bin/codex" ]
[ -x "$APP/Contents/Resources/libexec/claude" ]
[ -x "$APP/Contents/Resources/libexec/claude-code-proxy" ]
[ -f "$APP/Contents/Resources/xd.icns" ]

plutil -lint "$APP/Contents/Info.plist"
codesign --verify --deep --strict "$APP"
"$APP/Contents/MacOS/xd" --version

expected=$(uname -m)
for binary in \
  "$APP/Contents/MacOS/xd-desktop" \
  "$APP/Contents/Resources/libexec/xd-daemon" \
  "$APP/Contents/Resources/libexec/xd-tls-proxy" \
  "$APP/Contents/Resources/libexec/claude" \
  "$APP/Contents/Resources/libexec/claude-code-proxy"; do
  file "$binary" | grep -F "$expected"
done
