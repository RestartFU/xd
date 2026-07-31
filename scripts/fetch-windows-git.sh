#!/usr/bin/env sh
#
# Add pinned PortableGit to a Windows x86_64 staging tree.
#
#   fetch-windows-git.sh <staging-directory>
#
# Run inside MSYS2. PortableGit owns its HTTPS helper, CA bundle, OpenSSL, and
# Unix compatibility tools, so copying the whole verified payload is required.

set -eu

STAGE=${1:?staging directory}
GIT_VERSION=2.55.0.3
GIT_ASSET=PortableGit-2.55.0.3-64-bit.7z.exe
GIT_SHA256=ab00566336b5472120f9a52d34f2e79c5406535792acb0548001ffd0bd090e5d
GIT_URL="https://github.com/git-for-windows/git/releases/download/v2.55.0.windows.3/$GIT_ASSET"

case "$(uname -s)" in
  MINGW*|MSYS*) ;;
  *)
    echo "fetch-windows-git: MSYS2 on Windows is required" >&2
    exit 1
    ;;
esac

command -v curl >/dev/null 2>&1 || {
  echo "fetch-windows-git: curl is required" >&2
  exit 1
}
command -v cygpath >/dev/null 2>&1 || {
  echo "fetch-windows-git: cygpath is required" >&2
  exit 1
}
[ ! -e "$STAGE/git" ] || {
  echo "fetch-windows-git: destination already exists" >&2
  exit 1
}

WORK=$(mktemp -d "${TMPDIR:-/tmp}/xd-portable-git.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

curl --fail --location --silent --show-error \
  "$GIT_URL" --output "$WORK/$GIT_ASSET"
printf '%s  %s\n' "$GIT_SHA256" "$WORK/$GIT_ASSET" |
  sha256sum --check

destination=$(cygpath -w "$STAGE/git")
"$WORK/$GIT_ASSET" -y -gm2 -o "$destination"

"$STAGE/git/cmd/git.exe" --version | grep -F "git version 2.55.0.windows.3"
[ -x "$STAGE/git/mingw64/libexec/git-core/git-remote-https.exe" ]
[ -f "$STAGE/git/mingw64/ssl/certs/ca-bundle.crt" ]
[ -x "$STAGE/git/mingw64/bin/openssl.exe" ]

printf 'Windows PortableGit: %s\n' "$GIT_VERSION"
