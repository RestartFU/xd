#!/usr/bin/env bash
#
# Registers self-hosted runners for this repository and keeps them running.
#
#   ops/runner/setup.sh [count]
#
# Each runner is a container with a named volume holding its registration, and
# a restart policy, so it comes back after a crash, a daemon restart or a
# reboot without being registered again.
#
# Asks for as many runners as the count: one already running is left alone, so
# raising the count adds to them rather than replacing them. Replacing a live
# registration strands whatever GitHub has already dispatched to it -- the job
# waits for a runner that no longer exists -- so that is deliberate. Force it
# with --recreate when the image or the labels have changed.
#
# Needs a GitHub token with repo admin rights, from `gh auth` or GH_TOKEN, to
# ask for a registration token. Registration tokens last an hour and are only
# used at setup, so nothing long-lived is stored in the container.
#
# Remove them again with:
#
#   ops/runner/setup.sh --remove

set -euo pipefail

REPOSITORY="${RUNNER_REPOSITORY:-RestartFU/xd}"
IMAGE="${RUNNER_IMAGE:-xd-runner:latest}"
PREFIX="${RUNNER_PREFIX:-xd-runner}"
LABELS="${RUNNER_LABELS:-self-hosted,linux,x64,xd}"

cd "$(dirname "$0")"

say () { printf '%s\n' "$*"; }
die () { printf 'runner: %s\n' "$*" >&2; exit 1; }

command -v docker >/dev/null 2>&1 || die "Docker is needed."
command -v gh >/dev/null 2>&1 || die "the GitHub CLI is needed."

# Whatever socket this host's daemon actually listens on: rootless Docker puts
# it under the user's runtime directory rather than in /var/run.
SOCKET=$(docker context inspect --format '{{.Endpoints.docker.Host}}')
SOCKET=${SOCKET#unix://}
[ -S "$SOCKET" ] || die "cannot find the Docker socket at $SOCKET."

remove () {
  for container in $(docker ps -a --filter "name=^${PREFIX}-" --format '{{.Names}}'); do
    say "Removing $container…"
    docker rm -f "$container" >/dev/null
    docker volume rm "${container}-data" >/dev/null 2>&1 || true
  done
  say "Runners removed. They stay registered until GitHub reaps them; remove"
  say "them from the repository's runner settings if they linger."
  exit 0
}

[ "${1:-}" = "--remove" ] && remove

RECREATE=no
if [ "${1:-}" = "--recreate" ]; then
  RECREATE=yes
  shift
fi

COUNT=${1:-2}
case "$COUNT" in
  ''|*[!0-9]*) die "the count must be a number." ;;
esac

say "Building $IMAGE…"
docker build --quiet --tag "$IMAGE" . >/dev/null

for index in $(seq 1 "$COUNT"); do
  name="${PREFIX}-${index}"
  volume="${name}-data"

  if [ "$RECREATE" = no ] &&
     [ -n "$(docker ps --quiet --filter "name=^${name}$")" ]; then
    say "Leaving $name alone; it is already running."
    continue
  fi

  say "Registering $name…"
  token=$(gh api \
    --method POST \
    -H "Accept: application/vnd.github+json" \
    "repos/${REPOSITORY}/actions/runners/registration-token" \
    --jq .token)
  [ -n "$token" ] || die "GitHub returned no registration token."

  docker rm -f "$name" >/dev/null 2>&1 || true
  # A volume that already holds a configuration cannot be reconfigured in
  # place, and the work directory in it is only a cache: the layer cache that
  # makes these builds fast lives on the host's daemon, not in here.
  docker volume rm -f "$volume" >/dev/null 2>&1 || true
  docker volume create "$volume" >/dev/null

  # --replace so a re-run takes over its own registration rather than leaving
  # a dead one behind holding the name.
  docker run --rm \
    --user 0 \
    --volume "${volume}:/home/runner" \
    "$IMAGE" \
    ./config.sh \
      --unattended \
      --replace \
      --url "https://github.com/${REPOSITORY}" \
      --token "$token" \
      --name "$name" \
      --labels "$LABELS" \
      --work _work >/dev/null

  say "Starting $name…"
  docker run --detach \
    --name "$name" \
    --restart unless-stopped \
    --user 0 \
    --volume "${volume}:/home/runner" \
    --volume "${SOCKET}:/var/run/docker.sock" \
    "$IMAGE" \
    ./run.sh >/dev/null
done

say ""
say "$COUNT runner(s) listening for ${REPOSITORY}, labelled ${LABELS}."
say "They restart with Docker. Follow one with: docker logs -f ${PREFIX}-1"
