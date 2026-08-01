#!/bin/sh
set -eu
LIBEXEC=${0%/*}
BUNDLE=${LIBEXEC%/*}
exec "$BUNDLE/lib/ld-linux-x86-64.so.2" \
  --library-path "$BUNDLE/lib" \
  --argv0 curl \
  "$LIBEXEC/curl-bin" "$@"
