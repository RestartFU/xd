#!/usr/bin/env sh
#
# Build the pinned static whisper.cpp CLI and resident server.
#
#   build-native-whisper.sh <macos-arm64|windows-x86_64> <staging-directory>

set -eu

PLATFORM=${1:?native platform}
STAGE=${2:?staging directory}
WHISPER_VERSION=1.9.1
WHISPER_SHA256=147267177eef7b22ec3d2476dd514d1b12e160e176230b740e3d1bd600118447
WHISPER_URL="https://github.com/ggml-org/whisper.cpp/archive/refs/tags/v$WHISPER_VERSION.tar.gz"

case "$PLATFORM:$(uname -s):$(uname -m)" in
  macos-arm64:Darwin:arm64)
    EXECUTABLE=whisper
    BUILT_NAME=whisper-cli
    SERVER=whisper-server
    ;;
  windows-x86_64:MINGW*:x86_64|windows-x86_64:MSYS*:x86_64)
    EXECUTABLE=whisper.exe
    BUILT_NAME=whisper-cli.exe
    SERVER=whisper-server.exe
    ;;
  *)
    echo "build-native-whisper: platform does not match native host" >&2
    exit 1
    ;;
esac

for command in cmake curl; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "build-native-whisper: $command is required" >&2
    exit 1
  }
done
[ ! -e "$STAGE/libexec/$EXECUTABLE" ] || {
  echo "build-native-whisper: destination already exists" >&2
  exit 1
}
[ ! -e "$STAGE/libexec/$SERVER" ] || {
  echo "build-native-whisper: server destination already exists" >&2
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

# Compiling whisper.cpp depends on nothing but the version pinned above and
# the platform, both of which are in the key.
. "$(dirname "$0")/payload-cache.sh"
CACHE_KEY="$PLATFORM-whisper-server-$WHISPER_VERSION"

build_whisper()
{
  WORK=$(mktemp -d "${TMPDIR:-/tmp}/xd-native-whisper.XXXXXX")
  trap 'rm -rf "$WORK"' EXIT INT TERM

  curl --fail --location --silent --show-error \
    "$WHISPER_URL" --output "$WORK/whisper.tar.gz"
  checksum "$WHISPER_SHA256" "$WORK/whisper.tar.gz"
  mkdir "$WORK/source"
  tar -xzf "$WORK/whisper.tar.gz" \
    -C "$WORK/source" --strip-components=1

  cmake -S "$WORK/source" -B "$WORK/build" \
    -DCMAKE_BUILD_TYPE=Release \
    -DBUILD_SHARED_LIBS=OFF \
    -DWHISPER_BUILD_TESTS=OFF \
    -DWHISPER_BUILD_EXAMPLES=ON \
    -DWHISPER_BUILD_SERVER=ON \
    -DGGML_NATIVE=OFF \
    -DGGML_BACKEND_DL=OFF \
    -DGGML_OPENMP=OFF \
    -DGGML_CCACHE=OFF
  cmake --build "$WORK/build" --target whisper-cli whisper-server --parallel

  mkdir -p "$STAGE/libexec"
  install -m0755 \
    "$WORK/build/bin/$BUILT_NAME" \
    "$STAGE/libexec/$EXECUTABLE"
  install -m0755 \
    "$WORK/build/bin/$SERVER" \
    "$STAGE/libexec/$SERVER"
}

if payload_cached "$CACHE_KEY"; then
  payload_restore "$CACHE_KEY" "$STAGE"
else
  build_whisper
  payload_store \
    "$CACHE_KEY" \
    "$STAGE" \
    "libexec/$EXECUTABLE" \
    "libexec/$SERVER"
fi

# Built or restored, it has to run.
"$STAGE/libexec/$EXECUTABLE" --help >/dev/null
"$STAGE/libexec/$SERVER" --help >/dev/null

printf 'native whisper.cpp: %s\n' "$WHISPER_VERSION"
