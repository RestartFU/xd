# xd desktop (GPUI)

This directory contains xd's production Rust/GPUI desktop client for Linux,
macOS, and Windows.

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

On Windows, the desktop is native and local `xd-host` and tmux sessions run in
WSL. The archive includes a matching static Linux host, deployed into the
selected distribution on startup. Install git, tmux, OpenSSH client, CA
certificates, and assistant CLIs in WSL. `XD_WSL_DISTRIBUTION` selects a distro;
the default is the user's default WSL distribution. Project paths are Linux
paths. Settings remain on Windows, while chats and workspaces live in WSL.

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

Every push to `master` replaces the rolling Linux, macOS, and Windows nightly. Tagged
releases use the stable application id and install beside the nightly.

The bundle includes the GPUI desktop, `xd-host`, and tmux. Install Codex,
Claude Code, JCode, and/or GitHub Copilot CLI separately and make the commands
available on `PATH`.
Stable and nightly installation commands are in the main
[README](../README.md#install).

## Windows builds

Build `libexec/xd-host-linux` on Linux x86_64 with Rust 1.95.0's
`x86_64-unknown-linux-musl` target and `musl-tools`:

```sh
./scripts/build-windows-host.sh
```

Then build on Windows x86_64 with Rust 1.95.0 MSVC and the Windows SDK:

```powershell
./scripts/build-windows.ps1 -Profile nightly -HostPayload path/to/xd-host-linux
./scripts/smoke-windows.ps1 -Archive dist/windows/xd-nightly-windows-x86_64.zip
```

The build discovers the SDK's `fxc.exe`; `GPUI_FXC_PATH` can override its path.
The ZIP contains `xd.exe`, its static Linux host, setup instructions, and an
explicit WebView2 installer helper. The browser uses the Evergreen WebView2
Runtime on Windows. Run `install-webview2.ps1` if it is missing. WSL installation
and distro package installation remain explicit setup steps.

The reusable `.github/workflows/windows.yml` builds both payloads and verifies
the native frontend's stdio connection with an isolated WSL distribution. It
also supports manual dispatch independently of release and nightly builds.
