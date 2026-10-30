#!/bin/sh
set -eu
LIBEXEC=${0%/*}
exec "$LIBEXEC/curl-bin" "$@"
