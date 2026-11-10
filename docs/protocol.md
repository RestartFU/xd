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
- Unauthenticated frames are limited to 64 KiB. Authenticated frames have no
  configured byte cap; transfers are buffered in memory and remain subject to
  available memory and platform allocation limits.
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
that must stay responsive — `cancel`, `pause-all`, `voice-cancel`,
`agent-auth-cancel`, and `ping` — bypass the serialized command path entirely. Without ids, a `cancel`
answered promptly by the host would still sit behind a slow `diff-read` in
the reply stream. A `new-chat` that carries an id is also set up beside the
connection's other requests, a few at a time, since making its worktree can
take a while; its reply can come after replies to requests sent later.

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
`peer-pairing`, `daemon-stop`, and `app-client` are local-only. Revocation disconnects every
session for that device immediately. A revoked token or changed certificate
requires explicit re-pairing.

`daemon-info` returns protocol/build identity, PID, and the installed host binary
that an update restart re-executes (`executable`, null outside an installed
bundle), and whether it stops once the xd app that started it closes
(`stops_with_app`). `daemon-stop` acknowledges
before shutting down the shared runtime. `app-client` marks the connection as the
xd app's; a daemon started with `--stop-with-app` stops once no such connection has
been open for ten seconds. The local bridge sends it when `XD_APP_BRIDGE=1`. Remote listeners persist across daemon
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

Each chat and session also has `background_tasks`: how many background agents
and backgrounded shell commands its kept agent process is still running (for a
session, the sum over its chats). A chat that is not `working` but has
`background_tasks` above zero is waiting on them; show it as waiting rather than
idle. Only Claude chats have any. The count follows the agent's own reports, and
drops to zero when its process exits or is stopped, so after a daemon restart
nothing is waiting. The `background-tasks` event keeps it current; treat a
missing member as zero.

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

