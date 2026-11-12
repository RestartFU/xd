---
name: xd
description: A clear workspace for your coding agents.
colors:
  dark-background: "#0b0b0b"
  dark-sidebar: "#0f0f0f"
  dark-surface: "#111111"
  dark-surface-high: "#1b1b1b"
  dark-border: "#2a2a2a"
  dark-selected-surface: "#222222"
  dark-selected-border: "#7178e6"
  dark-text: "#f2f2f2"
  dark-codex-icon: "#ffffff"
  dark-claude-icon: "#d97757"
  dark-muted: "#adadad"
  dark-faint: "#929292"
  dark-accent: "#8b8ff8"
  dark-accent-ink: "#a9adff"
  dark-accent-hover: "#a6a9ff"
  dark-accent-text: "#0e1016"
  dark-rule: "#262626"
  dark-working: "#63d8a8"
  dark-waiting: "#f0c06a"
  dark-done: "#a9adff"
  dark-danger: "#ff94a2"
  dark-danger-surface: "#2a1416"
  dark-diff-added: "#142b20"
  dark-diff-removed: "#351a20"
  dark-selection: "#8b8ff840"
  dark-backdrop: "#000000b3"
  dark-chrome: "#0f0f0f"
  dark-composer: "#161616"
  dark-glass-backdrop: "#0b0b0b"
  dark-glass-pane: "#ffffff04"
  dark-glass-composer: "#161616d9"
  dark-glass-edge: "#ffffff1f"
  light-background: "#eef0f9"
  light-sidebar: "#f3f4fb"
  light-surface: "#f8f9fd"
  light-surface-high: "#eceef7"
  light-border: "#d3d7e6"
  light-selected-surface: "#dfe2f4"
  light-selected-border: "#6366e0"
  light-text: "#1b1e2c"
  light-codex-icon: "#1b1e2c"
  light-claude-icon: "#b85c3f"
  light-muted: "#4b5165"
  light-faint: "#5a6074"
  light-accent: "#5257d9"
  light-accent-ink: "#4045bb"
  light-accent-hover: "#4347c4"
  light-accent-text: "#ffffff"
  light-rule: "#dadde9"
  light-working: "#17603f"
  light-waiting: "#6c490a"
  light-done: "#4045bb"
  light-danger: "#97203a"
  light-danger-surface: "#fbe9ed"
  light-diff-added: "#dff2e6"
  light-diff-removed: "#f8e1e6"
  light-selection: "#5257d933"
  light-backdrop: "#1b1e2c66"
  light-chrome: "#f3f4fb"
  light-composer: "#ffffff"
  light-glass-backdrop: "#e4e7f4"
  light-glass-pane: "#ffffff8c"
  light-glass-composer: "#ffffffd0"
  light-glass-edge: "#1b1e2c1f"
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
    backgroundColor: "{colors.dark-composer}"
    textColor: "{colors.dark-text}"
    rounded: "{rounded.composer}"
    width: "736px"
---
# Design System: xd

## Overview

**Creative North Star: "A Clear Workspace"**

xd uses frosted glass, in the manner of macOS, to frame a generous conversation area. Projects contain sessions; each session contains separate chat tabs sharing one workspace. The sidebar stays compact, chat controls sit beside the conversation, and workspace tools remain available alongside it.

Dark is black glass: one near-black surface whose panes are only a shade lighter and set apart by hairlines. Light is frosted white over a flat cool grey. Both use an indigo accent for the primary action, links, focus and selection. The glass is painted inside the app, so the window stays opaque and the wallpaper and other windows never show through it. Android carries the same palette and hierarchy through native Material 3 layouts.

**Key Characteristics:**

- Two themes, Dark and Light, both frosted glass drawn inside an opaque window. Retired saved names load as Dark.
- Inter for prose and controls; JetBrains Mono for code and terminal text.
- Separate chat tabs share a session workspace and terminal.
- Real platform window controls; no decorative traffic lights.
- An indigo accent for actions, links, selection and focus; other color is reserved for meaningful status.

## Colors

The normative desktop values above come from `desktop/src/theme.rs`. Roles pair by the `dark-` and `light-` prefixes, with each theme's glass under `-glass-`. `selection`, `backdrop` and the glass `pane`, `composer` and `edge` include alpha as `#RRGGBBAA`; other values are opaque.

