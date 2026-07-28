# xd

Workspace-organized AI conversations. A GTK4/libadwaita desktop app in C.

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

The app does not talk to any AI API itself. It drives the coding-agent CLIs
already installed and authenticated on your machine — `claude`, `codex`, and
`opencode` for Cerebras — as subprocesses, streaming their JSONL output into
the UI.

For Cerebras, install [OpenCode](https://opencode.ai/docs/) and either export
`CEREBRAS_API_KEY` before starting xd or run `opencode auth login`, choose
Cerebras, and enter your API key. xd never stores the key.

## Install

Linux, x86_64:

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install.sh | sh
```

That fetches the latest nightly, puts it in `~/.local/opt/xd-nightly`, adds the
command `xd-nightly` and an entry in the app menu. No root, no package manager,
and nothing compiled: the bundle carries its own GTK and everything under it, so
it runs anywhere with glibc.

Chats and workspaces live in `~/.local/share/xd-nightly`, which is the nightly's
own — it installs beside a release rather than over it, and neither edits the
other's work. `sh -s -- --uninstall` takes it away again and leaves that
directory alone.

To try a pull request or a branch, the nightly builds it. The button beside the
update button in the sidebar takes a pull request link, a branch link, a number
or a branch name; it fetches that code, builds the bundle the way the nightly is
built and installs the result over itself, then offers the restart. What it is
given is remembered, so trying the next commit on the same branch is opening it
and pressing the one button. Docker and git are what it needs, and Linux is
where it runs; the update button is the way back to master's nightly.

Windows, x86_64:

```powershell
irm https://github.com/RestartFU/xd/releases/download/nightly/install.ps1 | iex
```

That downloads the latest nightly MSI, verifies its SHA256 checksum, and opens
the Windows installer. Approve the Windows elevation prompt. The MSI carries
GTK and the rest of its runtime; MSYS2 is not required on the installed
machine. It can also be downloaded directly from the
[nightly release](https://github.com/RestartFU/xd/releases/tag/nightly).

This first Windows client supports local chats and connecting to a paired Linux
daemon. Embedded terminals and hosting the daemon on Windows are not available
yet. The terminal button is omitted instead of exposing a control that cannot
work.

macOS 14 or newer, Apple Silicon:

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/download/nightly/install-macos.sh | sh
```

macOS needs its own native Mach-O build; the Linux ELF/glibc bundle cannot run
there. The installer verifies the download and puts the self-contained app in
`~/Applications/xd-nightly.app`. This first macOS build supports local chats,
the embedded terminal, and connecting to a paired Linux daemon. Hosting the
daemon on macOS is not available yet.

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

The app itself is deliberately *not* run inside Docker: it spawns the host's
agent CLI, which needs the host's own credentials and PATH. The launcher
invokes the bundled loader with `--library-path` rather than exporting
`LD_LIBRARY_PATH`, so child processes still use host libraries.

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

| Path            | What lives there                                     |
| --------------- | ---------------------------------------------------- |
| `src/tree/`     | Workspace tree: nodes, disk scanner, sidebar          |
| `src/chat/`     | Chat view, message rows, subprocess session           |
| `src/backend/`  | Agent CLI argv building and JSONL parsing             |
| `src/settings/` | Per-folder `.xd.json` settings and inheritance        |
| `src/storage/`  | SQLite: chats, messages, full-text search             |
| `tests/`        | Headless tests, no GTK required                       |

Workspace folders are real directories (default `~/Workspaces`), so they can be
browsed, moved and synced with ordinary tools. Chat messages live in SQLite at
`~/.local/share/xd/chats.db`, keyed by a stable folder UUID so renaming or
moving a folder never breaks its chats.
