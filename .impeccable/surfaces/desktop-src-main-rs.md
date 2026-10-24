---
version: 1
slug: "desktop-src-main-rs"
primary_target: "desktop/src/main.rs"
related_targets: ["mobile/androidApp/src/main/kotlin"]
---

# xd app shell: desktop and Android

Scope: the whole xd working UI. Desktop is GPUI (`desktop/src`); Android is Compose (`mobile/androidApp`). Mode: Operate. Themes: Dark and Light only.
Audience and task: the author, running many agent sessions. They pick a project, then a session, and work in chat with diff, terminal, and browser side by side. Shortcuts stay unchanged. Failure condition: "too flashy".

## Direction contract

THESIS: the library card catalog. Projects are drawers and sessions are index cards. A metal signal tab clipped to each card says what its agent is doing. This replaces the gray editor shell with its nav-rail mode switch: one catalog sidebar replaces the Projects/Sessions route split.

OWN-WORLD: neutral graphite ground (steered cooler, not walnut) and bone card-stock ink. Each card carries a thin red ruled line under its title. The current project's drawer name sits in an outlined label frame. Signal tabs differ by shape and hue: solid green (working), a taller solid amber flag (waiting on you; the only amber), and hollow blue (finished, unread). The primary action is a bone fill with ink text, and links and focus are ink blue. Type is Libre Franklin for the UI and JetBrains Mono only for code and terminal. Light mode is white card stock with ink.

STORY: at a glance the user sees which drawer holds an agent that needs them. They open it, pick the card, and work. Nothing vanishes: a failed tool run stays stamped in the transcript. Sessions carry no failed or queued state, so cards don't show one.

FIRST VIEWPORT: a 38px title bar with the mark, breadcrumb, and connection. A 296px catalog sidebar holds Find, then drawers: closed drawers are one 34px line with their tabs, and the open drawer has its label frame and 54px cards. The footer has New session, Terminal, and Settings. The centre is the session header and the chat column; the composer sits at the bottom. The right dock has the Changes/Files/Terminal/Browser tabs.

FORM: Card Catalog, position 7 of the ordered list (assigned), plus raises: state shown in tab form as well as hue, one reserved amber, stamped failures, the open drawer lifts, and a fixed card height and baseline. Seed key 7613b7f0; the roll printed no SEED REFERENCE line.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
