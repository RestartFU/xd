# xd host wire protocol

This is the normative JSON Lines contract used by `xd-host stdio`. The desktop
uses it over child-process stdin/stdout locally and over SSH remotely. Android
runs the same stdio endpoint through an SSH exec channel.

Any wire change must update the Rust host/desktop tests and the Kotlin
`commonTest` fixtures together. Server and clients must ignore unknown object
members unless explicitly documented otherwise.

## Transport and framing

- Application data is UTF-8 JSON Lines: one JSON object followed by `LF`
  (`0x0a`). Empty lines are ignored.
- Requests, replies, and events share one ordered, full-duplex stdio stream.
- Frames are limited to 96 MiB. An oversized frame is a protocol error and
  closes the connection.
- The transport must keep draining output while connected. A stalled reader is
  disconnected rather than allowed to block the host indefinitely.
- JSON member absence and JSON `null` are different. Unless an operation says
  otherwise, an optional member is omitted rather than sent as `null`.

## Requests, replies, and events

Every request is an object with a string `op` member:

```json
{"op":"ping"}
```

Every reply is an object with boolean `ok`. Success payloads add
operation-specific members. Errors have this shape:

```json
{"ok":false,"error":"Human-readable reason"}
```

### Request ids

A client should add a private `_xd_request` integer to every request. The
host echoes it on the matching reply, which lets replies be matched by id
rather than by arrival position:

```json
{"op":"cancel","chat":"chat-1","_xd_request":42}
{"ok":true,"_xd_request":42}
```

This matters because the host answers requests concurrently. The operations
that must stay responsive — `cancel`, `voice-cancel`, `agent-auth-cancel`, and
`ping` — bypass the serialized command path entirely. Without ids, a `cancel`
answered promptly by the host would still sit behind a slow `diff-read` in
the reply stream.

A host that does not echo the id answers strictly in order. A client
supporting that compatibility path must keep a FIFO slot for every request
written, and must retain the slot of a timed-out or cancelled caller — dropping
it shifts every later reply onto the wrong request.

### Events

An object is an event when it contains an `event` member. Events may appear
between a request and its reply and do not consume a reply slot:

```json
{"event":"tree","id":17}
```

`id` is a monotonic counter assigned at publication. **It is per-process and
in-memory only**: it does not survive a host
restart, and there is no resume-from-id operation. A reconnecting client must
take a fresh snapshot rather than try to replay what it missed.

Unknown events are ignored. A non-event reply with no pending request is a
protocol error.

### Ordering caveat

Replies and events reach the socket through different paths. A reply is written
by the fiber handling that request; asynchronous events are written by the
session's own outbound-queue fiber. Both share one mutex, so frames never
interleave mid-object, **but the relative order of a reply and a concurrently
published event is not guaranteed.**

A `text` or `tool` event already folded into a `chat` snapshot's `segment` can
therefore arrive after that snapshot's reply. Clients must not deduplicate live
turn output by arrival order; use the turn watermark described under `chat`.

## Transport authentication

Authentication and machine identity are established by the transport before
`xd-host stdio` starts. A local desktop owns the child process directly. Remote
desktop and mobile clients authenticate with SSH and verify the SSH host key.
There is no application-level `pair` or `hello` operation and no bearer token.
The first application frame may be any supported request.

The SSH account has the same authority as the host process. Clients must pin or
otherwise strictly verify the SSH host key before sending password or private-key
authentication material.

## State reads

### `tree`

```json
{"op":"tree"}
```

```json
{
  "ok": true,
  "folders": [
    {"id":"folder-1","name":"Project"},
    {"id":"folder-2","name":"Child","parent":"folder-1"}
  ],
  "chats": [
    {
      "id":"chat-1",
      "folder":"folder-1",
      "title":"Fix mobile layout",
      "backend":"codex",
      "working":false
    }
  ]
}
```

`parent` is absent for root folders. Arrays are complete snapshots. Clients
must tolerate a child appearing before its parent and treat ids as opaque.

### `chat`

