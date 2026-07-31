#!/bin/sh
#
# Claude's native build uses glibc. Start it with xd's loader and library
# closure so the bundled CLI also works where /lib64/ld-linux is absent.

set -e

LIBEXEC=${0%/*}
BUNDLE=${LIBEXEC%/*}

exec "$BUNDLE/lib/ld-linux-x86-64.so.2" \
     --library-path "$BUNDLE/lib" \
     "$LIBEXEC/claude-bin" "$@"