Neutral surfaces distinguish canvas, sidebar, panels, hover and selection. The only hue outside status is the indigo accent: an indigo fill for the primary action (dark ink in Dark, white in Light), and indigo for links, focus, the caret, selection and the selected-row border. Working is green, waiting is amber, and destructive actions are rose. Desktop marks questions and approvals with a steady hollow amber circle and ‘Waiting on you’ in session rows and the chat header. Chat tabs use the same ring. On desktop, a question, approval or quota stop docks inside the composer as a section above a hairline, marked only by the ring, with numbered choices; Send submits the editor's text as a typed answer. Once answered, the reply carries the question above its bubble in faint text after a reply arrow, on desktop and mobile. Status also has text or an icon.

Android uses complete Material color roles in `ui/Theme.kt`, with small platform-specific surface adjustments. Its conversation sits on the same glass backdrop as the desktop canvas; action, text and signal colors follow the same roles. Legacy accent preferences remain readable but do not recolor the main workspace.

## Typography

Inter is bundled for interface text, prose and headings. JetBrains Mono is bundled for source, terminal output, paths and branches. Desktop UI controls typically use 12–13px, body copy 14px, and the empty-chat heading 28px. Android maps Inter onto the Material type scale and respects system font scaling.

Keep the conversation readable at its actual window size. Titles truncate where necessary; explanatory text wraps and determines its own height.

## Layout

Desktop has a project sidebar, 256px by default and resizable by dragging its edge, a session header, a separate 43px chat-tab row, and a central conversation. The regular composer and chat measure cap at 736px; an empty conversation centers a 660px composer beneath its context, heading and description. A short transcript starts at the top; longer transcripts retain scrolling and the existing latest-output behavior.

Changes, Files, Browser and Terminal belong to the session workspace. The desktop side pane runs the workspace's full height beside the session column: its tabs sit level with the session header, its address bar or tool controls level with the chat tabs, and one rule closes both columns. Chat tabs control independent conversations. Desktop settings use a bounded panel up to 900×760px, with section navigation that changes to a horizontal strip in compact windows. The application minimum is 760×560px.

Outside an open chat, Android phones show a floating bar of icon tabs (Projects, Terminal and Usage) over the foot of the page, with a round + button just above its right end on Projects. The + opens a menu upward with New folder, New project and, nearest the button, New session, which asks which project when there is more than one; the page has no separate New row. Tablets show the same menu from a labelled New button. Lists scroll beneath the bar, and a long press names each tab. From 600dp a navigation rail with labels replaces the bar. Sessions open from Projects and have no tab of their own. A phone chat keeps one 56dp header row so the transcript gets the height: back to projects, the open chat's title (which opens the session's other chats and New chat) over the session and project, a named view button that swaps the chat for Changes, Files or Terminal, and More for chat settings and the chat and session actions. Back from a workspace view returns to the chat. Tablets keep the session header with scrolling chat tabs and workspace tabs below it. Touch controls preserve 48dp targets; dialogs and reading areas respect system insets and enlarged text.

## Elevation & Depth

