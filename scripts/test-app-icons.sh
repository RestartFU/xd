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
