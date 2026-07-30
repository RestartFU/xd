#!/bin/sh
#
# Build and install latest commit from Crystal rewrite branch.
#
# Host needs Docker and curl only. Build stays inside Docker; resulting
# self-contained Linux bundle installs through regular nightly installer.

set -eu

REPO=RestartFU/xd
BRANCH=rewrite/crystal-unified-daemon

say () { printf '%s\n' "$*"; }
die () { printf 'install: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] \
  || die "Crystal branch bundle currently supports Linux only."
case "$(uname -m)" in
  x86_64|amd64) ;;
  *) die "Crystal branch bundle currently supports x86_64 only." ;;
esac

command -v curl >/dev/null 2>&1 || die "curl is needed."
command -v docker >/dev/null 2>&1 || die "Docker is needed."
docker info >/dev/null 2>&1 \
  || die "Docker daemon is not available."
docker buildx version >/dev/null 2>&1 \
  || die "Docker Buildx is needed."

WORK=$(mktemp -d "${TMPDIR:-/tmp}/xd-branch-install.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

API_BRANCH=$(printf '%s' "$BRANCH" | sed 's|/|%2F|g')
METADATA=$(curl -fsSL --proto '=https' --tlsv1.2 \
  -H 'Cache-Control: no-cache' \
  "https://api.github.com/repos/$REPO/git/ref/heads/$API_BRANCH?cachebust=$(date +%s)") \
  || die "cannot resolve latest $BRANCH commit."
COMMIT=$(printf '%s\n' "$METADATA" \
  | sed -n 's/^[[:space:]]*"sha": "\([0-9a-f]\{40\}\)".*$/\1/p' \
  | head -n 1)
case "$COMMIT" in
  [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]*)
    ;;
  *) die "GitHub did not return a commit for $BRANCH." ;;
esac
SHORT=$(printf '%.7s' "$COMMIT")

say "Building $BRANCH at $SHORT through Docker…"
docker buildx build \
  --target bundle \
  --build-arg PROFILE=nightly \
  --build-arg COMMIT="$SHORT" \
  --output "type=local,dest=$WORK/bundle" \
  "https://github.com/$REPO.git#$COMMIT"

[ -x "$WORK/bundle/xd.sh" ] \
  || die "Docker did not produce an xd bundle."

curl -fsSL --proto '=https' --tlsv1.2 \
  -o "$WORK/install.sh" \
  "https://raw.githubusercontent.com/$REPO/$COMMIT/scripts/install.sh" \
  || die "cannot download installer from $SHORT."

sh "$WORK/install.sh" --from "$WORK/bundle"
