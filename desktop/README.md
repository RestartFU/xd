# xd desktop (GPUI)

This is the incremental Rust/GPUI replacement for xd's GTK desktop client.
It deliberately reuses the existing daemon and its documented JSON Lines
protocol; the Crystal desktop remains the production client until this crate
reaches feature parity.

GPUI is pinned because it is pre-1.0. Only the published Apache-2.0 `gpui`
crate is used. Zed's GPL component library is not a dependency.

Current milestone:

- native GPUI application shell;
- workspace/chat sidebar, transcript, queue indicator, and composer layout;
- variable-height virtualized transcript list;
- protocol framing and request-id matching primitives;
- daemon snapshot/event state reducers with unit tests.

Build and test this crate through the repository Dockerfile:

```sh
docker build --target gpui-desktop-check .
```

Every push to `feat/gpui-desktop` replaces the rolling `dev` GitHub
prerelease with the tested Linux x86_64 prototype. This channel is deliberately
separate from the production `nightly` release while daemon connectivity and
feature parity are still in progress.

Install it beside `xd` and `xd-nightly` as `xd-dev`:

```sh
curl -fsSL https://github.com/RestartFU/xd/releases/download/dev/install-dev.sh | sh
```
