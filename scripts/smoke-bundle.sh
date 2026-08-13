#!/bin/sh
#
# Prove bundle-owned runtime helpers work without finding host replacements
# through PATH. Run after assembly, before artifact export.

set -eu

BUNDLE=${1:?bundle directory}
BUNDLE=$(CDPATH='' cd -- "$BUNDLE" && pwd)
GIT="$BUNDLE/bin/git"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

require_file()
{
  test -f "$BUNDLE/$1" || {
    echo "bundle smoke: missing $1" >&2
    exit 1
  }
}

require_file share/icons/Adwaita/index.theme
cursor=$(find "$BUNDLE/share/icons/Adwaita/cursors" -type f -print -quit)
test -n "$cursor" || {
  echo "bundle smoke: missing cursor theme" >&2
  exit 1
}
app_icon=$(find "$BUNDLE/share/icons/hicolor/scalable/apps" -type f \
  -name 'com.restartfu.Xd*.svg' -print -quit)
test -n "$app_icon" || {
  echo "bundle smoke: missing application icon" >&2
  exit 1
}
require_file libexec/xd-host
require_file libexec/tmux
require_file libexec/install.sh
require_file libexec/curl
require_file libexec/openssl

# GPUI has no window without a Vulkan driver, and the loader finds one only
# through these manifests. lavapipe is the one that works anywhere.
require_file lib/libvulkan.so.1
require_file lib/libvulkan_lvp.so
require_file etc/vulkan/libvulkan_lvp.json.in

# Exercise the public entrypoint with only the host's base utilities visible.
# Preserve PATH because NixOS does not place those utilities in /usr/bin.
# This catches broken relocation/cache setup before an archive is published;
# the Git checks below separately prove no host Git can be selected.
mkdir -p "$WORK/launcher-home" "$WORK/launcher-runtime"
env -i \
  HOME="$WORK/launcher-home" \
  PATH="${PATH:-/usr/bin:/bin}" \
  XDG_RUNTIME_DIR="$WORK/launcher-runtime" \
  XDG_DATA_HOME="$WORK/launcher-home/data" \
  XDG_CACHE_HOME="$WORK/launcher-home/cache" \
  XDG_CONFIG_HOME="$WORK/launcher-home/config" \
  "$BUNDLE/xd.sh" --version | grep -E '^xd [0-9]'

"$BUNDLE/lib/ld-linux-x86-64.so.2" \
  --library-path "$BUNDLE/lib" \
  "$BUNDLE/libexec/xd-host" --version | grep -E '^xd-host [0-9]'

"$BUNDLE/lib/ld-linux-x86-64.so.2" \
  --library-path "$BUNDLE/lib" \
  "$BUNDLE/libexec/tmux" -V | grep -E '^tmux [0-9]'

git_clean()
{
  env -i \
    HOME="$WORK/home" \
    PATH="$BUNDLE/bin" \
    "$GIT" "$@"
}

mkdir -p "$WORK/home" "$WORK/repository"
git_clean --version
test "$(git_clean --exec-path)" = "$BUNDLE/libexec/git-core"

git_clean -C "$WORK/repository" init -q -b main
git_clean -C "$WORK/repository" config user.name "Bundle Test"
git_clean -C "$WORK/repository" config user.email bundle@example.com
printf 'before\n' > "$WORK/repository/file.txt"
git_clean -C "$WORK/repository" add file.txt
git_clean -C "$WORK/repository" commit -qm initial

"$BUNDLE/libexec/curl" --version | grep -F 'curl '
"$BUNDLE/libexec/openssl" dgst -sha256 "$BUNDLE/libexec/install.sh" \
  | grep -Eq '= [[:xdigit:]]{64}$'
printf 'after\n' > "$WORK/repository/file.txt"
git_clean -C "$WORK/repository" diff --check
git_clean -C "$WORK/repository" worktree add \
  -q "$WORK/worktree" -b smoke-worktree
test "$(git_clean -C "$WORK/repository" worktree list --porcelain |
  grep -c '^worktree ')" = 2

# This must reach bundled git-remote-https and its libcurl closure. Port 1 on
# loopback refuses immediately; a missing helper or library has different text.
set +e
remote_error=$(git_clean ls-remote https://127.0.0.1:1/none 2>&1)
remote_status=$?
set -e
test "$remote_status" -ne 0
printf '%s\n' "$remote_error" |
  grep -E 'Failed to connect|Could not connect|Connection refused' >/dev/null

echo "bundle runtime and Git smoke: ok"
