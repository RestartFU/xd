#!/bin/sh
#
# Git dispatches some subcommands to separate binaries in GIT_EXEC_PATH.
# Preserve the helper name in argv[0] while using xd's relocatable loader.

set -e

CORE=${0%/*}
LIBEXEC=${CORE%/*}
BUNDLE=${LIBEXEC%/*}
NAME=${0##*/}
HELPER="$LIBEXEC/git-core-real/$NAME"

exec "$BUNDLE/lib/ld-linux-x86-64.so.2" \
     --library-path "$BUNDLE/lib" \
     --argv0 "$NAME" \
     "$HELPER" "$@"
