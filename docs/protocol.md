# Remote wire protocol

This document is the normative contract between `xd serve` and every remote
client. The C server in `src/remote/server.c` is the current implementation.
Protocol version 1 is reported by a successful `hello`.

Any wire change must update `tests/test-remote.c` and the Kotlin `commonTest`
fixtures together. Server and clients must ignore unknown object members unless
a later protocol version explicitly says otherwise.

## Transport and framing

- TCP port 4001 by default.
- TLS begins immediately after TCP connection.
- The daemon uses a self-signed certificate. During pairing, the client accepts
  the presented leaf certificate and persists it with the issued token. Every
  later connection must require that exact leaf certificate.
- Application data is UTF-8 JSON Lines: one JSON object followed by `LF`
  (`0x0a`). Empty lines are ignored.
- Requests, replies, and events share one ordered, full-duplex connection.
- The daemon closes a slow connection before its queued outbound data reaches
  32 MiB.

JSON member absence and JSON `null` are different. Unless an operation says
otherwise, an optional member is omitted rather than sent as `null`.

## Requests, replies, and events

Every request is an object with a string `op` member:

```json
{"op":"ping"}
```

Every reply is an object with boolean `ok`. Success payloads add operation-
specific members. Errors have this shape:

```json
{"ok":false,"error":"Human-readable reason"}
```

There are no request ids. Non-event replies match requests strictly by
position. A client may pipeline requests, but it must keep a FIFO slot for
every request written. Cancelling a caller must not remove that slot: the
reply still has to be read and discarded or every later reply becomes
misassociated.

An object is an event when it contains an `event` member. Events may appear
between a request and its reply and do not consume a reply slot:

```json
{"event":"tree"}
```

Unknown events are ignored. A non-event reply received with no pending request
is ignored. Malformed JSON, a non-object line, or invalid required reply data
is a protocol error and clients should reconnect only when doing so can help.

## Authentication

`pair` and `hello` are greetings. All other operations require an authenticated
connection.

### Pair

Pairing is armed by starting `xd serve --pair`. Its displayed `XXXX-XXXX` code
is valid for five minutes and one matching submission.

```json
{"op":"pair","code":"4F2K-9QX1","name":"Pixel"}
{"ok":true,"token":"base64 device token"}
```

`code` and non-empty device `name` are required strings. A successful pair:

- authenticates the existing connection; do not send `hello` afterward;
- consumes the code;
- returns a random, long-lived bearer token;
- requires the client to persist token and pinned certificate atomically.

Pairing failure is terminal for that attempt. A valid code is consumed before
the device record is written, so even a storage failure requires a new code.

### Hello

Every later connection sends `hello` as its first request:

```json
{"op":"hello","token":"base64 device token"}
{"ok":true,"device":"Pixel","version":1}
```

An unknown or revoked token returns `ok:false`. Clients must stop retrying and
offer pairing again. A certificate mismatch must also stop retries: continuing
would disclose the bearer token to a different peer.

## State reads

### `tree`

Request:

```json
{"op":"tree"}
```

Success:

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

Request:

```json
{"op":"chat","chat":"chat-1"}
```

Success always includes:

- `ok: true`
- `title: string`
- `backend: string`
- `plan: boolean`
- `queue: string[]`, oldest first
- `working: boolean`
- `new_worktree: boolean`
- `has_messages: boolean`

Optional members:

- `commands: string[]`
- `queued: string`, compatibility alias for the first queue item
- `model`, `effort`, `access`: strings
- `context_used`, `context_window`: integer token counts
- `workdir: string`
- `linked_worktree: boolean`, present with `workdir`
- `worktrees: Worktree[]`

A worktree is:

```json
{
  "path":"/work/project",
  "branch":"main",
  "detached":false,
  "main":true,
  "current":true
}
```

`branch` is absent for a detached worktree.

When `working` is true, the reply also contains the live turn snapshot:

- `label: string`
- `working_for: integer`, elapsed seconds
- `items: LiveItem[]`, where each item has `tool: boolean` and `text: string`
- `segment: string`, optional unfinished assistant text

