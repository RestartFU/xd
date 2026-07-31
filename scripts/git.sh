#!/bin/sh
#
# Git is part of the relocatable xd bundle. Start it through xd's loader so
# repository features and agent-invoked Git commands work even when the host
# has no Git package or conventional /lib64 loader.

set -e

BIN=${0%/*}
BUNDLE=${BIN%/*}

export GIT_EXEC_PATH="$BUNDLE/libexec/git-core"
export GIT_TEMPLATE_DIR="${GIT_TEMPLATE_DIR:-$BUNDLE/share/git-core/templates}"
export GIT_SSL_CAINFO="${GIT_SSL_CAINFO:-$BUNDLE/etc/ssl/certs/ca-certificates.crt}"

exec "$BUNDLE/lib/ld-linux-x86-64.so.2" \
     --library-path "$BUNDLE/lib" \
     --argv0 git \
     "$BUNDLE/libexec/git-bin" "$@"
