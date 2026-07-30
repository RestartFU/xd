# Crystal rewrite parity ledger

The C application is the specification. Existing behavior is not redesigned
during the Crystal port. New Codex/Claude authentication, daemon status, and
pairing UI may add controls, but those controls must use the same visual
language.

Status:

- `[x]` implemented and verified
- `[~]` partially implemented; not parity-complete
- `[ ]` missing

An item only becomes `[x]` after its behavior is tested and its GTK result is
checked against the C application where visual behavior is involved.

## Window and application shell

- `[x]` Crystal GTK4/libadwaita application starts from the relocatable bundle.
- `[x]` Default window geometry is 1100x720.
- `[x]` Dark style, DM Sans, icons, MIME data, and minimal-host launch work.
- `[~]` Root horizontal `GtkPaned` has the C sidebar/chat child order and
  280-pixel initial position.
- `[~]` Window size, maximized state, and sidebar position are persisted;
  restart verification remains.
- `[x]` Restore the C overlay divider spanning the full header width.
- `[x]` Sidebar and chat headers share one vertical `GtkSizeGroup`.
- `[ ]` Restore active local/remote chat at startup.
- `[ ]` Clear text selection when clicking outside selectable message text.
- `[~]` Search exists, but exact window actions and Ctrl+K/Ctrl+F behavior need
  verification.

C sources: `src/xd-window.c`, `src/xd-app.c`.

## Sidebar and workspace tree

- `[x]` Local and one paired remote root share one sidebar.
- `[x]` Paired remote retains cached rows while offline and reconnects.
- `[~]` Folder/chat create, rename, move, delete, context, and settings actions
  route through one daemon endpoint interface.
- `[ ]` Match the C `GtkListView` tree rows, indentation, expanders, selection,
  activation, inline editing, drag/drop, and state dots exactly.
- `[ ]` Match local and remote root menus, confirmations, disabled states, and
  error presentation exactly.
- `[ ]` Match chat working/waiting/done/offline state transitions.
- `[ ]` Restore updater row and branch-build entry point.

C sources: `src/tree/sidebar.c`, `src/tree/fs-tree.c`,
`src/tree/xd-node.c`, `src/remote/remote-tree.c`, `src/ui/updater.c`.

## Chat transcript

- `[~]` Local and remote chats use one active endpoint path.
- `[~]` Stored messages, streamed text, tool rows, images, ask blocks,
  subagent/workflow/workspace cards, and inline diffs render.
- `[ ]` Port the exact `XdMessageRow` hierarchy, typography, selectable text,
  Markdown, syntax highlighting, code blocks, and copy controls.
- `[ ]` Port transcript pagination, four-page cache, and 100-message page size.
- `[ ]` Match bottom pinning, history loading, scroll restoration, and hidden
  scrollbar behavior.
- `[ ]` Match working dots, elapsed time, live reveal cadence, and turn labels.
- `[ ]` Match local/daemon/remote turn recovery when switching chats.
- `[ ]` Match context-window meter and token formatting.
- `[ ]` Match handover, retry, cancellation, and queued-turn timeline behavior.

C sources: `src/chat/chat-view.c`, `src/chat/message-row.c`,
`src/util/markdown.c`, `src/util/syntax.c`, `src/ui/dots.c`,
`src/chat/handover.c`.

## Composer and run controls

- `[x]` Composer and transcript share a 1040-pixel `AdwClamp`.
- `[~]` Multiline entry, Enter-to-send, Shift+Enter, cancel button state,
  attachment button, model, effort, access, build, and plan controls exist.
- `[~]` Image paste/chooser limits and previews exist.
- `[~]` Ask answers and queued messages exist.
- `[ ]` Match exact control order, labels, tooltips, picker popovers, spacing,
  sensitivity, and active states.
- `[ ]` Port queued-message drop/edit/steer controls.
- `[ ]` Port slash-command discovery and command bar.
- `[ ]` Port worktree chooser and new-worktree behavior exactly.
- `[ ]` Port optional voice recording/download/transcription behavior.
- `[ ]` Match context/branch/worktree line below composer.

C sources: `src/chat/chat-view.c`, `src/chat/model-picker.c`,
`src/chat/option-picker.c`, `src/chat/voice-input.c`.

## Terminal, files, and diff panes

- `[~]` Terminal is the end child of a vertical `GtkPaned` below the
  conversation/composer, with 260-pixel default and persisted height; per-chat
  visibility remains.
- `[~]` Files and diff share one stack at the right of the conversation plus
  terminal, with 420-pixel default and persisted width; exact pane content
  remains.
