# Remote wire protocol

This document is the normative contract between `xd serve` and every remote
client. The implementation is the Crystal daemon: `src/xd/protocol/` defines
framing and the operation vocabulary, and `src/xd/daemon/engine.cr` is the sole
dispatcher for local and remote clients alike. Protocol version 1 is reported
by a successful `hello`.

Any wire change must update `spec/xd/protocol/` and the Kotlin `commonTest`
fixtures together. Server and clients must ignore unknown object members unless
a later protocol version explicitly says otherwise.

## Transport and framing

- TCP port 4001 by default.
- TLS begins immediately after the TCP connection. There is no plaintext mode
  and no upgrade handshake.
- The daemon uses a self-signed certificate (`src/xd/daemon/certificate.cr`:
  RSA-2048, `CN=xd`, 3650 days). During pairing a client accepts the presented
  leaf and persists it alongside the issued token. Every later connection must
  require that exact leaf.
- Application data is UTF-8 JSON Lines: one JSON object followed by `LF`
  (`0x0a`). Empty lines are ignored.
- Requests, replies, and events share one ordered, full-duplex connection.
- Frames are limited to 64 KiB before authentication and 96 MiB afterwards
  (`Xd::Protocol::AUTH_FRAME_LIMIT` and `FRAME_LIMIT`). An oversized frame is a
  protocol error and closes the connection.
- Each session holds a bounded 256-event outbound queue
  (`Session::EVENT_QUEUE_SIZE`). **A client that stops draining its socket is
  disconnected** rather than allowed to apply backpressure to the daemon. A
  client must keep reading even while busy.

JSON member absence and JSON `null` are different. Unless an operation says
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
daemon echoes it on the matching reply, which lets replies be matched by id
rather than by arrival position:

```json
{"op":"cancel","chat":"chat-1","_xd_request":42}
{"ok":true,"_xd_request":42}
```

This matters because the daemon answers requests concurrently. The operations
that must stay responsive — `cancel`, `voice-cancel`, `agent-auth-cancel`, and
`ping` — bypass the serialized command path entirely
(`Engine#control_operation?`). Without ids, a `cancel` answered promptly by the
daemon would still sit behind a slow `diff-read` in the reply stream.

A daemon that does not echo the id answers strictly in order. A client
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
in-memory only** (`Xd::Daemon::EventBus`): it does not survive a daemon
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

## Authentication

`pair` and `hello` are the only operations allowed on an unauthenticated
connection (`Operation#authentication_required?`). Everything else is refused
with `Not authenticated. Say hello first.`

### Pair

Pairing is armed by `xd serve --pair`, or from the desktop app's **Add a
Device…** panel. The pairing window has no device name; the connecting client
supplies its automatic name when it submits the code:

```json
{"op":"peer-pairing"}
```

The displayed `XXXX-XXXX` code is valid for five minutes and one matching
submission. Its alphabet excludes `I`, `O`, `0`, and `1`.

```json
{"op":"pair","code":"4F2K-9QX1","name":"Pixel 9"}
{"ok":true,"token":"base64 device token","device":"Pixel 9"}
```

`code` is required, as is a non-empty `name` supplied by the connecting client.
The daemon normalizes and stores that name; it does not invent one. The local
owner can rename it later through device management. A successful pair:

- authenticates the existing connection; do not send `hello` afterward;
- consumes the code before writing the device record, so even a storage failure
  requires a new code;
- returns a long-lived bearer token and the authoritative device name;
- requires the client to persist token and pinned certificate atomically.

### Hello

Every later connection sends `hello` as its first request:

```json
{"op":"hello","token":"base64 device token"}
{"ok":true,"device":"Phone","version":1}
```

An unknown or revoked token returns `ok:false`. Clients must stop retrying and
offer pairing again. A certificate mismatch must also stop retries: continuing
would disclose the bearer token to a different peer.

### Device management

The daemon owner manages paired credentials through the local IPC endpoint.
These operations require local transport and are never accepted from a remote
or mobile client:

```json
{"op":"devices"}
{"ok":true,"devices":[
  {"id":"local opaque id","name":"Phone","created_at":0,
   "last_seen":0,"connected":true}
]}
{"op":"rename-device","device":"local opaque id","name":"Tablet"}
{"ok":true}
{"op":"revoke-device","device":"local opaque id"}
{"ok":true}
```

The `id` is only for the local management surface; clients must not expose or
persist it as a remote credential. Renaming changes the daemon-owned name.
Revoking deletes the token, disconnects every active session for that device,
and requires pairing again.