`items` are ordered. A client renders each non-tool item as finished assistant
text and each tool item as a tool row, then appends `segment`.

Reading a chat may start its first persisted queued message when no turn is
running.

### `messages`

```json
{"op":"messages","chat":"chat-1","limit":150}
```

`chat` is required. Positive `limit` returns at most that many recent messages;
zero or omission returns all messages when no turn is active.

```json
{
  "ok":true,
  "total_messages":2,
  "last_message_id":42,
  "messages":[
    {"role":"user","content":"Hello","at":1753700000},
    {
      "role":"assistant",
      "content":"Hi",
      "at":1753700001,
      "label":"Codex"
    }
  ]
}
```

Messages are oldest-to-newest within the returned slice. `at` is Unix time in
seconds. `label` is optional. During a turn, the response is bounded at the
turn's persisted transcript id; live items come from `chat` and events.
`last_message_id` identifies that boundary, not an event cursor.

### Other reads

| Operation | Request members | Success members and rules |
|---|---|---|
| `folder-context` | `folder: string` | `context: string \| null` |
| `list-dir` | optional `path: string` | `path: string`, `entries: string[]`; defaults to daemon home and lists non-hidden directories |
| `file-browse` | `chat`, `action`, optional relative `path` | `action:"list"` returns `entries:[{name,directory}]`; `action:"read"` returns UTF-8 `content` for a regular file no larger than 1 MiB |
| `agent-secrets` | none | `names: string[]`; values never cross the wire |
| `image-read` | absolute daemon `path`, optional `preview: boolean` | `mime:"image/png"`, base64 `data`; only daemon-created remote pastes |
| `diff-read` | described below | `output: string`, limited to 8 MiB |
| `ping` | none | no members beyond `ok` |

`file-browse` paths are relative to the chat workdir and may not escape it.
Hidden entries are omitted. Directories sort before files.

`diff-read` requires `chat` and one `read` value:

| `read` | Extra request members |
|---|---|
| `base` | none |
| `working-status` | none |
| `branch-status` | `base` |
| `working-all` | none |
| `branch-all` | `base` |
| `working-file` | relative `path` |
| `untracked-file` | relative `path` |
| `branch-file` | `base`, relative `path` |

## Tree and chat mutations

Mutation success is `{"ok":true}`. Creation operations also return string
`id`. Tree mutations normally produce a later `tree` event.

| Operation | Request members | Notes |
|---|---|---|
| `new-folder` | `name`, optional `parent` | omitted parent means workspace root; returns folder `id` |
| `rename-folder` | `folder`, `name` | folder names cannot be empty, hidden, or contain a separator |
| `move-folder` | `folder`, optional `parent` | omitted parent means root; cannot move inside itself |
| `trash-folder` | `folder` | moves to host trash rather than deleting permanently |
| `set-folder-context` | `folder`, `context: string \| null` | text is trimmed; empty text stores null |
| `new-chat` | `folder`, optional `title`, optional `workdir` | returns chat `id`; backend settings resolve on daemon |
| `rename-chat` | `chat`, non-empty `title` | broadcasts `tree` |
| `delete-chat` | `chat` | closes that chat's terminals and broadcasts `tree` |
| `set-option` | `chat`, `option`, optional string `value` | broadcasts chat `changed` |

`set-option.option` is one of:

- `model`
- `effort`
- `access`
- `plan`
- `backend`
- `new-worktree`
- `workspace`

`value` is always a JSON string, including booleans: `"true"` or `"false"`.
Sending JSON booleans is not equivalent and is read as an absent value.

`set-agent-secrets` has this shape:

```json
{
  "op":"set-agent-secrets",
  "entries":[
    {"name":"OPENAI_API_KEY"},
    {"name":"NEW_SECRET","value":"secret value"}
  ]
}
```

The array is the complete desired set. Omitted `value` preserves an existing
secret. A new secret requires a non-empty string value. Names must be unique,
valid environment variable names.

## Turns and queues

