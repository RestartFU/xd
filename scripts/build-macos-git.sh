#!/usr/bin/env sh
#
# Build a relocatable, app-owned Git payload for Apple Silicon.
#
#   build-macos-git.sh <staging-directory>
#
# Apple CommonCrypto and system libcurl avoid Homebrew runtime paths. Git's
# RUNTIME_PREFIX locates helpers and templates relative to git/bin/git.

set -eu

STAGE=${1:?staging directory}
GIT_VERSION=2.55.0
GIT_SHA256=457fdb04dc8728e007d4688695e6912e6f680727920f2a40bf11eacc17505357
CA_SHA256=3ff344e30b9b1ed2971044eabb438a08f2e2245ddb5f8ab1a3ad8b63ab4eaf91
GIT_URL="https://mirrors.edge.kernel.org/pub/software/scm/git/git-$GIT_VERSION.tar.xz"
CA_URL=https://curl.se/ca/cacert.pem

[ "$(uname -s)" = Darwin ] || {
  echo "build-macos-git: macOS is required" >&2
  exit 1
}
[ "$(uname -m)" = arm64 ] || {
  echo "build-macos-git: Apple Silicon is required" >&2
  exit 1
}
[ ! -e "$STAGE/git" ] || {
  echo "build-macos-git: destination already exists" >&2
  exit 1
}

for command in cc curl make shasum tar; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "build-macos-git: $command is required" >&2
    exit 1
  }
done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/xd-macos-git.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

curl --fail --location --silent --show-error \
  "$GIT_URL" --output "$WORK/git.tar.xz"
printf '%s  %s\n' "$GIT_SHA256" "$WORK/git.tar.xz" |
  shasum -a 256 --check
mkdir "$WORK/source"
tar -xJf "$WORK/git.tar.xz" -C "$WORK/source" --strip-components=1

jobs=$(sysctl -n hw.logicalcpu 2>/dev/null || printf '4')
flags='prefix=/ RUNTIME_PREFIX=YesPlease HAVE_NS_GET_EXECUTABLE_PATH=YesPlease'
flags="$flags NO_OPENSSL=YesPlease APPLE_COMMON_CRYPTO=YesPlease"
flags="$flags NO_GETTEXT=YesPlease NO_TCLTK=YesPlease NO_PERL=YesPlease"
flags="$flags NO_PYTHON=YesPlease NO_EXPAT=YesPlease"

# Word splitting is intentional: each entry above is one Make variable.
# shellcheck disable=SC2086
make -C "$WORK/source" -j"$jobs" $flags all
# shellcheck disable=SC2086
make -C "$WORK/source" $flags DESTDIR="$STAGE/git" install

mkdir -p "$STAGE/git/ssl/certs"
curl --fail --location --silent --show-error \
  "$CA_URL" --output "$STAGE/git/ssl/certs/ca-bundle.crt"
printf '%s  %s\n' \
  "$CA_SHA256" "$STAGE/git/ssl/certs/ca-bundle.crt" |
  shasum -a 256 --check

"$STAGE/git/bin/git" --version | grep -F "git version $GIT_VERSION"
actual_exec_path=$("$STAGE/git/bin/git" --exec-path)
test -d "$actual_exec_path"
actual_exec_path=$(cd "$actual_exec_path" && pwd -P)
expected_exec_path=$(cd "$STAGE/git/libexec/git-core" && pwd -P)
test "$actual_exec_path" = "$expected_exec_path"
[ -x "$STAGE/git/libexec/git-core/git-remote-https" ]
[ -d "$STAGE/git/share/git-core/templates" ]

printf 'macOS relocatable Git: %s\n' "$GIT_VERSION"
