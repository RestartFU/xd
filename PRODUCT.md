# Product

<!-- impeccable:product-schema 1 -->

## Platform

adaptive

Two clients share one product: the desktop app (Rust + GPUI 0.2.2, custom window chrome, Linux/macOS/Windows) and the Android app (Kotlin, `mobile/`). The desktop follows its own shell; Android follows Material 3.

## Users

Primarily the author, using xd as a daily driver: a developer running several AI coding agents (Codex, Claude Code, JCode, GitHub Copilot CLI) at once across multiple projects and Git worktrees, for long working sessions. Public users installing from GitHub releases come second.

## Product Purpose

xd is a local-first workspace for agent CLIs the user already has installed. It organizes work as projects, then sessions, with persistent terminals, native chat, diffs, files, and an integrated browser. One persistent daemon owns the sessions, so work keeps running after a window closes. Success means the user can see at a glance what every agent is doing, jump into the one that needs them, and never lose a session.

## Positioning

xd does not ship its own model or agent. It wraps the user's own CLIs, authentication, and configuration in one persistent, multi-session workspace that can also be reached from other machines and from Android over pinned TLS.

## Operating Context

- Many concurrent sessions across several projects (examples in the docs: Lunar, xd, decryptmypack, VPS, PotLounge), each on a branch or worktree.
- Native chat for Codex, Claude Code, and JCode, with streamed replies, activity cards (tool calls, shell runs, token counts), queued prompts, approvals, and questions. Copilot and existing terminal sessions use the CLI in a terminal.
- Side panes: changes/diff, files, shell terminal, and an integrated browser with per-chat tabs.
- Local mode, or paired remote mode (Settings, then Pair a device). Remote desktops and Android connect directly to the daemon.

## Capabilities and Constraints

- Desktop UI is GPUI: no CSS, no web components; everything is drawn in Rust in `desktop/src/main.rs` and its siblings. The palette lives in `desktop/src/theme.rs` as semantic roles.
- Themes: exactly two, Dark and Light (confirmed 2026-10-23). Saved legacy theme names load as Dark.
- The Linux browser pane is WebKitGTK over X11.
- Redesign scope (confirmed 2026-10-23): the whole UI, including navigation and flows, on both desktop and Android. All existing features and behavior stay.

## Brand Commitments

- The name is lowercase `xd`. There is an existing app icon in `desktop/assets/icons`.
- No other binding visual commitments. The current look was explicitly rejected ("the dark mode is low key ass").

## Evidence on Hand

- Real screenshots of the previous design: `docs/assets/xd-sessions.png`, `xd-projects.png`, `xd-terminal.png`, `xd-ssh.png`.
- No testimonials, metrics, or users to quote. None may be invented.

## Product Principles

1. Project, then session: the user picks a project first and then its sessions. That hierarchy is the primary navigation (confirmed 2026-10-23).
2. Work side by side: chat, diff, terminal, and browser stay visible together and resizable. Keyboard shortcuts and focus behavior do not change (confirmed).
3. The agent's output is the content. Chrome stays calm and out of its way: no glow, gradients, or motion that competes with work (confirmed: "too flashy" is the failure).
4. Session state (running, waiting on the user, idle, failed) and connection (local or remote, connected or reconnecting) are always legible (inferred from the activity and remote features).
