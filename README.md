# xd

Workspace-organized AI conversations. A plain GTK4 desktop app being rewritten
in Crystal around one daemon protocol for both local and paired clients.

Chats do not live in a flat list; they live in a tree of workspaces and folders,
and each chat inherits its parent chain's context — backend, model, working
directory, repository and project instructions.

```
Lunar
├── Proxy
│   ├── Implement rate limiting
│   └── Fix websocket reconnect
└── Dashboard
    └── UI Rewrite
Personal
└── Dotfiles
```

The app does not talk to AI APIs itself. Its bundle ships pinned native Codex
and Claude Code CLIs plus Git, runs them as subprocesses, and uses their normal
authentication/config directories.

## Test the Crystal rewrite

Linux x86_64, with Docker running:

```sh
curl -fsSL https://raw.githubusercontent.com/RestartFU/xd/refs/heads/rewrite/crystal-unified-daemon/scripts/install-branch.sh | sh
```

That resolves the branch's latest commit, builds its self-contained bundle
inside Docker, and installs it as `xd-nightly`. No host Crystal compiler, GTK
SDK, Codex, or Claude installation is needed. Existing nightly chats and
workspaces are preserved. Quit a running `xd-nightly` before updating; the
installer refuses to replace an active bundle so GNOME cannot reopen stale
code.

## Install

Linux, x86_64:

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install.sh | sh
```

That fetches the latest nightly, puts it in `~/.local/opt/xd-nightly`, adds the
command `xd-nightly` and an entry in the app menu. No root, no package manager,
and nothing compiled: the bundle carries its own GTK, Git, and everything under
them, so it runs anywhere with glibc.

Chats and workspaces live in `~/.local/share/xd-nightly`, which is the nightly's
own — it installs beside a release rather than over it, and neither edits the
other's work. `sh -s -- --uninstall` takes it away again and leaves that
directory alone.

To try a pull request or a branch, the nightly builds it. The button beside the
update button in the sidebar takes a pull request link, a branch link, a number
or a branch name; it fetches that code, builds the bundle the way the nightly is
built and installs the result over itself, then offers the restart. What it is
given is remembered, so trying the next commit on the same branch is opening it
and pressing the one button. Docker is what it needs; Git comes with xd. Linux
is where it runs, and the update button is the way back to master's nightly.

Crystal builds currently target Linux x86_64. Windows and macOS installers are
paused until they build this same Crystal client and daemon; old C artifacts
are not published under the new version.

## Build

Docker is the only requirement. Nothing is installed on the host.

```sh
./scripts/build.sh     # -> ./dist, a self-contained bundle
./scripts/test.sh      # headless test suite
```

`build.sh` produces a relocatable directory containing the binary, its whole
library closure (including the dynamic loader), GTK's support data and a
launcher. It runs on any glibc x86_64 host, including distributions with no
system GTK such as NixOS.

## Run

```sh
./dist/xd.sh
```

The app itself is deliberately *not* run inside Docker. Its daemon starts the
bundled Codex, Claude, and Git CLIs and uses credentials stored on the daemon
host. For a paired chat, authentication, CLI updates, repository operations,
and speech-model downloads all happen on that remote host. The launcher invokes
the bundled loader with `--library-path` rather than exporting
`LD_LIBRARY_PATH`, so ordinary child processes still use host libraries.

### Known wart

On non-Debian hosts, fontconfig prints parse warnings on startup for files under
`/usr/share/fontconfig/conf.avail`: that path is compiled into the bundled
library and the host's copy is a newer format. Fonts resolve correctly; the
noise is cosmetic.

## What it does

- Workspaces and folders are real directories under `~/Workspaces`, nested to
  any depth. Each carries a `.xd.json` with a UUID, so a folder can be renamed
  or moved without its chats losing track of it.
- A folder *refers* to a repository rather than containing one. Working
  directory, repository, backend, model and project instructions are set per
  folder and inherited by everything below; instructions accumulate from the
  root down, everything else is overridden by the nearest folder that sets it.
- New chats pick their own working directory and can stay in that checkout,
  reuse any worktree already registered with its repository, or create an
  isolated, request-named worktree under
  `../worktrees/<repository>/<worktree-name>/<repository>/` before the first
  message. Branches use the readable name plus a short stable suffix.
- The composer shows which assistant will answer and which branch, worktree and
  remote it is looking at.
- Replies stream in and are rendered as Markdown. Stopping sends SIGINT first,
  so the CLI's own session survives and the chat can still be resumed.
- `Ctrl+K` searches every message.

## Layout

| Path               | What lives there                                      |
| ------------------ | ----------------------------------------------------- |
| `src/xd/daemon/`   | Shared Engine, local/TLS transports, filesystem, PTY   |
| `src/xd/agent/`    | Bundled CLI lifecycle, protocols, auth, turn handling |
| `src/xd/ui/`       | GTK4/libadwaita client                                |
| `src/xd/storage/`  | SQLite chats, messages, sessions, workflow state      |
| `src/xd/workspace/` | Workspace tree, inherited settings, worktrees        |
| `spec/`            | Crystal behavior and protocol specs                   |

Legacy C source remains temporarily as parity reference. Docker, CI, bundles,
and installers build only `src/xd.cr`.

Workspace folders are real directories (default `~/Workspaces`), so they can be
browsed, moved and synced with ordinary tools. Chat messages live in SQLite at
`~/.local/share/xd/chats.db`, keyed by a stable folder UUID so renaming or
moving a folder never breaks its chats.
