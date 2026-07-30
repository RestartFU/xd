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
  Clean and restored non-Git profiles stay alive under fatal GTK warnings;
  scheduler-backed background workers cover restored Markdown rendering
  without raw-thread startup crashes.
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
  route through one daemon endpoint interface. Tree loads and every mutation
  leave GTK immediately; generation checks discard stale reloads after a newer
  local/remote state request.
- `[~]` C `GtkTreeListModel`/`GtkListView` rows, indentation, expanders,
  selection, activation, expansion persistence, and state dots are ported and
  populated-tree startup is screenshot-verified. Inline create/rename is
  gesture-tested for Enter, Escape, and click-away with fatal GTK criticals
  enabled. Folder drag/drop into a row and back to empty top-level space is
  filesystem- and screenshot-verified under the same fatal-critical check.
- `[x]` Local, remote-root, folder, and chat menus use the C `GMenu`/
  `GtkPopoverMenu` sections, labels, and action order. Folder/chat mutation
  failures use the same operation-specific `AdwAlertDialog` headings. The
  exact `09dc473` bundle is screenshot-verified with fatal GTK warnings:
  header create, folder create/rename, chat create/delete, trash confirmation,
  and duplicate-name error all survive menu close ordering. The exact
  `d01d497` bundle verifies the remote-root menu, TLS workspace creation,
  offline mutation error, and remove confirmation.
- `[~]` Chat working/waiting/done and remote-root offline transitions match C
  state rules, including animated dots and tooltips. A paired `d01d497`
  client keeps its cached child when the daemon stops, draws the root offline,
  then reconnects without pairing again and returns the root to idle. Chat
  state screenshot/event matrix remains.
- `[~]` Updater row, release-channel checks/install/restart, and branch-build
  entry point are ported. Nightly installed-path row and branch panel are
  screenshot-verified with fatal GTK criticals; real update/install remains.

C sources: `src/tree/sidebar.c`, `src/tree/fs-tree.c`,
`src/tree/xd-node.c`, `src/remote/remote-tree.c`, `src/ui/updater.c`.

## Chat transcript

- `[~]` Local and remote chats use one active endpoint path.
- `[x]` Chat open/state/history/search/send/cancel/model/options/queue calls
  leave GTK callbacks immediately and finish through main-loop idles. Request
  generations discard stale remote replies after chat or endpoint switches,
  and one pending Send cannot duplicate a turn while network latency is high.
  Request ids multiplex replies, and daemon control operations bypass slow
  repository commands, so Stop is not queued behind a diff or file read.
- `[~]` Stored messages, streamed text, tool rows, images, ask blocks,
  subagent/workflow/workspace cards, and inline diffs render. Contiguous tool
  activity immediately before a completed subagent now moves behind the same
  collapsed arrow toggle as C instead of remaining as a detached transcript
  row. A restored non-Git bundle profile containing generic tools, an inline
  diff, and a subagent card survives fatal GTK warnings; the broader paired
  card matrix remains.
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
  padding/geometry match. Image markers now retain exact prose/image order;
  daemon-backed loading, unavailable states, thumbnail geometry, linked
  captions, and the transparent Adwaita viewer use the C widget hierarchy and
  are installed-bundle verified.
