# The self-hosted runner that builds the Linux and Android artifacts.
#
# GitHub's own runner image, plus a Docker client. Every Linux job here builds
# inside Docker, so the runner needs a client but not a daemon: it talks to the
# daemon already running on the host, over the socket the container is started
# with. That keeps the layer cache on the host, which is the whole reason a
# self-hosted runner is faster than a fresh cloud one.
FROM ghcr.io/actions/actions-runner:latest

USER root

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
      git \
      gnupg \
 && install -m 0755 -d /etc/apt/keyrings \
 && curl -fsSL https://download.docker.com/linux/ubuntu/gpg \
      -o /etc/apt/keyrings/docker.asc \
 && chmod a+r /etc/apt/keyrings/docker.asc \
 && echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/docker.asc]" \
      "https://download.docker.com/linux/ubuntu" \
      "$(. /etc/os-release && echo "$VERSION_CODENAME")" \
      "stable" > /etc/apt/sources.list.d/docker.list \
 && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      -o /etc/apt/keyrings/githubcli.gpg \
 && chmod a+r /etc/apt/keyrings/githubcli.gpg \
 && echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/githubcli.gpg]" \
      "https://cli.github.com/packages stable main" \
      > /etc/apt/sources.list.d/github-cli.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      docker-ce-cli \
      docker-buildx-plugin \
      gh \
 && rm -rf /var/lib/apt/lists/*

# The host daemon is rootless, so a container running as root is the host's own
# unprivileged user and nothing more. Running as root is what lets it reach the
# mounted socket, which that user owns.
ENV DOCKER_HOST=unix:///var/run/docker.sock \
    RUNNER_ALLOW_RUNASROOT=1
