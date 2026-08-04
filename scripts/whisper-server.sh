#!/bin/sh
# Run the resident whisper.cpp server inside the private Linux bundle.

set -e

LIBEXEC=${0%/*}
BUNDLE=${LIBEXEC%/*}

cd "$BUNDLE/lib"

exec "$BUNDLE/lib/ld-linux-x86-64.so.2" \
     --library-path "$BUNDLE/lib" \
     "$LIBEXEC/whisper-server-bin" "$@"