- `[~]` Transcript requests use the C 100-message page size plus one boundary
  row; per-chat expanded limits and four-entry LRU widget-tree cache are
  ported. History rows materialize in four-message GTK idle batches, preserving
  order while input and redraws run between batches; stale chat switches stop
  the remaining work. Remote history scroll restoration now starts only after
  the final batch instead of racing the daemon reply. A 245-row
  installed-bundle transcript verifies the 100/45 history pills; five-chat GTK
  eviction verification remains.
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
- `[x]` Handover keeps the C 12,000-byte role-filtered boundary. The unified
  manager retries a silent stale resumed session exactly once without
  duplicating the user row, stores the C `(no reply)` fallback, advances
  backend `last_seen` only after success, and runs Stop/Steer through the same
  durable queue finish path. Stop requests arriving while authorization,
  worktree preparation, or other turn startup work is still running are
  retained until the session handle exists instead of leaving the UI stuck in
  `Stopping…`; backend interrupt failures preserve active-turn ownership so
  Stop can be retried instead of creating durable ghost-working state, and
  shutdown remains best-effort when a child still rejects interruption. The
  bundle from `21008c6` is verified at
  1100x720 with fatal GTK criticals: queue edit, Steer, Drop, Stop, retry,
  partial-output preservation, duration placement, and `(no reply)` all match
  storage and draw once. Event-driven metadata refresh no longer replays a
  queued turn snapshot over its pending ordered text delta.

C sources: `src/chat/chat-view.c`, `src/chat/message-row.c`,
`src/util/markdown.c`, `src/util/syntax.c`, `src/ui/dots.c`,
`src/chat/handover.c`.

## Composer and run controls

- `[x]` Composer and transcript share a 1040-pixel `AdwClamp`.
- `[~]` Multiline entry, Enter-to-send, Shift+Enter, cancel button state,
  attachment button, model, effort, access, build, and plan controls exist.
  Send/Stop stays icon-only and circular with the C icons, tooltips, colours,
  and empty-Enter queue steering behavior.
- `[~]` Image paste/chooser limits and previews use the C card hierarchy,
  scaled thumbnail pixels, caption, remove overlay, and margins. Both Unix and
  TLS clients send the same byte payload through the daemon. An installed
  nightly bundle verifies mixed prose/image ordering, image-only rows,
  thumbnail sizing, the transparent Adwaita viewer, and dialog close with
  fatal GTK criticals enabled. The rebuilt bundle also verifies chooser
  selection, the card/caption/remove-overlay geometry, and removal lifecycle
  after the generic `GListModel` wrapper fix. File decoding, scaling, and PNG
  encoding run on the bounded scheduler-backed worker pool, keeping chooser
  completion off GTK and avoiding unsafe raw-thread callbacks.
- `[~]` Ask questions stay bold in the transcript while answer controls use the
  exact C composer slot, flow layout, input row, and retirement lifecycle;
  installed GTK verification remains.
- `[~]` Control order, labels, tooltips, spacing, sensitivity, and active
  states match C. The combined assistant/model picker includes its icon,
  provider rail, persisted favorites, search, empty state, row shortcuts, and
  atomic daemon selection. An installed nightly bundle verifies all four
  popovers, favorite persistence, provider switching, Ctrl+2 selection, and
  Plan disabling access with fatal GTK criticals enabled.
- `[x]` Queued-message drop/edit/steer controls and multiline editor match C.
  Installed GTK verification covers persistence, promotion, cancellation,
  partial-output retention, next-turn placement, and empty queue retirement.
- `[~]` Slash-command discovery, filtering, layout, and insertion match C;
  installed GTK verification remains.
- `[~]` Worktree chooser uses the C descriptive popover, current/new/existing
  checkout rows, detached labels, locking rule, and daemon-owned selection.
  Effort and access use the same picker widget; the current/new checkout,
  effort, and access popovers are installed-bundle screenshot-verified.
- `[x]` Optional voice input matches the C composer position and lifecycle:
  first-use 548 MiB model confirmation/download/cancel, recording timer/stop,
  transcription, whitespace-safe insertion, errors, chat-switch cancellation,
  and shutdown cleanup. Microphone capture stays on the GTK client; model
  storage and transcription run through the selected chat's daemon, so paired
  chats install and execute Whisper on the remote machine. Targeted daemon
  events keep transcripts private to the requesting local/TLS connection. The
  relocatable Linux bundle carries the PulseAudio runtime and CPU-dispatched
  whisper.cpp build. macOS and Windows select PortAudio behind the same
  recorder contract; the Docker build compiles and links that backend without
  touching microphone hardware. Installed-bundle GTK proof verifies the idle
  microphone control; the current inline first-use prompt, progress bar, and
  responsive Cancel path pass fatal-warning GTK smoke. Actual paired TLS specs
  verify remote execution and event isolation. Model download and recorder
  callbacks run in scheduler-backed execution contexts, so progress and
  completion can return to GTK without freezing or crashing.
