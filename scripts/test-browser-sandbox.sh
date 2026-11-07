#!/usr/bin/env bash
#
# Run the assembled bundle's browser inside its real sandbox.
#
# The bundle build runs in Docker's default profile, which forbids the sandbox,
# and the bundle needs a glibc at least as new as its own, so use a container of
# the same distribution with only those restrictions lifted, as an ordinary
# user. Where the kernel can restrict user namespaces, also prove that doing so
# gets an explanation instead of a crash.
#
#   test-browser-sandbox.sh <bundle>

set -euo pipefail

bundle=$(cd "${1:?bundle directory}" && pwd -P)
repository=$(cd "$(dirname "$0")/.." && pwd -P)
restriction=kernel.apparmor_restrict_unprivileged_userns
staged=
original=
cleanup()
{
  # Put the host's own setting back however the script ends, so running it
  # locally cannot leave the restriction switched off.
  if [ -n "$original" ]; then
    sudo sysctl -w "$restriction=$original" >/dev/null
  fi
  if [ -n "$staged" ]; then
    rm -rf "$staged"
  fi
}
trap cleanup EXIT

# The test runs as an ordinary user who must be able to enter the bundle.
# BuildKit exports it private to whoever built it (mode 0700), and that is
# not always uid 1000: GitHub's runner is uid 1001. So run as the owner. A
# root-owned bundle runs as uid 1000 from a copy that user owns.
owner=$(stat -c %u "$bundle")
group=$(stat -c %g "$bundle")
if [ "$owner" = 0 ]; then
  owner=1000
  group=1000
  staged=$(mktemp -d)
  cp -a "$bundle/." "$staged/"
  chown -R "$owner:$group" "$staged"
  bundle=$staged
fi

run()
{
  docker run --rm \
    --security-opt seccomp=unconfined \
    --security-opt apparmor=unconfined \
    -v "$bundle:/bundle:ro" \
    -v "$repository/scripts:/scripts:ro" \
    xd-browser-sandbox \
    sh -c 'mkdir -m 1777 -p /tmp/.X11-unix
      exec setpriv --reuid="$2" --regid="$3" --clear-groups \
        env PATH="$PATH" sh /scripts/test-browser-chromium.sh /bundle "$1"' \
    sh "$1" "$owner" "$group"
}

docker build -t xd-browser-sandbox - <<'DOCKERFILE'
FROM debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd
RUN apt-get update && apt-get install -y --no-install-recommends \
      libdrm2 libgbm1 libwayland-client0 libwayland-cursor0 libwayland-egl1 \
      python3 xvfb \
 && rm -rf /var/lib/apt/lists/*
DOCKERFILE

if setting=$(sudo sysctl -n "$restriction" 2>/dev/null); then
  original=$setting
  sudo sysctl -w "$restriction=1"
  run sandbox-unavailable
  sudo sysctl -w "$restriction=0"
fi
run sandbox
