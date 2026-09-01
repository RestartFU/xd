#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

fail() {
  echo "release/install contract: $*" >&2
  exit 1
}

assert_contains() {
  local path=$1 expected=$2
  grep -Fq -- "$expected" "$path" || fail "$path lacks: $expected"
}

assert_not_contains() {
  local path=$1 unexpected=$2
  ! grep -Fq -- "$unexpected" "$path" || fail "$path still contains: $unexpected"
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

mkdir -p "$work/bin" "$work/home"
cat > "$work/bin/uname" <<'EOF'
#!/bin/sh
case "${1-}" in
  -s) printf '%s\n' Darwin ;;
  -m) printf '%s\n' arm64 ;;
  *) printf '%s\n' Darwin ;;
esac
EOF
chmod +x "$work/bin/uname"

if HOME="$work/home" scripts/install.sh --unknown --uninstall >/dev/null 2>&1; then
  fail "Linux installer accepted an unknown option"
fi
if PATH="$work/bin:$PATH" HOME="$work/home" \
    scripts/install-macos.sh --unknown --uninstall >/dev/null 2>&1; then
  fail "macOS installer accepted an unknown option"
fi

fixture="$work/bundle"
home="$work/home with & marker"
data="$work/data with & marker"
mkdir -p \
  "$fixture/bin" \
  "$fixture/etc" \
  "$fixture/share/applications" \
  "$home" \
  "$data"
cp scripts/xd.sh "$fixture/xd.sh"
chmod +x "$fixture/xd.sh"
cat > "$fixture/bin/xd" <<'EOF'
#!/bin/sh
printf '%s\n' 'xd 0.0.0'
EOF
chmod +x "$fixture/bin/xd"
cat > "$fixture/etc/fonts.conf.in" <<'EOF'
<fontconfig><dir>@BUNDLE@/share/fonts</dir></fontconfig>
EOF
cat > "$fixture/share/applications/com.restartfu.Xd.Nightly.desktop" <<'EOF'
[Desktop Entry]
Name=xd
Exec=xd
Icon=com.restartfu.Xd.Nightly
Type=Application
EOF

HOME="$home" XDG_DATA_HOME="$data" XDG_CONFIG_HOME="$work/config" \
  scripts/install.sh --from "$fixture" >/dev/null

desktop="$data/applications/com.restartfu.Xd.Nightly.desktop"
expected_exec="Exec=\"$home/.local/bin/xd-nightly\""
grep -Fqx -- "$expected_exec" "$desktop" \
  || fail "desktop executable path was not quoted exactly"

runtime="$work/runtime"
mkdir -p "$runtime"
HOME="$home" XDG_RUNTIME_DIR="$runtime" \
  "$home/.local/bin/xd-nightly" --version >/dev/null
