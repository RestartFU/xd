---
version: 1
slug: "desktop-src-main-rs"
primary_target: "desktop/src/main.rs"
related_targets: ["desktop/src/workspace_settings.rs", "mobile/androidApp/src/main/kotlin"]
---

# xd workspace: desktop and Android

Mode: Operate. Scope: the native working UI on desktop and Android. Users organize projects, open a session, and switch between independent chat tabs while retaining a shared workspace and tools.

## Direction contract

The user selected a T3 Code-like workspace, neutral charcoal with clear glass, a neutral silver Light theme, and real window controls without decorative Mac traffic lights. This replaces the former card-catalog visual world. `DESIGN.md` records the implemented tokens and platform differences.

Desktop uses a compact project/session sidebar and a generous conversation column. Separate chat tabs sit below the session header. Context and empty-state text are centered above a naturally sized composer. Settings has section navigation. Files, Changes, Browser and Terminal stay accessible alongside the conversation.

Android carries the same palette and session/chat hierarchy through Material 3. Phones use bottom navigation; tablets use a rail. Scrollable chat tabs reveal the active conversation, each chat restores its own draft and agent settings, and workspace tools remain shared.

## Verification contract

Use actual native captures with synthetic content, matching viewport/theme/state before and after. Cover desktop at 2560×1440, compact native layout, Android phone and tablet, both themes and enlarged text. Check independent drafts, tab close/reopen, new sibling chat, shared terminal output and files/changes, and real window controls. Public proof lives in `docs/assets/workspace-redesign`; runtime pairing and credentials stay out of public artifacts.