Desktop requests an opaque native window; nothing behind the window shows through. Both themes are glass drawn inside that opaque window: it paints one flat `glass-backdrop`, with no gradient or glow; page canvases (the chat, a project's page, the session column) leave it showing; and the sidebar, title bars, tab rows and composer are translucent panes (`glass-pane`, `glass-composer`) over it, edged with a `glass-edge` hairline. Cards, code blocks, the terminal, menus and dialogs keep opaque roles. Every alpha blends over the app itself. Native OS decorations remain responsible for window controls where provided. Floating menus and dialogs retain their existing shadows.

Android draws the same glass without compositor blur: the same flat backdrop and translucent panes (headers, the navigation rail and the composer) inside the app. The floating tab bar, which lists scroll beneath, takes the same frosted tone made opaque, so nothing shows through it. It retains native dialogs, ripples and navigation behavior.

## Shapes

Controls use 6px corners, chat tabs 7px, panels 8px, and the composer 16px on desktop. Fine borders separate chrome without decorative rules. Android keeps Material shapes and touch sizing, with a rounded composer and tonal active tabs.

## Components

**Project navigation.** Each project reveals its sessions. One sidebar row represents a session even when it contains several chats. The row restores that session's last active chat and aggregates working, waiting and unread activity. On desktop, drag a project's row to reorder it among its siblings; a line shows where it will land, and the order is saved on the host. Settled sessions collapse into a shelf pinned just above the sidebar footer, whose Terminal, Settings and Usage icons sit at its left edge; opened, the shelf grows upward to half the sidebar and scrolls past that.

**Chat tabs.** Each tab has an agent mark and title. Add creates another chat sharing the workspace. Close hides a tab without deleting history; Reopen on desktop and All chats on Android restore it. On phones the chat title's menu lists every chat, hidden ones included, and More holds Close chat. Tablet tabs keep inline close buttons. Explicit rename and confirmed delete remain separate actions. Desktop keeps the final visible chat open. Android can hide all tabs and shows a saved-chats state with reopen and create actions.

**Delegated agents.** While a chat's agents work, one quiet line above the composer says how many and what they are doing, with a single pulsing dot; opened, it lists each agent's task and elapsed time. A row, or Open on an agent's card, opens that agent as a temporary tab after the chat tabs (on phones, in place of the chat, listed in the title menu). The tab shows the task, the steps, the latest message or report, and the messages sent to it, over a box to message it. A message goes to the lead as an ordinary chat message that asks it to pass the words on; the chat shows it as the user's words under "To" and the agent's name, and the agent's tab under "Sent through the main chat". Agent tabs last only as long as the window and close without affecting the agent.

**Composer.** A bordered glass pane, using Material on Android, holds the editor, attachments, agent options and send action. On desktop, attached images are 64px cover-cropped tiles above the editor, each with an overlaid remove button. A tile opens a full-window viewer over a black 75% backdrop that fits the image, zooms by click, wheel or keyboard, pans by drag or arrow keys, steps through the draft's images, and closes on Escape or a click outside the image. With nothing typed after a turn was cut short, the send action becomes a filled play button that continues the chat. Empty-state text uses natural height and a clear gap above it. While another chat's metadata loads, its editor waits for the correct saved draft. On phones the composer is one row: attach, the message growing upward to six lines, the agent and model chip that opens the model picker, and send or stop.

**Host notices.** Warnings sit directly in the conversation with a small warning icon and muted, proportional text. They have no card, colored background, border, or heading. Keep the explanation selectable and allow it to wrap.

**Workspace tools.** Files, Changes, Browser and Terminal retain session scope across sibling chat switches. Agent history, options, approvals and drafts retain chat scope. Hosts without grouped-session support remain usable with singleton sessions and show an update message when adding a sibling chat.

**Settings.** Desktop groups Appearance, General, Agents, Devices and Shortcuts in one panel using existing capabilities. Android preserves its native settings dialog. Both offer Dark and Light, side by side, or stacked when the names would not fit (a very narrow panel, or enlarged text on a phone).

**Usage.** The chart icon opens a 348px flyout just above the sidebar footer, without dimming the work behind it. Tabs switch between Limits, Cost and Tokens. Each limit compares what is left with the time left as two thin bars, and stays neutral until it needs attention: amber when spending ahead of pace, rose when spent. Pooled accounts nest under their total, and pressing a row opens its details and reset credit. Cost and Tokens show a total, small period bars and ranked models, in neutral shades. The maximize button or a model row opens the full page, a large panel holding filters, model prices, charts and breakdowns.

**Buttons and fields.** Primary actions use the indigo action role; secondary and ghost controls use tonal hover and focus feedback. Destructive actions use the danger role and explicit confirmation where data is removed. Search and text fields retain visible focus and readable hints.

## Do's and Don'ts

### Do:

- Do use catalog display names for models and readable labels for known settings; preserve custom identifiers when no display name is available.
- Do use semantic ThemeColors or Material color roles in every theme.
- Do center the empty-state context and heading over the composer, allowing natural text height.
- Do preserve independent chat histories, drafts and agent settings when switching tabs.
- Do retain native window, keyboard, touch and system inset behavior.
- Do keep opaque reading surfaces beneath code and terminal output; prose may sit on the glass backdrop.

### Don't:

- Don't restore the retired red rules, bone palette or card-catalog metaphor.
- Don't make closing a chat tab delete its conversation.
- Don't use fake Mac window controls on other platforms.
- Don't let the desktop or other windows show through the app.
- Don't depend on color alone to identify selection or status.
- Don't scale desktop layouts into phone thumbnails; use native responsive structure.
