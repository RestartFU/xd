# Mobile development

The mobile client is a separate Gradle project under `mobile/`. It is a client
of `xd serve`, not a port of the GTK application or the agent subprocesses.

The shared Kotlin Multiplatform module owns the wire protocol, the multiplexed
call queue, reconnect policy, tree/chat stores, and transcript state machine.
Each application supplies the TLS socket, encrypted credential storage, and
native screens. The shared module targets Android, the JVM, and iOS; the
SwiftUI application arrives in the iOS client phase.

`docs/protocol.md` is the normative wire contract this module implements.

## Remote only

The mobile client is **remote only**, by construction and on purpose:

- It reaches the daemon exclusively over pinned TLS. There is no Unix-socket or
  local-IPC path, so it can only ever talk to `listen_remote`.
- It runs no agent, no Git, and no SQLite. The phone is a view onto a daemon
  that owns all of that.
- It stores no chat data on the device. `TreeStore`, `ChatSession`, and
  `TranscriptMachine` are in-memory and rebuilt from `tree`, `chat`, and
  `messages` on every connect. The only persisted record is the credential —
  host, port, device token, and pinned leaf certificate.
- It cannot pair further devices. The daemon restricts `peer-pairing` to local
  transport, so pairing authority stays on the daemon machine.

Do not add a local transport, an on-device database, or an offline cache
without revisiting this section first.

## iOS targets

`mobile/shared` declares `iosArm64` and `iosSimulatorArm64`. `iosX64` is
deliberately absent: the Intel simulator is dead weight and this repository's
macOS builders already require Apple Silicon.

The targets exist now so the protocol, store, and transcript code cannot drift
into being Android-shaped. What is present today:

- `iosMain` supplies `currentEpochMillis`, the module's only `expect`.
- The whole common suite — framing, call multiplexing, reconnect, transcript
  watermark — compiles and runs on the simulator.

The iOS platform glue (a Network.framework socket with
`sec_protocol_options_set_verify_block` pinning, and a Keychain
`CredentialStore`) lands with the SwiftUI application, for the same reason
`AndroidSocket` landed with the Android application: both are app-side plumbing
that must be compiled and exercised on a real host, not written blind.

**Apple targets cannot be compiled on Linux.** `make mobile-test` configures
them but skips every Apple compilation — `compileIosMainKotlinMetadata` reports
`SKIPPED`. The `ios` job in `.github/workflows/mobile.yml` runs on macOS and is
the only thing that actually builds them. If that job is removed, the iOS
source set rots silently.

## Requirement

- Docker with BuildKit

The checked-in wrapper, JDK 21, and Android SDK 35 run inside the build image.
No host Gradle, JDK, or Android SDK is required. Android Studio may still
create `mobile/local.properties` for IDE use; that file is ignored.

## Commands

From the repository root:

```sh
make mobile-test
make mobile-android
```

The test command builds the Docker `test` stage. The Android command exports
the debug APK to `dist/mobile/xd-mobile-debug.apk`.

`mobile-test` includes:

- split UTF-8 and 96 MiB framing limits;
- reply multiplexing by `_xd_request`, including out-of-order replies, the
  id-less compatibility path, and an abandoned call whose late reply must not
  shift onto the next caller;
- greeting, pairing, reconnect, and fatal pin/protocol behavior;
- the transcript transition matrix, including the turn watermark that keeps a
  snapshot-covered delta from being applied twice;
- a full FakeSocket vertical slice from pairing through streamed output;
- a real JSSE loopback TLS listener for TOFU, exact-leaf pinning, and mismatch
  detection.

The APK build also runs that suite, signs the debug APK, and verifies its
alignment and signature before exporting it.

Docker keeps the generated debug signing key in the
`xd-mobile-debug-signing` BuildKit cache. Rebuilding can update an installed
debug APK without losing pairing credentials. Deleting that cache rotates the
key; Android then requires uninstalling the old debug app.

## Nightly APK

The rolling nightly release publishes a non-debuggable
`com.restartfu.xd.mobile.nightly` APK as `xd-nightly-android.apk`, beside the
Linux, macOS, and Windows artifacts. Its `versionCode` is the workflow run
number, so each build supersedes the last; its application id is distinct from
the debug build, so both can be installed at once.

**The APK is only published once signing secrets exist.** An unsigned APK
cannot be installed and a rotating key cannot update one in place, so without a
keystore the nightly job builds nothing, the release omits the APK, and the run
records a warning. Every other artifact still publishes.

Two secrets are required:

- `ANDROID_NIGHTLY_KEYSTORE_BASE64`: base64-encoded PKCS#12 keystore;
- `ANDROID_NIGHTLY_KEYSTORE_PASSWORD`.

Two more are optional, for a keystore whose key differs from its store —
uncommon for PKCS#12, where one password normally covers both:

- `ANDROID_NIGHTLY_KEY_ALIAS`: defaults to `xd-nightly`;
- `ANDROID_NIGHTLY_KEY_PASSWORD`: defaults to the keystore password.

Generate a keystore and print the secret:

```sh
keytool -genkeypair -storetype PKCS12 -keystore xd-nightly.p12 \
  -alias xd-nightly -keyalg RSA -keysize 2048 -validity 10000 \
  -dname "CN=xd nightly"
base64 -w0 xd-nightly.p12
```

Signing material enters the Docker build only through BuildKit secret mounts;
it is never copied into an image layer.

**Keep that key.** Android rejects an update signed by a different key, so
losing or rotating it forces every user to uninstall the old nightly app.

To build one locally:

```sh
XD_ANDROID_KEYSTORE=./xd-nightly.p12 \
XD_ANDROID_KEYSTORE_PASSWORD=… \
XD_ANDROID_VERSION_CODE=1 \
./scripts/mobile-nightly.sh
```

## Try it on a phone

Start a pairing window on the machine that owns the workspaces:

```sh
./dist/xd.sh serve --pair
```

The daemon prints a single-use `XXXX-XXXX` code valid for five minutes. Install
the Docker-built APK, open it, and enter the daemon's reachable hostname or
Tailscale IP, port `4001`, device name, and code:

```sh
adb install -r dist/mobile/xd-mobile-debug.apk
```

After pairing, Android stores the device token and the exact leaf certificate
as one AES-256-GCM record backed by Android Keystore. Later connections accept
only that certificate. A certificate change stops reconnects and requires an
explicit forget-and-re-pair flow; there is no trust-anyway action.

The app deliberately keeps no foreground service. Leaving it backgrounds and
closes the socket; returning reconnects, reloads the tree, and reloads every
open chat from the daemon.

For IDE use, the Gradle wrapper remains available from `mobile/`:

```sh
./gradlew :shared:allTests
./gradlew :androidApp:assembleDebug
```

Direct wrapper use requires a local JDK 21 and Android SDK 35.
