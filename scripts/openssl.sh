#!/bin/sh
#
# OpenSSL is used only when creating a host certificate. Its private
# dependencies use the relative runtime path embedded during bundle assembly.

set -e

LIBEXEC=${0%/*}
BUNDLE=${LIBEXEC%/*}

export OPENSSL_CONF="${OPENSSL_CONF:-$BUNDLE/etc/ssl/openssl.cnf}"
export OPENSSL_MODULES="${OPENSSL_MODULES:-$BUNDLE/lib/ossl-modules}"

exec "$LIBEXEC/openssl-bin" "$@"
