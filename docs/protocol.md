# xd host wire protocol

This is the normative JSON Lines contract served by the persistent xd daemon.
The local desktop uses a private Unix socket through `xd-host stdio`; remote
desktop and Android use the same frames over pinned TLS.

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

Local connections authenticate through the user-owned 0600 Unix socket in a
0700 runtime directory. Remote connections use pinned TLS and must begin with
`pair` or `hello` before any workspace request or event is allowed.

A local `peer-pairing` request starts the remote listener and returns `host`,
`port`, `code`, `fingerprint`, and `expires_in:300`. The pairing code clients
copy is `xd2:<code>:<pin>`, where `pin` is the certificate's SHA-256 in unpadded
base64url. It holds no address: the person pairing enters a reachable `host` or
`host:port`, and the port defaults to 4001. Clients remove whitespace from a
pasted code before reading it. The code is one-use, expires in five minutes, and
accepts at most ten guesses.

```json
{"op":"pair","code":"ABCD-EFGH","name":"Laptop"}
{"ok":true,"token":"device-secret","device":"Laptop"}
{"op":"hello","token":"device-secret"}
{"ok":true,"device":"Laptop","version":1}
```

The client verifies the pairing code's SHA-256 leaf-certificate fingerprint before
sending either secret. Authentication replies precede event fan-out. Tokens are
stored hashed on the host. `devices`, `rename-device`, `revoke-device`,
`peer-pairing`, and `daemon-stop` are local-only. Revocation disconnects every
session for that device immediately. A revoked token or changed certificate
requires explicit re-pairing.

`daemon-info` returns protocol/build identity, PID, and the installed host binary
that an update restart re-executes (`executable`, null outside an installed
bundle). `daemon-stop` acknowledges
before shutting down the shared runtime. Remote listeners persist across daemon
restarts, but unused codes and pending backend requests do not.

## State reads

### `tree`

```json
{"op":"tree"}
```

```json
{
  "ok": true,
  "folders": [
    {"id":"folder-1","name":"Project","owner":"RestartFU"},
    {"id":"folder-2","name":"Child","parent":"folder-1"}
  ],
  "chats": [
    {
      "id":"chat-1",
      "folder":"folder-1",
      "title":"Fix mobile layout",
      "backend":"codex",
      "working":false,
      "session":"session-1"
    },
    {
      "id":"chat-2",
      "folder":"folder-1",
      "title":"Review changes",
      "backend":"claude",
      "working":false,
      "session":"session-1"
    }
  ],
  "sessions": [
    {
      "id":"session-1",
      "folder":"folder-1",
      "title":"Layout task",
      "chats":["chat-1","chat-2"],
      "working":false,
      "terminal_working":false,
      "terminal_chat":"session:session-1"
    }
  ]
}
```

`parent` is absent for root folders. Arrays are complete snapshots. Clients
must tolerate a child appearing before its parent and treat ids as opaque.

`owner` is the GitHub organization or user behind the folder's `origin` remote,
read from the folder's working directory on the host and absent when it has no
GitHub remote. It names the account whose avatar a client fetches with
`owner-icon` to stand in for a folder icon; it is not an identity or an
authorization, and a client that ignores it loses nothing but the avatar.

A folder contains persistent workspace sessions, each containing independent
chats. Each chat's `session` names its session; the matching `sessions` entry
provides its title, folder, ordered chat membership, and stable `terminal_chat`
resource. `branch` is present when known. Session `working` and
`terminal_working` aggregate activity across its members. A session's identity
and title survive deletion of its first chat while other members remain.

The `sessions` array and chat `session` members are additive. Existing chats
migrate into separate singleton sessions without changing their chat ids,
transcripts, drafts, options, or backend resume state. Existing clients may
continue reading `folders` and `chats`; clients reading an older host without
`sessions` may display one session per chat and retain its chat id as the
terminal owner.

A host that can settle sessions adds a top-level `"settlement":true`; hide
settling on one that does not. A settled session has `settled_at`, in seconds,
on its `sessions` entry and on each of its chats, and a session kept out of
automatic settling has `auto_settle_disabled:true` there. Both are absent
otherwise. Settling changes where a session is listed and nothing else: its
chats still answer `chat` and `messages` and can still be opened. List settled
sessions apart from active ones, most recently settled first. Clients that
ignore these members keep listing settled sessions with the active ones.

### `chat`

