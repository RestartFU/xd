# syntax=docker/dockerfile:1
#
# xd is built entirely inside Docker and *run* on the host. The final `bundle`
# stage emits a self-contained directory tree (binary + full library closure +
# GTK support data + launcher) so the result runs on any glibc-based x86_64
# host, including NixOS where there is no /lib64 loader and no system GTK.

# --- local speech engine ---------------------------------------------------
#
# whisper.cpp is built from pinned source because Debian does not package its
# C library or CLI. Dynamic CPU variants keep one bundle portable across x64
# generations while retaining optimized inference on newer machines.
FROM debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd AS voice-build

ARG WHISPER_VERSION=1.9.1
ARG WHISPER_SHA256=147267177eef7b22ec3d2476dd514d1b12e160e176230b740e3d1bd600118447

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      cmake \
      curl \
      g++ \
      make \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    curl --fail --location --silent --show-error \
      "https://github.com/ggml-org/whisper.cpp/archive/refs/tags/v${WHISPER_VERSION}.tar.gz" \
      --output /tmp/whisper.tar.gz; \
    printf '%s  %s\n' "$WHISPER_SHA256" /tmp/whisper.tar.gz \
      | sha256sum --check; \
    mkdir /source; \
    tar -xzf /tmp/whisper.tar.gz -C /source --strip-components=1; \
    cmake -S /source -B /build \
      -DCMAKE_BUILD_TYPE=Release \
      -DBUILD_SHARED_LIBS=ON \
      -DWHISPER_BUILD_TESTS=OFF \
      -DWHISPER_BUILD_EXAMPLES=ON \
      -DWHISPER_BUILD_SERVER=OFF \
      -DGGML_NATIVE=OFF \
      -DGGML_BACKEND_DL=ON \
      -DGGML_CPU_ALL_VARIANTS=ON \
      -DGGML_OPENMP=OFF \
      -DGGML_CCACHE=OFF; \
    cmake --build /build --target whisper-cli --parallel 4; \
    install -d /voice/lib /voice/libexec; \
    install -m0755 /build/bin/whisper-cli /voice/libexec/whisper-bin; \
    cp -a /build/bin/libwhisper.so* /build/bin/libggml*.so* /voice/lib/; \
    find /voice -type f -exec strip --strip-unneeded {} +; \
    rm -rf /build /source /tmp/whisper.tar.gz

