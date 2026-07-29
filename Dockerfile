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
      cmake \
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

# --- stage 2: local speech runtime ------------------------------------------
FROM deps AS whisper

RUN git clone --quiet --depth 1 --branch v1.9.1 \
      https://github.com/ggml-org/whisper.cpp.git /whisper \
 && test "$(git -C /whisper rev-parse HEAD)" = \
      f049fff95a089aa9969deb009cdd4892b3e74916 \
 && cmake -S /whisper -B /whisper/build \
      -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_INSTALL_PREFIX=/usr/local \
      -DCMAKE_INSTALL_BINDIR=lib \
      -DGGML_NATIVE=OFF \
      -DGGML_BACKEND_DL=ON \
      -DGGML_CPU_ALL_VARIANTS=ON \
      -DWHISPER_BUILD_EXAMPLES=OFF \
      -DWHISPER_BUILD_SERVER=OFF \
      -DWHISPER_BUILD_TESTS=OFF \
 && cmake --build /whisper/build --parallel \
 && cmake --install /whisper/build

# --- stage 3: the sources ---------------------------------------------------
FROM deps AS sources

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

# --- stage 4: headless tests (CI gate: docker build --target test .) --------
#
# From the sources rather than from the compiled app, so the gate does not wait
# on whisper.cpp. Voice is the window's: nothing the tests touch links it, and
# meson leaves it out when it is not there.
FROM sources AS test

ARG PROFILE=default
ARG COMMIT=

RUN meson setup /build --prefix=/usr --buildtype=release \
      -Dprofile="$PROFILE" -Dcommit="$COMMIT" \
 && meson test -C /build --print-errorlogs

# --- stage 5: compile the app, voice and all --------------------------------
FROM sources AS build

ARG PROFILE=default
ARG COMMIT=

COPY --from=whisper /usr/local/ /usr/local/
ENV PKG_CONFIG_PATH=/usr/local/lib/pkgconfig

RUN meson setup /build --prefix=/usr --buildtype=release \
      -Dprofile="$PROFILE" -Dcommit="$COMMIT" \
 && meson compile -C /build

# --- stage 5: assemble the redistributable bundle ---------------------------
FROM build AS staging

COPY scripts/bundle.sh /usr/local/bin/bundle.sh
COPY scripts/xd.sh /usr/local/share/xd-launcher.sh

RUN DESTDIR=/stage meson install -C /build --no-rebuild --quiet \
 && bash /usr/local/bin/bundle.sh /stage /out /usr/local/share/xd-launcher.sh

# --- stage 6: export --------------------------------------------------------
FROM scratch AS bundle

COPY --from=staging /out /