- `[x]` Header controls are toggle buttons whose active states match pane
  visibility.
- `[~]` Terminal RPC, PTY output, input, resize, replay, and kill operations use
  the shared daemon.
- `[ ]` Port terminal multi-session tabs, centered title, add/kill controls,
  close request, focus behavior, replay, and per-chat session retention.
- `[~]` File list/read/write RPC works, but C pane layout and behavior are not
  yet matched.
- `[~]` Working/branch diff RPC works, but C diff list, expansion, syntax,
  actions, and layout are not yet matched.
- `[ ]` Persist pane visibility per local/remote chat without duplicating
  daemon logic.
- `[ ]` Refresh repository panes after agent turns and terminal activity.

C sources: `src/chat/terminal-panel.c`, `src/chat/file-pane.c`,
`src/chat/diff-pane.c`, `src/chat/diff-view.c`,
`src/chat/diff-text.c`, `src/chat/git-actions.c`,
`src/settings/pane-state.c`.

## Dialogs and auxiliary UI

- `[x]` Pairing panel matches the C panel structure and routes into the durable
  remote connection.
- `[~]` Workspace create/rename/delete/settings/context and agent-secret
  dialogs exist.
- `[~]` Search dialog exists.
- `[ ]` Replace provisional GTK dialogs with the exact Adwaita dialog types,
  response appearance, focus, shortcuts, validation, and copy.
- `[ ]` Port directory browser.
- `[ ]` Port branch-build dialog.
- `[ ]` Match toasts and startup/database error presentation.

C sources: `src/remote/pair-dialog.c`, `src/settings/*-dialog.c`,
`src/chat/search-dialog.c`, `src/ui/dir-browser.c`,
`src/ui/branch-build-dialog.c`.

## Daemon, storage, and remote behavior

- `[x]` Local Unix IPC and remote TLS dispatch through one `Daemon::Engine`.
- `[x]` Remote pairing uses a short-lived code, token, and pinned certificate.
- `[x]` Stored remote reconnects while retaining endpoint subscribers.
- `[x]` Folder, chat, settings, message, send/cancel, file, diff, image, search,
  and terminal operations share the same protocol.
- `[~]` Crystal storage and agent behavior have broad specs, but every old C
  test/edge case still needs a mapped Crystal assertion.
- `[ ]` Restore active chat and per-device pane state without introducing a
  second local implementation path.

C sources: `src/storage`, `src/remote`, `src/chat/chat-session.c`,
`tests/*.c`.

## Agents and bundled CLIs

- `[x]` Pinned official Codex and Claude native binaries ship in the Linux
  bundle.
- `[x]` Agent execution resolves bundled binaries before host binaries.
- `[~]` Codex app-server and Claude stream-json turns work through the shared
  manager.
- `[ ]` Add Codex login/logout/status UI and verify bundled authentication.
- `[ ]` Add Claude login/logout/status UI and verify bundled authentication.
- `[ ]` Match all existing model, effort, access, plan/build, resume, and
  cancellation behavior.
- `[ ]` Remove Cerebras/OpenCode code, settings, icons, docs, and tests.

C sources: `src/backend`, Crystal sources: `src/xd/agent`.

## Packaging, developer workflow, and cleanup

- `[x]` Linux development, tests, and bundle builds require Docker only.
- `[x]` One-line branch installer builds the latest branch commit.
- `[x]` Linux bundle carries GTK, libadwaita, VTE, GL, fonts, icons, MIME, TLS,
  OpenSSL, Codex, and Claude.
- `[~]` macOS and Windows bundle scripts exist but need end-to-end verification.
- `[ ]` Replace stale Meson/C instructions and CI with Crystal Docker workflow.
- `[ ]` Remove Odin experiment and old C implementation only after every C
  behavior above has a verified Crystal replacement.
- `[ ]` Run clean-host installer, local daemon, paired daemon, reconnect,
  terminal, Codex, Claude, and screenshot verification.

## Required release evidence

- `[ ]` Crystal specs pass in Docker.
- `[ ]` Release bundle builds from a clean checkout in Docker.
- `[ ]` Bundle launches with isolated `HOME`, `XDG_DATA_HOME`, and
  `XDG_DATA_DIRS=/nonexistent`.
- `[ ]` 1100x720 screenshots match the C app for empty, populated, active turn,
  question, terminal, files, diff, search, settings, secrets, and pairing
  states.
- `[ ]` Local and paired clients pass the same protocol behavior suite.
- `[ ]` Codex and Claude authentication and one real turn each succeed from the
  shipped bundle.
