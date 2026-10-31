---
name: xd
description: A clear workspace for your coding agents.
colors:
  dark-background: "#1b1b1b"
  dark-sidebar: "#202020"
  dark-surface: "#292929"
  dark-surface-high: "#333333"
  dark-border: "#383838"
  dark-selected-surface: "#373737"
  dark-selected-border: "#858585"
  dark-text: "#f1f1f1"
  dark-muted: "#b1b1b1"
  dark-faint: "#ababab"
  dark-accent: "#dedede"
  dark-accent-ink: "#dedede"
  dark-accent-hover: "#f2f2f2"
  dark-accent-text: "#1b1b1b"
  dark-rule: "#383838"
  dark-working: "#8dceb0"
  dark-waiting: "#e2c28d"
  dark-done: "#dedede"
  dark-danger: "#efa7ad"
  dark-danger-surface: "#2a1616"
  dark-diff-added: "#263b30"
  dark-diff-removed: "#462c30"
  dark-selection: "#ffffff35"
  dark-backdrop: "#000000b3"
  dark-chrome-glass: "#292929d9"
  dark-sidebar-glass: "#202020df"
  dark-composer-glass: "#303030e8"
  light-background: "#fafafa"
  light-sidebar: "#e4e4e4"
  light-surface: "#ffffff"
  light-surface-high: "#efefef"
  light-border: "#d0d0d0"
  light-selected-surface: "#ededed"
  light-selected-border: "#7d7d7d"
  light-text: "#202020"
  light-muted: "#5a5a5a"
  light-faint: "#606060"
  light-accent: "#363636"
  light-accent-ink: "#444444"
  light-accent-hover: "#202020"
  light-accent-text: "#ffffff"
  light-rule: "#d0d0d0"
  light-working: "#226347"
  light-waiting: "#745319"
  light-done: "#444444"
  light-danger: "#a13242"
  light-danger-surface: "#fdeeee"
  light-diff-added: "#e6f5eb"
  light-diff-removed: "#fcebeb"
  light-selection: "#20202030"
  light-backdrop: "#18181b66"
  light-chrome-glass: "#f1f1f1dc"
  light-sidebar-glass: "#e4e4e4dc"
  light-composer-glass: "#ffffffd9"
typography:
  display:
    fontFamily: "Inter"
    fontSize: "28px"
    fontWeight: 500
  title:
    fontFamily: "Inter"
    fontSize: "13px"
    fontWeight: 500
  body:
    fontFamily: "Inter"
    fontSize: "14px"
    fontWeight: 400
  label:
    fontFamily: "Inter"
    fontSize: "12px"
    fontWeight: 400
  mono:
    fontFamily: "JetBrains Mono"
    fontSize: "12.5px"
    fontWeight: 400
    lineHeight: "19px"
rounded:
  control: "6px"
  tab: "7px"
  panel: "8px"
  composer: "16px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "24px"
components:
  button-primary:
    backgroundColor: "{colors.dark-accent}"
    textColor: "{colors.dark-accent-text}"
    rounded: "{rounded.control}"
    height: "30px"
    padding: "0 12px"
  button-secondary:
    backgroundColor: "{colors.dark-surface}"
    textColor: "{colors.dark-text}"
    rounded: "{rounded.control}"
    height: "30px"
  button-ghost:
    textColor: "{colors.dark-muted}"
    rounded: "{rounded.control}"
    height: "30px"
  chat-tab:
    backgroundColor: "{colors.dark-selected-surface}"
    textColor: "{colors.dark-text}"
    rounded: "{rounded.tab}"
    height: "30px"
  composer:
    backgroundColor: "{colors.dark-composer-glass}"
    textColor: "{colors.dark-text}"
    rounded: "{rounded.composer}"
    width: "736px"
---
# Design System: xd

## Overview

**Creative North Star: "A Clear Workspace"**

xd uses neutral charcoal and clear glass to frame a generous conversation area. Projects contain sessions; each session contains separate chat tabs sharing one workspace. The sidebar stays compact, chat controls sit beside the conversation, and workspace tools remain available alongside it.

Dark uses charcoal surfaces and silver actions. Light uses neutral silver chrome and white reading surfaces. Desktop translucency belongs to chrome and the composer; text, code and tool output keep a stable reading background. Android carries the same palette and hierarchy through native Material 3 layouts.

**Key Characteristics:**

- Two themes: Dark and Light. Retired saved names load as Dark.
- Inter for prose and controls; JetBrains Mono for code and terminal text.
- Separate chat tabs share a session workspace and terminal.
- Real platform window controls; no decorative traffic lights.
- Neutral selection and focus, with color reserved for meaningful status.

## Colors

The normative desktop values above come from `desktop/src/theme.rs`. Roles pair by the `dark-` and `light-` prefixes. `selection`, `backdrop` and the three glass roles include alpha as `#RRGGBBAA`; other values are opaque.

