# xd desktop (GPUI)

This is the incremental Rust/GPUI replacement for xd's GTK desktop client.
The dev package runs its own Rust daemon using the documented JSON Lines
protocol; the Crystal desktop remains the production client until this path
reaches feature parity.

GPUI is pinned because it is pre-1.0. Only the published Apache-2.0 `gpui`
crate is used. Zed's GPL component library is not a dependency.

Current milestone:

- native GPUI application shell;
- an isolated, app-owned Rust daemon and first-run state initialization;
- clickable workspace/chat sidebar backed by real daemon tree data;
- workspace and chat creation controls;
- first-message generated/existing-worktree selection and Rust-owned checkout creation;
- interactive composer with Enter-to-send and daemon-authoritative queueing;
- global/workspace prompt shortcuts with send-or-queue buttons and inline management;
- daemon-persisted assistant, model, effort, access, and plan controls;
- automatic bounded Codex/Claude sign-in detection in the Rust daemon;
- cross-device text draft synchronization with a short local debounce;
- background PNG attachment loading with synchronized thumbnail previews;
- live assistant text, activity, turn state, and queue event handling;
- shared expandable activity cards for tools, subagents, and workflow runs;
- bounded Markdown rendering with language-aware fenced-code highlighting;
- variable-height virtualized transcript list;
- bounded full-duplex protocol framing and request-id matching;
- daemon snapshot/event state reducers with unit tests.

Build and test this crate through the repository Dockerfile:

```sh
docker build --target gpui-desktop-check .
```

Every push to `feat/gpui-desktop` replaces the rolling `dev` GitHub
prerelease with the tested Linux x86_64 prototype. This channel is deliberately
separate from the production `nightly` release while daemon connectivity and
feature parity are still in progress.

The archive carries `xd-daemon-dev`, a pinned Codex package, and the pinned
Claude Code executable. The Rust daemon
owns persisted workspaces, chats, messages, drafts, options, shortcuts, queue
mutations, and Codex/Claude turn execution. The GPUI dev build does not require an
installed Crystal daemon.

The app connects to `XD_SOCKET` when set. Otherwise it uses
`$XDG_DATA_HOME/xd-dev/daemon.sock` (normally
`~/.local/share/xd-dev/daemon.sock`), starts its sibling `xd-daemon-dev`, and
owns that process for the lifetime of the GPUI app. Its database and Workspaces
directory live beside that socket, separate from `xd` and `xd-nightly`.

This is still a development channel. Several production-client management
surfaces have not yet been ported.

Install it beside `xd` and `xd-nightly` as `xd-dev`:

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/download/dev/install-dev.sh | sh
```
