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

Every build is signed with the checked-in key described under Nightly APK, so
a rebuild installs over the last one and keeps its pairing credentials.

## Nightly APK

The rolling nightly release publishes `xd-nightly-android.apk` beside the
Linux, macOS, and Windows artifacts. It is the same APK `make mobile-android`
produces: a test build requiring no secrets or other setup.

It is signed with `mobile/androidApp/debug.p12`, a fixed key checked into the
repository on purpose. Gradle otherwise generates one per machine and a CI
runner starts empty, so every nightly would carry a different signature and
Android would refuse to install over the last one -- "App not installed as
package conflicts with an existing package". One key for every build, local
and CI, means updates install in place.

That key is not a secret and must never be treated as one. It is the same idea
as Android's own debug keystore, whose password is public: it proves nothing
about who built the APK. Publishing this app properly would need a real key
kept out of the repository, and the nightly would then have to be signed with
it consistently.

If you already have an xd build installed that predates this, uninstall it
once; from then on updates apply normally.

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

## Markdown

Assistant messages are parsed as CommonMark with GitHub tables and
strikethrough, using `org.jetbrains:markdown` -- multiplatform down to the
Apple targets. The desktop uses the `markd` shard for the same job; a correct
parser is not something to hand-roll on either side.

Parsing produces a document of blocks and spans in `shared/.../markdown`, not
styled text, so each client draws it natively from one interpretation of the
source. Compose renders it today; SwiftUI will render the same document.

Only `http`, `https` and `mailto` links survive, matching the desktop's
`safe_link?`. Anything else keeps its text and loses the link, so a
`javascript:` or `file:` target can never be handed to the system. Images have
nowhere to be drawn in a transcript, so they read as their alt text.

A user's own message and system text are shown verbatim. They were typed
rather than authored as Markdown, and rendering them would eat characters the
sender meant literally.

Assistant output streams, so the parser is called on half-written documents
constantly; anything it cannot make sense of falls back to the literal text.

## Vector drawables

Android's `PathParser` reads numbers greedily and cannot split packed SVG arc
flags: `0 014.21` becomes the number 14.21 rather than flags `0` and `1`
followed by `4.21`, and the shape comes out mangled. SVG optimisers emit that
form routinely, so an icon pasted straight from one looks right everywhere
except on Android, and nothing in the build notices -- the drawable compiles
happily.

`make mobile-test` fails if any drawable packs arc flags. Separate them:
`a1 1 0 0 1 2 2`, never `a1 1 0 012 2`.

## Syntax colouring

Code in the transcript is coloured with the desktop's palette and language
list: expanded inline diffs, and fenced code blocks in assistant messages.
`mobile/shared/.../syntax` holds it, so an iOS client gets it unchanged.

Two things are shared with the desktop by construction rather than by hand:
the token colours come from `Xd::SyntaxToken#colour`, and the keyword, type
and constant sets in `Words.kt` are generated from `src/xd/syntax.cr` (899
words across 40 sets). Regenerate them when that file changes.

`languageForPath` is an exact port, so a file resolves to the same language on
both clients.

The scanner covers what code spends its time in: comments including nested
block comments, strings with escapes, Go and Odin raw strings, Rust raw
strings, Kotlin and TOML triple strings, numbers, keywords, types, constants
and call sites.

It deliberately does **not** implement the desktop's exotic lexing: heredocs,
Ruby and Crystal percent literals and regex, C# verbatim and raw strings, Bash
expansion and quoting state, Crystal macro delimiters, TOML table headers, and
YAML anchors. Those constructs render as plain text. That is a deliberate
trade: uncoloured reads fine, mis-coloured does not, and the desktop remains
the place to read a large diff closely.

## A running turn

While a turn runs, a row at the foot of the transcript counts how long it has
been going, with the same moving dots and the same wording as the desktop --
`TurnTiming` lives in `shared` so the two clients cannot drift apart on
phrasing. A turn can spend minutes between visible output, thinking or inside a
tool that prints nothing, and without this the phone looks like it dropped the
message.

The count comes from a start time, not from counting up. Opening a chat whose
turn is already running shows its real age: `chat` reports `working_for`, and
the transcript store turns that into a start time. A phone's clock can disagree
with the daemon's, so a negative elapsed time is clamped rather than shown.

## Panes

The desktop shows the conversation, terminal, files and diff side by side. A
phone has room for one at a time, so they are tabs over the same chat.

**Diff** reads whole patches with `working-all` and `branch-all` rather than
the desktop's per-file sections, because a phone shows one scrollable patch.
`branch-all` needs the branch point, so switching to it costs a `base` read
first.

**Files** lists and previews through `file-browse`, syntax colouring the
preview by path. It is read-only: the daemon supports `write`, but editing
code on a phone is not what this is for. Paths stay relative to the working
directory because the daemon refuses anything else.

**Terminal** attaches to the shared pty. The session lives on the daemon and
every attached device sees the same screen, so opening reuses an existing
terminal rather than starting a second one; replay rebuilds the scrollback,
resize frames included and in order, before live output is applied.

Because the daemon broadcasts raw pty bytes, the client has to interpret them.
`shared/.../terminal` holds a VT100/xterm subset: cursor movement, erase,
scrolling and SGR colour, including bright and 256-colour. Unsupported escapes
are consumed rather than printed, so they cost formatting instead of turning
the screen into noise.

Input goes straight to the pty as it is typed, with no submit step, so tab
completion, Ctrl-C and a live prompt behave as they should. Tapping the screen
raises the keyboard, and the cursor is drawn as a block where the next
character will land.

Android delivers typed characters as text edits rather than key events, so the
keyboard is driven through a one-pixel field the reader never sees; its value
is padded because a backspace on an already-empty field is otherwise
unobservable. A key bar supplies what a soft keyboard cannot send at all --
Esc, Tab, arrows, and a sticky Ctrl that applies to the next character.

One limit worth knowing: a full-screen application will not render faithfully.
The desktop keeps VTE for that.

## Images in the transcript

Sending an attachment stores the PNG on the daemon and leaves
`[image: /path.png]` on its own line in the message, so a sent image would
otherwise read as that literal text. The transcript recognises those markers
and draws the image, keeping prose and images in the order they were written.

The rule matches `Xd::Agent::ImageReference`: the whole line, and nothing else
on it. A message that merely mentions `[image: ...]` mid-sentence stays prose.

Bytes live on the daemon, so each one is fetched with `image-read`, which
serves only paths inside its own remote-paste directory. The scaled preview is
requested rather than the original: a transcript never needs full resolution.
Previews are bounded in height so a tall screenshot cannot push the rest of a
turn off screen, and tapping opens the image full screen.

An image sent from another machine, or cleaned up since, reports itself as
unavailable rather than failing the transcript.

## Attaching images

The composer can attach up to 4 images, which is the daemon's limit alongside
10 MiB per image and 20 MiB in total.

The daemon accepts **PNG only** and checks the signature, but a phone gallery
holds JPEG and HEIC, so images are decoded and re-encoded on the device. As on
the desktop they are scaled to fit 1920 first: a modern phone photo encoded to
PNG at full resolution runs to tens of megabytes and would simply be refused.
PNG is lossless, so when a scaled image still exceeds 10 MiB the only remedy
is fewer pixels; the encoder halves the bound until it fits.

Picking uses Android's photo picker, which grants access to the chosen items
only and needs no storage permission. The app still declares nothing beyond
`INTERNET`.
