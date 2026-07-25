# syntax=docker/dockerfile:1
#
# hy is built entirely inside Docker and *run* on the host. The final `bundle`
# stage emits a self-contained directory tree (binary + full library closure +
# GTK support data + launcher) so the result runs on any glibc-based x86_64
# host, including NixOS where there is no /lib64 loader and no system GTK.

# --- stage 1: build + runtime dependencies ----------------------------------
FROM debian:trixie-slim AS deps

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential \
      meson \
      ninja-build \
      pkg-config \
      gettext \
      desktop-file-utils \
      libgtk-4-dev \
      libadwaita-1-dev \
      libjson-glib-dev \
      libsqlite3-dev \
      librsvg2-common \
      adwaita-icon-theme \
      fonts-cantarell \
      fonts-dejavu-core \
      fontconfig \
      file \
      patchelf \
    && rm -rf /var/lib/apt/lists/*

# --- stage 2: compile -------------------------------------------------------
FROM deps AS build

WORKDIR /src
COPY meson.build ./
COPY data ./data
COPY src ./src
COPY tests ./tests

RUN meson setup /build --prefix=/usr --buildtype=release \
 && meson compile -C /build

# --- stage 3: headless tests (CI gate: docker build --target test .) --------
FROM build AS test

RUN meson test -C /build --print-errorlogs

# --- stage 4: assemble the redistributable bundle ---------------------------
FROM build AS staging

COPY scripts/bundle.sh /usr/local/bin/bundle.sh
COPY scripts/hy.sh /usr/local/share/hy-launcher.sh

RUN DESTDIR=/stage meson install -C /build --no-rebuild --quiet \
 && bash /usr/local/bin/bundle.sh /stage /out /usr/local/share/hy-launcher.sh

# --- stage 5: export --------------------------------------------------------
FROM scratch AS bundle

COPY --from=staging /out /
