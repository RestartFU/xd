# syntax=docker/dockerfile:1
#
# xd is built entirely inside Docker and *run* on the host. The final `bundle`
# stage emits a self-contained directory tree (binary + full library closure +
# GTK support data + launcher) so the result runs on any glibc-based x86_64
# host, including NixOS where there is no /lib64 loader and no system GTK.

# --- Crystal migration toolchain -------------------------------------------
#
# Pin the multi-platform manifest, not only the tag. Developers need Docker,
# never a host Crystal compiler. This stage remains separate while the
# behavior-parity suite is moved module by module from C.
FROM crystallang/crystal:1.21.0@sha256:32b7b908a8c3625ebd629053daf48b6f469deaf74aeb71ad101895096b1665fa AS crystal-toolchain

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      gir1.2-gtk-4.0 \
      gobject-introspection \
      libgirepository1.0-dev \
      libgtk-4-dev \
      libsqlite3-dev \
 && rm -rf /var/lib/apt/lists/*

FROM crystal-toolchain AS crystal

WORKDIR /src
COPY shard.yml shard.lock ./
RUN shards install --production --frozen
RUN ./bin/gi-crystal

COPY src/xd.cr ./src/xd.cr
COPY src/xd ./src/xd
COPY spec ./spec
COPY tests/fixtures ./tests/fixtures

RUN crystal spec --error-trace \
 && mkdir -p /crystal-build \
 && crystal build src/xd.cr --release --no-debug -o /crystal-build/xd \
 && /crystal-build/xd --version

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

# --- stage 3: compile -------------------------------------------------------
FROM deps AS build

COPY --from=whisper /usr/local/ /usr/local/
ENV PKG_CONFIG_PATH=/usr/local/lib/pkgconfig

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

# --- stage 4: headless tests (CI gate: docker build --target test .) --------
FROM build AS test

RUN meson test -C /build --print-errorlogs

# --- stage 5: assemble the redistributable bundle ---------------------------
FROM build AS staging

COPY scripts/bundle.sh /usr/local/bin/bundle.sh
COPY scripts/xd.sh /usr/local/share/xd-launcher.sh

RUN DESTDIR=/stage meson install -C /build --no-rebuild --quiet \
 && bash /usr/local/bin/bundle.sh /stage /out /usr/local/share/xd-launcher.sh

# --- stage 6: export --------------------------------------------------------
FROM scratch AS bundle

COPY --from=staging /out /
