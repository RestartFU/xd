#!/bin/sh
#
# Prove bundle-owned Git works without finding any host executable through
# PATH. Run after bundle assembly, before an artifact can be exported.

set -eu

BUNDLE=${1:?bundle directory}
GIT="$BUNDLE/bin/git"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

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

echo "bundle Git smoke: ok"
