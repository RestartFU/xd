# The host: assistants, updates, and memory

One persistent daemon, the xd host, owns local and remote sessions. Work continues
when clients disconnect, and the host applies its own rules (such as
[auto-settle](sessions.md#auto-settle)) while the app is closed.

## Installed assistants

Install and sign in to your assistant CLI on the machine running the xd host.
xd discovers tools through your login shell and reuses their existing
configuration and login state. For remote sessions, the CLI and login belong on
the remote machine. **Settings → Agents → Accounts** refreshes the CLI's current
status. Agent binaries and separate copies of their credentials are not included
in xd.

## Updates

Installed Linux and macOS builds check their own release channel for app updates
shortly after startup and every four minutes: nightly builds follow the nightly
release and release builds follow tagged releases. Dev builds check the latest
`master` commit every minute, and confirming an update builds that commit on your
machine, which needs Docker on Linux.

xd also compares installed Codex and Claude Code versions with their npm releases
every 30 minutes; only the package names are sent.

When something is out of date, an **Update** button appears beside the terminal
and Settings buttons. It lists what will change, and nothing is installed until
you confirm. **Check for updates** under **Settings → General → Updates** checks
manually. Updates always target the local machine, including while you are
viewing a remote workspace.

Restart is blocked while chats are running, a chat's background agents or shells
are still at work, or terminal tabs are open; close the terminal tabs first.
Stored conversations can resume afterward.

Source builds and Windows installations require a manual app update. Set
`XD_DISABLE_AUTO_UPDATE=1` to disable background checks.

## Resuming cut-off turns

After a turn is stopped, cut off, or ended by a usage limit, the composer's send
button becomes a play button while the composer is empty; pressing it continues
the chat.

Turn on **Settings → General → Resume chats after a restart** to have xd continue
any turn that a crash, reboot, or daemon stop cut off, the next time the app
connects to that daemon, local or paired. It posts a visible message in the chat
and the agent continues with no prompt from you, so it is off by default. A
resumed turn that is cut off again waits for you.

The setting belongs to each device, but the host hands out the waiting chats
only once: the first device to ask decides for all of them, and any device that
connects later finds nothing to resume.

- A desktop always asks when it connects. With its setting on it resumes the
  chats; with it off it drops them, so a later change of mind does not restart
  old work.
- An Android device asks only when its setting is on, and then resumes them.
  With the setting off it leaves them waiting for the next device.

Either way, the play button still continues any chat by hand.

## Memory limits

On Linux, a container's memory limit can be set to stop every process in it at
once when it runs out (cgroup v2 `memory.oom.group`). There, a single heavy build
would take down the daemon, every chat and every terminal.

When the host runs in such a container, it watches the memory the kernel cannot
reclaim (page cache does not count) and, once that reaches 92% of the limit,
stops the largest process tree of the same user in the container, such as a
compiler or build tool, whether xd started it or not. It never stops an agent, xd
itself, or the shell that leads each xd terminal, but a shell started inside a
terminal, and anything running in it, can be stopped. The tree is frozen and then
killed with `SIGKILL`, so it gets no chance to save its state. The host log, and
the chat the work ran under when there is one, say what was stopped.

Set `XD_MEMORY_GUARD_PERCENT` to a value from 50 to 99 to change the threshold,
or to `off` to turn this off.

## Credits

The CLI discovery and update behavior adapt portions of
[T3 Code](https://github.com/pingdotgg/t3code) for xd's Rust backend. Its MIT
notice is included in `data/licenses/t3code-LICENSE` and packaged builds.
