#!/bin/sh
set -eu

case "$*" in
  "login status")
    if test -f "$AUTH_STATE/codex"; then
      echo "Logged in using ChatGPT" >&2
      exit 0
    fi
    echo "Not logged in" >&2
    exit 1
    ;;
  "auth status --json")
    if test -f "$AUTH_STATE/claude"; then
      printf '%s\n' '{"loggedIn":true,"authMethod":"claudeAi","apiProvider":"firstParty"}'
      exit 0
    fi
    printf '%s\n' '{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}'
    exit 1
    ;;
  "login --device-auth")
    if test "${AUTH_NOISY:-0}" = 1; then
      limit=${AUTH_NOISY_LINES:-25000}
      line=0
      while test "$line" -lt "$limit"; do
        printf 'Waiting for browser sign-in %s\r' "$line"
        line=$((line + 1))
      done
    fi
    printf '\033[90mFollow these steps to sign in:\033[0m\n'
    printf '1. Open this link in your browser\n'
    printf ' https://auth.openai.com/codex/device\n'
    printf '2. Enter this one-time code (expires in 15 minutes)\n'
    printf ' \033[94mABCD-EFGH\033[0m\n'
    IFS= read -r _
    touch "$AUTH_STATE/codex"
    ;;
  "auth login")
    printf 'Opening browser to sign in…\n'
    printf '%s\n' "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&state=test"
    printf 'Paste code here if prompted > '
    IFS= read -r code
    test "$code" = "CLAUDE-1234"
    touch "$AUTH_STATE/claude"
    ;;
  "logout")
    rm -f "$AUTH_STATE/codex"
    ;;
  "auth logout")
    rm -f "$AUTH_STATE/claude"
    ;;
  *)
    echo "unexpected arguments: $*" >&2
    exit 2
    ;;
esac