### `send`

```json
{"op":"send","chat":"chat-1","text":"Continue"}
```

`chat` is required. `text` may be empty only when attachments exist. When no
turn is active, this starts one. When a turn is active or the daemon is
quiescing, the same request appends to the persistent queue. Both paths return
the same `{"ok":true}`; clients learn the chosen path from `turn-started` or
`queued`.

Optional image attachments:

```json
{
  "op":"send",
  "chat":"chat-1",
  "text":"Inspect this",
  "attachments":[
    {"mime":"image/png","data":"base64 PNG bytes"}
  ]
}
```

When present, `attachments` contains 1 to 4 PNGs. Each decoded image is at most
10 MiB; decoded total is at most 20 MiB. Validate encoded length before base64
decoding. Supplied filenames are not part of the wire contract.

### Queue control

```json
{"op":"queue","chat":"chat-1","text":"Then run tests"}
{"op":"drop-queue","chat":"chat-1","index":0}
{"op":"drop-queue","chat":"chat-1"}
{"op":"cancel","chat":"chat-1"}
```

`queue` appends non-empty text. `drop-queue` removes one zero-based position
when `index` exists, otherwise clears the whole queue. `cancel` stops an active
turn; when none is active, it attempts to start the first queued message.

## Terminals

Terminal sessions belong to the daemon and are shared by all devices.

| Operation | Request members | Success |
|---|---|---|
| `terminal-list` | `chat` | `terminals: Terminal[]` |
| `terminal-open` | `chat`, optional `columns`, `rows`, `reuse` | `id`; dimensions default to 80x24 and clamp to 1..1000 |
| `terminal-input` | `terminal`, base64 `data` | done |
| `terminal-resize` | `terminal`, optional `columns`, `rows` | done, then `terminal-resized` event |
| `terminal-kill` | `terminal` | idempotent done |

A terminal snapshot is:

```json
{
  "id":"terminal-1",
  "title":"shell",
  "columns":80,
  "rows":24,
  "replay":[
    {"data":"base64 terminal bytes"},
    {"columns":120,"rows":40}
  ]
}
```

Replay items are ordered and contain either output `data` or a geometry pair.
Apply them from the listed initial dimensions in order.

## Events

Events broadcast to every authenticated connection. Most chat events contain
`chat`; `tree` and daemon-wide `changed` do not.

| Event | Members | Meaning |
|---|---|---|
| `tree` | none | reload complete tree snapshot |
| `changed` | optional `chat` | reload named chat/messages, or all open chats when absent |
| `commands` | `chat`, `backend`, `commands:string[]` | replace backend command suggestions |
| `turn-started` | `chat`, `label` | turn began; reload messages to include user input |
| `text` | `chat`, `text` | append assistant text delta |
| `tool` | `chat`, `text` | finish current text segment, then append tool row |
| `turn-finished` | `chat`, `ok`, `waiting`, optional `error` | discard live state and reload authoritative messages |
| `queued` | `chat`, `queue:string[]`, optional `text` | replace full queue; `text` is oldest item |
| `terminal-opened` | `chat`, `terminal`, `title`, `columns`, `rows` | shared terminal created |
| `terminal-output` | `chat`, `terminal`, base64 `data` | ordered terminal bytes |
| `terminal-resized` | `chat`, `terminal`, `columns`, `rows` | canonical terminal geometry changed |
| `terminal-closed` | `chat`, `terminal` | shared terminal ended |

`text` values are deltas, not replacements. `queued.queue` is a replacement,
not an append. Events can precede the reply that caused them; `send` commonly
broadcasts `turn-started` before its success reply.

## Reconnection and synchronization

Events have no ids, are not persisted, and are not replayed. There is no resume
cursor. After every successful `hello`, including reconnects, clients must:

1. read `tree`;
2. read `chat` and `messages` for every open chat;
3. read `terminal-list` for every open terminal pane;
4. continue applying live events.

This full refresh is normal mobile behavior, not an exceptional recovery.
Clients must not claim exact resume from a last event id.
