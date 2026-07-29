# Mobile development

The mobile client is a separate Gradle project under `mobile/`. It is a client
of `xd serve`, not a port of the GTK application or the agent subprocesses.

The shared Kotlin Multiplatform module owns the wire protocol, positional call
queue, reconnect policy, tree/chat stores, and transcript state machine. The
Android application supplies the TLS socket, encrypted credential storage, and
native Compose screens. The shared module targets Android and the JVM. Native
iOS targets arrive with the iOS client phase.

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

- split UTF-8 and 48 MiB framing limits;
- positional reply matching after caller cancellation;
- greeting, pairing, reconnect, and fatal pin/protocol behavior;
- the transcript transition matrix;
- a full FakeSocket vertical slice from pairing through streamed output;
- a real JSSE loopback TLS listener for TOFU, exact-leaf pinning, and mismatch
  detection.

The APK build also runs that suite, signs the debug APK, and verifies its
alignment and signature before exporting it.

## Nightly signing

The rolling nightly release builds a non-debuggable
`com.restartfu.xd.mobile.nightly` APK. Its signing material enters the Docker
build only through BuildKit secret mounts; it is not copied into an image
layer. Configure these GitHub Actions secrets before enabling the nightly job:

- `ANDROID_NIGHTLY_KEYSTORE_BASE64`: base64-encoded PKCS#12 keystore;
- `ANDROID_NIGHTLY_KEYSTORE_PASSWORD`;
- `ANDROID_NIGHTLY_KEY_ALIAS`;
- `ANDROID_NIGHTLY_KEY_PASSWORD`.

Keep that key stable. Android rejects an update signed by a different key, so
rotating it requires uninstalling the old nightly app.

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
