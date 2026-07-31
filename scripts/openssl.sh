#!/bin/sh
#
# OpenSSL is used only when creating a daemon certificate. Use xd's loader so
# certificate creation works on hosts without a conventional glibc layout.

set -e

LIBEXEC=${0%/*}
BUNDLE=${LIBEXEC%/*}

export OPENSSL_CONF="${OPENSSL_CONF:-$BUNDLE/etc/ssl/openssl.cnf}"
export OPENSSL_MODULES="${OPENSSL_MODULES:-$BUNDLE/lib/ossl-modules}"

exec "$BUNDLE/lib/ld-linux-x86-64.so.2" \
     --library-path "$BUNDLE/lib" \
     "$LIBEXEC/openssl-bin" "$@"
