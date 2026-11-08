# syntax=docker/dockerfile:1
#
# xd is built entirely inside Docker and *run* on the host. The final `bundle`
# stage emits a relocatable directory tree (binary + private library closure +
# display support data + launcher). The host supplies its glibc and graphics
# driver so those two tightly coupled parts of the platform always agree.

# --- GPUI desktop ----------------------------------------------------------
FROM rust:1.95-slim-trixie@sha256:28846ec5a6bcfcddb93f403ba7071bd579787852b2f2ac3839965620e8bd9456 AS gpui-toolchain

ENV PATH="/usr/local/cargo/bin:${PATH}"

RUN rustup component add rustfmt clippy \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      bzip2 \
      ca-certificates \
      clang \
      cmake \
      curl \
      g++ \
      gcc \
      git \
      libasound2t64 \
      libatk-bridge2.0-0t64 \
      libatk1.0-0t64 \
      libatspi2.0-0t64 \
      libcairo2 \
      libcups2t64 \
      libdbus-1-3 \
      libexpat1 \
      libfontconfig-dev \
      libgbm1 \
      libglib2.0-dev \
      libnspr4 \
      libnss3 \
      libpango-1.0-0 \
      libssl-dev \
      libudev1 \
      libva-dev \
      libvulkan1 \
      libwayland-dev \
      libx11-xcb-dev \
      libxcomposite1 \
      libxdamage1 \
      libxext6 \
      libxfixes3 \
      libxkbcommon-x11-dev \
      libxrandr2 \
      libzstd-dev \
      lld \
      make \
      ninja-build \
      pkg-config \
      python3 \
      tmux \
 && rm -rf /var/lib/apt/lists/*

# Chromium for the Linux browser, through CEF. Its version must match the `cef`
# crate in desktop/Cargo.toml. The crate expects Release and Resources flattened
# and recognizes a distribution by archive.json; otherwise it downloads its own,
# which nothing here would have verified.
ARG CEF_ARCHIVE=cef_binary_156.0.15+gcf671c0+chromium-156.0.8078.80_linux64_minimal.tar.bz2
ARG CEF_SHA256=6873f975bb047316680ea1839980ed5bd4c33c8cd4ce45761c94211856605388
ARG CEF_SHA1=09235427222e026a4f34399742df27c5d98a4f8a
ENV CEF_PATH=/opt/cef
RUN curl -fsSL --retry 3 -o /tmp/cef.tar.bz2 \
      "https://cef-builds.spotifycdn.com/$(printf '%s' "$CEF_ARCHIVE" | sed 's/+/%2B/g')" \
 && echo "$CEF_SHA256  /tmp/cef.tar.bz2" | sha256sum -c - \
 && mkdir -p "$CEF_PATH" \
 && tar -xjf /tmp/cef.tar.bz2 --strip-components=1 -C "$CEF_PATH" \
 && rm /tmp/cef.tar.bz2 \
 && mv "$CEF_PATH"/Release/* "$CEF_PATH"/Resources/* "$CEF_PATH"/ \
 && rmdir "$CEF_PATH/Release" "$CEF_PATH/Resources" \
 && printf '{"type":"minimal","name":"%s","sha1":"%s"}\n' \
      "$CEF_ARCHIVE" "$CEF_SHA1" > "$CEF_PATH/archive.json"

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

FROM terminal-core-source AS terminal-core-clippy

RUN cargo fmt --check \
 && cargo clippy --locked --all-targets -- -D warnings \
 && touch /terminal-core-clippy-passed

FROM gpui-toolchain AS transport-core-clippy

WORKDIR /src/transport-core
COPY transport-core/Cargo.toml transport-core/Cargo.lock ./
COPY transport-core/src ./src
RUN cargo fmt --check \
 && cargo clippy --locked --all-targets -- -D warnings \
 && touch /transport-core-clippy-passed

FROM gpui-toolchain AS gpui-desktop-source

WORKDIR /src/desktop
COPY desktop/Cargo.toml desktop/Cargo.lock ./
COPY terminal-core/Cargo.toml /src/terminal-core/Cargo.toml
COPY transport-core/Cargo.toml /src/transport-core/Cargo.toml
RUN mkdir -p src /src/terminal-core/src /src/transport-core/src \
 && touch src/lib.rs /src/terminal-core/src/lib.rs /src/transport-core/src/lib.rs \
 && cargo fetch --locked \
 && rm -rf src /src/terminal-core/src /src/transport-core/src
COPY desktop/assets ./assets
COPY data/fonts /src/data/fonts
COPY terminal-core/src /src/terminal-core/src
COPY transport-core/src /src/transport-core/src
COPY desktop/src ./src
COPY desktop/tests ./tests

FROM gpui-desktop-source AS gpui-desktop-tests

RUN cargo fmt --check \
 && cargo test --locked \
 && touch /gpui-tests-passed

FROM gpui-desktop-source AS gpui-desktop-clippy

RUN cargo fmt --check \
 && cargo clippy --locked --all-targets -- -D warnings \
 && touch /gpui-clippy-passed

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
COPY transport-core/Cargo.toml /src/transport-core/Cargo.toml
RUN mkdir -p src /src/terminal-core/src /src/transport-core/src \
 && touch src/lib.rs /src/terminal-core/src/lib.rs /src/transport-core/src/lib.rs \
 && cargo fetch --locked \
 && rm -rf src /src/terminal-core/src /src/transport-core/src
COPY terminal-core/src /src/terminal-core/src
COPY transport-core/src /src/transport-core/src
COPY host/src ./src
COPY tests/fixtures/codex-exec.jsonl tests/fixtures/codex-recoverable-error.jsonl tests/fixtures/claude-stream.jsonl /src/tests/fixtures/

FROM rust-host-source AS rust-host-tests

COPY host/tests ./tests

RUN cargo fmt --check \
 && cargo test --locked \
 && touch /rust-host-tests-passed

FROM rust-host-source AS rust-host-clippy

COPY host/tests ./tests

RUN cargo fmt --check \
 && cargo clippy --locked --all-targets -- -D warnings \
 && touch /rust-host-clippy-passed

FROM rust-host-source AS rust-host-release

ARG COMMIT=development
ARG XD_COMMIT=$COMMIT
ENV XD_COMMIT=$XD_COMMIT

RUN cargo build --locked --release \
 && test -x target/release/xd-host

FROM debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd AS gpui-desktop-check

COPY --from=terminal-core-tests /terminal-core-tests-passed /terminal-core-tests-passed
COPY --from=gpui-desktop-tests /gpui-tests-passed /gpui-tests-passed
COPY --from=rust-host-tests /rust-host-tests-passed /rust-host-tests-passed

# --- bundle runtime closure ------------------------------------------------
FROM debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd AS bundle-tools

ENV DEBIAN_FRONTEND=noninteractive

ARG GIT_VERSION=2.47.3

RUN apt-get update && apt-get install -y --no-install-recommends \
      adwaita-icon-theme \
      binutils \
      ca-certificates \
      curl \
      desktop-file-utils \
      file \
      fontconfig \
      fonts-dejavu-core \
      fonts-jetbrains-mono \
      fonts-noto-color-emoji \
      git \
      libasound2t64 \
      libatk-bridge2.0-0t64 \
      libatk1.0-0t64 \
      libatspi2.0-0t64 \
      libcairo2 \
      libcups2t64 \
      libdbus-1-3 \
      libexpat1 \
      libfontconfig1 \
      libgbm1 \
      libglib2.0-0t64 \
      libnspr4 \
      libnss3 \
      libpango-1.0-0 \
      libsqlite3-dev \
      libssl-dev \
      libudev1 \
      libvulkan1 \
      libwayland-client0 \
      libx11-data \
      libx11-xcb1 \
      libxcomposite1 \
      libxdamage1 \
      libxext6 \
      libxfixes3 \
      libxkbcommon-x11-0 \
      libxrandr2 \
      openssl \
      patchelf \
      python3 \
      tmux \
      xkb-data \
      xvfb \
    && git --version | grep -F "git version $GIT_VERSION" \
    && rm -rf /var/lib/apt/lists/*

# Keep the established CI target name, now backed entirely by Rust.
FROM gpui-desktop-check AS test

# Every crate's formatting and lints: all a pull request waits for. Built only
# on request, so a lint can't hold up a nightly or a release.
FROM scratch AS clippy

COPY --from=terminal-core-clippy /terminal-core-clippy-passed /terminal-core-clippy-passed
COPY --from=transport-core-clippy /transport-core-clippy-passed /transport-core-clippy-passed
COPY --from=gpui-desktop-clippy /gpui-clippy-passed /gpui-clippy-passed
COPY --from=rust-host-clippy /rust-host-clippy-passed /rust-host-clippy-passed

# --- assemble redistributable bundle ---------------------------------------
FROM bundle-tools AS staging

ARG PROFILE=default

WORKDIR /src
COPY data ./data
COPY scripts/bundle.sh /usr/local/bin/bundle.sh
COPY scripts/smoke-bundle.sh /usr/local/bin/smoke-bundle.sh
COPY scripts/test-browser-chromium.sh /usr/local/bin/test-browser-chromium.sh
COPY scripts/xd.sh /usr/local/share/xd-launcher.sh
COPY scripts/install.sh /stage/usr/libexec/install.sh
COPY scripts/curl.sh /stage/usr/libexec/curl
COPY scripts/git.sh /stage/usr/bin/git
COPY scripts/openssl.sh /stage/usr/libexec/openssl
RUN install -m0755 /usr/bin/tmux /stage/usr/libexec/tmux \
 && install -Dm0644 /usr/share/doc/tmux/copyright /stage/usr/share/licenses/tmux-LICENSE
COPY --from=gpui-desktop-release /src/desktop/target/release/xd-desktop /stage/usr/bin/xd
COPY --from=rust-host-release /src/host/target/release/xd-host /stage/usr/libexec/xd-host
COPY --from=gpui-toolchain /opt/cef/ /tmp/cef/

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
      /stage/usr/lib \
      /stage/usr/share/licenses/xd \
      /stage/usr/share/icons/hicolor/scalable/apps; \
    cp -a \
      /tmp/cef/libcef.so \
      /tmp/cef/libvk_swiftshader.so \
      /tmp/cef/vk_swiftshader_icd.json \
      /tmp/cef/v8_context_snapshot.bin \
      /tmp/cef/icudtl.dat \
      /tmp/cef/chrome_100_percent.pak \
      /tmp/cef/chrome_200_percent.pak \
      /tmp/cef/resources.pak \
      /tmp/cef/locales \
      /stage/usr/lib/; \
    install -m0644 /tmp/cef/LICENSE.txt /stage/usr/share/licenses/xd/cef-LICENSE.txt; \
    install -m0644 /tmp/cef/CREDITS.html /stage/usr/share/licenses/xd/chromium-CREDITS.html; \
    install -m0644 data/fonts/Inter-*.ttf /stage/usr/share/fonts/xd/; \
    install -m0644 \
      data/licenses/alacritty-terminal-LICENSE-APACHE \
      /stage/usr/share/licenses/xd/alacritty-terminal-LICENSE-APACHE; \
    install -m0644 data/fonts/jetbrains-mono/OFL.txt /stage/usr/share/licenses/xd/JetBrainsMono-OFL.txt; \
    install -m0644 data/licenses/t3code-LICENSE /stage/usr/share/licenses/xd/t3code-LICENSE; \
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
      /stage/usr/libexec/install.sh \
      /stage/usr/libexec/openssl; \
    desktop-file-validate "/stage/usr/share/applications/$app_id.desktop"; \
    bash /usr/local/bin/bundle.sh \
      /stage /out /usr/local/share/xd-launcher.sh; \
    sh /usr/local/bin/smoke-bundle.sh /out; \
    sh /usr/local/bin/test-browser-chromium.sh /out no-sandbox

# --- export ----------------------------------------------------------------
FROM scratch AS bundle

COPY --from=staging /out /
