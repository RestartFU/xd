#!/bin/sh
#
# Git is part of the relocatable xd bundle. Its private dependencies are found
# through the relative runtime path embedded during bundle assembly.

set -e

BIN=${0%/*}
BUNDLE=${BIN%/*}

export GIT_EXEC_PATH="$BUNDLE/libexec/git-core"
export GIT_TEMPLATE_DIR="${GIT_TEMPLATE_DIR:-$BUNDLE/share/git-core/templates}"
export GIT_SSL_CAINFO="${GIT_SSL_CAINFO:-$BUNDLE/etc/ssl/certs/ca-certificates.crt}"

exec "$BUNDLE/libexec/git" "$@"
