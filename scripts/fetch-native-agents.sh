#!/usr/bin/env sh
#
# Add pinned native assistants and Claude mode proxy to a staging tree.
#
#   fetch-native-agents.sh <macos-arm64|windows-x86_64> <staging-directory>
#
# Set XD_NATIVE_FETCH_SKIP_EXEC=1 only for cross-host archive inspection. Native
# release builds execute both tools and verify their reported pinned versions.

set -eu

PLATFORM=${1:?native platform}
STAGE=${2:?staging directory}

CODEX_VERSION=0.146.0
CLAUDE_VERSION=2.1.220
CLAUDE_PROXY_VERSION=0.1.30

case "$PLATFORM" in
  macos-arm64)
    CODEX_ASSET=codex-package-aarch64-apple-darwin.tar.gz
    CODEX_BINARY=codex
    CODEX_SHA256=cd961b480f6dfc4703bd244601f1927231fa31a587cb9046ccdffa6c4c29e7d5
    CLAUDE_PLATFORM=darwin-arm64
    CLAUDE_BINARY=claude
    CLAUDE_SHA256=8addc857f3fe64d5a0368af9ee50321b50afb4a6918ba3ef018ab84f5dbbe081
    CLAUDE_PROXY_ASSET=claude-code-proxy-darwin-arm64.tar.gz
    CLAUDE_PROXY_BINARY=claude-code-proxy
    CLAUDE_PROXY_SHA256=23732b1a189db57ce24fd80e0f181a1c199fbba17542cd02008f02fb09630d32
    ;;
  windows-x86_64)
    CODEX_ASSET=codex-package-x86_64-pc-windows-msvc.tar.gz
    CODEX_BINARY=codex.exe
    CODEX_SHA256=a945559cc0da3437c022d53e5f601f9e8c95980d717c9aad82997e4582ecd55e
    CLAUDE_PLATFORM=win32-x64
    CLAUDE_BINARY=claude.exe
    CLAUDE_SHA256=af5bf1f1b2aadffc768eccd787084c6fdf9ba81624cbe96c1c6d9ac1a1550231
    CLAUDE_PROXY_ASSET=claude-code-proxy-windows-amd64.zip
    CLAUDE_PROXY_BINARY=claude-code-proxy.exe
    CLAUDE_PROXY_SHA256=7ee1e9c275de326e97ea7914f9eafa74ed7fb6bfa60223e3fafc0e0daf02e233
    ;;
  *)
    echo "fetch-native-agents: unsupported platform: $PLATFORM" >&2
    exit 1
    ;;
esac

command -v curl >/dev/null 2>&1 || {
  echo "fetch-native-agents: curl is required" >&2
  exit 1
}
command -v tar >/dev/null 2>&1 || {
  echo "fetch-native-agents: tar is required" >&2
  exit 1
}

checksum()
{
  expected=$1
  path=$2
  if [ "$(uname -s)" = Darwin ]; then
    printf '%s  %s\n' "$expected" "$path" | shasum -a 256 -c
  elif command -v sha256sum >/dev/null 2>&1; then
    printf '%s  %s\n' "$expected" "$path" | sha256sum -c
  else
    printf '%s  %s\n' "$expected" "$path" | shasum -a 256 -c
  fi
}

[ ! -e "$STAGE/libexec/codex-package" ] || {
  echo "fetch-native-agents: Codex destination already exists" >&2
  exit 1
}
[ ! -e "$STAGE/libexec/$CLAUDE_BINARY" ] || {
  echo "fetch-native-agents: Claude destination already exists" >&2
  exit 1
}
[ ! -e "$STAGE/libexec/$CLAUDE_PROXY_BINARY" ] || {
  echo "fetch-native-agents: Claude mode proxy destination already exists" >&2
  exit 1
}

WORK=$(mktemp -d "${TMPDIR:-/tmp}/xd-native-agents.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$STAGE/libexec/codex-package"

CODEX_URL="https://github.com/openai/codex/releases/download/rust-v$CODEX_VERSION/$CODEX_ASSET"
CLAUDE_URL="https://downloads.claude.ai/claude-code-releases/$CLAUDE_VERSION/$CLAUDE_PLATFORM/$CLAUDE_BINARY"
CLAUDE_PROXY_URL="https://github.com/raine/claude-code-proxy/releases/download/v$CLAUDE_PROXY_VERSION/$CLAUDE_PROXY_ASSET"

curl --fail --location --silent --show-error \
  "$CODEX_URL" --output "$WORK/codex.tar.gz"
checksum "$CODEX_SHA256" "$WORK/codex.tar.gz"
tar -xzf "$WORK/codex.tar.gz" -C "$STAGE/libexec/codex-package"

curl --fail --location --silent --show-error \
  "$CLAUDE_URL" --output "$STAGE/libexec/$CLAUDE_BINARY"
checksum "$CLAUDE_SHA256" "$STAGE/libexec/$CLAUDE_BINARY"
curl --fail --location --silent --show-error \
  "$CLAUDE_PROXY_URL" --output "$WORK/claude-code-proxy-archive"
checksum "$CLAUDE_PROXY_SHA256" "$WORK/claude-code-proxy-archive"
if [ "$PLATFORM" = windows-x86_64 ]; then
  command -v unzip >/dev/null 2>&1 || {
    echo "fetch-native-agents: unzip is required for Windows" >&2
    exit 1
  }
  unzip -q "$WORK/claude-code-proxy-archive" -d "$STAGE/libexec"
else
  tar -xzf "$WORK/claude-code-proxy-archive" -C "$STAGE/libexec"
fi
chmod 0755 \
  "$STAGE/libexec/codex-package/bin/$CODEX_BINARY" \
  "$STAGE/libexec/$CLAUDE_BINARY" \
  "$STAGE/libexec/$CLAUDE_PROXY_BINARY"

if [ "${XD_NATIVE_FETCH_SKIP_EXEC:-0}" != 1 ]; then
  "$STAGE/libexec/codex-package/bin/$CODEX_BINARY" --version |
    grep -F "$CODEX_VERSION"
  "$STAGE/libexec/$CLAUDE_BINARY" --version |
    grep -F "$CLAUDE_VERSION"
  "$STAGE/libexec/$CLAUDE_PROXY_BINARY" --version |
    grep -F "$CLAUDE_PROXY_VERSION"
fi

printf 'native agents: Codex %s, Claude %s, Claude mode proxy %s\n' \
  "$CODEX_VERSION" "$CLAUDE_VERSION" "$CLAUDE_PROXY_VERSION"