fonts="$runtime/xd-$(id -u)/xd-nightly/fonts.conf"
escaped_home=${home//&/&amp;}
grep -Fq -- "<dir>$escaped_home/.local/opt/xd-nightly/share/fonts</dir>" "$fonts" \
  || fail "fontconfig bundle path was corrupted"

cat > "$work/bin/apksigner" <<'EOF'
#!/bin/sh
printf '%s\n' 'Signer #1 certificate SHA-256 digest: fac77d3eb6b167cd2334a1497f9b5606af120af178e5d90a7e429586e6a7fc20'
EOF
cat > "$work/bin/aapt2" <<'EOF'
#!/bin/sh
printf '%s\n' 'A: android:debuggable=false'
EOF
chmod +x "$work/bin/apksigner" "$work/bin/aapt2"
if mobile/verify-release.sh fixture.apk "$work/bin/apksigner" "$work/bin/aapt2" \
    >/dev/null 2>&1; then
  fail "Android artifact guard accepted the public debug certificate"
fi

cat > "$work/bin/apksigner" <<'EOF'
#!/bin/sh
printf '%s\n' 'Signer #1 certificate SHA-256 digest: 0123456789abcdef'
EOF
cat > "$work/bin/aapt2" <<'EOF'
#!/bin/sh
printf '%s\n' 'A: android:debuggable(0x0101000f)=0xffffffff'
EOF
if mobile/verify-release.sh fixture.apk "$work/bin/apksigner" "$work/bin/aapt2" \
    >/dev/null 2>&1; then
  fail "Android artifact guard accepted a debuggable APK"
fi

cat > "$work/bin/aapt2" <<'EOF'
#!/bin/sh
printf '%s\n' 'A: android:debuggable(0x0101000f)=0x0'
EOF
mobile/verify-release.sh fixture.apk "$work/bin/apksigner" "$work/bin/aapt2" \
  >/dev/null

assert_contains mobile/androidApp/build.gradle.kts 'signingConfigs.create("release")'
assert_contains mobile/androidApp/build.gradle.kts 'System.getenv("XD_ANDROID_KEYSTORE")'
assert_contains mobile/androidApp/build.gradle.kts 'com.restartfu.xd.app'
assert_contains mobile/androidApp/build.gradle.kts 'applicationIdSuffix = ".debug"'
assert_contains mobile/Dockerfile ':androidApp:assembleRelease'
assert_contains mobile/Dockerfile '--no-configuration-cache'
assert_contains mobile/Dockerfile '--no-build-cache'
assert_contains mobile/Dockerfile 'FROM scratch AS release-apk'
assert_contains mobile/Dockerfile '/xd-mobile-debug.apk'
assert_contains mobile/verify-release.sh 'verify --verbose --print-certs'
assert_not_contains Dockerfile 'COPY host/tests ./tests'
assert_contains scripts/mobile-build.sh 'XD_MOBILE_RELEASE'
assert_contains scripts/mobile-build.sh 'target=release-apk'
assert_contains scripts/mobile-build.sh '--no-cache-filter release'
assert_contains .github/workflows/release.yml 'ANDROID_RELEASE_KEYSTORE_BASE64'
assert_contains .github/workflows/nightly.yml 'ANDROID_RELEASE_KEYSTORE_BASE64'
assert_contains .github/workflows/release.yml 'XD_MOBILE_RELEASE: 1'
assert_contains .github/workflows/release.yml 'id: android_signing'
assert_contains .github/workflows/release.yml 'available=false'
assert_contains .github/workflows/release.yml "if: steps.android_signing.outputs.available == 'true'"
assert_contains .github/workflows/release.yml 'if [[ -f xd-android.apk ]]'
assert_contains .github/workflows/release.yml '"${assets[@]}"'
assert_contains .github/workflows/nightly.yml 'XD_MOBILE_RELEASE: 1'
assert_contains .github/workflows/nightly.yml 'XD_MOBILE_CHANNEL=nightly'
assert_not_contains .github/workflows/release.yml 'xd-mobile-debug.apk xd-android.apk'
assert_not_contains .github/workflows/nightly.yml 'xd-mobile-debug.apk xd-nightly-android.apk'

assert_contains .github/workflows/nightly.yml 'cancel-in-progress: false'
assert_not_contains .github/workflows/nightly.yml 'gh release upload nightly --clobber'
assert_not_contains .github/workflows/nightly.yml 'gh release delete nightly'
assert_contains .github/workflows/nightly.yml 'gh release create "$candidate" --draft'
assert_contains .github/workflows/nightly.yml 'gh release edit "$candidate"'
assert_contains .github/workflows/nightly.yml 'rollback_nightly'
assert_contains .github/workflows/nightly.yml 'git/refs/tags/nightly'

assert_contains .github/workflows/release.yml $'permissions:\n  contents: read'
assert_contains .github/workflows/nightly.yml $'permissions:\n  contents: read'
assert_contains .github/workflows/release.yml 'persist-credentials: false'
assert_contains .github/workflows/nightly.yml 'persist-credentials: false'

echo "release and install contracts: ok"