A host that can pause every chat at once adds a top-level `"pause_all":true`.
One that can also pause a single project's chats adds `"pause_folders":true`.
Clients show a pause button on each project only then, since an older host
ignores `folder` and would pause everything. A chat that
[`pause-all`](#pause-all-and-resume-all) stopped has `paused:true` until it
resumes or any turn starts in it; the member is absent otherwise. While any chat
in a project is paused, that project's button resumes them.

A host that keeps pull requests linked to chats adds a top-level
`"linked_pull_requests":true`. Each chat with links then has `pull_requests`, a
list of `{url, state}` oldest first, where `url` is the canonical
`https://host/owner/repo/pull/N` and `state` is `open`, `closed` or `merged`,
absent until the host has asked GitHub. The member is absent when the chat has
none. See [Linked pull requests](#linked-pull-requests).

A host whose agents can start agents of their own adds a top-level
`"child_chats":true`. A chat such an agent started has `parent`, the id of the
chat that started it, while that chat is still listed. When it works in a
session of its own, that session's `sessions` entry has `parent`, the id of the
parent chat's session; clients list it right below that session, indented. A
chat started in its parent's own session is just another of its tabs. See
[Agents an agent starts](#agents-an-agent-starts).

### `background-task-output`

Read-only request: `{"op":"background-task-output","chat":"<chat-id>"}`.
Optionally pass `"task":"<task-id>"` to read only that live task. The host accepts
ids, never client-supplied file paths. This reads the current kept process only;
completed tasks and tasks from an exited or replaced process are absent.

```json
{"ok":true,"chat":"c1","tasks":[{
  "id":"watch-1","kind":"Monitor","description":"Watch CI checks",
  "started_at":1794140000,"output_file":"/tmp/claude/tasks/watch-1.output",
  "lines":[{"text":"Checking CI status…","error":false},
           {"text":"/bin/sh: jq: command not found","error":true}],
  "truncated":false
}]}
```

`started_at` is seconds since the epoch when the host first observed the task.
Kind and description come from Claude's structured task/tool messages, with
text parsing as a fallback for older tool results. `output_file` is the absolute
path Claude reported, or null when it did not report one. The host opens only
that path, refuses symlinks and non-regular files, and never waits on a FIFO.
Each snapshot reads at most 4 KiB and returns at most the latest 20 lines,
including a partial final line. `truncated` indicates older output was omitted.
`error` is a case-insensitive heuristic for error text, not an exit status.
An unavailable file has `lines:[]` and a short `output_error`, such as
`"Output file unavailable."` or `"Output file not reported."`.

Expand the waiting row to show task kind, description, elapsed time and output,
highlighting error lines. Keep it collapsed by default. Refresh gently (about
every 2.5 seconds) only while expanded and visible. Output is fetched on demand
and is not broadcast through the count-only `background-tasks` event. An empty
`tasks` array means no matching live tasks remain. Older hosts can reject this
additive operation; show a brief load error without interrupting the chat.

### `chat`

```json
{"op":"chat","chat":"chat-1"}
```

Success always includes `ok`, `title`, `backend`, `session_mode`, `auth_state`, `commands`,
`plan`, `queue` (oldest first), `working`, `background_tasks`, `resumable`, `effort`,
`new_worktree`, and `has_messages`. `background_tasks` is the count described under
`tree`. `resumable` is true when the chat's last turn was cut short: stopped,
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

`session_mode` is `chat` for Codex/Claude sessions. Tree chat summaries
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
| `agent-secrets` | optional `folder` | `names: string[]` for that project's own set, or the host-wide set without `folder`; values never cross the wire |
| `agent-clis` | none | detected assistant versions |
| `agent-auth` | none | `providers:[…]` as in [Assistant accounts](#assistant-accounts), and `multiple_accounts: boolean`, false on a host that keeps only the user's own sign-in; checks every account again |
| `host-update` | optional `action` of `status`, `check`, `install`, `restart` | `version`, `channel`, `state`, `supported`, `available`, optional `latest` and `error` |
| `agent-cli-updates` | optional `action` of `status`, `check`, `install`; `install` requires `provider` | `providers:[{provider,display_name,state,available,updatable, optional version, latest, detail}]`; `state` is `idle`, `updating`, `updated`, or `failed`; each change is also sent as an `agent-cli-update` event holding one provider |
| `resume-interrupted` | `resume: boolean` | `resumed: string[]`. Answers for every chat cut off since the last answer, or left waiting on background agents or commands that a restart stopped, which wait in storage until then. Local and authenticated remote clients may answer, and the first answer settles them: `true` sends a visible continuation message to each (one that says the background work stopped, for a waiting chat), unless the chat has moved on, and `false` discards them |
| `agent-catalog` | none | `backends: [{id, name, default_model, models:[{id,name,context_window}], efforts:[string]}]` |
| `owner-icon` | `owner: string` GitHub login from a folder's `owner` | `owner`, and either `mime` of `image/png` or `image/jpeg` with base64 `data`, `pending: true` while the host fetches, `retry: true` when the last fetch failed, or none of these when the account has no avatar; an uncached avatar is answered as pending and sent to the asking connection as an `owner-icon` event |
| `workflow-status` | `text: string` captured `workflow_run` marker | `name`, `state`, optional `conclusion`, `jobs:[{id,name,state, optional conclusion, optional log}]`; fetches GitHub Actions status on the host |
| `usage-summary` | `days` of `1`, `7`, `30`, or `90`, optional IANA `time_zone`, optional `now_ms`, optional `refresh: boolean`, optional `generation` | `pending: true`; the asking connection then receives a `usage-summary` event with `generation` and either `error` or `summary:{read_at_ms,resolution,window_label,periods,buckets,sessions,sources,pricing}`, read from the host's Claude Code and Codex transcripts and priced from a daily LiteLLM price list |
| `usage-limits` | optional IANA `time_zone`, optional `generation` | `pending: true`; the asking connection then receives a `usage-limits` event with `generation`, `host_now_ms`, and `accounts:[{provider,account_key,plan,checked_at_ms,windows,reset_credits,unavailable,account_rank}]`, asked of each account of each installed assistant; `account_rank` is the account's 1-based place in the host's list and is only sent when the assistant has several accounts; `account_key` is a hash, so no address crosses the wire |
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
original `settled_at`. No terminal closes, and work an agent is still doing goes
on. The host stops the process of an agent with nothing running, within two
minutes of settling or after 30 idle minutes in any session, and the next message
resumes the conversation in a new process.

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
- `on_merge` settles a session when its pull requests merge. A closed pull
  request settles it whenever either of these two rules is on; with both off the
  host settles nothing by itself.
- `keep_pull_requests` stops inactivity from settling a session that has a pull
  request. Merging and closing still do.

A session's pull requests are those linked to any of its chats and the pull
request of its checked-out branch, unless the user unlinked that one and no chat
in the session still links it. The session settles once every one of them has finished. A pull
request that finished before the user's latest message in the session does not
count. The rules skip sessions the operations above would refuse and
sessions the user un-settled. `settled_at` is then when the session went quiet,
not when the sweep ran. `set-auto-settle-settings` changes only the members it
names, broadcasts `auto-settle-settings`, and applies at once.

### Deleting idle sessions

```json
{"op":"auto-delete-settings"}
{"ok":true,"after_days":null}
{"op":"set-auto-delete-settings","after_days":30}
```

`after_days`, from 1 to 365, deletes a session after that many days without
activity, on the same schedule as the settling rules. It is `null`, and nothing
is deleted, until a client chooses a number of days. A request without
`after_days` changes nothing. `set-auto-delete-settings` broadcasts
`auto-delete-settings` and applies at once.

The rule skips the sessions automatic settling would skip: a member chat is
running a turn, has a queued message, or is waiting on an approval or question,
the user un-settled the session, or `set-session-auto-settle` keeps it out of
the rules. Typing in the session's terminals, and output from them, counts as
activity. Settled sessions are deleted like active ones.

Deleting a session closes its terminals, as `delete-session` does, and removes
each worktree xd generated for it with `git worktree remove`, which never
forces. The branch stays in the repository with every commit on it. Ignored
files in the worktree, such as build output, go with it. The whole session
stays, and the next sweep tries again, if one of those worktrees has
uncommitted changes or untracked files, or a detached HEAD whose commits no
branch or tag holds. A worktree xd did not generate, one now on a different
branch from the one xd checked out there, or one another session or project
works in, is left where it is. The host records each worktree it generates;
worktrees from before that record count only when they sit exactly where xd
puts its own and still have the branch xd named after their chat. If Git refuses a worktree after the session is
deleted, the worktree stays on disk with everything in it. Each deletion
broadcasts `tree`, and `worktrees-changed` when a worktree went with it.

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

With `"steer":true`, a message that would have been queued is steered at once,
as `steer-queue` describes, and the reply adds `steered`. With no turn running
the member has no effect.

Each attachment is a file with a `name`, a media type in `mime`, and base64
`data`:

```json
{"attachments":[
  {"name":"screen.png","mime":"image/png","data":"base64…"},
  {"name":"Report.pdf","mime":"application/pdf","data":"base64…"}
]}
```

At most 10 attachments, of which at most 4 are images. Files have no per-file
or combined byte cap. Images are each at most 10 MiB and 20 MiB in total. An `image/png` attachment must be a PNG and is passed to the
agent as an image, on a line `[image: <host path>]`. Any other media type is a
file: the host writes it under its sent name, the last component of `name`, in
a private directory of its own and appends `[file: <host path>]`, which the
agent reads from disk. An unusable name becomes `file`.

A message takes from the chat's draft exactly the attachments it carries: each
one removes one equal entry (same `mime` and `data`, and for a file the same
kept name). Anything else in the draft
stays, such as a file another device added meanwhile, or the files an image-only
client never saw. A message sent apart from the draft, such as a shortcut,
leaves the draft's attachments alone.

When a chat's first message creates its worktree, the worktree's branch is
named from the message's first line, so the send never waits on a model. Then,
in the background, a low-cost model chooses a short name and the host renames
the branch to `xd/<name>-<chat suffix>`. It does this only while the branch is
still the one xd made. The model is `worktree_model` with `worktree_backend`
when a client gives them and the CLI still offers that model. Otherwise it is
the assistant's naming model from `chat-title-settings`, whether or not titles
are enabled. An assistant that offers none of those keeps the name from the
message. A client may give `worktree_name` to use that name instead, or
`generate_worktree_name: false` to keep the name from the message. `new-chat`
takes the same fields, with the first message in `prompt`, for a worktree it
creates. A worktree that `new-chat` creates without `prompt` is named for the
chat's placeholder title, and the chat's first message renames it in the same
way.

### `set-draft`

```json
{"op":"set-draft","chat":"chat-1","text":"unfinished"}
{"ok":true,"draft":"unfinished","draft_revision":4}
```

The host persists one composer draft per chat and broadcasts a `draft` event
after every update. `text` is required but may be empty and is limited to 1
MiB. Optional `attachments` replaces the synchronized attachment list, in the
same form and limits as `send`; omit it for ordinary text edits so large payloads
are not resent on every keystroke. An empty array clears them. A draft is shared
last-writer-wins: each list a client sends replaces the one before it. Without
`"files":true`, which a client that shows files as well as images sends, the
list replaces only the draft's images: the files the client cannot see are kept
after them. The client's own list keeps to
a message's limits, and a draft's files and images each keep to their own: at
most 10 files with no byte cap, and at most 4 images and 20 MiB of those, so
files kept beside a client's images never refuse them; a message sent from it
must still fit a message's limits. A
client that shows only images skips the other files. Large attachment drafts are
stored in bounded SQLite chunks (schema 26) so no single SQLite value contains
the full file payload. The `chat` snapshot returns `draft`,
`draft_revision`, `draft_attachments`, and `draft_files` so a newly connected
device starts with the same composer. `draft_attachments` holds only the draft's
PNG images and `draft_files` its other files, so a client that knows only images
never sees a file; a client that shows files reads both.

### `queue`, `drop-queue`, `edit-queue`, `steer-queue`

```json
{"op":"queue","chat":"chat-1","text":"next"}
{"op":"drop-queue","chat":"chat-1","index":0}
```

`queue` requires non-empty `text`. `drop-queue` without `index` clears the whole
queue. With `index`, an optional `text` names the message the client saw there;
the host refuses the drop if a different message now holds that place. `chat` replies and `queued` events carry the queue's `queue_revision`, which every change
to the queue increases. `drop-queue`, `edit-queue`, `steer-queue`, and `reorder-queue` take
an optional `queue_revision`, the one the client saw; the host refuses any of them if the
queue has changed since. Messages have no identity but their place and texts can repeat,
so this is what keeps one of two equal messages from standing in for the other, even
after changes that leave the same texts behind. All four broadcast a `queued` event carrying the new queue, so every
attached device sees what is waiting and either can steer it.

`steer-queue` takes `chat`, `index`, and the `text` the client saw at that
index, and redirects the running turn to that message. The reply adds `steered`:

- `true`: the message went into the running turn. The agent keeps its process
  and turn, and the message leaves the queue. Claude stops its reply and answers
  this message, after any running tool finishes. Codex takes it within the turn.
- `false`: the turn could not take it, because its agent has no process to write
  to or it is waiting on an approval or question. The message moves to
  the front of the queue and the turn is stopped.

A message the agent refuses or never reads returns to the front of the queue, in
the order it was sent, and leaves the transcript. If it was never read, the agent's
process is ended first so the message can't be answered twice. If no turn is left
when a steer fails, the message starts a new turn instead of waiting in the queue.

A steered message is stored as an ordinary `user` row. While the turn runs it
arrives as a `tool` event and `items` entry whose text is `steer\n` followed by
the message, with its stored row in `message_id`. Clients show that as the user's
message; the marker is only ever seen live, never in `messages`.

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

### `pause-all` and `resume-all`

```json
{"op":"pause-all","folder":"folder-1"}
{"ok":true,"paused":["chat-1","chat-2"]}
{"op":"resume-all","folder":"folder-1"}
{"ok":true,"resumed":["chat-1","chat-2"]}
```

With `folder`, both keep to the chats in that folder and in any folder or
project inside it. Without it, they cover every chat.

`pause-all` stops every chat that is `working`, and every chat whose agent is
running background tasks between turns, so nothing goes on while the user is
away. Each one it stops is marked `paused` in `tree`, keeps its queue instead of
starting the next message, and becomes `resumable`. A paused chat is not
settled or deleted by the automatic rules. A chat that was already idle is left
alone. `pause-all` is a control operation, like `cancel`. It stops a chat that
a message is being sent to, even one that was idle, once that send has started
its turn, after the others.
The host never continues a paused chat by itself: not after a restart, not on
another account after a usage limit, and not for an agent's background report.

`resume-all` continues each paused chat as [`resume-turn`](#resume-turn) does,
then lets its queue run as usual. A paused chat that can no longer be resumed,
because it has moved on or is still stopping, just loses its mark and runs its
queue. Sending a message, resuming, or steering in one paused chat picks up only
that chat. Both operations publish `changed` for each chat they touch and `tree`.

### Chat quotas

```json
{"op":"chat-quota-settings"}
{"ok":true,"percent":null}
{"op":"set-chat-quota-settings","percent":10}
{"op":"set-chat-quota","chat":"chat-1","percent":20}
{"ok":true,"resumed":true}
```

A host that can limit chats to a share of their account's usage adds a
top-level `"chat_quotas":true` to `tree`. A quota is a percentage of the usage
limit the chat's account works under: its 5-hour limit, or its weekly limit when
it has no 5-hour one. A chat that has used its quota stops, and the host asks
the user before it goes on.

`percent`, a whole number from 1 to 100, is the host's default quota for every
chat. It is `null`, and no chat is limited, until a client chooses one.
`set-chat-quota-settings` takes `percent`, a number or `null`, replies like
`chat-quota-settings`, broadcasts `chat-quota-settings`, and applies at once. A request without `percent` changes
nothing.

`set-chat-quota` gives one chat its own quota: `percent` from 1 to 100, or
`null` for no limit at all. With `inherit:true` instead of `percent`, the chat
follows the host's default again. Each of these starts the chat's count again
from zero. When the chat had stopped at its quota, it then goes on: the turn the
quota cut short continues as [`resume-turn`](#resume-turn) would, and then its
queue runs. `resumed` says whether it went on. The request publishes `changed`
for the chat and `tree`.

The `chat` reply has `quota:{percent, used, own}` while a quota applies to the
chat. `percent` is the quota, `used` is how much of the limit the chat has used
since its count began, in the same percentage points, and `own` is true when
the chat has its own quota rather than the host's default. A chat that stopped
at its quota has `quota_reached:true`, in both `tree` and `chat`, until the user
gives it a new quota; the member is absent otherwise.

The host measures a chat's use by reading its account's limits about once a
minute while the chat works on it, and when a turn ends. A rise in the limit is
shared evenly among the chats that worked on the account since the last
reading, including chats without a quota, so the figure is an estimate. An
account whose limits cannot be read, such as one signed in with an API key,
uses nothing against a quota.

Once `used` reaches `percent`, the host stops the chat as
[`pause-all`](#pause-all-and-resume-all) would: its turn ends and is
`resumable`, its agent's background work stops, and anything sent to it waits
in its queue. It is not `paused`. `resume-turn`, `steer-queue`, and
`resume-all` leave it alone; the host never continues it by itself. A chat that
reaches its quota as a turn ends is marked the same way, with nothing to resume.
Clients show the chat's quota in place of its resume button and offer to
continue with a new one, prefilled with a larger quota than the last, or
without a limit.

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
are not Claude's, or that Claude has not reported a task for yet. An agent
Claude runs as a shell command (its record's `launcher` is `Claude`) counts as
Claude's, and Claude reports a task for it once the command runs in the
background. A control
operation: answered even while a serialized command is slow. The agent's
record reports the outcome.

### Subagent records

Each delegated agent is one `tool` message whose content is `subagent_run\n`
followed by a JSON object. The host rewrites that message in place as the
agent works rather than appending new rows, so a stored transcript holds only
the newest snapshot.

```json
{"key":"toolu_01","task_id":"a22197aff550fdfd9","provider":"Claude","agent":"Explore","model":"claude-haiku-5-5",
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
and `summary` its final report. `task_id` is Claude's own id for the agent, the
one its lead addresses messages to. `model`, `prompt`, `activity`, `latest`,
`summary`, `tokens`, `duration_ms`, `started_at`, and `task_id` are absent until
known; ignore unknown members.

Neither Claude Code nor Codex takes a message for one of its agents from a
client, so the clients ask the lead to pass one on. Such a message is an
ordinary `send` to the lead's chat, steered into the running turn when there is
one, in this exact form:

```text
[xd: message for agent "Map the parser" · id a22197aff550fdfd9]
Also check the lexer.

Pass the message above to that agent word for word with your agent messaging tool, addressed by its id. If it has finished, resume it with the message. Then carry on with what you were doing; don't act on the message yourself.
```

The description has `"` replaced by `'` and runs of whitespace collapsed to one
space. The id is the record's `task_id`, or for a Codex agent its `key`, the
thread it runs in. Shell tasks and agents run as a command take no messages.
Clients show such a message as its middle lines, addressed to the agent.

An agent can also be another assistant that the chat's agent runs as a shell
command: Claude running `codex exec` or the codex-opussy `codex-agent` script,
or Codex running `claude -p`. Its record has the same shape, keyed by the
command's tool call. `provider` is the assistant doing the work and `launcher`
the one whose shell runs it; `launcher` is absent for an agent started through
a subagent tool. The command's output, without any wrapper footer, is its
`summary`. Such an agent's `steps` is empty, because its tool calls happen in
another process.

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

JCode is no longer supported. Chats created with it keep `backend: "jcode"` and
still appear in `tree`, open with `chat`, and page through `messages`, so their
history stays readable. `send`, `resume-turn`, and `set-option` on such a chat,
and `new-chat` with `backend: "jcode"`, fail with `JCode is no longer supported.
Start a new chat with Codex or Claude Code.` Folder and host defaults that named
JCode fall back to the usual defaults when the database opens. Clients should
show any backend they do not recognize with a generic label and icon rather
than hide or reject the chat.

### `set-usage-prices`

Replaces the host's custom model prices and aliases. Takes `prices` and `aliases`
in the shape `usage-prices` returns; leaving `aliases` out keeps the saved ones, so a
client that edits only prices cannot revert another client's mappings. Hosts that do this
answer `usage-prices` and `set-usage-prices` with `aliases_optional: true`; `input` and `output` are required, and a blank
cache rate uses the input rate. Answers with the saved values.

### Assistant accounts

Codex and Claude Code can each have several accounts. `default` is the user's
own CLI sign-in and runs in the CLI's usual home, as before accounts existed.
Each account added in xd gets a private home under the host's data directory
(`CODEX_HOME` or `CLAUDE_CONFIG_DIR`), where the CLI keeps its own credentials;
the host never reads or sends a token. Everything else in the user's home
(configuration, MCP servers, skills, session transcripts) is linked in and shared.
Claude's `.claude.json` belongs to each account; the user's MCP servers and
project settings are copied into it when the account is added and again before
a new Claude process starts as it while nothing else runs as it. Removing an
account waits for any check running as it before its home is deleted.

Each provider in `agent-auth` and each `agent-auth-changed` event has the
pre-accounts members `provider`, `display_name`, `state`, `needs_input`, and
optional `detail`, `login_url`, `device_code`, all describing the `default`
account, plus:

- `accounts:[{id, default, pending, label, email, plan, state, detail, login_url, device_code, needs_input, in_use, busy, problem, limited_until_ms, limited_reason, checked_at_ms, windows:[{kind,label,used_percent,resets_at_ms}]}]`
  in priority order. `label` is the address when the host knows it. `pending`
  marks an account still signing in for the first time. `in_use` marks the
  account a new chat would start on. `busy` marks the account the provider's
  one sign-in or sign-out is running for. `problem` is `mismatch` when the
  account's home is now signed in to a different login, or `duplicate` when an
  earlier account is the same login; either keeps it from taking turns.
  `limited_until_ms` and `limited_reason` (such as `5-hour limit`) say when an
  account that is out of usage is back.
- `auto_switch: boolean`, `adding: boolean`, and optional `notice`, the outcome
  of the last attempt to add an account.

Unlike `usage-limits`, these carry addresses: they go only to the user's own
authenticated devices, which need them to tell accounts apart.

| Op | Request | Effect |
|---|---|---|
| `agent-auth-start` | `provider`, optional `account` (default `default`) | Signs that account in again in its own home |
| `agent-auth-add` | `provider` | Signs in a new account in a new home. It joins the list once the CLI succeeds and the host has read who it is; a login already in the list is refused and its home deleted. |
| `agent-auth-input` | `provider`, `input` | Sends a code the CLI asked for to the provider's running sign-in |
| `agent-auth-cancel` | `provider` | Stops the provider's running sign-in; a new account being added is discarded |
| `agent-auth-logout` | `provider`, optional `account` | Signs the `default` account out. Any other account is signed out and removed, its home deleted, after the agent processes kept running as it are stopped; while a chat is working on it, removal is refused and reported as the provider's `notice`. Signing an added account in again stops and refuses the same way, and an account takes no turns while it signs in. |
| `agent-accounts-order` | `provider`, `accounts: string[]` naming each account once | Sets the priority order |
| `agent-accounts-auto-switch` | `provider`, `enabled: boolean` | Turns automatic switching on or off |

A provider runs one sign-in or sign-out at a time. Each answers `ok: true` and
reports progress through `agent-auth-changed`.

Each turn runs on one account, chosen when it starts. A chat stays on the
account its session belongs to while that account is signed in, is still the
login it was added as (an added account's login must have been read since it
last signed in or the host started), and has usage left; otherwise the turn runs on the first
such account in priority order. With switching off, turns run on the first
signed-in account. When a turn is turned away for usage and switching is on,
the host holds the account back until its limit resets, adds an
`account_switch` tool row to the chat, and continues it on the next account as
`resume-turn` would, but only while the turn that ran out is still the chat's
latest and still resumable. New chats start on the first usable account in
priority order. A session is filed under its account and the login that account
was signed in to, so a session never follows the user's own CLI sign-in to a
different login. Before a Codex session is resumed, the host reads the
account's login again (a local `account/read`) unless it read it in the last
two minutes or since the account last signed in or out; if it cannot, the chat
starts a new session given its messages rather than resume. With several
accounts and none able to take a turn, the turn is refused rather than run on
the default. A kept process is reused only for the same login.
Codex sessions stored before accounts existed name no login; as the host starts
they are filed under the login Codex has then, which is the only record of
who made them, or as an unknown login when it cannot be read, so that no later
login resumes them.
`CODEX_HOME` and `CLAUDE_CONFIG_DIR` from agent secrets are ignored: the account
decides the home. An account whose login the host cannot read is not added. A Claude Code session moves with the chat. A Codex session
cannot open on another account, so the new account starts a session given the
chat's messages as text, and the row says so.

The row's text is `account_switch\n` followed by one sentence, such as
`Switched to b@example.com: a@example.com reached its 5-hour limit (back in 2 h 5 min). Continuing there.`
or, when every account is out, `Every Codex account is out of usage. The first is back in 3 h.`

### `usage-consume-reset-credit`

Spends one banked reset credit on the host's signed-in account. Takes `provider`
(`codex`), an `idempotency_key` naming the attempt, which is reused when retrying
it, and the `account_key` from `usage-limits`; the host spends it as the account
last seen signed in as that login, and refuses if the key is missing or that
account is no longer signed in to it. Answers `pending: true`, then sends the asking connection a `usage-reset-credit`
event with `outcome` of `reset`, `nothingToReset`, `noCredit`, or `alreadyRedeemed`
or `error`, and in both cases the `idempotency_key` of the attempt it answers. This cannot be undone, so clients must confirm first.

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
was.

`set-agent-secrets` replaces one secret set: the host-wide set, or a project's own
with `folder`. It takes `entries:[{name, value}]` of at most 256 unique environment
variable names. An entry without `value` keeps the stored value, so a client can keep
a secret without ever reading it; a new name needs a value. Agents get the host-wide
set with each enclosing project's set laid over it.

`set-shortcuts` replies with the resulting shortcut sets and broadcasts a
`shortcuts-changed` event so open chats can refresh their buttons.

### Git actions

`git-state` takes a `chat` and an optional request correlation string. It
acknowledges immediately, reads repository state away from the connection
loop, and publishes `git-state` with `visible`, `action`, `label`, `enabled`,
and an optional pull-request `url`. That `url` is only there when the branch is
clean and pushed. A request with `pull_request: true` also asks for the branch's
pull request whatever state the working tree is in, and the event then carries
`pull_request: {branch, url}`: an empty `url` means the branch has none, and a
missing `url` means GitHub could not say. The member is left out outside a
repository, on a detached HEAD, and when HEAD moved during the lookup.
These lookups can finish out of order, so a client that overlaps them should send
a request correlation string, which the event echoes as `request`, and keep only
the latest answer.

`git-action` takes a `chat`, one of `commit`, `push`, `create-pr`, or `view-pr`,
and an optional request correlation string. Commits also require `message`;
pull-request creation requires `title` and accepts `body`. It acknowledges
immediately and publishes `git-action-finished`. Successful events contain the
new repository action state and an optional URL; failures contain `success:
false` and `error`. The host rechecks the advertised action before mutating
the repository so a stale client cannot run the wrong operation.

Three synchronous operations back the Changes pane; each takes a `chat` and
works in that chat's checkout:

- `git-status` answers `branch`, `base` (the default branch, without `origin/`),
  `upstream` (empty when none), `ahead`, `behind`, `staged`, `unstaged`,
  `untracked`, `conflicted`, and `clean`.
- `git-commit` takes a `message` of 1 to 4096 bytes after trimming, stages every
  change, commits, and answers like `git-status`.
- `git-pr-status` asks GitHub's CLI for the checked-out branch's pull request and
  answers `url`, empty when there is none or GitHub could not say. A pull request it
  finds is linked to the chat.

### Linked pull requests

A chat can have any number of pull requests linked to it. Clients list them
from `tree` (see above) and change them with two requests:

```json
{"op":"link-pull-requests","chat":"chat-1","urls":["https://github.com/o/r/pull/7/files"]}
{"ok":true,"linked":["https://github.com/o/r/pull/7"],"invalid":[]}
{"op":"unlink-pull-request","chat":"chat-1","url":"https://github.com/o/r/pull/7"}
{"ok":true,"changed":true}
```

`link-pull-requests` takes 1 to 16 addresses, keeps those that are GitHub-style
pull request addresses in their canonical form, and answers the rest in
`invalid`. A chat keeps at most 32. `unlink-pull-request` marks the link
removed rather than forgetting it. Both broadcast `tree` when the list changed.

The host also links pull requests by itself: the one a `git-state` lookup with
`pull_request: true` or `git-pr-status` finds for the chat's branch, the one
`git-pr-create` or the `create-pr` action opens, and any an agent opens by
running `gh pr create`. These never bring back a link the user removed; a
`link-pull-requests` request does. The same lookups ask GitHub again about the
chat's open links and broadcast `tree` when a state changed, as does automatic
settling when its lookups find one.

Agents link the rest themselves. The daemon starts Claude and Codex with an MCP
server, `xd-host mcp --database PATH --workspaces PATH --chat ID`, named `xd`,
whose tool `link_pull_requests` takes `urls` and sends `link-pull-requests`
with `"source":"agent"` for its chat. The tools are approved in advance, and the
agent's instructions say when to use them.

### Agents an agent starts

The same `xd` MCP server gives an agent four more tools, each sent to the daemon
as a request for the server's chat:

| Tool | Request | Does |
| --- | --- | --- |
| `start_agent` | `start-agent` | Starts a chat under this one and sends it `prompt`. |
| `list_agents` | `list-agents` | Lists this chat's agents, or the one named by `agent`. |
| `message_agent` | `message-agent` | Sends one of them `text`, queued behind a running turn. |
| `stop_agent` | `stop-agent` | Stops one of them, as `cancel` would. |

```json
{"op":"start-agent","chat":"lead","prompt":"Fix CI on #12","title":"CI","assistant":"codex","workspace":"new_worktree"}
{"ok":true,"agent":"worker","backend":"codex","model":"gpt-5.5","reports":true}
{"op":"list-agents","chat":"lead"}
{"ok":true,"agents":[{"id":"worker","title":"CI","assistant":"codex","model":"gpt-5.5","status":"working","reply":"…"}]}
```

`start-agent` makes an ordinary chat in the parent's project. `assistant` is
`claude` or `codex` and defaults to the parent's; on the same assistant the
model and effort default to the parent's too. The chat always gets the parent's
workspace access. `workspace` is `new_worktree`, the default, for a git worktree
of its own, or `this_worktree` for another tab of the parent's session. The new
chat then runs like any other: its turns go to the account xd chooses for them,
in priority order, and it keeps running whatever happens to the parent's agent
process. A chat that another agent started cannot start agents itself, and a
chat can have at most 10 agents at work at once.

With `notify` left true, each turn of the new chat that ends for good, neither
stopped nor moving to another account, sends the parent a message beginning
`[xd] Agent` with the reply or error. A parent at work gets it queued; a paused
parent gets it in its queue without resuming. Statuses in `list-agents` are
`working`, `paused`, `waiting for an answer`, `stopped with an error`,
`finished` and `idle`.

The parent's transcript also gets one card per agent: a `tool` row holding a
`subagent_run` record (see the subagent records above) with `key` set to
`chat:<agent chat id>` and `chat` set to that id. The host writes it when the
agent's first turn starts, removes it if that start fails, and rewrites it in
place, with a `record` event, whenever a turn ends: `status` is `completed` or
`failed`, with `summary` the start of the reply or the error (a message that
could not start a turn is a failure too), or `stopped` for a turn that was
stopped. The card says only how the last turn went; clients
show what the tree says of the chat now (`working`, `paused`,
`question_waiting`) over it, and open the chat when the card is clicked. Older
clients show it as an ordinary agent card. The report message itself starts
`[xd] Agent "<title>" (<chat id>) is <ending>.`, or `[xd] An agent (<chat id>)
is <ending>.` before the agent has a title, where `<ending>` is `done with its
turn`, `stopped with an error` or `waiting for an answer`; clients show such a
user row as one line naming the agent rather than the whole message. A chat
whose tree entry has `parent` shows a link back to that chat.

Starting an agent creates its chat and starts its first turn before answering,
so the server waits up to 300 seconds for the daemon, and Codex is started with
`mcp_servers.xd.tool_timeout_sec=300` so it waits as long for the tool.

`move-folder` takes a required `folder` and an optional `parent`. Omit `parent`
to move the folder to the workspace root. A folder cannot be moved into itself
or one of its descendants, and the host rejects a destination name collision.

To reorder, send `before` or `after` with the id of another folder instead of
`parent`. The order is saved and every client sees it in the next `tree`. The
folder also takes that folder's parent, so anchoring on a folder with a different
parent moves it on disk. Send `keep_parent: true` to refuse that move and report
an error instead. A host that honors this field adds a top-level
`"folder_reorder":true` to `tree`; older hosts ignore the field and would move the
folder, so don't offer reordering without it.

## Events

Chat-scoped events carry a `chat` member; ignore those for other chats.

| Event | Members | Meaning |
| --- | --- | --- |
| `tree` | — | Workspace tree changed; refetch `tree`. |
| `auto-settle-settings` | `after_days`, `on_merge`, `keep_pull_requests` | The host's automatic settling rules changed. |
| `auto-delete-settings` | `after_days` | The host's automatic deletion rule changed. |
| `chat-quota-settings` | `percent` | The host's default chat quota changed. |
| `changed` | `chat` | Stored chat state changed; refetch when idle. |
| `worktrees-changed` | — | A Git worktree changed; refetch chat state and worktree lists. |
| `shortcuts-changed` | optional `folder` | Global or workspace prompt shortcuts changed; refetch chat state. |
| `queued` | `chat`, `queue`, `text` | Queue replaced. `text` is the first item. |
| `draft` | `chat`, `draft`, `draft_revision`, optional `draft_attachments` and `draft_files` | Composer text changed; attachments are present only when replaced, images in `draft_attachments` and other files in `draft_files`. |
| `commands` | `chat`, `backend`, `commands` | Slash-command list for a backend. |
| `turn-started` | `chat`, `label`, `turn_id`, `turn_sequence` | A turn began. |
| `text` | `chat`, `text`, `turn_id`, `turn_sequence` | Assistant text delta. |
| `tool` | `chat`, `text`, `workdir`, `context`, `turn_id`, `turn_sequence`, optional `record`, `message_id` | Tool row. `record` marks a row the host will rewrite; `message_id` is its stored id. |
| `record` | `chat`, `text`, `record`, `message_id`, optional `turn_id`, `turn_sequence` | A record row was rewritten, such as a subagent's progress; replace the row with that `message_id`. Arrives without turn members when a background agent reports while no turn is running. |
| `turn-finished` | `chat`, `turn_id`, `turn_sequence`, `ok`, `waiting`, `silent`, `duration`, `last_message_id`, optional `error`, `question`, `options`, `accepts_input` | The turn ended. |
| `background-tasks` | `chat`, `background_tasks` | How many background agents and shells the chat's agent process is still running changed. Between turns a chat with any is waiting on them; see `tree`. |
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
the same marker for each image it attached, after any text, and there every such
line is an image, because what a user types is not Markdown. Each other file it
attached is a line `[file: <host path>]`, which clients show by its name. Markers in subagent reports
are not handled and stay text.

### Tagged questions

Claude asks with its own `AskUserQuestion` tool, at every access level. The host
turns it into an `item/tool/requestUserInput` interaction (see the chat snapshot's
`interactions`), and clients show it as a question card. Codex only offers its
`request_user_input` tool in Plan mode, so in other modes the host asks Codex to
put a question in its reply as a block instead:

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
