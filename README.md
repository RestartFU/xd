# hy

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

The app does not talk to any AI API itself. It drives the CLIs already
installed and authenticated on your machine — `claude` and `codex` — as
subprocesses, streaming their JSONL output into the UI.

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
./dist/hy.sh
```

The app itself is deliberately *not* run inside Docker: it spawns the host's
`claude` and `codex`, which need the host's own credentials and PATH. The
launcher invokes the bundled loader with `--library-path` rather than exporting
`LD_LIBRARY_PATH`, so those child processes still use host libraries.

### Known wart

On non-Debian hosts, fontconfig prints parse warnings on startup for files under
`/usr/share/fontconfig/conf.avail`: that path is compiled into the bundled
library and the host's copy is a newer format. Fonts resolve correctly; the
noise is cosmetic.

## What it does

- Workspaces and folders are real directories under `~/Workspaces`, nested to
  any depth. Each carries a `.hy.json` with a UUID, so a folder can be renamed
  or moved without its chats losing track of it.
- A folder *refers* to a repository rather than containing one. Working
  directory, repository, backend, model and project instructions are set per
  folder and inherited by everything below; instructions accumulate from the
  root down, everything else is overridden by the nearest folder that sets it.
- New chats pick their own working directory, so two chats in the same folder
  can point at different checkouts.
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
| `src/backend/`  | `claude` / `codex` argv building and JSONL parsing    |
| `src/settings/` | Per-folder `.hy.json` settings and inheritance        |
| `src/storage/`  | SQLite: chats, messages, full-text search             |
| `tests/`        | Headless tests, no GTK required                       |

Workspace folders are real directories (default `~/Workspaces`), so they can be
browsed, moved and synced with ordinary tools. Chat messages live in SQLite at
`~/.local/share/hy/chats.db`, keyed by a stable folder UUID so renaming or
moving a folder never breaks its chats.
