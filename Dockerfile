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
      gir1.2-adw-1 \
      gir1.2-gtk-4.0 \
      gir1.2-vte-3.91 \
      gobject-introspection \
      libgirepository1.0-dev \
      libgtk-4-dev \
      libadwaita-1-dev \
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
      glib-networking \
      libegl-mesa0 \
      libegl1 \
      libgl1-mesa-dri \
      libadwaita-1-dev \
      libgtk-4-dev \
      libsqlite3-dev \
      libssl-dev \
      libvte-2.91-gtk4-dev \
      libx11-data \
      librsvg2-common \
      openssl \
      patchelf \
      shared-mime-info \
      xkb-data \
    && rm -rf /var/lib/apt/lists/*

# Keep established CI target name.
FROM crystal AS test

# --- assemble redistributable bundle ---------------------------------------
FROM bundle-tools AS staging

ARG PROFILE=default

WORKDIR /src
COPY data ./data
COPY scripts/bundle.sh /usr/local/bin/bundle.sh
COPY scripts/xd.sh /usr/local/share/xd-launcher.sh
COPY scripts/claude.sh /stage/usr/libexec/claude
COPY scripts/openssl.sh /stage/usr/libexec/openssl
COPY --from=crystal /crystal-build/xd /stage/usr/bin/xd
COPY --from=agent-binaries /agents/ /stage/usr/libexec/

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
    mv /usr/bin/openssl /stage/usr/libexec/openssl-bin; \
    chmod 0755 /stage/usr/libexec/claude /stage/usr/libexec/openssl; \
    desktop-file-validate "/stage/usr/share/applications/$app_id.desktop"; \
    bash /usr/local/bin/bundle.sh \
      /stage /out /usr/local/share/xd-launcher.sh

# --- export ----------------------------------------------------------------
FROM scratch AS bundle

COPY --from=staging /out /
