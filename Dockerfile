# syntax=docker/dockerfile:1
#
# xd is built entirely inside Docker and *run* on the host. The final `bundle`
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
      git \
      desktop-file-utils \
      libgtk-4-dev \
      libadwaita-1-dev \
      libjson-glib-dev \
      libsqlite3-dev \
      libcmark-dev \
      libpulse-dev \
      libsoup-3.0-dev \
      libvte-2.91-gtk4-dev \
      libegl1 \
      libgl1-mesa-dri \
      libegl-mesa0 \
      librsvg2-common \
      xkb-data \
      libx11-data \
      adwaita-icon-theme \
      fonts-cantarell \
      fonts-inter \
      fonts-jetbrains-mono \
      fonts-dejavu-core \
      fonts-noto-color-emoji \
      fontconfig \
      openssl \
      glib-networking \
      ca-certificates \
      file \
      patchelf \
    && rm -rf /var/lib/apt/lists/*

# --- stage 2: compile -------------------------------------------------------
FROM deps AS build

# Which build this is: "nightly" gives the app its own id, settings, data
# directory and workspaces, so it installs beside a release rather than over
# it. See meson_options.txt.
ARG PROFILE=default

# The commit being built, so the binary can say which one it is. scripts/build.sh
# fills it in from the checkout; there is no repository in here to read.
ARG COMMIT=

WORKDIR /src
COPY meson.build meson_options.txt ./
COPY data ./data
COPY src ./src
COPY tests ./tests

RUN meson setup /build --prefix=/usr --buildtype=release \
      -Dprofile="$PROFILE" -Dcommit="$COMMIT" \
 && meson compile -C /build

# --- stage 3: headless tests (CI gate: docker build --target test .) --------
FROM build AS test

RUN meson test -C /build --print-errorlogs

# --- stage 4: assemble the redistributable bundle ---------------------------
FROM build AS staging

COPY scripts/bundle.sh /usr/local/bin/bundle.sh
COPY scripts/xd.sh /usr/local/share/xd-launcher.sh

RUN DESTDIR=/stage meson install -C /build --no-rebuild --quiet \
 && bash /usr/local/bin/bundle.sh /stage /out /usr/local/share/xd-launcher.sh

# --- stage 5: export --------------------------------------------------------
FROM scratch AS bundle

COPY --from=staging /out /
