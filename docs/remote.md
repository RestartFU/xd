# Remote daemon

The goal is synced devices: several machines showing the same chats because
there is one copy of everything, on the machine the work happens on.

## Shape

One binary, two modes. `hy serve` runs the daemon; `hy` with no arguments is
the client. The daemon owns the SQLite database, the workspace tree on disk,
and the agent processes -- backends, folder inheritance and the ask-block
parser are the same code either way, not a reimplementation.

    hy serve                 # listens on 4001
    hy serve --pair          # prints a short-lived pairing code

A remote appears in the sidebar as its own root beside the local workspaces,
its folders and chats underneath, drawn from the same HyNode model with a
remote tree implementation in place of the filesystem one.

## Pairing

The daemon prints a code (`4F2K-9QX1`) good for sixty seconds and one use. A
client sends it once and receives a long-lived device token, kept in its
settings. The daemon keeps paired devices in a table with names and last-seen
times, each revocable on its own.

## Transport

Newline-delimited JSON over TLS, one connection held open -- the same framing
the CLI parsers already read, so the machinery exists. Requests fetch the
tree, chats and messages; a subscription pushes turn events (text-delta,
tool-use, finished) to every connected client, so two machines watching one
chat both see it stream.

## Why TLS is not optional

The daemon runs agents at whatever access their chat allows, including one
that executes commands without asking. A pairing token is therefore remote
code execution on that machine: over plaintext it is readable by anyone on
the network. The daemon's self-signed certificate is pinned by the client at
pairing time -- trust on first use, as SSH does with host keys -- and the
pairing code is short-lived and single-use for the same reason.

## Port

4001 by default.
