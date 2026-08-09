#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

assert_contains() {
  local path=$1
  local expected=$2
  if ! grep -Fq -- "$expected" "$path"; then
    echo "$path does not contain the Windows downloader contract: $expected" >&2
    exit 1
  fi
}

assert_contains installer/windows/xd.wxs 'EmbedCab="no"'
assert_contains installer/windows/xd.wxs 'CabinetTemplate="xd{0}.cab"'
assert_contains installer/windows/xd.wxs 'MaximumUncompressedMediaSize="2048"'
assert_contains installer/windows/bundle.wxs 'Compressed="no"'
assert_contains installer/windows/bundle.wxs 'DownloadUrl="$(DownloadBase)/{2}"'
assert_contains scripts/package-windows.ps1 'installer\windows\bundle.wxs'
assert_contains scripts/package-windows.ps1 '$cabAsset'
assert_contains scripts/package-windows.ps1 '$setupAsset'
assert_contains scripts/install.ps1 '$setupAsset'
assert_contains desktop/src/source_build.rs 'xd-nightly-windows-x86_64-setup.exe'
assert_contains daemon-rs/src/self_update.rs 'xd-windows-x86_64-setup.exe'
assert_contains .github/workflows/nightly.yml 'xd-nightly-windows-x86_64-setup.exe'
assert_contains .github/workflows/release.yml 'xd-windows-x86_64-setup.exe'
