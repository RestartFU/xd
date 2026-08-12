#!/usr/bin/env bash
# Keep every place that states the version saying the same thing.
#
#   bump-version.sh {patch|minor|major} [--dry-run]
#   bump-version.sh --current
#
# Prints the version it moved to. --dry-run prints it without writing, which
# is how the release workflow checks a bump is possible before doing anything
# it would have to undo, and --current prints what the source states without
# touching anything -- so that everything asking the same question asks it
# here rather than keeping its own copy of the answer.
#
# Every file is checked against the current version first: a version that has
# drifted apart in one file is a release that ships two different numbers, and
# that is worth stopping for.

set -euo pipefail

cd "$(dirname "$0")/.."

bump="${1:-}"
dry_run="${2:-}"

case "$bump" in
  patch|minor|major|--current) ;;
  *)
    printf 'usage: %s {patch|minor|major} [--dry-run]\n' "$0" >&2
    printf '       %s --current\n' "$0" >&2
    exit 2
    ;;
esac

if [[ -n "$dry_run" && "$dry_run" != --dry-run ]]; then
  printf 'unknown argument: %s\n' "$dry_run" >&2
  exit 2
fi

# The first version line is the package's own; the ones under it belong to
# dependencies. Quitting at the first match says that, and says it in the sed
# both GNU and BSD understand: `0,/re/` is a GNU range, and a `}` straight
# after a command is a flag to BSD, which is what macOS runs.
current=$(sed -nE \
  '/^[[:space:]]*version = "[0-9]+\.[0-9]+\.[0-9]+"/{
     s/^[[:space:]]*version = "([0-9]+\.[0-9]+\.[0-9]+)".*/\1/p
     q
   }' \
  desktop/Cargo.toml)

if [[ ! "$current" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  printf 'cannot read current semantic version\n' >&2
  exit 1
fi

if [[ "$bump" == --current ]]; then
  printf '%s\n' "$current"
  exit 0
fi

major="${BASH_REMATCH[1]}"
minor="${BASH_REMATCH[2]}"
patch="${BASH_REMATCH[3]}"

for manifest in daemon-rs/Cargo.toml; do
  grep -m1 -qx "version = \"$current\"" "$manifest" \
    || { printf '%s version mismatch\n' "$manifest" >&2; exit 1; }
done

grep -A1 -m1 '^name = "xd-desktop"$' desktop/Cargo.lock \
  | grep -qx "version = \"$current\"" \
  || { printf 'desktop/Cargo.lock version mismatch\n' >&2; exit 1; }
grep -A1 -m1 '^name = "xd-host"$' daemon-rs/Cargo.lock \
  | grep -qx "version = \"$current\"" \
  || { printf 'daemon-rs/Cargo.lock version mismatch\n' >&2; exit 1; }

case "$bump" in
  patch) patch=$((patch + 1)) ;;
  minor) minor=$((minor + 1)); patch=0 ;;
  major) major=$((major + 1)); minor=0; patch=0 ;;
esac

to="$major.$minor.$patch"

if [[ "$dry_run" == --dry-run ]]; then
  printf '%s\n' "$to"
  exit 0
fi

# Written with awk into a new file rather than edited in place with sed: the
# in-place flag takes a mandatory backup suffix on BSD and none on GNU, and
# "only the first match" is a GNU-only range. Whoever cuts a release should be
# able to do it from whichever machine they are sitting at.
rewrite () {
  file=$1
  shift
  awk "$@" "$file" > "$file.bump" && mv "$file.bump" "$file"
}

# The package's own version is the first one stated; the rest belong to
# dependencies and are not ours to move.
for manifest in \
  desktop/Cargo.toml \
  daemon-rs/Cargo.toml
do
  rewrite "$manifest" -v current="$current" -v to="$to" '
    !moved && $0 == "version = \"" current "\"" {
      print "version = \"" to "\""
      moved = 1
      next
    }
    { print }
  '
done

# A lockfile states the same version again under the package that has it,
# inside the block that names it.
for package in xd-desktop:desktop/Cargo.lock xd-host:daemon-rs/Cargo.lock; do
  rewrite "${package#*:}" \
    -v package="${package%%:*}" -v current="$current" -v to="$to" '
    $0 == "name = \"" package "\"" { inside = 1 }
    $0 == "" { inside = 0 }
    inside && !moved && $0 == "version = \"" current "\"" {
      print "version = \"" to "\""
      inside = 0
      moved = 1
      next
    }
    { print }
  '
done

printf '%s\n' "$to"