- `[~]` Context/branch/worktree line below composer uses the C copy, ellipsis,
  tooltip, and geometry. The daemon computes it once for local and TLS clients;
  installed GTK geometry is verified; live branch-change verification remains.

C sources: `src/chat/chat-view.c`, `src/chat/model-picker.c`,
`src/chat/option-picker.c`, `src/chat/voice-input.c`.

## Terminal, files, and diff panes

- `[~]` Terminal is the end child of a vertical `GtkPaned` below the
  conversation/composer, with 260-pixel default and persisted height. The
  exact-commit `2485a89` bundle verifies bottom placement, divider, restored
  height, and restored session/output at 1100x720. Multi-chat and paired
  visibility matrices remain.
- `[~]` Files and diff share one stack at the right of the conversation plus
  terminal, with 420-pixel default and persisted width. Local installed-bundle
  geometry and restart are verified; paired persistence remains.
- `[x]` Header controls are toggle buttons whose active states match pane
  visibility.
- `[x]` Header Git action uses the C one-button state machine (`Commit`,
  `Push`, `Create PR`, `View PR`) and exact Adwaita commit/error dialogs.
  State and actions run only in the shared daemon; request tokens keep
  asynchronous results scoped to their initiating client. The exact
  `64f6423` bundle is verified under `G_DEBUG=fatal-warnings`: a working patch
  becomes `Nothing Changed` after commit, the button advances to `Push`, and a
  refused push opens `Git Refused` without leaking into the chat footer.
- `[~]` Terminal RPC, PTY output, input, resize, replay, and kill operations use
  the shared daemon. List/open/kill/input/resize requests never wait in GTK,
  and duplicate asynchronous opens are suppressed. An installed local client
  verifies each operation across UI disconnect/reconnect; paired TLS proof
  remains.
- `[~]` Daemon-backed multi-session tabs, centered single-session title,
  add/kill controls, close request, focus, replay, resize, and per-chat view
  retention work. C palette/font, copy/paste, URL handling, and daemon-terminal
  Backspace behavior are ported. The `2485a89` bundle survives one-tab,
  two-tab, detach, and replay transitions under `G_DEBUG=fatal-warnings`;
  compensating minimum widths keep the C negative-margin tab fill without
  negative GTK measurements. Offline status, reconnect reconciliation, and
  pending-kill retry are implemented; paired reconnect UI verification remains.
- `[~]` File list/read/write uses the C `AdwBin`/toast overlay, exact
  header/list/preview/status stack, directory-first UTF-8 collation, hidden
  filtering, back/refresh modified guards, Ctrl+S save state, 1 MiB and binary
  statuses, and the shared 14-language syntax highlighter with the C 8,000-line
  cap. An exact-commit installed bundle is verified at 1100x720 with fatal GTK
  criticals: root/nested navigation, row geometry, Crystal colours, modified
  sensitivity, guard/save toasts, persisted edits, and binary/large-file
  status pages all match. Pane errors no longer leak into the chat footer.
  Reads/writes are asynchronous and generation-scoped so stale remote replies
  cannot replace a newly selected chat. The 8,000-line syntax scan runs on a
  worker thread and applies at most 256 coloured spans per GTK idle turn,
  cancelling stale work after edits or navigation. Explicit request
  cancellation and paired-TLS latency proof remain.