```json
{"op":"chat","chat":"chat-1"}
```

Success always includes `ok`, `title`, `backend`, `auth_state`, `commands`,
`plan`, `queue` (oldest first), `working`, `effort`, `new_worktree`, and
`has_messages`.

Optional members: `auth_detail`, `queued` (compatibility alias for the first
queue item), `model`, `access`, `context_used`, `context_window`, `workdir`,
`linked_worktree`, `worktrees`, `selected_worktree`.

A worktree is:

```json
{"path":"/work/project","branch":"main","detached":false,"main":true,"current":true}
```

`branch` is absent for a detached worktree.

`selected_worktree` is present only when the chat has no messages, is not
requesting a new worktree, has an original checkout, and is currently using a
linked worktree. Its value is the selected worktree's absolute path; clients
may offer removal only while this member is present.

When a turn is live the reply also carries its snapshot:

- `label`: string
- `turn_id`, `turn_sequence`: integers — the turn watermark
- `working_for`: elapsed seconds
- `items`: ordered array of `{"tool":bool,"text":string}`
- `segment`: unfinished assistant text

A client renders each non-tool item as finished assistant text, each tool item
as a tool row, then appends `segment`.

**Turn watermark.** `turn_sequence` counts the `text` and `tool` events already
folded into this snapshot. A later event for the same `turn_id` whose
`turn_sequence` is less than or equal to the snapshot's is already represented
and must be dropped; a higher one must be applied. An event carrying a lower
`turn_id` is stale. Given the ordering caveat above, this is the only correct
way to reconcile a snapshot against concurrent live output.

`working` may be true with no turn snapshot present, when a queued message is
durably marked as working but its turn has not started yet.

Reading a chat may start its first persisted queued message when no turn is
running.

### `messages`

```json
{"op":"messages","chat":"chat-1","limit":150}
```

`chat` is required. A positive `limit` returns at most that many recent
messages; zero or omission returns all messages when no turn is active. An
optional nonnegative `offset` changes the request to a zero-based, oldest-first
slice through the active transcript boundary. Offset requests require a
positive limit and return the actual clamped offset in the response;
`turn_start` may still identify the preceding user message when a slice begins
in the middle of a turn. Cursor-aware clients may instead send one positive
message id as `before` or `after`; the two cursors are mutually exclusive,
cannot be combined with `offset`, and require a positive limit. Cursor pages
remain oldest-to-newest and do not shift when another device appends a turn.

```json
{
  "ok": true,
  "total_messages": 2,
  "last_message_id": 42,
  "offset": 0,
  "has_older": false,
  "has_newer": false,
  "messages": [
    {"id":41,"role":"user","content":"Hello","at":1753700000},
    {"id":42,"role":"assistant","content":"Hi","at":1753700001,"label":"Codex"}
  ]
}
```

Messages are oldest-to-newest within the returned slice. `id` is the stable
persisted cursor, `at` is Unix seconds, and `label` is optional. Roles include
`user`, `assistant`, `tool`, `event`, `error`, and `duration`; render an unknown
role as system text. `duration` rows carry an elapsed-seconds string and are
normally hidden. `has_older` and `has_newer` describe rows outside the returned
cursor page.

For recent requests, `total_messages` greater than the returned count means
older messages exist. Offset slices may have both older and newer messages
outside the returned range.
During a turn the response is bounded at that turn's persisted transcript id;
live items come from `chat` and events. `last_message_id` identifies that
boundary, not an event cursor.

### Other reads

