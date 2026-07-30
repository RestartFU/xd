#!/bin/sh
#
# whisper.cpp is linked against the speech libraries and CPU backends carried
# by the bundle. Keep those libraries out of host processes while giving the
# transcription subprocess the same minimal-host support as xd itself.

set -e

LIBEXEC=${0%/*}
BUNDLE=${LIBEXEC%/*}

# ggml selects the fastest compatible CPU backend from executable directory
# and current directory. Backends live with the rest of bundle libraries.
cd "$BUNDLE/lib"

exec "$BUNDLE/lib/ld-linux-x86-64.so.2" \
     --library-path "$BUNDLE/lib" \
     "$LIBEXEC/whisper-bin" "$@"