- `[~]` Working and branch scopes use the C header, linked toggles, refresh,
  summary/empty/error states, and virtual `GtkListView` file sections.
  Per-file expansion, collapsed-path memory, 80-row chunks, scroll restoration,
  syntax gutters, full-row backgrounds, untracked patches, and summary totals
  are ported. An exact-commit installed bundle is verified at 1100x720 with
  fatal GTK criticals across multi-file working changes, branch-vs-main,
  refresh after collapse, clean/error states, and a 180-line patch; the
  80→81 chunk boundary has no visual seam and collapsing preserves its header
  position. Diff state/base/patch requests are asynchronous and
  generation-scoped; pure patch parsing and file-section calculation run on a
  worker thread before GTK receives the virtual model. Explicit cancellation,
  paired-TLS latency, and live Git-head refresh proof remain. Agent-native edit,
  write, multi-edit, NotebookEdit, apply-patch, and file-change payloads also
  produce inline unified diffs without a Git repository or Git executable.
- `[~]` Pane visibility persists per local/remote chat in the same typed
  `a{su}` device map. Local restart/UI restoration is verified; multi-chat and
  paired restart matrices remain.
- `[x]` Agent turns and completed Git actions refresh repository state and the
  visible diff. The daemon monitors HEAD signatures for eight recently active
  chats and publishes one transport-neutral event for terminal/external
  commits and checkouts. The exact `ecdf162` bundle changes the context from
  `feature` to `monitor-proof` after an external checkout while action/diff
  state stays synchronized and fatal GTK warnings remain silent.

C sources: `src/chat/terminal-panel.c`, `src/chat/file-pane.c`,
`src/chat/diff-pane.c`, `src/chat/diff-view.c`,
`src/chat/diff-text.c`, `src/chat/git-actions.c`,
`src/settings/pane-state.c`.

## Dialogs and auxiliary UI

- `[x]` Pairing panel matches the C panel structure and routes into the durable
  remote connection. Cancel and Escape remain available while connecting;
  cancellation closes the returned client before credentials can be stored,
  matching the C prompt lifecycle.
- `[~]` Workspace create/rename/delete/settings/context and agent-secret
  dialogs exist.
- `[x]` Search dialog matches the C Adwaita widget tree, empty/result states,
  sizing, focus, and row activation; installed-bundle UI matrix passes with
  fatal GTK criticals.
- `[~]` Folder settings now uses the C `AdwPreferencesDialog`, groups, combo,
  entry and path rows, inherited subtitles, suffix controls, and save-on-close
  lifecycle. Initial state and saving are asynchronous; path browsing stays
  daemon-backed for both transports. The exact
  `634879f` bundle is screenshot-verified and persists a browser-chosen path
  on close with fatal GTK warnings enabled. Agent Context now matches the C
  undecorated 620×500 panel, editor frame, status, footer, async load/save,
  busy state, Escape, and Ctrl+Enter behavior through the same endpoint. Agent
  Secrets now matches the C 700×500 local/remote/folder panel, copy, row
  lifecycle, inline validation, busy state, shortcuts, and value-withholding
  protocol. The exact `ce4199e` binary is installed-bundle verified: Context
  loads, saves and cancels; Secrets validates, creates, reloads name-only,
  preserves a blank existing value, and saves by Ctrl+Enter with fatal GTK
  warnings enabled.
- `[x]` C directory-browser hierarchy, row factory, navigation keys, dismissal
  semantics, and styling are ported. Both local and TLS sources list only
  through the daemon `list-dir` operation, and new-chat creation waits for its
  chosen daemon-side working directory. Installed-bundle screenshots cover
  selection, Enter descent, Backspace ascent, and missing-path error state.
  Escape dismisses valid and error states in 30–32 ms while preserving the C
  “use folder defaults” callback. The footer has the exact C hints and no
  added Cancel action. Daemon specs cover hidden/file filtering and missing or
  non-directory errors.
