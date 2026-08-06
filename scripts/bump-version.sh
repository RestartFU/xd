#!/usr/bin/env bash
# Keep every place that states the version saying the same thing.
#
#   bump-version.sh {patch|minor|major} [--dry-run]
#
# Prints the version it moved to. --dry-run prints it without writing, which
# is how the release workflow checks a bump is possible before doing anything
# it would have to undo.
#
# Every file is checked against the current version first: a version that has
# drifted apart in one file is a release that ships two different numbers, and
# that is worth stopping for.

set -euo pipefail

cd "$(dirname "$0")/.."

bump="${1:-}"
dry_run="${2:-}"

case "$bump" in
  patch|minor|major) ;;
  *)
    printf 'usage: %s {patch|minor|major} [--dry-run]\n' "$0" >&2
    exit 2
    ;;
esac

if [[ -n "$dry_run" && "$dry_run" != --dry-run ]]; then
  printf 'unknown argument: %s\n' "$dry_run" >&2
  exit 2
fi

current=$(sed -nE \
  '0,/^[[:space:]]*version = "([0-9]+\.[0-9]+\.[0-9]+)"/{s//\1/p}' \
  desktop/Cargo.toml)

if [[ ! "$current" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  printf 'cannot read current semantic version\n' >&2
  exit 1
fi

major="${BASH_REMATCH[1]}"
minor="${BASH_REMATCH[2]}"
patch="${BASH_REMATCH[3]}"

for manifest in daemon-rs/Cargo.toml tls-proxy-rs/Cargo.toml; do
  grep -m1 -qx "version = \"$current\"" "$manifest" \
    || { printf '%s version mismatch\n' "$manifest" >&2; exit 1; }
done

grep -A1 -m1 '^name = "xd-desktop"$' desktop/Cargo.lock \
  | grep -qx "version = \"$current\"" \
  || { printf 'desktop/Cargo.lock version mismatch\n' >&2; exit 1; }
grep -A1 -m1 '^name = "xd-daemon"$' daemon-rs/Cargo.lock \
  | grep -qx "version = \"$current\"" \
  || { printf 'daemon-rs/Cargo.lock version mismatch\n' >&2; exit 1; }

case "$bump" in
  patch) patch=$((patch + 1)) ;;
  minor) minor=$((minor + 1)); patch=0 ;;
  major) major=$((major + 1)); minor=0; patch=0 ;;
esac

next="$major.$minor.$patch"

if [[ "$dry_run" == --dry-run ]]; then
  printf '%s\n' "$next"
  exit 0
fi

for manifest in \
  desktop/Cargo.toml \
  daemon-rs/Cargo.toml \
  tls-proxy-rs/Cargo.toml
do
  sed -i \
    "0,/^version = \"$current\"\$/s//version = \"$next\"/" \
    "$manifest"
done

sed -i \
  "/^name = \"xd-desktop\"\$/,/^\$/ s/^version = \"$current\"\$/version = \"$next\"/" \
  desktop/Cargo.lock
sed -i \
  "/^name = \"xd-daemon\"\$/,/^\$/ s/^version = \"$current\"\$/version = \"$next\"/" \
  daemon-rs/Cargo.lock

printf '%s\n' "$next"
