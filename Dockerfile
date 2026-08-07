# syntax=docker/dockerfile:1
#
# xd is built entirely inside Docker and *run* on the host. The final `bundle`
# stage emits a self-contained directory tree (binary + full library closure +
# GTK support data + launcher) so the result runs on any glibc-based x86_64
# host, including NixOS where there is no /lib64 loader and no system GTK.

# --- GPUI desktop ----------------------------------------------------------
FROM rust:1.95-slim-trixie@sha256:28846ec5a6bcfcddb93f403ba7071bd579787852b2f2ac3839965620e8bd9456 AS gpui-toolchain

ENV PATH="/usr/local/cargo/bin:${PATH}"

RUN rustup component add rustfmt \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      clang \
      cmake \
      g++ \
      gcc \
      git \
      libasound2-dev \
      libfontconfig-dev \
      libglib2.0-dev \
      libssl-dev \
      libva-dev \
      libvulkan1 \
      libwayland-dev \
      libx11-xcb-dev \
      libxkbcommon-x11-dev \
      libzstd-dev \
      lld \
      make \
      pkg-config \
 && rm -rf /var/lib/apt/lists/*

FROM gpui-toolchain AS gpui-desktop-source

WORKDIR /src/desktop
COPY desktop/Cargo.toml desktop/Cargo.lock ./
RUN mkdir -p src \
 && touch src/lib.rs \
 && cargo fetch --locked \
 && rm -rf src
COPY desktop/src ./src

FROM gpui-desktop-source AS gpui-desktop-tests

RUN cargo fmt --check \
 && cargo test --locked \
 && touch /gpui-tests-passed

FROM gpui-desktop-source AS gpui-desktop-windows-check

RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64 \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add x86_64-pc-windows-gnu

RUN cargo check --locked --target x86_64-pc-windows-gnu \
 && touch /gpui-windows-check-passed

FROM gpui-desktop-source AS gpui-desktop-release

ARG COMMIT=development
ARG XD_COMMIT=$COMMIT
ENV XD_COMMIT=$XD_COMMIT

RUN cargo build --locked --release \
 && test -x target/release/xd-desktop

FROM gpui-toolchain AS rust-daemon-source

WORKDIR /src/daemon-rs
COPY daemon-rs/Cargo.toml daemon-rs/Cargo.lock ./
RUN mkdir -p src \
 && touch src/lib.rs \
 && cargo fetch --locked \
 && rm -rf src
COPY daemon-rs/src ./src
COPY tests/fixtures/codex-exec.jsonl tests/fixtures/codex-recoverable-error.jsonl tests/fixtures/claude-stream.jsonl /src/tests/fixtures/

FROM rust-daemon-source AS rust-daemon-tests

RUN cargo fmt --check \
 && cargo test --locked \
 && touch /rust-daemon-tests-passed

FROM rust-daemon-source AS rust-daemon-windows-check

RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64 \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add x86_64-pc-windows-gnu

RUN cargo check --locked --target x86_64-pc-windows-gnu \
 && touch /rust-daemon-windows-check-passed

FROM rust-daemon-source AS rust-daemon-release

ARG COMMIT=development
ARG XD_COMMIT=$COMMIT
ENV XD_COMMIT=$XD_COMMIT

RUN cargo build --locked --release \
 && test -x target/release/xd-daemon

# The remote TLS terminator is dependency-isolated from the daemon so the
# existing desktop and daemon lockfiles remain untouched during the rewrite.
# Direct dependency versions are exact; this helper's generated lockfile is
# intentionally not part of the feature branch.
FROM gpui-toolchain AS rust-tls-proxy-source

WORKDIR /src/tls-proxy-rs
COPY tls-proxy-rs/Cargo.toml ./
RUN mkdir -p src \
 && touch src/main.rs \
 && cargo fetch \
 && rm -rf src
COPY tls-proxy-rs/src ./src

FROM rust-tls-proxy-source AS rust-tls-proxy-tests

RUN cargo fmt --check \
 && cargo test \
 && touch /rust-tls-proxy-tests-passed

FROM rust-tls-proxy-source AS rust-tls-proxy-release

RUN cargo build --release \
 && test -x target/release/xd-tls-proxy

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
      -DWHISPER_BUILD_SERVER=ON \
      -DGGML_NATIVE=OFF \
      -DGGML_BACKEND_DL=ON \
      -DGGML_CPU_ALL_VARIANTS=ON \
      -DGGML_OPENMP=OFF \
      -DGGML_CCACHE=OFF; \
    cmake --build /build --target whisper-cli whisper-server --parallel 4; \
    install -d /voice/lib /voice/libexec; \
    install -m0755 /build/bin/whisper-cli /voice/libexec/whisper-bin; \
    install -m0755 /build/bin/whisper-server /voice/libexec/whisper-server-bin; \
    cp -a /build/bin/libwhisper.so* /build/bin/libggml*.so* /voice/lib/; \
    find /voice -type f -exec strip --strip-unneeded {} +; \
    rm -rf /build /source /tmp/whisper.tar.gz

# --- bundled agent CLIs ----------------------------------------------------
#
# Codex and Claude are official native builds; Claude mode's MIT proxy is
# pinned from its upstream release. Keep Codex's whole package:
# its binary discovers the bundled ripgrep, sandbox and shell relative to its
# package metadata. Claude is renamed because libexec/claude is a small wrapper
# that starts it through the bundle's loader on hosts such as NixOS.
FROM debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd AS agent-binaries

ARG CODEX_VERSION=0.146.0
ARG CODEX_SHA256=3c89125af1d7c98abec8beb551292ef99daca52e204e5852a9139feae2c467e5
ARG CLAUDE_VERSION=2.1.220
ARG CLAUDE_SHA256=674f61f20ff306f3100cf9200e4c36c4b70278b5bef2884549819b942a89c863
ARG CLAUDE_PROXY_VERSION=0.1.30
ARG CLAUDE_PROXY_SHA256=afa08d97141be5003b3abcbfa482f58e9c119c128ef955f92776816dbcae96a0

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
    curl --fail --location --silent --show-error \
      "https://github.com/raine/claude-code-proxy/releases/download/v${CLAUDE_PROXY_VERSION}/claude-code-proxy-linux-amd64.tar.gz" \
      --output /downloads/claude-code-proxy.tar.gz; \
    printf '%s  %s\n' "$CLAUDE_PROXY_SHA256" /downloads/claude-code-proxy.tar.gz \
      | sha256sum --check; \
    tar -xzf /downloads/claude-code-proxy.tar.gz -C /agents; \
    chmod 0755 /agents/claude-bin; \
    /agents/codex --version | grep -F "$CODEX_VERSION"; \
    /agents/claude-bin --version | grep -F "$CLAUDE_VERSION"; \
    /agents/claude-code-proxy --version | grep -F "$CLAUDE_PROXY_VERSION"; \
    rm -rf /downloads

FROM debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd AS gpui-desktop-check

COPY --from=gpui-desktop-tests /gpui-tests-passed /gpui-tests-passed
COPY --from=gpui-desktop-windows-check /gpui-windows-check-passed /gpui-windows-check-passed
COPY --from=gpui-desktop-release /src/desktop/target/release/xd-desktop /xd-desktop
COPY --from=rust-daemon-tests /rust-daemon-tests-passed /rust-daemon-tests-passed
COPY --from=rust-daemon-windows-check /rust-daemon-windows-check-passed /rust-daemon-windows-check-passed
COPY --from=rust-daemon-release /src/daemon-rs/target/release/xd-daemon /xd-daemon
COPY --from=rust-tls-proxy-tests /rust-tls-proxy-tests-passed /rust-tls-proxy-tests-passed
COPY --from=rust-tls-proxy-release /src/tls-proxy-rs/target/release/xd-tls-proxy /xd-tls-proxy
COPY --from=agent-binaries /agents/codex-package /codex-package
COPY --from=agent-binaries /agents/claude-bin /claude
COPY --from=agent-binaries /agents/claude-code-proxy /claude-code-proxy
COPY --from=voice-build /voice /voice

# --- bundle runtime closure ------------------------------------------------
FROM debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd AS bundle-tools

ENV DEBIAN_FRONTEND=noninteractive

ARG GIT_VERSION=2.47.3

RUN apt-get update && apt-get install -y --no-install-recommends \
      adwaita-icon-theme \
      ca-certificates \
      curl \
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
      libasound2 \
      libfontconfig1 \
      libgtk-4-dev \
      libportaudio2 \
      libpulse-dev \
      libsqlite3-dev \
      libssl-dev \
      libvulkan1 \
      libvte-2.91-gtk4-dev \
      libwayland-client0 \
      libx11-data \
      libx11-xcb1 \
      libxkbcommon-x11-0 \
      librsvg2-common \
      openssl \
      patchelf \
      shared-mime-info \
      xkb-data \
    && git --version | grep -F "git version $GIT_VERSION" \
    && rm -rf /var/lib/apt/lists/*

# Keep the established CI target name, now backed entirely by Rust.
FROM gpui-desktop-check AS test

# --- assemble redistributable bundle ---------------------------------------
FROM bundle-tools AS staging

ARG PROFILE=default

WORKDIR /src
COPY data ./data
COPY scripts/bundle.sh /usr/local/bin/bundle.sh
COPY scripts/smoke-bundle.sh /usr/local/bin/smoke-bundle.sh
COPY scripts/xd.sh /usr/local/share/xd-launcher.sh
COPY scripts/install.sh /stage/usr/libexec/install.sh
COPY scripts/claude.sh /stage/usr/libexec/claude
COPY scripts/curl.sh /stage/usr/libexec/curl
COPY scripts/git.sh /stage/usr/bin/git
COPY scripts/git-helper.sh /stage/usr/libexec/git-helper
COPY scripts/openssl.sh /stage/usr/libexec/openssl
COPY scripts/whisper.sh /stage/usr/libexec/whisper
COPY scripts/whisper-server.sh /stage/usr/libexec/whisper-server
COPY --from=gpui-desktop-release /src/desktop/target/release/xd-desktop /stage/usr/bin/xd
COPY --from=rust-daemon-release /src/daemon-rs/target/release/xd-daemon /stage/usr/libexec/xd-daemon
COPY --from=rust-tls-proxy-release /src/tls-proxy-rs/target/release/xd-tls-proxy /stage/usr/libexec/xd-tls-proxy
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
      settings_path=/com/restartfu/Xd/; \
    fi; \
    install -d \
      /stage/usr/share/applications \
      /stage/usr/share/fonts/xd \
      /stage/usr/share/glib-2.0/schemas \
      /stage/usr/share/licenses/xd \
      /stage/usr/share/icons/hicolor/scalable/apps \
      /stage/usr/share/icons/hicolor/symbolic/apps; \
    install -m0644 \
      data/fonts/DMSans-Variable.ttf \
      /stage/usr/share/fonts/xd/DMSans-Variable.ttf; \
    install -m0644 \
      data/licenses/claude-code-proxy-LICENSE \
      /stage/usr/share/licenses/xd/claude-code-proxy-LICENSE; \
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
      data/icons/hicolor/scalable/apps/xd-backend-claude-mode.svg \
      /stage/usr/share/icons/hicolor/scalable/apps/xd-backend-claude-mode.svg; \
    install -m0644 \
      data/icons/hicolor/symbolic/apps/xd-backend-codex-symbolic.svg \
      /stage/usr/share/icons/hicolor/symbolic/apps/xd-backend-codex-symbolic.svg; \
    install -m0644 \
      data/icons/hicolor/symbolic/apps/xd-download-symbolic.svg \
      /stage/usr/share/icons/hicolor/symbolic/apps/xd-download-symbolic.svg; \
    install -Dm755 /usr/bin/git /stage/usr/libexec/git-bin; \
    cp -a /usr/lib/git-core /stage/usr/libexec/git-core-real; \
    mkdir -p /stage/usr/share/git-core; \
    cp -a /usr/share/git-core/templates /stage/usr/share/git-core/; \
    mv /usr/bin/openssl /stage/usr/libexec/openssl-bin; \
    mv /usr/bin/curl /stage/usr/libexec/curl-bin; \
    chmod 0755 \
      /stage/usr/bin/git \
      /stage/usr/libexec/claude \
      /stage/usr/libexec/curl \
      /stage/usr/libexec/git-helper \
      /stage/usr/libexec/install.sh \
      /stage/usr/libexec/openssl \
      /stage/usr/libexec/whisper \
      /stage/usr/libexec/whisper-server; \
    desktop-file-validate "/stage/usr/share/applications/$app_id.desktop"; \
    bash /usr/local/bin/bundle.sh \
      /stage /out /usr/local/share/xd-launcher.sh; \
    sh /usr/local/bin/smoke-bundle.sh /out

# --- export ----------------------------------------------------------------
FROM scratch AS bundle

COPY --from=staging /out /