- `[x]` Branch/PR parsing, shell-safe fetch/build/install command, exact panel,
  bounded live output, stop, persistence, and dialog-independent run lifecycle
  match C. Panel is screenshot-verified from an installed-path nightly bundle
  with zero GTK stderr. A deliberately infinite noisy build proves GTK
  scheduler responsiveness and bounded Stop. Close, Escape, focus loss, and
  successful-install dismissal all use deferred cleanup outside Mutter focus
  notification; installed-bundle runtime checks close the panel in 8–110 ms
  while leaving the main window responsive. A clean isolated home fetched the
  latest branch commit, built the complete 1014 MiB bundle, installed and
  launched it, then ran the panel's exact generated command over that live
  install. In-app replacement uses an atomic old/new bundle swap with rollback,
  while direct installers reject active bundles.
- `[x]` File save/guard failures use the C `AdwToastOverlay` placement and
  messages. Startup and database failures keep the normal 1100×720 app surface
  behind one modal `AdwAlertDialog`; database failures use the exact C heading
  and Quit-only lifecycle. A deliberately invalid SQLite path is
  installed-bundle screenshot-verified under `G_DEBUG=fatal-warnings`, and
  Quit closes the process cleanly.

C sources: `src/remote/pair-dialog.c`, `src/settings/*-dialog.c`,
`src/chat/search-dialog.c`, `src/ui/dir-browser.c`,
`src/ui/branch-build-dialog.c`.

## Daemon, storage, and remote behavior

- `[x]` Platform-local IPC and remote TLS dispatch through one `Daemon::Engine`.
- `[x]` Continuously readable Codex/Claude pipes, client/server sockets,
  terminal PTYs, updater/auth streams, and voice streams yield cooperatively
  after bounded chunks. Regression bursts of 100,000 CLI lines and 20,000
  daemon events keep scheduler heartbeats below 250 milliseconds instead of
  starving GTK. The client UI additionally coalesces adjacent text deltas and
  drains one event source in 32-event batches, preventing fast agent output
  from flooding GLib's idle queue; ordering/bounds/rescheduling have specs.
- `[x]` Remote pairing uses a short-lived code, token, and pinned certificate.
- `[x]` Stored remote reconnects while retaining endpoint subscribers.
- `[x]` Folder, chat, settings, message, send/cancel, file, diff, image, search,
  and terminal operations share the same protocol.
- `[x]` Actual Unix-socket and paired TLS clients pass one normalized stateful
  matrix against separate Engine instances: workspace/context/settings,
  daemon-owned secrets, directory and file operations, Git diff, atomic model
  selection, effort/access/Plan, queue edits, send/cancel, transcript/search,
  terminal open/resize/kill, voice-model ownership, events, and deletion. The
  authenticated Windows-loopback build passes that same matrix in Docker.
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
- `[x]` Pinned Git, its exec-path helpers, templates, HTTPS transport, and CA
  roots ship in the Linux bundle; a no-host-Git smoke test covers repository
  creation, commits, diffs, worktrees, and HTTPS helper loading.
- `[x]` Agent execution resolves bundled binaries before host binaries.
- `[~]` Daemon-owned CLI updater runs the official `codex update` and
  `claude update` commands asynchronously, checks structured before/after
  versions, blocks replacement while turns run, blocks new turns during
  replacement, and reloads the pooled Codex app-server after success. Local/TLS
  protocol operations drive a clean account-panel version row and Update CLIs
  button against the selected daemon. An actual paired TLS client checks and
  updates both fixture binaries on the remote machine. Real official-release
  replacement remains.
- `[~]` Codex app-server and Claude stream-json turns work through the shared
  manager.