### What a remote client cannot do

`peer-pairing` requires an authenticated connection **and** local transport
(`Engine#peer_pairing`). A paired remote device cannot mint pairing codes for
further devices, cannot open listeners, enable TLS, or manage other paired
devices. Pairing and device-management authority stays on the daemon machine.

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
`plan`, `queue` (oldest first), `working`, `effort`, `claude_mode`,
`new_worktree`, and
`has_messages`.

Optional members: `auth_detail`, `queued` (compatibility alias for the first
queue item), `model`, `access`, `context_used`, `context_window`, `workdir`,
`linked_worktree`, `worktrees`.

A worktree is:

```json
{"path":"/work/project","branch":"main","detached":false,"main":true,"current":true}
```

`branch` is absent for a detached worktree.

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
messages; zero or omission returns all messages when no turn is active.

```json
{
  "ok": true,
  "total_messages": 2,
  "last_message_id": 42,
  "messages": [
    {"role":"user","content":"Hello","at":1753700000},
    {"role":"assistant","content":"Hi","at":1753700001,"label":"Codex"}
  ]
}
```

Messages are oldest-to-newest within the returned slice. `at` is Unix seconds
and `label` is optional. Roles include `user`, `assistant`, `tool`, `event`,
`error`, and `duration`; render an unknown role as system text. `duration` rows
carry an elapsed-seconds string and are normally hidden.

`total_messages` greater than the returned count means older messages exist.
During a turn the response is bounded at that turn's persisted transcript id;
live items come from `chat` and events. `last_message_id` identifies that
boundary, not an event cursor.

### Other reads

| Operation | Request members | Success members and rules |
|---|---|---|
| `folder-context` | `folder: string` | `context: string \| null` |
| `folder-settings` | `folder: string` | inherited folder settings |
| `list-dir` | optional `path: string` | `path`, `entries: string[]`; defaults to daemon home, lists non-hidden directories |
| `file-browse` | `chat`, `action`, optional relative `path` | `action:"list"` returns `entries:[{name,directory}]`; `action:"read"` returns UTF-8 `content` for a regular file no larger than 1 MiB |
| `agent-secrets` | none | `names: string[]`; values never cross the wire |
| `agent-clis` | none | bundled assistant versions |
| `daemon-update` | optional `action` of `status`, `check`, `install`, `restart` | `version`, `channel`, `state`, `supported`, `available`, optional `latest` and `error` |
| `agent-catalog` | none | `backends: [{id, name, default_model, models:[{id,name,context_window}], efforts:[string]}]` |
| `image-read` | absolute daemon `path`, optional `preview: boolean` | `mime:"image/png"`, base64 `data`; only daemon-created remote pastes |
| `voice-model` | `chat` | `available: boolean` — whether *this daemon* has the speech model on disk |
| `search` | `query` | matching stored messages |
| `diff-read` | `chat` plus one `read` of `base`, `working-status`, or `branch-status` (with `base`) | `output: string`, limited to 8 MiB |
| `ping` | none | no members beyond `ok` |

`file-browse` paths are relative to the chat workdir and may not escape it.
Hidden entries are omitted. Directories sort before files.

## Intents

The daemon is the only writer. A client never edits state directly: it sends an
intent, and the daemon applies it and broadcasts what happened. Two devices
acting at once are therefore ordered by the daemon rather than racing.

### `new-chat`

```json
{"op":"new-chat","folder":"folder-1","title":"Optional"}
{"ok":true,"id":"chat-9"}
```

`folder` is **required**. `title` defaults to `New Chat`. The new chat inherits
its folder's backend and model.

### `send`

```json
{"op":"send","chat":"chat-1","text":"hello"}
{"ok":true,"queued":false}
```

Needs `text`, `attachments`, or both. `queued` is true when a turn was already
running and this message was appended to the queue instead of started — one
turn per chat is enforced by the daemon.

Attachments are PNG only:

```json
{"attachments":[{"mime":"image/png","data":"base64…"}]}
```

At most 4 images, each at most 10 MiB, 20 MiB in total.

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

Options: `model`, `effort`, `access`, `plan`, `fast`, `claude-mode`, `backend`,
`new-worktree`, `workspace`. Boolean options take `"true"`/`"false"`.
`fast` and `claude-mode` are Codex-only; Claude mode routes the selected Codex
model through Claude Code and does not support `ultra` effort. An unknown
option is an error.