```json
{"op":"chat","chat":"chat-1"}
```

Success always includes `ok`, `title`, `backend`, `session_mode`, `auth_state`, `commands`,
`plan`, `queue` (oldest first), `working`, `resumable`, `effort`, `new_worktree`, and
`has_messages`. `resumable` is true when the chat's last turn was cut short: stopped,
interrupted by the host going down, or ended by a usage limit. It clears when any
turn starts. It also includes `session`, `session_title`, and
`terminal_chat` for its shared workspace session. The chat's `title` belongs to
that conversation; the session title is managed independently.

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
- `turn_epoch`: string identifying the current daemon runtime
- `turn_id`, `turn_sequence`: integers — the turn watermark
- `working_for`: elapsed seconds
- `items`: ordered array of `{"tool":bool,"text":string}`; record rows also
  carry `record` and `message_id`, and hold the record's newest text
- `segment`: unfinished assistant text

A client renders each non-tool item as finished assistant text, each tool item
as a tool row, then appends `segment`.

**Turn watermark.** A new `turn_epoch` resets the client watermark. `turn_sequence` counts the `text` and `tool` events already
folded into this snapshot. A later event for the same `turn_id` whose
`turn_sequence` is less than or equal to the snapshot's is already represented
and must be dropped; a higher one must be applied. An event carrying a lower
`turn_id` is stale. Given the ordering caveat above, this is the only correct
way to reconcile a snapshot against concurrent live output.

`session_mode` is `chat` for Codex/Claude/JCode sessions. Tree chat summaries
include the same member. The host deletes legacy `terminal` chats and their
history when opening the database, retaining their worktrees. `terminal-open`
continues to open shell panes; `terminal-open-agent` is no longer supported.