Neutral surfaces distinguish canvas, sidebar, panels, hover and selection. Primary actions use a silver fill with dark ink in Dark, and charcoal with white ink in Light. Links and focus stay neutral. Working is green, waiting is amber, and destructive actions are rose. Status also has text or an icon.

Android uses complete Material color roles in `ui/Theme.kt`, with small platform-specific surface adjustments. Its charcoal conversation surface matches the desktop canvas; silver action, text and signal colors follow the same roles. Legacy accent preferences remain readable but do not recolor the main workspace.

## Typography

Inter is bundled for interface text, prose and headings. JetBrains Mono is bundled for source, terminal output, paths and branches. Desktop UI controls typically use 12–13px, body copy 14px, and the empty-chat heading 28px. Android maps Inter onto the Material type scale and respects system font scaling.

Keep the conversation readable at its actual window size. Titles truncate where necessary; explanatory text wraps and determines its own height.

## Layout

Desktop has a 256px project sidebar, a session header, a separate 43px chat-tab row, and a central conversation. The regular composer and chat measure cap at 736px; an empty conversation centers a 660px composer beneath its context, heading and description. A short transcript starts at the top; longer transcripts retain scrolling and the existing latest-output behavior.

Changes, Files, Browser and Terminal belong to the session workspace. Chat tabs control independent conversations. Desktop settings use a bounded panel up to 900×760px, with section navigation that changes to a horizontal strip in compact windows. The application minimum is 760×560px.

Android uses bottom navigation outside an open chat on compact screens and a navigation rail from 600dp. Phone chats use the header back action to return to projects. Chat tabs scroll horizontally and reveal the selected chat. Changes, Files and Terminal remain separate workspace tabs below them. Touch controls preserve 48dp targets; dialogs and reading areas respect system insets and enlarged text.

## Elevation & Depth

Desktop requests a blurred native window background and uses translucent title, sidebar and composer roles. The compositor determines the final blur; unsupported environments retain the neutral tonal hierarchy. Native OS decorations remain responsible for window controls where provided. Reading surfaces stay opaque, and floating menus and dialogs retain their existing shadows.

Android expresses the material through opaque layered neutral surfaces and restrained outlines, without compositor blur. It retains native dialogs, ripples and navigation behavior.

## Shapes

Controls use 6px corners, chat tabs 7px, panels 8px, and the composer 16px on desktop. Fine borders separate chrome without decorative rules. Android keeps Material shapes and touch sizing, with a rounded composer and tonal active tabs.

## Components

**Project navigation.** Each project reveals its sessions. One sidebar row represents a session even when it contains several chats. The row restores that session's last active chat and aggregates working, waiting and unread activity.

**Chat tabs.** Each tab has an agent mark and title. Add creates another chat sharing the workspace. Close hides a tab without deleting history; Reopen on desktop and All chats on Android restore it. Phone tabs omit inline close buttons; All chats contains Close chat. Explicit rename and confirmed delete remain separate actions. Desktop keeps the final visible chat open. Android can hide all tabs and shows a saved-chats state with reopen and create actions.

**Composer.** A bordered translucent desktop surface, or opaque Material surface on Android, holds the editor, attachments, agent options and send action. Empty-state text uses natural height and a clear gap above it. While another chat's metadata loads, its editor waits for the correct saved draft.

**Workspace tools.** Files, Changes, Browser and Terminal retain session scope across sibling chat switches. Agent history, options, approvals and drafts retain chat scope. Hosts without grouped-session support remain usable with singleton sessions and show an update message when adding a sibling chat.

**Settings.** Desktop groups Appearance, General, Agents, Devices and Shortcuts in one panel using existing capabilities. Android preserves its native settings dialog. Both offer Dark and Light.

**Buttons and fields.** Primary actions use the neutral action role; secondary and ghost controls use tonal hover and focus feedback. Destructive actions use the danger role and explicit confirmation where data is removed. Search and text fields retain visible focus and readable hints.

## Do's and Don'ts

### Do:

- Do use catalog display names for models and readable labels for known settings; preserve custom identifiers when no display name is available.
- Do use semantic ThemeColors or Material color roles in both themes.
- Do center the empty-state context and heading over the composer, allowing natural text height.
- Do preserve independent chat histories, drafts and agent settings when switching tabs.
- Do retain native window, keyboard, touch and system inset behavior.
- Do keep opaque reading surfaces beneath prose, code and terminal output.

### Don't:

- Don't restore the retired red rules, bone palette or card-catalog metaphor.
- Don't make closing a chat tab delete its conversation.
- Don't use fake Mac window controls on other platforms.
- Don't depend on translucency or color alone to identify selection or status.
- Don't scale desktop layouts into phone thumbnails; use native responsive structure.
