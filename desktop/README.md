# xd desktop (GPUI)

This directory contains xd's production Rust/GPUI desktop client for Linux and
macOS.

The application presents workspace-organized Codex, Claude Code, JCode, GitHub
Copilot, and terminal sessions. It owns navigation, cached UI state, terminal emulation, chat input,
themes, diffs, and the desktop window. Terminal escape processing is backed by
the in-process `alacritty_terminal` crate. Persistent chat/workspace
state and agent orchestration are provided by the bundled `xd-host` process.

## Process model

There is no background xd daemon or listening socket.

- Local mode starts `xd-host stdio` as a child of the desktop and stops it when
  the window closes.
- Remote mode runs `xd-host stdio` through the user's persisted SSH command.
- Codex, Claude Code, JCode, GitHub Copilot, and terminal tabs run in bundled tmux sessions on the
  selected local or remote machine so they can be reattached after reconnects.
- Local and remote modes never coexist in one window.

See [remote desktop over SSH](../docs/remote.md) for the remote process shape.

## Build and test

Build and test this crate through the repository Dockerfile:

```sh
docker build --target gpui-desktop-check .
```

Every push to `master` replaces the rolling Linux and macOS nightly. Tagged
releases use the stable application id and install beside the nightly.

The bundle includes the GPUI desktop, `xd-host`, and tmux. Install Codex,
Claude Code, JCode, and/or GitHub Copilot CLI separately and make the commands
available on `PATH`.
Stable and nightly installation commands are in the main
[README](../README.md#install).
