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

## Integrated browser

Click **Browser** in the workspace header to open a pane beside the active
session. Enter a URL or `localhost:3000`, use Back, Forward, and Reload to
navigate, and drag the divider to resize the pane. Clicking a chat or terminal
link opens it in the Browser pane automatically. The last address is saved
per workspace; closing the pane hides its page and preserves navigation history
when reopened during the same desktop session.

The browser runs on the desktop machine in both local and SSH modes. A remote
development server needs an SSH port forward or a reachable URL; `localhost`
always means the machine displaying xd. For example, forward port 3000 with
`ssh -L 3000:localhost:3000 user@server` before opening `localhost:3000`.

macOS uses the system WKWebView. Linux uses GTK3/WebKitGTK 4.1 through X11, so
Wayland desktops need XWayland and a working `DISPLAY`. The Linux bundle ships
the browser engine, auxiliary processes, TLS backend, and GTK/media resources.
The host supplies `bubblewrap` (`bwrap`), `xdg-dbus-proxy`, and its GBM/DRM and
Wayland graphics libraries. WebKit's process sandbox stays enabled. Browser
toolkit settings are restored before xd starts terminals, agents, or its host
service.

## Build and test

Build and test this crate through the repository Dockerfile:

```sh
docker build --target gpui-desktop-check .
```

Linux source builds additionally need GTK3 and WebKitGTK 4.1 development
packages (`libgtk-3-dev` and `libwebkit2gtk-4.1-dev` on Debian). Docker installs
these dependencies automatically.

Every push to `master` replaces the rolling Linux and macOS nightly. Tagged
releases use the stable application id and install beside the nightly.

The bundle includes the GPUI desktop, `xd-host`, and tmux. Install Codex,
Claude Code, JCode, and/or GitHub Copilot CLI separately and make the commands
available on `PATH`.
Stable and nightly installation commands are in the main
[README](../README.md#install).
