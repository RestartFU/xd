# Mobile development

The Android client under `mobile/` is a remote view onto an xd host. It now uses
the same architecture as remote desktop mode:

```text
Android ── SSH exec stdin/stdout ── xd-host stdio ── private Unix socket ── xd-host broker
```

There is no mobile daemon, pairing code, bearer token, TLS listener, local IPC,
on-device agent, Git process, SQLite database, or offline chat cache.

## Connection and security

The setup screen accepts:

- hostname or Tailscale IP;
- SSH port, default `22`;
- SSH username;
- either an SSH password or an imported private key with an optional passphrase.

The first connection stops before SSH authentication and displays the presented
SSH host-key algorithm and SHA-256 fingerprint. The user must compare it with a
trusted source and explicitly confirm it. Android then reconnects and pins the
exact host-key bytes and algorithm. A changed host key is fatal until the saved
connection is forgotten.

Password or private-key material, connection details, and the pinned host key are
stored as one AES-256-GCM record backed by Android Keystore. The app never offers
a trust-anyway path after a key mismatch.

The remote command is:

```sh
exec "$HOME/.local/share/xd/runtime/v1/xd-host" stdio --persistent \
  --data "$HOME/.local/share/xd"
```

The matching stable xd host must already be installed on the remote machine.
Connecting once with the desktop deploys the current host automatically. Mobile
does not upload native host binaries because the phone cannot determine or ship
every remote OS and architecture safely.

JSch supplies the Android SSH client. Bouncy Castle is bundled for modern EdDSA
and XDH algorithms on Android versions whose platform providers do not expose
them.

## Runtime behavior

The shared Kotlin Multiplatform module owns JSON Lines framing, request matching,
reconnect policy, tree/chat stores, terminal events, and transcript state. The
Android source set supplies SSH and encrypted credential storage.

The app keeps no foreground service. Leaving it closes SSH, but the remote host
broker continues to own active terminals and agent turns. Returning reconnects
to that broker and takes fresh snapshots because the event stream itself is not
resumable.

The mobile client remains remote-only by design. Do not add local transport,
on-device agents, an offline database, or a cache without revisiting this model.

## Build requirements

- Docker with BuildKit

The checked-in Gradle wrapper, JDK 21, and Android SDK 35 run inside the build
image. No host Gradle, JDK, or Android SDK is required.

From the repository root:

```sh
make mobile-test
make mobile-android
```

`mobile-test` builds the Docker test stage and runs the shared JVM and Android
unit suites. `mobile-android` also builds, signs, aligns, verifies, and exports:

```text
dist/mobile/xd-mobile-debug.apk
```

Install it with:

```sh
adb install -r dist/mobile/xd-mobile-debug.apk
```

## Nightly APK

The rolling nightly release publishes `xd-nightly-android.apk` beside the Linux
and macOS artifacts. Debug and nightly APKs use the checked-in
`mobile/androidApp/debug.p12` key so updates install over earlier builds.

That key is intentionally public and proves nothing about who built the APK. A
production release would require a consistently protected release key.

## iOS targets

`mobile/shared` declares `iosArm64` and `iosSimulatorArm64` so common protocol and
state code cannot drift into Android-only APIs. Linux Docker builds configure but
cannot compile Apple targets. The macOS job in `.github/workflows/mobile.yml` is
the build authority for them.

The current SSH transport is Android-only. An iOS application will need native
SSH transport and Keychain-backed credentials while preserving the same strict
host-key confirmation contract.
