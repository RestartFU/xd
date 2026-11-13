<p align="center">
  <img src="data/icons/hicolor/scalable/apps/com.restartfu.Xd.svg" width="88" alt="xd">
</p>

<h1 align="center">xd</h1>

<p align="center">
  A local-first desktop workspace for user-installed Codex and Claude Code.
  <br>
  Run many agents across projects and worktrees, side by side with their diffs, terminals, and browser.
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#features">Features</a> ·
  <a href="#documentation">Docs</a> ·
  <a href="https://github.com/RestartFU/xd/releases/latest">Latest release</a>
</p>

<p align="center">
  <img src="docs/assets/readme/workbench.png" alt="An xd session with a Claude Code chat, its uncommitted changes in the side pane, and a shell terminal below">
</p>

xd doesn't ship a model or an agent. It wraps the CLIs, logins, and configuration
you already have in one persistent workspace. A single daemon owns every session,
and you can reach it from another computer or your Android phone. Closing xd
stops the daemon it started, along with its work; run the daemon yourself to keep
work going while the app is closed.

## Features

### Chat with your agents

- **Native chat** for Codex and Claude Code: streamed replies, activity cards for
  tool calls and shell runs, queued messages, and inline approvals and questions
  showing the command or diff they would apply.
- **Live subagent cards** show each delegated agent's model, current step, tools,
  tokens, and elapsed time, and keep its report once it finishes. Stop a Claude
  agent or send it to the background from its card.
- **Background work stays visible.** A chat whose agent still has background agents
  or shells running shows as waiting on them, with each task's latest output.
- **Attachments both ways.** Attach files with the paperclip, by dropping them, or
  by pasting an image. Images an agent names in a reply appear in the chat.

### Work side by side

- **Project → session → chat tabs.** A session works in the project's checkout
  or a checkout you pick, or in a new Git worktree on its own branch. Choose a new
  worktree to keep agents from editing the same files. A session's chat tabs share
  its workspace.
- **Side pane and terminal** next to every chat: an integrated browser, files, and
  changes on the side, a shell below, all resizable and remembered per chat.
- **Agents use the browser too.** On Linux, Claude and Codex can open pages in
  their chat's browser, read them, click, type, and take screenshots while you
  watch.
- **Pull requests in place.** Projects, worktrees, and branches live in one
  sidebar, and a chat can link up to 32 pull requests: its branch's PR, ones
  opened from xd or by the agent, and any you add from the session header.
- **Settle finished sessions** with one click. Idle and merged sessions settle by
  themselves.

### Anywhere, on your terms

- **One daemon** owns local and remote sessions; clients come and go.
- **Direct remote connections** over pinned TLS, with expiring pairing codes and
  revocable devices. No relay, no SSH.
- **An Android app** with the same projects, chats, approvals, terminal, and usage.
- **Usage and limits** (<kbd>Ctrl</kbd>+<kbd>U</kbd>): cost and tokens by provider,
  model, and day, plus your Codex and Claude subscription limits across accounts.
- **Uses your own tools.** xd discovers your installed CLIs and reuses their
  authentication and configuration. Glassy dark and light themes included.

<table align="center">
  <tr>
    <td align="center"><img src="docs/assets/readme/usage-limits.png" width="300" alt="The Usage popup listing Codex and Claude Code session and weekly limits, grouped by account"></td>
    <td align="center"><img src="docs/assets/readme/android-chat.png" width="300" alt="A chat in the Android app"></td>
  </tr>
  <tr>
    <td align="center"><sub>Subscription limits by account (<kbd>Ctrl</kbd>+<kbd>U</kbd>)</sub></td>
    <td align="center"><sub>A chat on Android</sub></td>
  </tr>
</table>

## Install

**Linux** (x86_64)

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/latest/download/install.sh | sh -s -- --release
```

**macOS**

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/latest/download/install-macos.sh | sh -s -- --release
```