- `[~]` Codex status, device login, browser link, cancellation, logout, and
  structured login state run through the daemon-owned authentication service.
  The panel exposes the browser action and one-time code without showing raw
  CLI output. Local UI and an actual paired TLS client cover status, start,
  cancellation, process cleanup, logout, and stale-instruction retirement.
  Authentication pipe reads explicitly yield to GTK and publish only changed
  structured instructions. A million-line synthetic CLI login leaves Escape
  responsive in 21 ms and the visible Close button responsive in 110 ms while
  cancellation cleans up the process. Every actual manager launch is gated by
  daemon-owned structured auth state, including queued and remote callers, so
  a signed-out turn never reaches the provider or stores a false user row.
  Unix and authenticated TLS rejection are covered before the fake launcher.
  Chat state exposes the selected backend's auth state. The composer disables
  turn input while unsigned and offers a direct Sign In action that opens the
  same clean account panel against the selected local or remote endpoint;
  daemon auth events re-enable it without reopening the chat.
  One real bundled OAuth completion remains.
- `[~]` Claude status, browser login, pasted-code input, cancellation, and
  logout use the same structured service and panel. Local UI and an actual
  paired TLS client cover clean code entry, signed-in state, sign-out, and
  remote credential ownership. Ctrl+V is handled explicitly for the visible
  authorization-code row so modal key capture cannot suppress clipboard text.
  One real bundled OAuth completion remains.
- `[~]` Model, effort, access, plan/build, resume, and cancellation run through
  the shared manager. Assistant/model changes are atomic and append the same
  visible `Switched to …` transcript event for local and TLS clients without
  duplicating unchanged selections. Manager specs verify exact selected
  backend/model/effort, Plan's temporary access override, and restoration of
  the stored access mode. The shared local/TLS protocol matrix also verifies
  exact model, effort, stored access, temporary Plan access, prompt, daemon
  secret environment, send, and cancel. Installed GTK matrix remains.
- `[x]` Cerebras/OpenCode code, registration, assets, fixtures, and tests are
  removed; Claude Code and Codex are the only backends.

C sources: `src/backend`, Crystal sources: `src/xd/agent`.

## Packaging, developer workflow, and cleanup

- `[x]` Linux development, tests, and bundle builds require Docker only.
- `[x]` One-line branch installer builds the latest branch commit.
- `[x]` Installers reject replacement while xd is running, preventing GNOME
  from reactivating a stale GtkApplication after an update.
- `[x]` Linux bundle carries GTK, libadwaita, VTE, GL, fonts, icons, MIME, TLS,
  OpenSSL, Git, Codex, and Claude.
- `[~]` Replace paused legacy macOS and Windows packaging with Crystal-native
  builds; do not publish old C artifacts under Crystal releases. Runtime
  discovery now handles both flat payloads and macOS `Contents/Resources`,
  prepares relocatable GTK/GIO/font caches in-process, resolves bundled tools
  and portable Git, and removes the Git pane's POSIX-shell dependency. Windows
  local IPC uses authenticated loopback with a 256-bit endpoint token protected
  to the current user by DPAPI, bounded handshakes, startup locking, and
  stale-endpoint recovery. Native build/bundle jobs plus platform terminal,
  PortAudio delivery, and actual-host verification remain.
- `[x]` README and CI use the Crystal Docker workflow; no active workflow builds
  or publishes Meson/C artifacts.
- `[ ]` Remove Odin experiment and old C implementation only after every C
  behavior above has a verified Crystal replacement.
- `[ ]` Run clean-host installer, local daemon, paired daemon, reconnect,
  terminal, Codex, Claude, and screenshot verification.

## Required release evidence

- `[x]` Crystal specs pass in Docker (294 examples).
- `[x]` Crystal release binary builds in Docker.
- `[x]` Bundle launches with isolated `HOME`, `XDG_DATA_HOME`, and
  `XDG_DATA_DIRS=/nonexistent`.
- `[ ]` 1100x720 screenshots match the C app for empty, populated, active turn,
  question, terminal, files, diff, search, settings, secrets, and pairing
  states.
- `[x]` Local and paired clients pass the same protocol behavior suite.
- `[ ]` Codex and Claude authentication and one real turn each succeed from the
  shipped bundle.