Selecting a model atomically requires sending `backend` alongside `value`;
without `backend` only the model string is stored. The atomic form validates
both, clears an unsupported effort, and appends a visible `Switched to …`
transcript event.

`daemon-update` updates the daemon's own installation, so a paired device can
bring a machine forward without a shell on it. `install` replaces the files,
which is safe while turns run because the process keeps the binary it already
mapped; `restart` drops every connection and loses any running turn, so the
two are separate actions and neither happens on its own. `supported` is false
where the installation cannot replace itself -- anything but a Linux bundle
install -- and both `install` and `restart` are refused there.

`agent-catalog` lists the assistants and models this daemon can run. The
desktop reads its own compiled-in catalog because it ships with the daemon; a
separately released client must ask, since `set-option` validates the model id
and a hard-coded list would be refused as soon as one is added or retired.

### Voice

Only microphone capture belongs to the client. The daemon owns the speech model
and the whisper binary, so a remote chat transcribes on the remote machine —
which is also the machine whose CPU and disk are being spent.

```json
{"op":"voice-model-download","chat":"chat-1","request":"a1b2…"}
{"op":"voice-transcribe","chat":"chat-1","request":"a1b2…","audio":"<base64 WAV>"}
{"op":"voice-cancel","request":"a1b2…"}
```

`request` is a client-chosen token, at most 128 bytes, that ties the job to the
`voice` events answering it. The daemon keys jobs on the connection as well, so
it need only be unique among one client's own outstanding requests, and one
client cannot cancel another's job.

All three reply `ok` as soon as the job *starts*. Everything that matters —
download progress, the transcript, failures — arrives as `voice` events, because
downloading 574 MB or running whisper takes far longer than a request may.

`audio` is a base64 WAV of 16 kHz mono 16-bit little-endian PCM, at most 64 MiB
decoded. The daemon does not resample: any other rate or channel count is
rejected. `src/xd/voice/data.cr` writes and validates this header.

### Folder and chat mutations

`new-folder`, `rename-folder`, `move-folder`, `trash-folder`, `rename-chat`,
`delete-chat`, `set-folder-context`, and `set-folder-settings` all reply with
`ok` alone and broadcast a `tree` event.

## Events

Chat-scoped events carry a `chat` member; ignore those for other chats.

| Event | Members | Meaning |
| --- | --- | --- |
| `tree` | — | Workspace tree changed; refetch `tree`. |
| `changed` | `chat` | Stored chat state changed; refetch when idle. |
| `queued` | `chat`, `queue`, `text` | Queue replaced. `text` is the first item. |
| `commands` | `chat`, `backend`, `commands` | Slash-command list for a backend. |
| `turn-started` | `chat`, `label`, `turn_id`, `turn_sequence` | A turn began. |
| `text` | `chat`, `text`, `turn_id`, `turn_sequence` | Assistant text delta. |
| `tool` | `chat`, `text`, `workdir`, `context`, `turn_id`, `turn_sequence` | Tool row. |
| `turn-finished` | `chat`, `turn_id`, `turn_sequence`, `ok`, `waiting`, `silent`, `duration`, `last_message_id`, optional `error`, `question`, `options`, `accepts_input` | The turn ended. |
| `repository-changed` | `chat` | HEAD moved outside the app. |
| `git-state`, `git-action-finished`, `git-draft-finished` | `chat`, … | Git pane state. |
| `terminal-output`, `terminal-closed` | `terminal`, … | Shared pty; output is base64. |
| `voice` | `request`, `state` of `downloading` (with `progress`, `-1` until the size is known), `ready`, `transcribed` (with `text`), `cancelled`, or `error` (with `error`) | Voice job progress, addressed to the connection that asked and naming no chat. |
| `auth`, `agent-auth-changed` | … | Assistant authentication state. |

### Tagged questions

Both bundled assistants run non-interactively, so a question is a block in the
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
blocks, which the daemon strips before storage. That is deliberate: it lets
every client render its own buttons, and lets a client that reopens a chat find
the question without having seen the event. A client must therefore strip these
blocks before rendering, or show raw tags. `src/xd/agent/ask.cr` is the
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
   action — the token is remote code execution on the daemon machine.
2. Keep reading the socket at all times, or be disconnected at 256 queued
   events.
3. Re-snapshot on reconnect. There is no event replay.
4. Deduplicate live turn output by turn watermark, never by arrival order.
5. Treat a refused `hello` and a certificate mismatch as terminal: stop
   retrying and require explicit re-pairing.