| Operation | Request members | Success members and rules |
|---|---|---|
| `folder-context` | `folder: string` | `context: string \| null` |
| `folder-settings` | `folder: string` | inherited folder settings |
| `shortcuts` | optional `folder: string` | host-wide `global`, folder-owned `workspace`, and merged `effective` prompt arrays |
| `list-dir` | optional `path: string` | `path`, `entries: string[]`; defaults to host home, lists non-hidden directories |
| `file-browse` | `chat`, `action`, optional relative `path`, `content`, and `original` | `action:"list"` returns `entries:[{name,directory}]`; `action:"read"` returns UTF-8 `content` for a regular file no larger than 1 MiB; `action:"write"` saves bounded UTF-8 text and rejects a stale optional `original` |
| `agent-secrets` | none | `names: string[]`; values never cross the wire |
| `agent-clis` | none | detected assistant versions |
| `host-update` | optional `action` of `status`, `check`, `install`, `restart` | `version`, `channel`, `state`, `supported`, `available`, optional `latest` and `error` |
| `agent-catalog` | none | `backends: [{id, name, default_model, models:[{id,name,context_window}], efforts:[string]}]` |
| `workflow-status` | `text: string` captured `workflow_run` marker | `name`, `state`, optional `conclusion`, `jobs:[{id,name,state, optional conclusion, optional log}]`; fetches GitHub Actions status on the host |
| `image-read` | absolute host `path`, optional `preview: boolean` | `mime:"image/png"`, base64 `data`; only host-created remote pastes |
| `search` | `query` | matching stored messages |
| `diff-read` | `chat` plus one `read` of `base`, `working-status`, `branch-status` (with `base`), `working-all`, `branch-all` (with `base`), `working-file`/`untracked-file` (with a safe relative `path`), or `branch-file` (with `base` and `path`) | `output: string`, limited to 8 MiB |
| `ping` | none | no members beyond `ok` |

`file-browse` paths are relative to the chat workdir and may not escape it.
Hidden entries are omitted. Directories sort before files.

## Intents

The host is the only writer. A client never edits state directly: it sends an
intent, and the host applies it and broadcasts what happened. Two devices
acting at once are therefore ordered by the host rather than racing.

### `new-chat`

```json
{"op":"new-chat","folder":"folder-1","title":"Optional","workdir":"/optional/path"}
{"ok":true,"id":"chat-9"}
```

`folder` is **required**. `title` defaults to `New Chat`. The new chat inherits
its folder's backend and model. When `workdir` is omitted it also inherits the
folder's effective working directory; a supplied path selects a per-chat
working directory on the host machine.

### `set-shortcuts`

```json
{"op":"set-shortcuts","shortcuts":["Review the current diff","Run the tests"]}
{"op":"set-shortcuts","folder":"folder-1","shortcuts":["Check this workspace"]}
```

Without `folder`, the operation replaces host-wide shortcuts. With a folder
id it replaces that workspace's shortcuts. Blank and duplicate prompts are
removed. Chats receive global prompts followed by prompts inherited from their
workspace ancestry and display them as send buttons. Activating one uses the
normal `send` intent, so it is queued automatically while a turn is active.

### `new-folder`

```json
{"op":"new-folder","name":"Project","repo":"/home/user/project"}
{"ok":true,"id":"folder-1"}
```

`name` is required and creates a top-level workspace folder unless `parent`
contains an existing folder id. `repo` is optional; when present it must be an
existing directory on the host and becomes the new folder's repository and
fallback working directory. An explicit folder working-directory setting still
takes precedence. The selected repository is not moved or modified.

### `move-chat`

```json
{"op":"move-chat","chat":"chat-9","folder":"folder-2"}
{"ok":true}
```

Both `chat` and `folder` are required. The target folder must exist. Moving a
chat changes which folder settings it inherits; the chat id and transcript stay
the same. The host broadcasts a `tree` event after a successful move.

### `send`

```json
{"op":"send","chat":"chat-1","text":"hello"}
{"ok":true,"queued":false}
```

Needs `text`, `attachments`, or both. `queued` is true when a turn was already
running and this message was appended to the queue instead of started — one
turn per chat is enforced by the host.

Attachments are PNG only:

```json
{"attachments":[{"mime":"image/png","data":"base64…"}]}
```

At most 4 images, each at most 10 MiB, 20 MiB in total.

Desktop clients may provide `worktree_name`, `worktree_backend`, and
`worktree_model` when the chat is configured to create a new worktree. The
name is used directly as the hint when supplied; otherwise the host uses that
configured Git-writing assistant to choose a short worktree name before the
first turn. Older or other clients may omit them and the host falls back to
a prompt slug.