**Windows** (x86_64): download `xd-windows-x86_64.msi` from the
[latest release](https://github.com/RestartFU/xd/releases/latest) and run it. xd
opens when the installer finishes, and is in the Start menu from then on.

None of the installers needs root or administrator access. Nightly desktop builds are on
the [nightly release](https://github.com/RestartFU/xd/releases/tag/nightly), along
with the Android APK when signing credentials are configured.

<details>
<summary><b>Windows notes</b></summary>

The window and browser run natively; local projects, agents and persistent sessions
run in your default WSL distribution, where xd runs its own matching Linux host.

1. Install WSL and a Linux distribution: `wsl --install` in PowerShell, then finish
   the distribution's first-time setup. See
   [Microsoft's guide](https://learn.microsoft.com/windows/wsl/install).
2. Inside the distribution, install git, tmux, an SSH client and CA certificates.
   On Ubuntu or Debian: `sudo apt update && sudo apt install git tmux openssh-client ca-certificates`.
3. Install Codex, Claude Code or GitHub Copilot CLI inside the same distribution,
   on its `PATH`.

Projects and files use Linux paths, such as `/home/you/project`. Browser `localhost`
addresses are Windows's own; WSL 2 forwards servers running inside it. Set
`XD_WSL_DISTRIBUTION` to use a distribution other than the default.

The installer needs no administrator rights: it installs for your account under
`%LOCALAPPDATA%\Programs\xd`. Run a newer installer to update, and remove xd from
Windows Settings like any other app; your chats and settings are kept apart from
the program and survive both. The nightly build installs as a separate app, xd
Nightly, with its own settings.

The browser pane uses Microsoft's WebView2 Runtime, which Windows 11 includes. If it
is missing, the pane says so and links to Microsoft's installer.

If xd closes unexpectedly it says so, and saves a report to
`%LOCALAPPDATA%\xd\crash.log`.

</details>

## Getting started

1. **Install your assistants.** Install and sign in to Codex and/or Claude Code on
   the machine running the xd host, and keep their commands on `PATH`. Agent
   binaries and credentials are not included in xd.
2. **Add a project.** Press `+` beside **Projects** in the sidebar and choose a folder or repository.
3. **Start a session** in it and send a message. Open more chat tabs, the side
   pane, or the terminal as you go.
4. **Pair another device** (optional): open **Settings → Devices → Pair a device**
   on the host and copy the code. On the remote desktop or Android app, enter an
   address that reaches the host (LAN, Tailscale, or server; port 4001 by default)
   and paste the code. See [remote connections](docs/remote.md).

Installed Linux and macOS builds update themselves: an **Update** button appears
when a new xd, Codex, or Claude Code release is out, and nothing installs until you
confirm.

## Documentation

| Guide | What it covers |
| --- | --- |
| [Sessions and layout](docs/sessions.md) | Side pane and terminal, settling, auto-settle, and auto-delete |
| [The host](docs/host.md) | Assistant discovery, updates, resuming cut-off turns, memory limits |
| [Remote connections](docs/remote.md) | Pairing devices, switching hosts, shared sessions, device management |
| [Mobile development](docs/mobile.md) | The Android client and how it talks to the daemon |
| [Protocol](docs/protocol.md) | The client–daemon wire protocol |

## Build from source

```sh
./scripts/build.sh
./scripts/test.sh
```

Windows builds use `scripts/build-windows.ps1`, which packages the app as an MSI with
WiX 3 around a matching static Linux host from `scripts/build-windows-host.sh`. The
`windows` GitHub Actions workflow builds both parts, installs the MSI, drives the
installed window with real pointer input (`scripts/test-windows-ui.ps1`), and checks
a real Windows-to-WSL connection. `scripts/build-windows-icon.sh` regenerates the
Windows icon from the app's vector artwork.

## Acknowledgements

The CLI discovery and update behavior adapt portions of
[T3 Code](https://github.com/pingdotgg/t3code) for xd's Rust backend. Its MIT
notice is included in `data/licenses/t3code-LICENSE` and packaged builds.
