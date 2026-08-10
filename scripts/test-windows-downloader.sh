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

assert_not_contains() {
  local path=$1
  local unexpected=$2
  if grep -Fq -- "$unexpected" "$path"; then
    echo "$path still exposes the internal Windows payload: $unexpected" >&2
    exit 1
  fi
}

assert_contains installer/windows/xd.wxs 'EmbedCab="no"'
assert_contains installer/windows/xd.wxs 'CabinetTemplate="xd{0}.cab"'
assert_contains installer/windows/xd.wxs 'MaximumUncompressedMediaSize="2048"'
assert_contains installer/windows/bundle.wxs 'Compressed="no"'
assert_contains installer/windows/bundle.wxs 'DownloadUrl="$(MsiDownloadUrl)"'
assert_contains installer/windows/bundle.wxs 'DownloadUrl="$(CabDownloadUrl)"'
assert_contains installer/windows/bundle.wxs 'SourceFile="$(CabPath)"'
assert_contains scripts/package-windows.ps1 'installer\windows\bundle.wxs'
assert_contains scripts/package-windows.ps1 '-d "CabPath=$cabOutput"'
assert_contains scripts/package-windows.ps1 '-d "MsiDownloadUrl=$base/$msiPayloadAsset"'
assert_contains scripts/package-windows.ps1 '-d "CabDownloadUrl=$base/$cabPayloadAsset"'
assert_contains scripts/package-windows.ps1 '$TestDownloadBase'
assert_contains scripts/package-windows.ps1 '$cabAsset'
assert_contains scripts/package-windows.ps1 '$setupAsset'
assert_contains scripts/install.ps1 '$setupAsset'
assert_contains desktop/src/source_build.rs 'xd-nightly-windows-x86_64-setup.exe'
assert_contains daemon-rs/src/self_update.rs 'xd-windows-x86_64-setup.exe'
assert_contains .github/workflows/nightly.yml 'xd-nightly-windows-x86_64-setup.exe'
assert_contains .github/workflows/nightly.yml '-TestDownloadBase http://127.0.0.1:18765'
assert_contains .github/workflows/nightly.yml '-SetupPath installer-test/xd-nightly-windows-x86_64-setup-download-test.exe'
assert_contains .github/workflows/nightly.yml 'xd-nightly-windows-x86_64-msi.payload'
assert_contains .github/workflows/nightly.yml 'xd-nightly-windows-x86_64-cab.payload'
assert_contains .github/workflows/release.yml 'xd-windows-x86_64-setup.exe'
assert_contains .github/workflows/release.yml '-SetupPath installer-test/xd-windows-x86_64-setup-download-test.exe'
assert_contains .github/workflows/release.yml 'xd-windows-x86_64-msi.payload'
assert_contains .github/workflows/release.yml 'xd-windows-x86_64-cab.payload'
assert_not_contains .github/workflows/nightly.yml 'artifacts/xd-nightly-windows-x86_64.msi'
assert_not_contains .github/workflows/nightly.yml 'artifacts/xd1.cab'
assert_not_contains .github/workflows/release.yml 'artifacts/xd-windows-x86_64.msi'
assert_not_contains .github/workflows/release.yml 'artifacts/xd1.cab'
