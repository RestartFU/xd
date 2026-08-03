#!/usr/bin/env sh
#
# Reuse of build products that depend on nothing but a pinned version.
#
# Git, OpenSSL and whisper.cpp are each fetched by version, checked against a
# published digest and compiled the same way every time, so a machine that has
# built one already learns nothing from building it again. Most of a macOS or
# Windows run is those three compiles.
#
# XD_PAYLOAD_CACHE names a directory to keep them in. Unset -- which is every
# build outside CI -- means build everything, so a developer's tree is never
# affected by something a cache decided.
#
# Entries mirror the staging tree they belong in, so restoring one is a copy
# and the caller's own verification still runs against what it restored: a
# cache that has gone bad fails the build rather than shipping.

payload_cached()
{
  [ -n "${XD_PAYLOAD_CACHE:-}" ] || return 1
  [ -d "$XD_PAYLOAD_CACHE/$1" ]
}

payload_restore()
{
  mkdir -p "$2"
  cp -pR "$XD_PAYLOAD_CACHE/$1/." "$2/"
}

# payload_store <key> <staging-directory> <path-relative-to-it>...
payload_store()
{
  [ -n "${XD_PAYLOAD_CACHE:-}" ] || return 0

  payload_key=$1
  payload_stage=$2
  shift 2
  payload_entry="$XD_PAYLOAD_CACHE/$payload_key"

  rm -rf "$payload_entry.new"
  for payload_path in "$@"; do
    mkdir -p "$payload_entry.new/$(dirname "$payload_path")"
    cp -pR "$payload_stage/$payload_path" "$payload_entry.new/$payload_path"
  done
  rm -rf "$payload_entry"
  mv "$payload_entry.new" "$payload_entry"
}