Snapshots also include `interactions`, `interaction_revision`, and
`interaction_epoch`. Each interaction is `{request_id, method, params}` from
the backend, with an opaque host-scoped request ID. Reconcile snapshots/events
by revision within an epoch; a different epoch starts a new revision history.
A client must clear pending prompts when it observes a new daemon epoch.

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
| `new-chat-sources` | `folder: string` | `branches:[{name,reference,oid}]`, `pull_requests:[{number,title,oid}]`, optional `pull_request_error`; branch discovery works without GitHub CLI |
| `shortcuts` | optional `folder: string` | host-wide `global`, folder-owned `workspace`, and merged `effective` prompt arrays |
| `chat-title-settings` | none | `enabled: boolean`, `models: {codex, claude}`, each the model that will title that assistant's chats, or `null` if its CLI offers none of the choices |
| `list-dir` | optional `path: string` | `path`, `entries: string[]`; defaults to host home, lists non-hidden directories |
| `file-browse` | `chat`, `action`, optional relative `path`, `content`, and `original` | `action:"list"` returns `entries:[{name,directory}]`; `action:"read"` returns UTF-8 `content` for a regular file no larger than 1 MiB; `action:"write"` saves bounded UTF-8 text and rejects a stale optional `original` |
| `agent-secrets` | none | `names: string[]`; values never cross the wire |
| `agent-clis` | none | detected assistant versions |
| `host-update` | optional `action` of `status`, `check`, `install`, `restart` | `version`, `channel`, `state`, `supported`, `available`, optional `latest` and `error` |
| `agent-cli-updates` | optional `action` of `status`, `check`, `install`; `install` requires `provider` | `providers:[{provider,display_name,state,available,updatable, optional version, latest, detail}]`; `state` is `idle`, `updating`, `updated`, or `failed`; each change is also sent as an `agent-cli-update` event holding one provider |
| `resume-interrupted` | `resume: boolean` | `resumed: string[]`. Answers for every chat cut off since the last answer, which wait in storage until then. Local and authenticated remote clients may answer, and the first answer settles them: `true` sends a visible continuation message to each, unless the chat has moved on, and `false` discards them |
| `agent-catalog` | none | `backends: [{id, name, default_model, models:[{id,name,context_window}], efforts:[string]}]` |
| `owner-icon` | `owner: string` GitHub login from a folder's `owner` | `owner`, and either `mime` of `image/png` or `image/jpeg` with base64 `data`, `pending: true` while the host fetches, `retry: true` when the last fetch failed, or none of these when the account has no avatar; an uncached avatar is answered as pending and sent to the asking connection as an `owner-icon` event |
| `workflow-status` | `text: string` captured `workflow_run` marker | `name`, `state`, optional `conclusion`, `jobs:[{id,name,state, optional conclusion, optional log}]`; fetches GitHub Actions status on the host |
| `usage-summary` | `days` of `1`, `7`, `30`, or `90`, optional IANA `time_zone`, optional `now_ms`, optional `refresh: boolean`, optional `generation` | `pending: true`; the asking connection then receives a `usage-summary` event with `generation` and either `error` or `summary:{read_at_ms,resolution,window_label,periods,buckets,sessions,sources,pricing}`, read from the host's Claude Code and Codex transcripts and priced from a daily LiteLLM price list |
| `usage-limits` | optional IANA `time_zone`, optional `generation` | `pending: true`; the asking connection then receives a `usage-limits` event with `generation`, `host_now_ms`, and `accounts:[{provider,account_key,plan,checked_at_ms,windows,reset_credits,unavailable}]`, asked of each installed assistant; `account_key` is a hash, so no address crosses the wire |
| `usage-prices` | none | saved `prices:{model:{input,output,cache_read,cache_write}}` in USD per million tokens, and `aliases:{model:target}` |
| `image-read` | absolute host `path`, optional `preview: boolean` | `mime` of `image/png`, `image/jpeg`, `image/gif`, or `image/webp`, and base64 `data`; only images the host stored itself: remote pastes and [images in replies](#images-in-replies) |
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
{"op":"new-chat","folder":"folder-1","title":"Optional","worktree":{"kind":"branch","ref":"refs/heads/feature","oid":"0123456789abcdef0123456789abcdef01234567"}}
{"ok":true,"id":"chat-9","backend":"codex","session":"session-9","terminal_chat":"session:session-9"}
```

Without `session`, `folder` is required and the operation creates a new session
containing the new chat. `title` defaults to `New Chat` and also supplies the
initial session title. Optional `backend` selects the chat's assistant;
otherwise the folder and host defaults apply. Without `backend`, the chat starts
on the folder's model if it has one; otherwise it takes the model last chosen
with `set-option` for that assistant, else the assistant's own default. Its effort is
the one last chosen for that assistant, else the default the assistant reports
for the chat's model, else `high`. A chat's first `send` waits for model discovery if
either value is unset, and moves a chat on a model the assistant no longer
offers to its default. `worktree` may select `new`, an
`existing` path, a full local or remote-tracking branch ref plus its listed
commit, or a pull request number plus its listed head commit. Branch and pull
request selections create an isolated managed `xd/*` worktree and are
revalidated before checkout. The legacy `workdir` field remains supported when
`worktree` is omitted.

To add an independent chat to an existing session:

```json
{"op":"new-chat","session":"session-9","title":"Review","backend":"claude"}
{"ok":true,"id":"chat-10","backend":"claude","session":"session-9","terminal_chat":"session:session-9"}
```

Here `folder` is optional; if supplied, it must match the session's folder.
`workdir` and worktree selection are refused because the new chat shares the
session's current checkout and branch. Joining creates no additional worktree.
Agent options, drafts, queues, history, backend resume state, and approvals
remain scoped to the returned chat `id`.

### Session mutations

```json
{"op":"rename-session","session":"session-9","title":"Review layout"}
{"op":"move-session","session":"session-9","folder":"folder-2"}
{"op":"move-session","session":"session-9","before":"session-3"}
{"op":"delete-session","session":"session-9"}
```

Each operation requires a session id, replies with `ok`, and broadcasts `tree`.
`rename-session` requires a nonempty title. `move-session` takes either a target
`folder`, `before`, or `after`; ordering anchors are session ids. Moving keeps
all member chats together and preserves their shared checkout.

`delete-session` deletes every member chat and its conversation state, stops
its agents, and closes its shared terminals. It retains checkout files and
worktrees. `delete-chat` with an actual chat id removes only that conversation;
its reply includes `session` and `session_deleted`. Shared terminals close only
when the final member is deleted. `rename-chat` changes only the conversation
title. Hiding a client tab does not require either deletion operation.

### Settling sessions

```json
{"op":"settle-session","session":"session-9"}
{"op":"unsettle-session","session":"session-9"}
{"op":"set-session-auto-settle","session":"session-9","enabled":false}
```

Each operation requires a session id, replies with `ok`, and broadcasts `tree`.

`settle-session` moves finished work out of the active list without deleting it.
It is refused while any member chat is running a turn, has a queued message, or
is waiting on an approval or question. Settling a settled session keeps its
original `settled_at`. No agent process stops and no terminal closes.

`unsettle-session` returns a session to the active list and keeps the automatic
rules away from it until new activity. `set-session-auto-settle` with
`enabled:false` keeps a session out of the rules for good; settling by hand
still works. A message sent to any member chat, or a turn the agent starts on its
own, un-settles the session and hands it back to the rules.

```json
{"op":"auto-settle-settings"}
{"ok":true,"after_days":3,"on_merge":true,"keep_pull_requests":false}
{"op":"set-auto-settle-settings","after_days":null,"keep_pull_requests":true}
```

The daemon applies these rules about once a minute, whether or not a client is
connected:

- `after_days`, from 1 to 90, settles a session after that many days without
  activity. Here `null` is meaningful: it turns this rule off.
- `on_merge` settles a session when the pull request of its checked-out branch
  merges. A closed pull request settles it whenever either of these two rules is
  on; with both off the host settles nothing by itself.
- `keep_pull_requests` stops inactivity from settling a session that has a pull
  request. Merging and closing still do.

A pull request that finished before the user's latest message in the session
does not count. The rules skip sessions the operations above would refuse and
sessions the user un-settled. `settled_at` is then when the session went quiet,
not when the sweep ran. `set-auto-settle-settings` changes only the members it
names, broadcasts `auto-settle-settings`, and applies at once.

### Shared session terminals

Use the session's `terminal_chat` value as the `chat` parameter for
`terminal-list` and `terminal-open`:

```json
{"op":"terminal-open","chat":"session:session-9","reuse":true}
```

This resource remains valid while any chat in the session remains, including
when the initial chat has been deleted. Terminal ids returned by these
operations are used unchanged for input, resize, and kill requests. Terminal
activity events use the same shared resource in their `chat` member. Clients
should associate that activity with the session's current members. Existing
terminal requests using actual chat ids resolve the same session resources.
Previously opened terminals are adopted without restarting their processes or
losing replay output. For existing clients, terminal events may also carry an
observed legacy chat owner; repeated frames retain the same terminal id and
sequence and must not duplicate output. Agent, approval, file, and Git requests
continue to use an actual chat id; file and Git operations resolve its shared
session workspace.

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

Supply `chat` plus either a target `folder`, `before`, or `after`; ordering
anchors here are actual chat ids. The target folder must exist. Moving between
folders moves the chat's entire workspace session, preserving its checkout,
chat ids, and transcripts. The destination supplies inherited folder settings.
Ordering around a chat in another session reorders the whole source session;
ordering within the same session reorders its chats. The host broadcasts a
`tree` event after a successful move.

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

### `resume-turn`

```json
{"op":"resume-turn","chat":"chat-1"}
{"ok":true,"queued":false}
```

Continues a cut-short turn by sending `Continue where you left off.` as a visible
user message. Fails when the chat is not `resumable` or has moved on since, so
clients pressing it together start one turn. Anything queued runs after that turn.

### `respond`

```json
{"op":"respond","chat":"chat-1","request_id":"opaque-id","response":{"decision":"accept"}}
{"op":"respond","chat":"chat-1","request_id":"opaque-id","response":{"answers":{"question-id":{"answers":["Keep it"]}}}}
```

Responds to a pending Codex app-server or Claude Code approval or question. Approval decisions
are `accept`, `acceptForSession`, `decline`, or `cancel`. Permission decisions
grant only the permissions requested by the backend. Question responses must
answer every question in the request. Stale, already answered, or foreign-chat
IDs are refused. Cancellation clears pending requests.

`interaction` publishes `{chat,request_id,method,params,interaction_revision,
interaction_epoch}`. `interaction-resolved` removes one ID; `interactions-cleared`
removes all prompts for the chat. Both carry the revision and epoch. Pending
requests survive client disconnects and are available from `chat` snapshots.

### `subagent`

```json
{"op":"subagent","chat":"chat-1","key":"toolu_01","action":"stop"}
{"op":"subagent","chat":"chat-1","key":"toolu_01","action":"background"}
```

Controls a running Claude Code subagent. `key` is the `key` from its
`subagent_run` record. `stop` ends the agent; `background` lets the turn carry
on without waiting for it. The host refuses agents that have finished, that
are not Claude's, or that Claude has not reported a task for yet. A control
operation: answered even while a serialized command is slow. The agent's
record reports the outcome.

### Subagent records

Each delegated agent is one `tool` message whose content is `subagent_run\n`
followed by a JSON object. The host rewrites that message in place as the
agent works rather than appending new rows, so a stored transcript holds only
the newest snapshot.

```json
{"key":"toolu_01","provider":"Claude","agent":"Explore","model":"claude-haiku-5-5",
 "description":"Map the parser","prompt":"…","status":"running","background":false,
 "activity":"Reading runtime.rs","steps":[{"tool":"Read","text":"host/src/runtime.rs"}],
 "tool_uses":3,"tokens":9100,"duration_ms":23000,"started_at":1753700000000,
 "latest":"…","summary":"…"}
```

`status` is `running`, `completed`, `failed`, or `stopped`. `running` holds only
while the process reporting on the agent lives: the host marks the record
`stopped` when that process ends, and on its next start if it died first.
`steps` keeps the newest 40 tool calls; `tool_uses` counts all of them. `started_at` is Unix
milliseconds, for a live elapsed time. `latest` is the agent's newest message
and `summary` its final report. `model`, `prompt`, `activity`, `latest`,
`summary`, `tokens`, `duration_ms`, and `started_at` are absent until known;
ignore unknown members.

### `set-option`

```json
{"op":"set-option","chat":"chat-1","option":"effort","value":"high"}
```

Options: `model`, `effort`, `access`, `plan`, `fast`, `backend`,
`new-worktree`, `workspace`. Boolean options take `"true"`/`"false"`.
`fast` is Codex-only. An unknown option is an error.

`new-worktree` and `workspace` change the shared session workspace atomically
for every member chat. They are allowed only before any member has a user
message or running turn. A pending new worktree is created once on the first
member's send; subsequent chats share it. Other agent options remain per chat.

Selecting a model atomically requires sending `backend` alongside `value`;
without `backend` only the model string is stored. The atomic form validates
both and clears an unsupported effort. Changing the model adds nothing to the
transcript.

Each `model` or `effort` selection is also remembered for that chat's assistant,
separately from every other assistant, and new chats on it start from there.

`host-update` updates the host's own installation, so a paired device can
bring a machine forward without a shell on it. `install` replaces the files,
which is safe while turns run because the process keeps the binary it already
mapped; `restart` drops every connection and loses any running turn, so the
two are separate actions and neither happens on its own. Native Linux bundles
and macOS apps support this flow. A dev host checks the `master` branch every
minute instead of a release and refuses `install`, because nothing is published
for it; the desktop builds that commit from source. `supported` is false where the installation
cannot replace itself, such as source builds, and both `install` and `restart`
are refused there.

`agent-cli-updates` covers the assistant CLIs a user installed themselves,
currently Codex and Claude Code. `available` compares the installed version
with the newest release of the CLI's npm package, and `install` runs the CLI's
own `update` command so it can follow however the CLI was installed. `updatable`
is false when the installed release's help lists no such command; `install` is
then refused and the update has to be done by hand. An updater
that exits cleanly without changing the version is reported as `failed` and that
release is no longer offered. `check` runs in the background and answers with the
last known state; changes arrive as events. Restarting the host is refused while
an update runs.

`agent-catalog` lists the assistants and models this host can run. The
desktop reads its own compiled-in catalog because it ships with the host; a
separately released client must ask, since `set-option` validates the model id
and a hard-coded list would be refused as soon as one is added or retired.

### `set-usage-prices`

Replaces the host's custom model prices and aliases. Takes `prices` and `aliases`
in the shape `usage-prices` returns; `input` and `output` are required, and a blank
cache rate uses the input rate. Answers with the saved values.

### `usage-consume-reset-credit`

Spends one banked reset credit on the host's signed-in account. Takes `provider`
(`codex`), an `idempotency_key` naming the attempt, which is reused when retrying
it, and the `account_key` from `usage-limits`; the host refuses if that is missing
or no longer the signed-in account. Answers `pending: true`, then sends the asking connection a `usage-reset-credit`
event with `outcome` of `reset`, `nothingToReset`, `noCredit`, or `alreadyRedeemed`
and the `idempotency_key` it settles, or `error`. This cannot be undone, so clients must confirm first.

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
worktree reported by `selected_worktree` may be removed, and no chat in the
session may have a user message or running turn. The target must be a clean,
registered linked worktree outside the repository's main checkout, and no other
session may reference it. The host removes the checkout without force and
keeps the Git branch, then returns all sibling chats to their original
checkout. A successful request replies with `ok` alone.

### Folder and chat mutations

`new-folder`, `rename-folder`, `move-folder`, `trash-folder`, `rename-chat`,
`move-chat`, `set-folder-context`, and `set-folder-settings` acknowledge with
`ok` and broadcast a `tree` event. `new-folder` also returns its id;
`delete-chat` additionally returns the session deletion fields described above.

`set-chat-title-settings` takes `enabled: boolean`, or `backend` of `codex` or
`claude` with a `model` to choose or `null` to go back to the built-in choice, or
both. It replies like `chat-title-settings`. The settings apply to the whole host.

While titles are enabled, a chat's first message also names it. The host asks a
low-cost model in the background, `gpt-6-luna` for Codex and `claude-haiku-5-5`
for Claude unless another was chosen, and then publishes `tree` and `changed`. The
title replaces only a placeholder name such as `New Chat`, on the chat and on its
session; a name given in the meantime stays. A failed attempt leaves the name as it
was. JCode chats are not titled.

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
| `auto-settle-settings` | `after_days`, `on_merge`, `keep_pull_requests` | The host's automatic settling rules changed. |
| `changed` | `chat` | Stored chat state changed; refetch when idle. |
| `worktrees-changed` | — | A Git worktree changed; refetch chat state and worktree lists. |
| `shortcuts-changed` | optional `folder` | Global or workspace prompt shortcuts changed; refetch chat state. |
| `queued` | `chat`, `queue`, `text` | Queue replaced. `text` is the first item. |
| `draft` | `chat`, `draft`, `draft_revision`, optional `draft_attachments` | Composer text changed; attachments are present only when replaced. |
| `commands` | `chat`, `backend`, `commands` | Slash-command list for a backend. |
| `turn-started` | `chat`, `label`, `turn_id`, `turn_sequence` | A turn began. |
| `text` | `chat`, `text`, `turn_id`, `turn_sequence` | Assistant text delta. |
| `tool` | `chat`, `text`, `workdir`, `context`, `turn_id`, `turn_sequence`, optional `record`, `message_id` | Tool row. `record` marks a row the host will rewrite; `message_id` is its stored id. |
| `record` | `chat`, `text`, `record`, `message_id`, optional `turn_id`, `turn_sequence` | A record row was rewritten, such as a subagent's progress; replace the row with that `message_id`. Arrives without turn members when a background agent reports while no turn is running. |
| `turn-finished` | `chat`, `turn_id`, `turn_sequence`, `ok`, `waiting`, `silent`, `duration`, `last_message_id`, optional `error`, `question`, `options`, `accepts_input` | The turn ended. |
| `repository-changed` | `chat` | HEAD moved outside the app. |
| `git-state`, `git-action-finished`, `git-draft-finished` | `chat`, … | Git pane state. |
| `terminal-output`, `terminal-closed` | `terminal`, … | Shared pty; output is base64. |
| `auth`, `agent-auth-changed` | … | Assistant authentication state. |
| `owner-icon` | `owner`, optional `mime` and base64 `data`, optional `retry` | A pending `owner-icon` fetch finished. Without `mime` and `data` the account has no avatar; keep the folder icon. With `retry` the fetch failed instead, so ask again later; the host answers from a short cooldown rather than refetching each time. |

### Images in replies

An assistant shows the user an image by putting a marker on a line of its own,
outside any code fence:

```
[image: /absolute/path/to/shot.png]
```

Assistants are told this in their instructions. A relative path starts from the
chat's working directory. The host handles the marker before the text is stored
or sent, and a line that could still become one is withheld until it ends, so no
client ever sees the original path:

- A PNG, JPEG, GIF, or WebP file of at most 10 MiB is copied into the host's
  private image store and the marker is rewritten to point at the copy, which is
  the path to pass to `image-read`. The copy remains after the original is gone,
  and an identical image shares it. Clients show the first frame of a GIF.
- Anything else becomes the text
  `Image unavailable: <path> (<reason>)`, as does every image past the 32nd in a
  turn.

A client draws a marker line where it stands in the reply, and leaves one in a
code fence, or with anything else on its line, as text. A user's message carries
the same marker for each attachment, after any text, and there every such line is
an image, because what a user types is not Markdown. Markers in subagent reports
are not handled and stay text.

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
