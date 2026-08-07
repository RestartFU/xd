# xd desktop (GPUI)

This is xd's Rust/GPUI desktop. It runs the Rust daemon using the documented
JSON Lines protocol and is the production Linux and macOS client.

GPUI is pinned because it is pre-1.0. Only the published Apache-2.0 `gpui`
crate is used. Zed's GPL component library is not a dependency.

Current milestone:

- native GPUI application shell;
- an app-owned Rust daemon and first-run state initialization;
- clickable workspace/chat sidebar backed by real daemon tree data;
- direct chat/workspace drag-and-drop with root drops and cycle-safe nesting;
- persisted active-chat restoration with deletion-safe fallback;
- persisted workspace expansion state with stale-folder cleanup;
- workspace and chat creation controls with per-chat daemon-side working-directory selection;
- daemon-backed folder browsing for local and paired existing repositories and workspace path defaults;
- first-message generated/existing-worktree selection, configured-writer AI naming, and Rust-owned checkout creation;
- interactive composer with Enter-to-send and daemon-authoritative queueing;
- bounded queued-message previews with send-now and remove controls;
- global/workspace prompt shortcuts with send-or-queue buttons and inline management;
- chat-scoped Claude slash-command discovery with filtered composer suggestions;
- daemon-persisted assistant, model, effort, access, and plan controls, with
  searchable provider views and locally persisted favorite models;
- Codex Fast mode backed by the priority service tier for new and resumed turns;
- daemon-owned, loopback-only Claude mode routing, account management, and GPUI control for Codex models;
- Rust-owned Codex/Claude account status, sign-in, cancellation, authorization-code input, and sign-out;
- bounded, asynchronous bundled Codex/Claude version inventory in Assistant Accounts;
- daemon-owned Whisper base.en download and bounded streaming transcription with live partial text;
- native GPUI microphone capture with cancel/stop controls and synchronized live dictation drafts;
- cross-device text draft synchronization with a short local debounce;
- background PNG attachment loading with synchronized thumbnail previews;
- lazy, sandboxed previews and a dismissible full-size viewer for persisted sent images;
- live assistant text, activity, turn state, and queue event handling;
- structured agent questions with option buttons and custom-answer input;
- opt-in local speech for completed `<speak>` sections (`espeak-ng`/`espeak`
  on Linux and the native `say` synthesizer on macOS);
- best-effort Discord Rich Presence with private, fixed conversation-state labels;
- shared Rust-owned PTY terminal with bounded replay and GPUI input controls;
- simultaneously visible repository and terminal panes with draggable, persisted dividers;
- workdir-relative directory browsing and conflict-guarded UTF-8 file editing for Git and non-Git chats;
- shared expandable activity cards for tools, subagents, and live workflow runs
  with non-blocking, last-good status refresh;
- bounded Markdown rendering with language-aware fenced-code highlighting;
- variable-height virtualized transcript list with automatic, cursor-stable
  bidirectional paging and a bounded retained message window;
- bounded full-duplex protocol framing and request-id matching;
- off-thread daemon startup with bounded automatic reconnection and manual retry;
- mobile-compatible daemon update state and restart controls;
- certificate-pinned TLS pairing backed by private remote-session IPC and revocation;
- daemon snapshot/event state reducers with unit tests;
- nightly-only source builds for validated GitHub branches, pull requests, and
  commits on Linux and macOS, with bounded output and process-group cancellation.

Build and test this crate through the repository Dockerfile:

```sh
docker build --target gpui-desktop-check .
```

Every push to `master` replaces the rolling Linux and macOS nightly. Tagged
releases use the stable application id and install beside the nightly.

The archive carries `xd-daemon`, a pinned Codex package, the pinned
Claude Code executable, and a private pinned whisper.cpp runtime. The Rust daemon
owns persisted workspaces, chats, messages, drafts, options, shortcuts, queue
mutations, and Codex/Claude turn execution.

The app connects to `XD_SOCKET` when set. Otherwise it uses
`$XDG_DATA_HOME/xd/daemon.sock` by default, starts its bundled `xd-daemon`, and
owns that process for the lifetime of the app. Nightly bundles set the data
name to `xd-nightly` in their launcher.

The bundled Rust daemon also supports the established headless pairing flow:

```sh
xd-daemon serve --socket /path/to/daemon.sock --pair
```

To run the daemon against a specific data directory, stop the previous daemon
first and adopt all of its state paths as one unit:

```sh
xd-daemon serve --data ~/.local/share/xd-nightly
```

This keeps the existing database, managed workspace paths, and compatible
secret files in place. Existing `server-cert.pem` and `server-key.pem` files are
also reused so paired clients keep the same pinned daemon identity. `--data`
cannot be combined with individual socket, database, or workspace overrides,
and the Rust daemon refuses to open the database while another daemon is
listening on that data root's socket.

Selective spoken replies are disabled by default. Linux uses `espeak-ng` (or
the older `espeak`) when installed; macOS uses its built-in speech synthesizer.
No text is sent to a speech service.

Stable and nightly install commands for both platforms are in the repository's
main [README](../README.md#install).