# --- Crystal migration toolchain -------------------------------------------
#
# Pin the multi-platform manifest, not only the tag. Developers need Docker,
# never a host Crystal compiler. This stage remains separate while the
# behavior-parity suite is moved module by module from C.
FROM crystallang/crystal:1.21.0@sha256:32b7b908a8c3625ebd629053daf48b6f469deaf74aeb71ad101895096b1665fa AS crystal-toolchain

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      gir1.2-adw-1 \
      gir1.2-gtk-4.0 \
      gir1.2-vte-3.91 \
      gobject-introspection \
      libgirepository1.0-dev \
      libgtk-4-dev \
      libadwaita-1-dev \
      libpulse-dev \
      libsqlite3-dev \
      libvte-2.91-gtk4-dev \
 && rm -rf /var/lib/apt/lists/*

FROM crystal-toolchain AS crystal

ARG PROFILE=default
ARG COMMIT=

WORKDIR /src
COPY shard.yml shard.lock ./
RUN shards install --production --frozen
COPY bindings ./bindings
RUN ./bin/gi-crystal

COPY src/xd.cr ./src/xd.cr
COPY src/xd ./src/xd
COPY spec ./spec
COPY tests/fixtures ./tests/fixtures

RUN test "$PROFILE" = default || test "$PROFILE" = nightly \
 && XD_BUILD_PROFILE="$PROFILE" XD_BUILD_COMMIT="$COMMIT" \
      crystal spec --error-trace \
 && mkdir -p /crystal-build \
 && XD_BUILD_PROFILE="$PROFILE" XD_BUILD_COMMIT="$COMMIT" \
      crystal build src/xd.cr --release --no-debug -o /crystal-build/xd \
 && /crystal-build/xd --version

# --- bundled agent CLIs ----------------------------------------------------
#
# Both agents are official native Linux builds. Keep Codex's whole package:
# its binary discovers the bundled ripgrep, sandbox and shell relative to its
# package metadata. Claude is renamed because libexec/claude is a small wrapper
# that starts it through the bundle's loader on hosts such as NixOS.
FROM debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd AS agent-binaries

ARG CODEX_VERSION=0.146.0
ARG CODEX_SHA256=3c89125af1d7c98abec8beb551292ef99daca52e204e5852a9139feae2c467e5
ARG CLAUDE_VERSION=2.1.220
ARG CLAUDE_SHA256=674f61f20ff306f3100cf9200e4c36c4b70278b5bef2884549819b942a89c863

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    test "$(dpkg --print-architecture)" = amd64; \
    mkdir -p /agents/codex-package /downloads; \
    curl --fail --location --silent --show-error \
      "https://releases.openai.com/codex/releases/${CODEX_VERSION}/codex-package-x86_64-unknown-linux-musl.tar.gz" \
      --output /downloads/codex.tar.gz; \
    printf '%s  %s\n' "$CODEX_SHA256" /downloads/codex.tar.gz \
      | sha256sum --check; \
    tar -xzf /downloads/codex.tar.gz -C /agents/codex-package; \
    ln -s codex-package/bin/codex /agents/codex; \
    curl --fail --location --silent --show-error \
      "https://downloads.claude.ai/claude-code-releases/${CLAUDE_VERSION}/linux-x64/claude" \
      --output /agents/claude-bin; \
    printf '%s  %s\n' "$CLAUDE_SHA256" /agents/claude-bin \
      | sha256sum --check; \
    chmod 0755 /agents/claude-bin; \
    /agents/codex --version | grep -F "$CODEX_VERSION"; \
    /agents/claude-bin --version | grep -F "$CLAUDE_VERSION"; \
    rm -rf /downloads

# --- bundle runtime closure ------------------------------------------------
FROM debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd AS bundle-tools

ENV DEBIAN_FRONTEND=noninteractive

ARG GIT_VERSION=2.47.3

RUN apt-get update && apt-get install -y --no-install-recommends \
      adwaita-icon-theme \
      ca-certificates \
      desktop-file-utils \
      file \
      fontconfig \
      fonts-cantarell \
      fonts-dejavu-core \
      fonts-inter \
      fonts-jetbrains-mono \
      fonts-noto-color-emoji \
      git \
      glib-networking \
      libegl-mesa0 \
      libegl1 \
      libgl1-mesa-dri \
      libadwaita-1-dev \
      libgtk-4-dev \
      libpulse-dev \
      libsqlite3-dev \
      libssl-dev \
      libvte-2.91-gtk4-dev \
      libx11-data \
      librsvg2-common \
      openssl \
      patchelf \
      shared-mime-info \
      xkb-data \
    && git --version | grep -F "git version $GIT_VERSION" \
    && rm -rf /var/lib/apt/lists/*

# Keep established CI target name.
FROM crystal AS test

# --- assemble redistributable bundle ---------------------------------------
FROM bundle-tools AS staging

ARG PROFILE=default

WORKDIR /src
COPY data ./data
COPY scripts/bundle.sh /usr/local/bin/bundle.sh
COPY scripts/smoke-bundle.sh /usr/local/bin/smoke-bundle.sh
COPY scripts/xd.sh /usr/local/share/xd-launcher.sh
COPY scripts/claude.sh /stage/usr/libexec/claude
COPY scripts/git.sh /stage/usr/bin/git
COPY scripts/git-helper.sh /stage/usr/libexec/git-helper
COPY scripts/openssl.sh /stage/usr/libexec/openssl
COPY scripts/whisper.sh /stage/usr/libexec/whisper
COPY --from=crystal /crystal-build/xd /stage/usr/bin/xd
COPY --from=agent-binaries /agents/ /stage/usr/libexec/
COPY --from=voice-build /voice/libexec/ /stage/usr/libexec/
COPY --from=voice-build /voice/lib/ /stage/usr/lib/

RUN set -eux; \
    test "$PROFILE" = default || test "$PROFILE" = nightly; \
    if [ "$PROFILE" = nightly ]; then \
      app_id=com.restartfu.Xd.Nightly; \
      app_name='xd (Nightly)'; \
      settings_path=/com/restartfu/XdNightly/; \
    else \
      app_id=com.restartfu.Xd; \
      app_name=xd; \
      settings_path=/com/restartfu/Hy/; \
    fi; \
    install -d \
      /stage/usr/share/applications \
      /stage/usr/share/fonts/xd \
      /stage/usr/share/glib-2.0/schemas \
      /stage/usr/share/icons/hicolor/scalable/apps \
      /stage/usr/share/icons/hicolor/symbolic/apps; \
    install -m0644 \
      data/fonts/DMSans-Variable.ttf \
      /stage/usr/share/fonts/xd/DMSans-Variable.ttf; \
    sed \
      -e "s|@APP_ID@|$app_id|g" \
      -e "s|@APP_NAME@|$app_name|g" \
      data/com.restartfu.Xd.desktop.in \
      > "/stage/usr/share/applications/$app_id.desktop"; \
    sed \
      -e "s|@APP_ID@|$app_id|g" \
      -e "s|@SETTINGS_PATH@|$settings_path|g" \
      data/com.restartfu.Xd.gschema.xml.in \
      > "/stage/usr/share/glib-2.0/schemas/$app_id.gschema.xml"; \
    install -m0644 \
      data/icons/hicolor/scalable/apps/com.restartfu.Xd.svg \
      "/stage/usr/share/icons/hicolor/scalable/apps/$app_id.svg"; \
    install -m0644 \
      data/icons/hicolor/scalable/apps/xd-backend-claude.svg \
      /stage/usr/share/icons/hicolor/scalable/apps/xd-backend-claude.svg; \
    install -m0644 \
      data/icons/hicolor/symbolic/apps/xd-backend-codex-symbolic.svg \
      /stage/usr/share/icons/hicolor/symbolic/apps/xd-backend-codex-symbolic.svg; \
    install -Dm755 /usr/bin/git /stage/usr/libexec/git-bin; \
    cp -a /usr/lib/git-core /stage/usr/libexec/git-core-real; \
    mkdir -p /stage/usr/share/git-core; \
    cp -a /usr/share/git-core/templates /stage/usr/share/git-core/; \
    mv /usr/bin/openssl /stage/usr/libexec/openssl-bin; \
    chmod 0755 \
      /stage/usr/bin/git \
      /stage/usr/libexec/claude \
      /stage/usr/libexec/git-helper \
      /stage/usr/libexec/openssl \
      /stage/usr/libexec/whisper; \
    desktop-file-validate "/stage/usr/share/applications/$app_id.desktop"; \
    bash /usr/local/bin/bundle.sh \
      /stage /out /usr/local/share/xd-launcher.sh; \
    sh /usr/local/bin/smoke-bundle.sh /out

# --- export ----------------------------------------------------------------
FROM scratch AS bundle

COPY --from=staging /out /