### `set-draft`

```json
{"op":"set-draft","chat":"chat-1","text":"unfinished"}
{"ok":true,"draft":"unfinished","draft_revision":4}
```

The host persists one composer draft per chat and broadcasts a `draft` event
after every update. `text` is required but may be empty and is limited to 1
MiB. Optional `attachments` replaces the synchronized PNG preview list; omit it
for ordinary text edits so large image payloads are not resent on every
keystroke. An empty array clears previews. The `chat` snapshot returns `draft`,
`draft_revision`, and `draft_attachments` so a newly connected device starts
with the same composer.

### `queue`, `drop-queue`, `edit-queue`, `steer-queue`

```json
{"op":"queue","chat":"chat-1","text":"next"}
{"op":"drop-queue","chat":"chat-1","index":0}
```

`queue` requires non-empty `text`. `drop-queue` without `index` clears the whole
queue. All four broadcast a `queued` event carrying the new queue, so every
attached device sees what is waiting and either can steer it.

### `cancel`

```json
{"op":"cancel","chat":"chat-1"}
```

Stops the running turn. A control operation: answered even while a serialized
command is slow.

### `set-option`

```json
{"op":"set-option","chat":"chat-1","option":"effort","value":"high"}
```

Options: `model`, `effort`, `access`, `plan`, `fast`, `backend`,
`new-worktree`, `workspace`. Boolean options take `"true"`/`"false"`.
`fast` is Codex-only. An unknown option is an error.

Selecting a model atomically requires sending `backend` alongside `value`;
without `backend` only the model string is stored. The atomic form validates
both, clears an unsupported effort, and appends a visible `Switched to …`
transcript event.

`host-update` updates the host's own installation, so a paired device can
bring a machine forward without a shell on it. `install` replaces the files,
which is safe while turns run because the process keeps the binary it already
mapped; `restart` drops every connection and loses any running turn, so the
two are separate actions and neither happens on its own. Native Linux bundles
and macOS apps support this flow. `supported` is false where the installation
cannot replace itself, such as source builds, and both `install` and `restart`
are refused there.

`agent-catalog` lists the assistants and models this host can run. The
desktop reads its own compiled-in catalog because it ships with the host; a
separately released client must ask, since `set-option` validates the model id
and a hard-coded list would be refused as soon as one is added or retired.

### `workflow-status`

```json
{
  "op":"workflow-status",
  "text":"workflow_run\\n123\\nhttps://github.com/RestartFU/xd/actions/runs/123"
}
```

The host validates the captured marker, then fetches the run and its jobs from
GitHub using its configured token. Active jobs omit a terminal conclusion and
include their latest step name in `log`; clients should show a loading indicator
rather than an `In progress` label. The endpoint is intended for mobile clients,
while the desktop card may fetch the same data directly.

### `remove-worktree`

```json
{"op":"remove-worktree","chat":"chat-1","worktree":"/absolute/path/to/worktree"}
{"ok":true}
```

The `chat` and absolute `worktree` path are required. Only the selected linked
worktree reported by `selected_worktree` may be removed, and the chat must not
have any messages. The target must be a clean, registered worktree that is
neither the repository's main checkout nor its current checkout, and no other
chat may reference it. The host removes the checkout without force and keeps
the Git branch, then returns the chat to its original checkout. A successful
request replies with `ok` alone.

### Folder and chat mutations

`new-folder`, `rename-folder`, `move-folder`, `trash-folder`, `rename-chat`,
`move-chat`, `delete-chat`, `set-folder-context`, and `set-folder-settings` all
reply with `ok` alone and broadcast a `tree` event.

`set-shortcuts` replies with the resulting shortcut sets and broadcasts a
`shortcuts-changed` event so open chats can refresh their buttons.

### Git actions

`git-state` takes a `chat` and an optional request correlation string. It
acknowledges immediately, reads repository state away from the connection
loop, and publishes `git-state` with `visible`, `action`, `label`, `enabled`,
and an optional pull-request `url`.

