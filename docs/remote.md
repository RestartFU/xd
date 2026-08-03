# Remote daemon

The goal is synced devices: several machines showing the same chats because
there is one copy of everything, on the machine the work happens on.

## Shape

One binary, two modes. `xd serve` runs the daemon; `xd` with no arguments is
the client. The daemon owns the SQLite database, the workspace tree on disk,
and the agent processes -- backends, folder inheritance and the ask-block
parser are the same code either way, not a reimplementation.

    xd serve                 # listens on 4001
    xd serve --pair          # starts or attaches, then prints a pairing code

When the desktop app already owns the local daemon, `xd serve --pair` attaches
through its private local IPC endpoint. It asks that same daemon to enable TLS
and exits after printing the code; it never tries to start a competing daemon.
The app exposes the same action as **Add a Device…**. On the other machine,
choose **Connect to a Machine…** and enter the shown address, port, and code.
Both clients then attach to one Engine: they see the same saved chat and the
same in-progress turn. The host desktop or foreground `xd serve` process must
remain open while another device uses it.

A remote appears in the sidebar as its own root beside the local workspaces,
its folders and chats underneath, drawn from the same XdNode model with a
remote tree implementation in place of the filesystem one.

The Android client pairs the same way and speaks the same protocol; it is
remote-only by construction. See [mobile development](mobile.md) and the
normative [wire protocol](protocol.md).

## Pairing

The daemon prints a code (`4F2K-9QX7`) good for five minutes and one use. The
connecting client supplies its automatic device name with the code and
receives a long-lived device token saved in its settings; the owner can rename
it afterward. The daemon keeps paired devices in a table with owner-managed
names and last-seen times, each revocable on its own. The local **Manage
Devices…** panel can list, rename, and revoke them; revocation immediately
disconnects active sessions and requires pairing again.

## Transport

Newline-delimited JSON over TLS, one connection held open -- the same framing
the CLI parsers already read, so the machinery exists. Requests fetch the
tree, chats and messages; a subscription pushes turn events (text-delta,
tool-use, finished) to every connected client, so two machines watching one
chat both see it stream.

## Many clients at once

Every paired device may be connected at the same time, and they stay in step
without polling.

The daemon is the only writer. A client never edits state directly: it sends
an intent -- send this message, rename this folder, switch this chat's model
-- and the daemon applies it and broadcasts what happened. Two devices acting
at once are therefore ordered by the daemon rather than racing in the
database, and neither can end up holding a version of the truth the other
does not have.

Events are live notifications, not a durable log. Each carries a monotonic id,
but that counter lives in the daemon process only: it does not survive a
restart, and there is no operation to resume from it. On every connection and
reconnection a client reloads the tree and every open chat, transcript, and
terminal list before it continues applying live events. Reloading is the
recovery path for a device that was closed, asleep, or off the network.

Because replies and events are written by different fibers, a client cannot
order a snapshot against live output by arrival alone. Live turn events carry
`turn_id` and `turn_sequence`, and a chat snapshot carries the same pair, so a
client can tell exactly which deltas its snapshot already contains.

Turn output fans out to every subscriber, not only the device that sent the
message. Watching a chat from a second machine shows the same reply arriving
in the same order.

One turn per chat stays the rule, enforced by the daemon. A second device
sending into a chat that is already working queues its message the way the
composer already does locally, and the queued message is itself broadcast --
so both devices can see what is waiting, and either can steer it.

Terminal sessions are shared rather than per-device: the pty lives on the
daemon, and every device attached to that chat sees the same screen and can
type into it. A terminal that only one device could see would not be the same
machine's terminal.

Opening the terminal pane first attaches to sessions the daemon already owns;
the plus button explicitly creates another. Input and resize intents travel
over the authenticated TLS connection, while pty output is broadcast as
base64-encoded terminal events. The daemon retains a bounded output history so
a device joining later can reconstruct the screen before consuming live output;
because raw terminal state can only be replayed from its beginning, a runaway
session that exceeds the hard replay limit is closed instead of serving a
corrupt tail. Rows and columns are canonical daemon state too: a resize is
broadcast in order before later output, so every attached emulator interprets
cursor movement and wrapping against the same geometry. Replay retains those
resize frames between output frames, so a late device interprets older output
at the geometry that produced it rather than replaying everything at today's
size. Closing a tab kills that shared session for every attached device.

What stays local to a device is what describes that device rather than the
work: which panes are open, how wide they are, window size. Those are read
from the device's own settings, not the daemon.

## Why TLS is not optional

The daemon runs agents at whatever access their chat allows, including one
that executes commands without asking. A pairing token is therefore remote
code execution on that machine: over plaintext it is readable by anyone on
the network. The daemon's self-signed certificate is pinned by the client at
pairing time -- trust on first use, as SSH does with host keys -- and the
pairing code is short-lived and single-use for the same reason.

## Port

4001 by default.
