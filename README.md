# xd

A local-first desktop workspace for user-installed Codex, Claude Code, JCode, and GitHub Copilot CLI.

![xd sessions](docs/assets/xd-sessions.png)

<table>
  <tr>
    <td><img src="docs/assets/xd-projects.png" alt="Projects and sessions"></td>
    <td><img src="docs/assets/xd-terminal.png" alt="Persistent terminal"></td>
  </tr>
</table>


## Highlights

- Native chat for Codex, Claude Code, and JCode, with streamed replies, activity cards, queues, and optional shell panes.
- Live subagent cards: each delegated agent shows its model, current step, tools, tokens, and elapsed time, and keeps its task, steps, and report once it finishes. Claude agents can be stopped or sent to the background from the card.
- Tool approvals and questions inline in the chat, with the command or diff they would apply, on desktop and Android.
- One persistent daemon owns local and remote sessions; work continues when clients disconnect.
- Direct remote connections over pinned TLS, with expiring pairing codes and revocable devices.
- Projects, Git worktrees, branches, and pull requests in one workspace.
- An integrated browser with separate tabs for each chat and shared site data.
- Uses your installed CLI tools, authentication, and configuration.
- Dark and light themes, chosen in Settings.

Each chat remembers its browser tabs and whether the pane is open. Use the
Browser button to show it, then `+` in its tab bar to open another tab.

## Install

Linux x86_64:

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/latest/download/install.sh | sh -s -- --release
```

macOS:

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/latest/download/install-macos.sh | sh -s -- --release
```

Windows x86_64: download `xd-windows-x86_64.zip` from the
[latest release](https://github.com/RestartFU/xd/releases/latest), extract the
whole archive, and run `xd.exe`. The desktop and browser run natively;
local projects and persistent sessions run in your default WSL distribution.
Install git, tmux, CA certificates, and your assistant CLIs
inside WSL first. The included README has setup instructions and a WebView2
Runtime installer helper. Set `XD_WSL_DISTRIBUTION` to select another distro.

Nightly desktop builds are available from the [nightly release](https://github.com/RestartFU/xd/releases/tag/nightly). The Android APK is included when signing credentials are configured.
The Linux and macOS installers need no root access. Install the assistants you use separately and keep their commands on `PATH`.

## Build

```sh
./scripts/build.sh
./scripts/test.sh
```

See [mobile development](docs/mobile.md) and [remote usage](docs/remote.md) for details.

Windows builds use `scripts/build-windows.ps1` with a matching static Linux host
from `scripts/build-windows-host.sh`. The `windows` GitHub Actions workflow
builds and checks both parts, including a real Windows-to-WSL connection.
