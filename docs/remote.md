# Remote desktop over SSH

xd has no network daemon, pairing code, listening port, or background service.
Remote mode uses the SSH command configured on the home screen, for example:

```sh
ssh zenomc.org -p 22
```

The command is parsed as an SSH invocation, so normal SSH options such as
`-i`, `-J`, and `-o` are supported. Authentication, host-key verification,
jump hosts, and forwarding policy remain SSH's responsibility.

## Connection model

Local and remote modes are mutually exclusive. The selected mode is persisted,
and **Disconnect** returns the app to local mode.

In local mode the desktop starts its bundled `xd-host stdio` process and owns
it for the lifetime of the window. In remote mode the desktop runs the same
host through SSH:

```text
desktop ── stdin/stdout ── ssh host ── xd-host stdio
```

There is no Unix-socket forwarding and nothing listens after the SSH session
ends. The remote machine must have the matching stable or nightly xd bundle
installed under `~/.local/opt/xd*/`; its state stays under the corresponding
`~/.local/share/xd*/` directory.

## Agent and terminal sessions

Codex, Claude Code, JCode, GitHub Copilot, and plain terminal tabs also run through SSH. xd launches
the bundled tmux on the selected machine so a CLI survives a transient window
or SSH disconnect and can be reattached on reconnect. tmux is an implementation
detail: users do not need to install or configure it separately.

The desktop keeps one event stream open while connected and refreshes its
cached tree, conversations, and terminal sessions after reconnecting. Closing
the app closes the stdio host; active terminal and agent processes remain in
their tmux sessions.

## Security

Remote access has the same authority as the SSH account. xd does not add a
second credential or expose a TCP port. Use normal SSH keys, host-key checking,
and any restrictions required for that account.

## Mobile

Android uses the same SSH-only model through an in-process SSH client. It accepts
password or imported private-key authentication, requires explicit SHA-256 host-key
fingerprint confirmation before authentication, pins the exact host key, and runs
`xd-host stdio` through an SSH exec channel. See [mobile.md](mobile.md).