`git-action` takes a `chat`, one of `commit`, `push`, `create-pr`, or `view-pr`,
and an optional request correlation string. Commits also require `message`;
pull-request creation requires `title` and accepts `body`. It acknowledges
immediately and publishes `git-action-finished`. Successful events contain the
new repository action state and an optional URL; failures contain `success:
false` and `error`. The host rechecks the advertised action before mutating
the repository so a stale client cannot run the wrong operation.

`move-folder` takes a required `folder` and an optional `parent`. Omit `parent`
to move the folder to the workspace root. A folder cannot be moved into itself
or one of its descendants, and the host rejects a destination name collision.

## Events

Chat-scoped events carry a `chat` member; ignore those for other chats.

| Event | Members | Meaning |
| --- | --- | --- |
| `tree` | — | Workspace tree changed; refetch `tree`. |
| `changed` | `chat` | Stored chat state changed; refetch when idle. |
| `worktrees-changed` | — | A Git worktree changed; refetch chat state and worktree lists. |
| `shortcuts-changed` | optional `folder` | Global or workspace prompt shortcuts changed; refetch chat state. |
| `queued` | `chat`, `queue`, `text` | Queue replaced. `text` is the first item. |
| `draft` | `chat`, `draft`, `draft_revision`, optional `draft_attachments` | Composer text changed; attachments are present only when replaced. |
| `commands` | `chat`, `backend`, `commands` | Slash-command list for a backend. |
| `turn-started` | `chat`, `label`, `turn_id`, `turn_sequence` | A turn began. |
| `text` | `chat`, `text`, `turn_id`, `turn_sequence` | Assistant text delta. |
| `tool` | `chat`, `text`, `workdir`, `context`, `turn_id`, `turn_sequence` | Tool row. |
| `turn-finished` | `chat`, `turn_id`, `turn_sequence`, `ok`, `waiting`, `silent`, `duration`, `last_message_id`, optional `error`, `question`, `options`, `accepts_input` | The turn ended. |
| `repository-changed` | `chat` | HEAD moved outside the app. |
| `git-state`, `git-action-finished`, `git-draft-finished` | `chat`, … | Git pane state. |
| `terminal-output`, `terminal-closed` | `terminal`, … | Shared pty; output is base64. |
| `auth`, `agent-auth-changed` | … | Assistant authentication state. |

### Tagged questions

Structured assistant turns run non-interactively, so a question is a block in the
reply rather than a prompt:

```
<ask>
Which implementation?
- Keep the parser
- Replace the parser
<input>
</ask>
```

The question is the first line, each option a `- ` or `* ` line (two to six of
them), and `<input>` means a typed answer is also accepted. `turn-finished`
reports the parsed form in `question`, `options`, and `accepts_input`.

The block is **also stored with the message, verbatim** — unlike workspace
blocks, which the host strips before storage. That is deliberate: it lets
every client render its own buttons, and lets a client that reopens a chat find
the question without having seen the event. A client must therefore strip these
blocks before rendering, or show raw tags. `host/src/ask.rs` is the
reference parser; the mobile client mirrors it in
`shared/.../model/Ask.kt`.

Answering is an ordinary `send`. There is no separate answer operation.

`text` deltas are already filtered to visible output: ask-blocks and workspace
blocks are withheld until complete, so a client appends deltas literally.

Turn output fans out to every subscriber, not only the device that sent the
message. After `turn-finished` a client should refetch `messages`; the durable
transcript is authoritative and the live segment is discarded.

## Client obligations

1. Pin the exact leaf certificate after pairing. Never offer a trust-anyway
   action — the token is remote code execution on the host machine.
2. Keep reading the socket at all times, or be disconnected at 256 queued
   events.
3. Re-snapshot on reconnect. There is no event replay.
4. Deduplicate live turn output by turn watermark, never by arrival order.
5. Treat a refused `hello` and a certificate mismatch as terminal: stop
   retrying and require explicit re-pairing.
