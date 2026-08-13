#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

fail_if_present() {
  local pattern=$1
  shift
  if grep -Fq -- "$pattern" "$@"; then
    echo "removed runtime packaging returned: $pattern" >&2
    exit 1
  fi
}

packaging=(
  Dockerfile
  scripts/build-macos.sh
  scripts/build-windows.ps1
  scripts/bundle.sh
  scripts/package-windows.ps1
  scripts/smoke-bundle.sh
  scripts/smoke-macos.sh
  scripts/xd.sh
  installer/macos/xd-rust-launcher.sh
  desktop/src/channel.rs
)

for pattern in \
  codex-package \
  claude-bin \
  claude-code-proxy \
  XD_CODEX_EXECUTABLE \
  XD_CLAUDE_EXECUTABLE \
  XD_JCODE_EXECUTABLE \
  XD_CLAUDE_PROXY_EXECUTABLE \
  whisper-server \
  whisper.cpp \
  XD_WHISPER_SERVER \
  voice-build \
  libasound \
  pipewire \
  libgtk \
  libadwaita \
  libvte \
  gdk-pixbuf \
  glib-networking \
  libegl \
  libgl1 \
  GSETTINGS_SCHEMA_DIR \
  GDK_PIXBUF_MODULE_FILE \
  GSK_RENDERER
do
  fail_if_present "$pattern" "${packaging[@]}"
done

test ! -e scripts/claude.sh
test ! -e daemon-rs/src/claude_proxy.rs
test ! -e data/licenses/claude-code-proxy-LICENSE
test ! -e data/icons/hicolor/scalable/apps/xd-backend-claude-mode.svg
test ! -e scripts/whisper.sh
test ! -e scripts/whisper-server.sh
test ! -e daemon-rs/src/voice.rs
test ! -e desktop/src/voice_input.rs
test ! -e data/com.restartfu.Xd.gschema.xml.in
test ! -e data/icons/hicolor/scalable/apps/xd-backend-claude.svg
test ! -e data/icons/hicolor/symbolic/apps/xd-backend-codex-symbolic.svg
test ! -e data/icons/hicolor/symbolic/apps/xd-download-symbolic.svg

grep -Fq 'user-installed Codex, Claude Code, and JCode' README.md
