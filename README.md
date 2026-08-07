# xd

xd is a Rust/GPUI desktop client for workspace-organized Codex and Claude Code
conversations. Chats inherit their folder's working directory, repository,
backend, model, and project instructions.

![xd workspace, chat, terminal, and diff panes](docs/assets/xd-showcase.png)

## Install

Linux x86_64:

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install.sh | sh
```

macOS (Apple Silicon or Intel):

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install-macos.sh | sh
```

Both desktop installers require no root access. The bundles include the GPUI
desktop, Rust daemon, Codex, Claude Code, and their native runtime helpers.
The Linux build also includes speech input; macOS microphone capture is still
being ported. Windows desktop artifacts are not currently published.

Nightly data lives in `~/.local/share/xd-nightly` on Linux and
`~/Library/Application Support/xd-nightly` on macOS. Uninstalling the app does
not delete chats or workspaces.

## What it does

- Organizes chats in real workspace folders that can be nested, moved, or
  renamed without losing their conversations.
- Inherits folder settings and instructions down the workspace tree.
- Runs bundled Codex and Claude Code CLIs with their normal authentication and
  configuration.
- Supports existing checkouts and isolated Git worktrees.
- Streams Markdown responses, tool calls, inline file diffs, images, and voice
  messages.
- Supports local use and paired clients over the same daemon protocol,
  including an Android client that pairs with a running `xd serve`.
- Searches all stored messages with `Ctrl+K`.

## Build and test

Linux builds require Docker only and do not install dependencies on the host:

```sh
./scripts/build.sh     # self-contained bundle in ./dist
./scripts/test.sh      # complete headless test suite
./dist/xd.sh           # run the built app
make mobile-test       # shared Kotlin tests
make mobile-android    # -> ./dist/mobile/xd-mobile-debug.apk
```

Native macOS builds require Rust, `librsvg` from Homebrew, and Apple command
line tools:

```sh
PROFILE=nightly ./scripts/build-macos.sh
# -> ./dist/macos/xd-nightly-macos-{arm64,x86_64}.zip
```

Mobile builds use their own Docker image and also require nothing beyond
Docker. See [mobile development](docs/mobile.md).

The Linux desktop lives in `desktop/`, the daemon in `daemon-rs/`, and the
private remote TLS helper in `tls-proxy-rs/`.
