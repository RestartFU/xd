# xd

xd is a GTK4 desktop client for workspace-organized Codex and Claude Code
conversations. Chats inherit their folder's working directory, repository,
backend, model, and project instructions.

![xd workspace, chat, terminal, and diff panes](docs/assets/xd-showcase.png)

## Install

Linux x86_64:

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install.sh | sh
```

macOS Apple Silicon:

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install-macos.sh | sh
```

Windows x86_64 (PowerShell):

```powershell
irm https://github.com/RestartFU/xd/releases/download/nightly/install.ps1 | iex
```

The Linux and macOS installers require no root access; Windows uses the
standard MSI installer and may request UAC approval. Every platform receives a
self-contained app with GTK, Git, Codex, Claude Code, speech support, and native
runtime libraries. No system package-manager installation is required. Installed
Linux builds check for new releases in the background and only download an
update after you click the update button. Linux nightly builds can also build
and install a branch, pull request, or commit; that source-build action requires
Docker.

Nightly data lives in `~/.local/share/xd-nightly` on Linux and the equivalent
platform data directory on macOS and Windows. Uninstalling the app does not
delete chats or workspaces.

## What it does

- Organizes chats in real workspace folders that can be nested, moved, or
  renamed without losing their conversations.
- Inherits folder settings and instructions down the workspace tree.
- Runs bundled Codex and Claude Code CLIs with their normal authentication and
  configuration.
- Supports existing checkouts and isolated Git worktrees.
- Streams Markdown responses, tool calls, inline file diffs, images, and voice
  messages.
- Supports local use and paired clients over the same daemon protocol.
- Searches all stored messages with `Ctrl+K`.

## Build and test

Linux builds require Docker only and do not install dependencies on the host:

```sh
./scripts/build.sh     # self-contained bundle in ./dist
./scripts/test.sh      # complete headless test suite
./dist/xd.sh           # run the built app
```

Native release builders:

```sh
./scripts/build-macos.sh ./macos-dist nightly
./scripts/build-windows.sh ./windows-dist nightly
```

The macOS builder requires Apple Silicon and its Homebrew build dependencies.
The Windows builder runs in an x86_64 MSYS2 UCRT64 shell. Their outputs are
self-contained; end users do not need those build dependencies.

xd is written in Crystal. The client and daemon are built from `src/xd.cr`;
behavior and protocol specs live under `spec/`.
