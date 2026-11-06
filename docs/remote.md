# Paired remote connections

xd uses one persistent daemon on the host machine. Desktop and Android connect
directly over TLS; SSH is not part of the normal connection flow.

## Pair a device

1. Open xd on the host and choose **Settings → Pair a device**.
2. Copy the full pairing code.
3. In **Pair with a host** on the remote desktop or Android app, enter a
   reachable LAN, Tailscale, or server address and paste the code.

The address is `host` or `host:port`, and the port defaults to 4001. The pairing
code holds the one-use code and the SHA-256 certificate fingerprint, but no
address, so you choose the route. It expires after five minutes. The client verifies
that certificate before sending the code, receives a device token, and remembers
it for subsequent connections. **New code** replaces the current code.

The listener starts only when local pairing is requested, normally on port
4001. Its bind address and port are persisted and restored when the daemon
restarts. The address you enter can differ from the bind address, so a Tailscale
address can be used while the listener accepts connections on all interfaces.
The selected port must be reachable; this release does not provide a relay or
NAT traversal service.

## Switch between hosts

The connection status in the title bar connects to the remembered host using its
saved device token, without a new pairing code. While the connection is down,
pressing it retries immediately. It opens **Pair with a host** only when no host
is remembered or the remembered host has refused this device.

**Settings → Devices → Disconnect** returns to the local host. The remote keeps
running and stays remembered, so the status can reconnect to it later. **Connect**
in the same section pairs with a different host.

## Shared sessions

```text
Local desktop ── stdio bridge ── private Unix socket ──┐
                                                    ├── xd daemon ── agents / shells / state
Remote desktop / Android ── pinned TLS + device token ┘
```

Clients share the same chats, drafts, queues, pending approvals, and running
turns. Disconnecting or closing a client leaves the daemon running. Reconnecting
takes fresh snapshots and reconciles live output using turn watermarks. A daemon
restart interrupts active turns, preserves recorded history and backend thread
IDs, and reports interrupted work rather than silently continuing it.

The host must have xd and the assistant CLIs installed. Starting the desktop
starts its daemon automatically. For a headless host, run:

```sh
xd-host daemon --data "$HOME/.local/share/xd"
```

Use the foreground command with your service manager if it must restart at boot.
Stable, nightly, and dev data directories have independent daemon instances.

## Device management and identity

**Settings → Paired devices** lists remembered devices. Revocation requires a
second click and immediately closes their active connections. Device management,
new pairing codes, and daemon shutdown are available only on the local socket.
Remote clients authenticate before receiving workspace events or performing work.

The private TLS identity lives in `DATA/daemon/identity.json`; keep it when
upgrading. Replacing it changes the fingerprint and requires a new pairing.
Clients refuse changed certificates and revoked tokens instead of retrying them
indefinitely. Android stores the token and identity with Android Keystore;
Unix desktop settings containing device credentials are written with mode 0600.

## Daemon maintenance

`xd-host stdio --data DIR` starts the daemon on demand and bridges JSON Lines.
Concurrent launches reuse the same daemon. A private lock prevents duplicate
state writers, and a stale socket is repaired after the daemon exits.

```sh
xd-host daemon-status --data "$HOME/.local/share/xd"
xd-host daemon-stop --data "$HOME/.local/share/xd"
```

The daemon checks the connected bridge's build identity. If an older build is
still running after an update, stop it before reconnecting. Its log is
`DATA/daemon.log`. Stopping the daemon interrupts work on every connected device.

The integrated browser runs on the client machine. For remote previews use a
reachable host URL; `localhost` still refers to the client displaying xd.
