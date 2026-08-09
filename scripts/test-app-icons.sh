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

assert_contains desktop/Cargo.toml '[build-dependencies]'
assert_contains desktop/Cargo.toml 'winresource = "0.1.30"'
assert_contains desktop/build.rs 'set_icon("assets/xd.ico")'
assert_contains desktop/build.rs 'cargo:rerun-if-changed=assets/xd.ico'
assert_contains Dockerfile 'COPY desktop/build.rs ./build.rs'
test -s desktop/assets/xd.ico

assert_contains installer/windows/xd.wxs '<Icon Id="xd.ico" SourceFile="$(IconPath)" />'
assert_contains installer/windows/xd.wxs '<Property Id="ARPPRODUCTICON" Value="xd.ico" />'
assert_contains installer/windows/xd.wxs 'Icon="xd.ico"'
assert_contains installer/windows/bundle.wxs 'IconSourceFile="$(IconPath)"'
assert_contains scripts/package-windows.ps1 '-d "IconPath=$iconPath"'

assert_contains installer/macos/Info.plist.in '<string>xd.icns</string>'
assert_contains installer/macos/Info.plist.in '<string>@BUILD_VERSION@</string>'
assert_contains scripts/build-macos.sh 'BUILD_VERSION='
assert_contains scripts/build-macos.sh 's|@BUILD_VERSION@|$BUILD_VERSION|g'
assert_contains scripts/smoke-macos.sh 'CFBundleIconFile'
assert_contains scripts/smoke-macos.sh 'CFBundleVersion'
