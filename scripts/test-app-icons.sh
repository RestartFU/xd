#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

assert_contains() {
  local path=$1
  local expected=$2
  if ! grep -Fq -- "$expected" "$path"; then
    echo "$path does not contain the app icon contract: $expected" >&2
    exit 1
  fi
}

assert_contains installer/macos/Info.plist.in '<string>xd.icns</string>'
assert_contains installer/macos/Info.plist.in '<string>@BUILD_VERSION@</string>'
assert_contains scripts/build-macos.sh 'BUILD_VERSION='
assert_contains scripts/build-macos.sh 's|@BUILD_VERSION@|$BUILD_VERSION|g'
assert_contains scripts/smoke-macos.sh 'CFBundleIconFile'
assert_contains scripts/smoke-macos.sh 'CFBundleVersion'

# Windows: GPUI gives the window, and with it the taskbar and Alt+Tab, icon
# resource 1 of the executable; the installer shows the same artwork in the
# Start menu and in the list of installed apps.
assert_contains desktop/build.rs '1 ICON'
assert_contains desktop/build.rs 'installer/windows/xd.ico'
assert_contains installer/windows/xd.wxs 'ARPPRODUCTICON'
assert_contains scripts/build-windows.ps1 'installer/windows/xd.ico'
assert_contains scripts/smoke-windows.ps1 'xd.exe has no application icon.'
assert_contains scripts/test-windows-ui.ps1 'the window has an icon'

# An ICO file with every size from the title bar's 16 pixels to Explorer's 256.
icon_header=$(od -An -tu1 -N6 installer/windows/xd.ico | tr -s ' ' | sed 's/^ //')
if [ "$icon_header" != '0 0 1 0 9 0' ]; then
  echo "installer/windows/xd.ico is not a nine-image icon: $icon_header" >&2
  exit 1
fi
