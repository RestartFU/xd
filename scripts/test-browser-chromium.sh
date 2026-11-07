#!/bin/sh
#
# Load a page in the assembled bundle's real browser, through its public
# launcher. Linking is not enough: Chromium only finds out at run time whether
# its helpers, data files, and security modules are where it needs them.
#
#   test-browser-chromium.sh <bundle> <no-sandbox|sandbox|sandbox-unavailable>

set -eu

bundle=$(CDPATH='' cd -- "${1:?bundle directory}" && pwd -P)
mode=${2:?no-sandbox, sandbox, or sandbox-unavailable}
display=:${XD_TEST_DISPLAY:-97}
port=${XD_TEST_PORT:-18097}
work=$(mktemp -d)
xvfb=
server=
cleanup()
{
  for process in $xvfb $server; do
    kill "$process" 2>/dev/null || true
  done
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

mkdir -p "$work/site" "$work/home" "$work/runtime"
chmod 700 "$work/runtime"
printf '<!doctype html><title>bundle</title><p>loaded</p>\n' > "$work/site/index.html"

Xvfb "$display" -screen 0 800x600x24 -nolisten tcp >"$work/xvfb.log" 2>&1 &
xvfb=$!
(cd "$work/site" && exec python3 -m http.server "$port" --bind 127.0.0.1) \
  >"$work/server.log" 2>&1 &
server=$!

attempts=0
until python3 -c "import socket; socket.create_connection(('127.0.0.1', $port), 1).close()" \
    2>/dev/null && test -e "/tmp/.X11-unix/X${display#:}"; do
  attempts=$((attempts + 1))
  test "$attempts" -lt 100 || {
    echo "browser smoke: the display or page server did not start" >&2
    exit 1
  }
  sleep 0.1
done

expected=0
case "$mode" in
  no-sandbox) set -- XD_CHROMIUM_NO_SANDBOX=1 ;;
  sandbox) set -- ;;
  sandbox-unavailable)
    set --
    expected=3
    ;;
  *)
    echo "browser smoke: unknown mode $mode" >&2
    exit 2
    ;;
esac

set +e
env -i \
  DISPLAY="$display" \
  HOME="$work/home" \
  PATH="${PATH:-/usr/bin:/bin}" \
  XDG_RUNTIME_DIR="$work/runtime" \
  XDG_DATA_HOME="$work/home/data" \
  XDG_CACHE_HOME="$work/home/cache" \
  XDG_CONFIG_HOME="$work/home/config" \
  XD_BROWSER_SMOKE_TEST_URL="http://127.0.0.1:$port/" \
  "$@" \
  "$bundle/xd.sh" >"$work/browser.log" 2>&1
status=$?
set -e

if [ "$status" -ne "$expected" ]; then
  tail -n 40 "$work/browser.log" >&2
  echo "browser smoke ($mode): exit $status, expected $expected" >&2
  exit 1
fi
echo "browser smoke ($mode): ok"
