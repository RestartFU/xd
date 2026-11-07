# Mobile development

The Android client under `mobile/` is a remote view onto an xd daemon:

```text
Android ── pinned TLS + device token ── xd daemon
```

## Connection and credentials

Open **Settings → Pair a device** on the host and copy its pairing code. On
Android's pairing screen, enter a LAN or Tailscale address that reaches the host
and paste the code. The code pins the exact host certificate before authentication. A one-use,
five-minute code exchanges for a remembered device token. Revocation or a
certificate change stops reconnect attempts and requires fresh pairing.

Android stores the endpoint, certificate pin, and token as one AES-256-GCM
credential record protected by Android Keystore. Existing SSH records are not
used by the new transport: pair again after upgrading.

## Runtime behavior

The shared Kotlin module owns JSON Lines framing, request matching, reconnect
policy, tree/chat stores, terminal events, and transcript state. Android supplies
pinned TLS transport and encrypted credential storage. Navigation follows
**Project → Session → Chat tabs**. A session owns one workspace; each chat keeps
its own history, draft, attachments, assistant, model, and agent settings.
Switching tabs restores that chat without replacing the other conversations.
Native chat retains activity, questions, and approvals.

**Changes**, **Files**, and **Terminal** belong to the shared session. Switching
chat tabs keeps the same workspace, open file state, changes view, and shell.
Closing a chat tab hides it without deleting its history or stopping its agent;
use **All chats** to close or reopen it. Phone chats hide the bottom app navigation
and inline close buttons; the header back action returns to projects. Permanent deletion is a separate action with a
confirmation.

**Settings**, the gear in the header, holds the theme, **Read replies aloud** for
text the agent marks with `<speak>` tags, the all-permissions switch, the host's
prompt shortcuts, host updates, and disconnecting. The shortcuts edited there appear
in every project; a project's **⋮** menu edits the ones only that project adds.
**New project** can browse the host's folders for the repository instead of typing
its path.

A session's **⋮** menu settles, renames, or deletes it, and keeps it out of
auto-settle. Settled sessions move to the **Settled** shelf below the projects,
most recently settled first; **Undo** appears for a moment after settling, and the
undo arrow on a shelf row brings one back. Sending a settled session a message
also brings it back. **Settings → Auto-settle** changes the host's rules. A
project's **⋮** menu renames or deletes it. After a turn is stopped, cut off, or
ended by a usage limit, the send button becomes a play button while the composer
is empty, and pressing it continues the chat. Tap a queued message to move it
earlier or later.

**New session** chooses where the session works, as on the desktop: one of the
project's checkouts, a new worktree, or a branch or open pull request, which the
host checks out in its own managed worktree. **Changes** shows the branch, how far
it is ahead or behind, and what is uncommitted; **Commit all** commits every change
with the message typed beside it. When the pushed branch has a pull request, its
number links to it from Changes and from the session header.

Settings also manages the host as the desktop does: **Chat titles** turns titling
on and picks each assistant's title model, **Assistant accounts** signs the host's
Codex and Claude CLIs in and updates them, and **Agent secrets** edits the
environment variables agents get; secret values are write-only and never sent
back. A project's **⋮** menu edits its **Project context** and its own secrets.
**Resume chats after a restart** continues turns a host restart cut off the next
time this phone connects; with it off the phone does not answer, so a desktop set
to resume them still can. Paired devices can only be managed on the host itself.

Older hosts appear as one chat per session. Update the connected host before
adding another chat inside an existing session; Android displays an update
message instead of creating a separate session. The host automatically deletes
legacy terminal chats and their saved history when opening the database; their
worktrees are retained.

Leaving the app closes its connection; returning reconnects and takes fresh
snapshots. Work and pending approvals continue in the host daemon. There is no
on-device agent, database, offline chat cache, or background mobile service.

See [paired connections](remote.md) for host setup and device management.

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

## Published APKs

Stable releases publish `xd-android.apk` and its SHA-256 checksum. A stable
release fails if signing credentials or the Android artifact are missing.

The rolling nightly release publishes `xd-nightly-android.apk` beside the Linux
and macOS artifacts. Published stable and nightly APKs require the protected CI
release key and are checked after building for a valid non-debug signature and
non-debuggable manifest. Nightly, stable, and dev builds use separate package
IDs and matching remote data channels.

APKs published before protected signing used the public debug certificate. They
cannot safely update in place to a private trust root, so the protected build
uses a new package ID and installs beside the legacy app. Remove the legacy app
after reconnecting the protected build; Android does not migrate the old app's credentials automatically.

## iOS targets

`mobile/shared` declares `iosArm64` and `iosSimulatorArm64` so common protocol and
state code cannot drift into Android-only APIs. Linux Docker builds configure but
cannot compile Apple targets. The macOS job in `.github/workflows/mobile.yml` is
the build authority for them.

The current TLS transport is Android-only. An iOS application will need native
pinned TLS transport and Keychain-backed device credentials.
