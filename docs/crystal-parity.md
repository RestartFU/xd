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
- `[~]` Active local/remote chat uses the C `local:`/`remote:` setting, restores
  its ancestor branch, and clears on deletion/removal. Local installed-bundle
  restart is UI-verified with fatal GTK criticals; paired reconnect restore
  remains.
- `[~]` C capture-phase outside-click clearing is ported for selectable message
  text; clipboard/selection interaction verification remains.
- `[x]` Search is a real window action and Ctrl+K/Ctrl+F both open it from the
  installed bundle with fatal GTK criticals enabled.

C sources: `src/xd-window.c`, `src/xd-app.c`.

## Sidebar and workspace tree

- `[x]` Local and one paired remote root share one sidebar.
- `[x]` Paired remote retains cached rows while offline and reconnects.
- `[~]` Folder/chat create, rename, move, delete, context, and settings actions
  route through one daemon endpoint interface.
- `[~]` C `GtkTreeListModel`/`GtkListView` rows, indentation, expanders,
  selection, activation, expansion persistence, and state dots are ported and
  populated-tree startup is screenshot-verified. Inline create/rename is
  gesture-tested for Enter, Escape, and click-away with fatal GTK criticals
  enabled. Folder drag/drop into a row and back to empty top-level space is
  filesystem- and screenshot-verified under the same fatal-critical check.
- `[ ]` Match local and remote root menus, confirmations, disabled states, and
  error presentation exactly.
- `[~]` Chat working/waiting/done and remote-root offline transitions match C
  state rules, including animated dots and tooltips; screenshot/event matrix
  remains.
- `[~]` Updater row, release-channel checks/install/restart, and branch-build
  entry point are ported. Nightly installed-path row and branch panel are
  screenshot-verified with fatal GTK criticals; real update/install remains.

C sources: `src/tree/sidebar.c`, `src/tree/fs-tree.c`,
`src/tree/xd-node.c`, `src/remote/remote-tree.c`, `src/ui/updater.c`.

## Chat transcript

- `[~]` Local and remote chats use one active endpoint path.
- `[~]` Stored messages, streamed text, tool rows, images, ask blocks,
  subagent/workflow/workspace cards, and inline diffs render.
- `[~]` Port the exact `XdMessageRow` hierarchy, typography, selectable text,
  Markdown, syntax highlighting, code blocks, and copy controls. CommonMark,
  safe links, streaming fragments, lists, tables, and Pango validation now
  match all 18 C Markdown test cases. Basic `AdwBin`/card/body hierarchy,
  user-only bubbles, literal streaming, source tooltips, and host link opening
  are integrated. Fenced code, unfinished fences, diff fences, and tables use
  the C card split and geometry. Populated installed-bundle rows are
  screenshot-verified at 1100x720 with fatal GTK criticals. Exact 14-language
  path detection, stateful token scanning, token palette, unified-diff parser,
  independent old/new lexer state, gutters, metadata, row limits, and slice
  recovery are ported. Inline diff cards are installed-bundle
  screenshot-verified at 1100x720 with fatal GTK criticals: full-width
  contiguous row backgrounds, syntax colours, horizontal scrolling, and C
  padding/geometry match. Image cards remain.
- `[~]` Transcript requests use the C 100-message page size plus one boundary
  row; per-chat expanded limits and four-entry LRU widget-tree cache are
  ported. A 245-row installed-bundle transcript verifies the 100/45 history
  pills; five-chat GTK eviction verification remains.
- `[~]` Hidden scrollbar, bottom pinning, user-scroll opt-out, history loading,
  and frame-stable scroll restoration are ported. Installed-bundle proof at
  1100x720 keeps marker 146 at the same y-position while inserting 100 older
  rows, with fatal GTK criticals enabled and empty stderr. Live streaming
  stays bottom-pinned through partial and settled frames from an external
  daemon turn; live wheel opt-out verification remains.
- `[~]` One C-shaped working row stays last, uses the same animated dots,
  second/minute/hour elapsed labels, and pauses both timers when bottom follow
  is disabled. Stored duration rows move above their turn exactly as in C.
  The 33-millisecond reveal, 80-millisecond initial delay, two-character live
  tail, 100-millisecond settle, and UTF-8 prefixes have deterministic specs.
  An installed nightly bundle shows partial and settled live frames, correct
  Stop state, and zero fatal GTK criticals; off-bottom pause and source-tooltip
  interaction verification remain.
- `[~]` Daemon active-turn snapshots carry label, elapsed time, completed
  text/tool items, and the safe current segment over the shared endpoint.
  Installed local-daemon proof switches away and back during a turn, restores
  the initiating user row and live output, then appends a later delta once and
  in order. Paired TLS UI recovery verification remains.
