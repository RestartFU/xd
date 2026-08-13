# syntax=docker/dockerfile:1
#
# xd is built entirely inside Docker and *run* on the host. The final `bundle`
# stage emits a self-contained directory tree (binary + full library closure +
# display support data + launcher) so the result runs on any glibc-based x86_64
# host, including NixOS where there is no loader at the standard path.

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

ARG BUILD_JOBS=1
ENV CARGO_BUILD_JOBS=$BUILD_JOBS

FROM gpui-toolchain AS terminal-core-source

WORKDIR /src/terminal-core
COPY terminal-core/Cargo.toml terminal-core/Cargo.lock ./
RUN mkdir -p src \
 && touch src/lib.rs \
 && cargo fetch --locked \
 && rm -rf src
COPY terminal-core/src ./src

FROM terminal-core-source AS terminal-core-tests

RUN cargo fmt --check \
 && cargo test --locked \
 && touch /terminal-core-tests-passed

FROM gpui-toolchain AS gpui-desktop-source

WORKDIR /src/desktop
COPY desktop/Cargo.toml desktop/Cargo.lock ./
COPY terminal-core/Cargo.toml /src/terminal-core/Cargo.toml
RUN mkdir -p src /src/terminal-core/src \
 && touch src/lib.rs /src/terminal-core/src/lib.rs \
 && cargo fetch --locked \
 && rm -rf src /src/terminal-core/src
COPY desktop/assets ./assets
COPY data/fonts /src/data/fonts
COPY terminal-core/src /src/terminal-core/src
COPY desktop/src ./src

FROM gpui-desktop-source AS gpui-desktop-tests

RUN cargo fmt --check \
 && cargo test --locked \
 && touch /gpui-tests-passed

FROM gpui-desktop-source AS gpui-desktop-release

ARG COMMIT=development
ARG XD_COMMIT=$COMMIT
ENV XD_COMMIT=$XD_COMMIT

RUN cargo build --locked --release \
 && test -x target/release/xd-desktop

FROM gpui-toolchain AS rust-host-source

WORKDIR /src/host
COPY host/Cargo.toml host/Cargo.lock ./
COPY terminal-core/Cargo.toml /src/terminal-core/Cargo.toml
RUN mkdir -p src /src/terminal-core/src \
 && touch src/lib.rs /src/terminal-core/src/lib.rs \
 && cargo fetch --locked \
 && rm -rf src /src/terminal-core/src
COPY terminal-core/src /src/terminal-core/src
COPY host/src ./src
COPY tests/fixtures/codex-exec.jsonl tests/fixtures/codex-recoverable-error.jsonl tests/fixtures/claude-stream.jsonl /src/tests/fixtures/

FROM rust-host-source AS rust-host-tests

RUN cargo fmt --check \
 && cargo test --locked \
 && touch /rust-host-tests-passed

FROM rust-host-source AS rust-host-release

ARG COMMIT=development
ARG XD_COMMIT=$COMMIT
ENV XD_COMMIT=$XD_COMMIT

RUN cargo build --locked --release \
 && test -x target/release/xd-host

FROM debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd AS gpui-desktop-check

COPY --from=terminal-core-tests /terminal-core-tests-passed /terminal-core-tests-passed
COPY --from=gpui-desktop-tests /gpui-tests-passed /gpui-tests-passed
COPY --from=gpui-desktop-release /src/desktop/target/release/xd-desktop /xd-desktop
COPY --from=rust-host-tests /rust-host-tests-passed /rust-host-tests-passed
COPY --from=rust-host-release /src/host/target/release/xd-host /xd-host

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
      fonts-dejavu-core \
      fonts-jetbrains-mono \
      fonts-noto-color-emoji \
      git \
      libfontconfig1 \
      libsqlite3-dev \
      libssl-dev \
      libvulkan1 \
      libwayland-client0 \
      libx11-data \
      libx11-xcb1 \
      libxkbcommon-x11-0 \
      openssl \
      patchelf \
      tmux \
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
COPY scripts/curl.sh /stage/usr/libexec/curl
COPY scripts/git.sh /stage/usr/bin/git
COPY scripts/git-helper.sh /stage/usr/libexec/git-helper
COPY scripts/openssl.sh /stage/usr/libexec/openssl
RUN install -m0755 /usr/bin/tmux /stage/usr/libexec/tmux \
 && install -Dm0644 /usr/share/doc/tmux/copyright /stage/usr/share/licenses/tmux-LICENSE
COPY --from=gpui-desktop-release /src/desktop/target/release/xd-desktop /stage/usr/bin/xd
COPY --from=rust-host-release /src/host/target/release/xd-host /stage/usr/libexec/xd-host

RUN set -eux; \
    test "$PROFILE" = default || test "$PROFILE" = nightly; \
    if [ "$PROFILE" = nightly ]; then \
      app_id=com.restartfu.Xd.Nightly; \
      app_name='xd (Nightly)'; \
    else \
      app_id=com.restartfu.Xd; \
      app_name=xd; \
    fi; \
    install -d \
      /stage/usr/share/applications \
      /stage/usr/share/fonts/xd \
      /stage/usr/share/licenses/xd \
      /stage/usr/share/icons/hicolor/scalable/apps; \
    install -m0644 \
      data/fonts/DMSans-Variable.ttf \
      /stage/usr/share/fonts/xd/DMSans-Variable.ttf; \
    install -m0644 \
      data/licenses/alacritty-terminal-LICENSE-APACHE \
      /stage/usr/share/licenses/xd/alacritty-terminal-LICENSE-APACHE; \
    sed \
      -e "s|@APP_ID@|$app_id|g" \
      -e "s|@APP_NAME@|$app_name|g" \
      data/com.restartfu.Xd.desktop.in \
      > "/stage/usr/share/applications/$app_id.desktop"; \
    install -m0644 \
      data/icons/hicolor/scalable/apps/com.restartfu.Xd.svg \
      "/stage/usr/share/icons/hicolor/scalable/apps/$app_id.svg"; \
    install -Dm755 /usr/bin/git /stage/usr/libexec/git-bin; \
    cp -a /usr/lib/git-core /stage/usr/libexec/git-core-real; \
    mkdir -p /stage/usr/share/git-core; \
    cp -a /usr/share/git-core/templates /stage/usr/share/git-core/; \
    mv /usr/bin/openssl /stage/usr/libexec/openssl-bin; \
    mv /usr/bin/curl /stage/usr/libexec/curl-bin; \
    chmod 0755 \
      /stage/usr/bin/git \
      /stage/usr/libexec/curl \
      /stage/usr/libexec/git-helper \
      /stage/usr/libexec/install.sh \
      /stage/usr/libexec/openssl; \
    desktop-file-validate "/stage/usr/share/applications/$app_id.desktop"; \
    bash /usr/local/bin/bundle.sh \
      /stage /out /usr/local/share/xd-launcher.sh; \
    sh /usr/local/bin/smoke-bundle.sh /out

# --- export ----------------------------------------------------------------
FROM scratch AS bundle

COPY --from=staging /out /