- `[~]` Context-window meter uses the C 108-pixel progress bar, compact token
  formatting, raw-count tooltip, and 75/90-percent warning states; installed
  GTK verification remains.
- `[ ]` Match handover, retry, cancellation, and queued-turn timeline behavior.

C sources: `src/chat/chat-view.c`, `src/chat/message-row.c`,
`src/util/markdown.c`, `src/util/syntax.c`, `src/ui/dots.c`,
`src/chat/handover.c`.

## Composer and run controls

- `[x]` Composer and transcript share a 1040-pixel `AdwClamp`.
- `[~]` Multiline entry, Enter-to-send, Shift+Enter, cancel button state,
  attachment button, model, effort, access, build, and plan controls exist.
- `[~]` Image paste/chooser limits and previews exist.
- `[~]` Ask questions stay bold in the transcript while answer controls use the
  exact C composer slot, flow layout, input row, and retirement lifecycle;
  installed GTK verification remains.
- `[~]` Control order, labels, tooltips, spacing, sensitivity, and active
  states match C. The combined assistant/model picker includes its icon,
  provider rail, persisted favorites, search, empty state, row shortcuts, and
  atomic daemon selection. An installed nightly bundle verifies all four
  popovers, favorite persistence, provider switching, Ctrl+2 selection, and
  Plan disabling access with fatal GTK criticals enabled.
- `[~]` Queued-message drop/edit/steer controls and multiline editor match C;
  installed GTK verification remains.
- `[~]` Slash-command discovery, filtering, layout, and insertion match C;
  installed GTK verification remains.
- `[~]` Worktree chooser uses the C descriptive popover, current/new/existing
  checkout rows, detached labels, locking rule, and daemon-owned selection.
  Effort and access use the same picker widget; the current/new checkout,
  effort, and access popovers are installed-bundle screenshot-verified.
- `[ ]` Port optional voice recording/download/transcription behavior.
- `[~]` Context/branch/worktree line below composer uses the C copy, ellipsis,
  tooltip, and geometry. The daemon computes it once for Unix and TLS clients;
  installed GTK geometry is verified; live branch-change verification remains.

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
- `[~]` Daemon-backed multi-session tabs, centered single-session title,
  add/kill controls, close request, focus, replay, resize, and per-chat view
  retention work. C palette/font, copy/paste, URL handling, and daemon-terminal
  Backspace behavior are ported. Offline status, reconnect reconciliation, and
  pending-kill retry are implemented; paired reconnect UI verification remains.
- `[~]` File list/read/write RPC works, but C pane layout and behavior are not
  yet matched.
- `[~]` Working/branch diff RPC works, but C diff list, expansion, syntax,
  actions, and layout are not yet matched.
- `[~]` Pane visibility persists per local/remote chat in the same typed
  `a{su}` device map; restart/UI verification remains.
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
- `[x]` Search dialog matches the C Adwaita widget tree, empty/result states,
  sizing, focus, and row activation; installed-bundle UI matrix passes with
  fatal GTK criticals.
- `[ ]` Replace provisional GTK dialogs with the exact Adwaita dialog types,
  response appearance, focus, shortcuts, validation, and copy.
- `[ ]` Port directory browser.
- `[~]` Branch/PR parsing, shell-safe fetch/build/install command, exact panel,
  bounded live output, stop, persistence, and dialog-independent run lifecycle
  match C. Panel is screenshot-verified from an installed-path nightly bundle
  with zero GTK stderr; real branch installation remains.
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
- `[x]` Checkout context and worktree identity are daemon-owned protocol state;
  local and paired clients do not probe Git through separate UI logic.
- `[~]` Crystal storage and agent behavior have broad specs, but every old C
  test/edge case still needs a mapped Crystal assertion.
- `[~]` Active chat and per-device pane state restore through the same endpoint
  path. Local active-chat restart is UI-verified; paired reconnect and pane
  state restart matrices remain.

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
- `[x]` Cerebras/OpenCode code, registration, assets, fixtures, and tests are
  removed; Claude Code and Codex are the only backends.

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

- `[x]` Crystal specs pass in Docker (212 examples).
- `[ ]` Release bundle builds from a clean checkout in Docker.
- `[x]` Bundle launches with isolated `HOME`, `XDG_DATA_HOME`, and
  `XDG_DATA_DIRS=/nonexistent`.
- `[ ]` 1100x720 screenshots match the C app for empty, populated, active turn,
  question, terminal, files, diff, search, settings, secrets, and pairing
  states.
- `[ ]` Local and paired clients pass the same protocol behavior suite.
- `[ ]` Codex and Claude authentication and one real turn each succeed from the
  shipped bundle.
