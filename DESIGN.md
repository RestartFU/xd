---
name: xd
description: A clear workspace for your coding agents.
colors:
  dark-background: "#0b0b0b"
  dark-sidebar: "#0f0f0f"
  dark-surface: "#111111"
  dark-surface-high: "#1b1b1b"
  dark-border: "#2a2a2a"
  dark-control-border: "#6e6e6e"
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
  light-control-border: "#7a7f92"
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
  page-title:
    fontFamily: "Inter"
    fontSize: "22px"
    fontWeight: 600
  heading:
    fontFamily: "Inter"
    fontSize: "16px"
    fontWeight: 600
  body:
    fontFamily: "Inter"
    fontSize: "14px"
    fontWeight: 400
  title:
    fontFamily: "Inter"
    fontSize: "13px"
    fontWeight: 500
  label:
    fontFamily: "Inter"
    fontSize: "12px"
    fontWeight: 400
  caption:
    fontFamily: "Inter"
    fontSize: "11px"
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
  dialog: "10px"
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
    borderColor: "{colors.dark-control-border}"
    rounded: "{rounded.control}"
    height: "30px"
  button-ghost:
    textColor: "{colors.dark-muted}"
    rounded: "{rounded.control}"
    height: "30px"
  button-compact:
    textColor: "{colors.dark-muted}"
    rounded: "{rounded.control}"
    height: "28px"
  icon-button:
    textColor: "{colors.dark-muted}"
    rounded: "{rounded.control}"
    size: "28px"
  menu:
    backgroundColor: "{colors.dark-surface}"
    textColor: "{colors.dark-text}"
    rounded: "{rounded.panel}"
  dialog:
    backgroundColor: "{colors.dark-surface}"
    textColor: "{colors.dark-text}"
    rounded: "{rounded.dialog}"
    width: "450px"
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
- Status is a shape and a word as well as a color, the same four marks on desktop and Android.
- Everything a pointer reaches, the keyboard reaches; what the keyboard is on shows a ring.
- One Interface size scales the whole desktop window; Android follows the system's font scale.

## Colors

The normative desktop values above come from `desktop/src/theme.rs`. Roles pair by the `dark-` and `light-` prefixes, with each theme's glass under `-glass-`. `selection`, `backdrop` and the glass `pane`, `composer` and `edge` include alpha as `#RRGGBBAA`; other values are opaque.

Neutral surfaces distinguish canvas, sidebar, panels, hover and selection. The only hue outside status is the indigo accent: an indigo fill for the primary action (dark ink in Dark, white in Light), and indigo for links, focus, the caret, selection and the selected-row border. Working is green, waiting is amber, and destructive actions are rose. Desktop marks questions and approvals with a steady hollow amber circle and ‘Waiting on you’ in session rows and the chat header. Chat tabs use the same ring. On desktop, a question, approval or quota stop docks inside the composer as a section above a hairline, marked only by the ring, with numbered choices; Send submits the editor's text as a typed answer. Once answered, the reply carries the question above its bubble in faint text after a reply arrow, on desktop and mobile. Status also has text or an icon.

**Status marks.** A session, a chat tab and a closed project or folder row say their state with one of four marks, the same shapes on both clients: a filled dot in `working` that pulses while the agent works, a hollow ring in `waiting` for a turn waiting on you, a small check mark in `done` for a turn that finished unread, and two bars in `muted` for paused. Each carries its name: under the pointer on desktop and in the row's semantics on Android, as Working, Waiting on you, Done and Paused. A count beside a mark says what it counts. A check mark means Done and nothing else, so Settle is a lidded box and Bring back the same box with an arrow out of it.

**Terminal colors.** The terminal draws on each theme's opaque ground with that theme's own sixteen ANSI colors, the same sets on desktop and Android. Dark keeps the set the terminal has always had; every color of the Light set reads at 4.5:1 or better on the Light ground, so its white and bright white are dark. A cell that sets its own background keeps readable ink: where the Light set would put text under 3:1 against that background, the text is drawn white on a dark background and in the set's black on a light one. Changing theme tells each open terminal its new colors, so a program started afterwards picks for the right ground.

On desktop the selected sidebar row, chat or agent tab, browser and side-pane tab, segment and Settings section carries a 2px `selected-border` bar beside its quiet `selected-surface` fill: at the leading edge of a row in a list, at the foot of a tab or a segment, kept clear of the corners. The fill alone does not reach 3:1 against what is around it; the bar does on every ground. A selection is the `selection` role wherever text can be selected: the transcript, the editors and single-line fields. A control that cannot be used now is drawn in the `faint` ink on its own ground at full strength, never dimmed by opacity.

Android uses complete Material color roles in `ui/Theme.kt`, with small platform-specific surface adjustments. Its conversation sits on the same glass backdrop as the desktop canvas; action, text and signal colors follow the same roles. Legacy accent preferences remain readable but do not recolor the main workspace.

## Typography

Inter is bundled for interface text, prose and headings. JetBrains Mono is bundled for source, terminal output, paths and branches. Android maps Inter onto the Material type scale and respects system font scaling.

Desktop text sizes are named by role in `desktop/src/metrics.rs`, and a size is chosen by what the text is: caption 11px for a count, a state or a stamp at the end of a row and the words in a pill, and nothing is set smaller; label 12px for text that explains or annotates, such as notes, hints, tooltips and header chips; control 13px for the words on a control and the titles of rows, such as buttons, fields, menu rows, tabs, and session and project names; body 14px for prose that is read, the transcript and what a dialog says. Headings are 16px for a dialog, a panel or a section, 22px for a page title or a leading figure, 28px for the one large line of a page (the empty-chat heading, a Settings section) and 36px for the Usage total. Monospace is 12.5px on a 19px line for code blocks, diffs, the terminal and the file editor; a path or a branch set inside a label takes the label's size. Line heights are named there too, one for each way text is set: a wrapping caption 13px, a label 17px (18px in a list row, 19px in notes that run on, 15px in a tile), a control or a choice 20px, body 21px, the transcript's prose 23px, monospace 18, 19 or 20px by its room, a title 28px and the Usage figure 40px.

Keep the conversation readable at its actual window size. Titles truncate where necessary; explanatory text wraps and determines its own height.

## Layout

Desktop has a project sidebar, 256px by default and resizable by dragging its edge, a session header, a separate 43px chat-tab row, and a central conversation. The regular composer and chat measure cap at 736px; an empty conversation centers a 660px composer beneath its context, heading and description. A short transcript starts at the top; longer transcripts retain scrolling and the existing latest-output behavior.

Changes, Files, Browser and Terminal belong to the session workspace. The desktop side pane runs the workspace's full height beside the session column: its tabs sit level with the session header, its address bar or tool controls level with the chat tabs, and one rule closes both columns. Chat tabs control independent conversations. Desktop settings is a full page below the title bar, with its navigation in a 248px column (208px under a 900px window). The application minimum is 760×560px.

**Scrolling.** A scroll area whose content runs past it shows a thin thumb at its right edge, in the theme's `scroll-thumb` ink, for as long as it overflows: where the view is, and that there is more. It takes no room and no clicks, and reads at 3:1 or better on every ground it is drawn on.

**Interface size.** Settings › Appearance offers 90, 100, 115, 130 and 150%, and Ctrl/Cmd with `=`, `-` and `0` step through them and return to 100%. The whole window scales together and at once: text, controls, spacing, icons, the terminal's cells and tooltips. Pages in the integrated browser keep their own size; its toolbar scales. Every length in this document is a logical pixel, what it measures at 100%. Breakpoints and saved pane widths are logical too, so at a larger size a window lays out as a smaller window does at 100%: the 760×560px minimum holds about 507×373 logical pixels at 150%, where the landing page keeps the sidebar at its 220px minimum, an open session has the sidebar step aside rather than fall under its own 384px (the session header then starts with a Show sidebar button, which lays the sidebar over the session until Escape, a press outside it, or a row taking the window somewhere else; Ctrl/Cmd+K shows it too, with the keyboard in its find field), and an open browser narrows to 200px before the session gives up anything. At 90% no text draws under the 11 window pixels of a caption at 100% and no pointer target under 24, so captions and labels meet at 11. The window's own size on screen is not scaled, and hairline borders, the caret and shadows keep their size so they stay sharp.

The session header fits what it holds to its room. Its title keeps about a dozen letters and the side pane and terminal buttons always show; pull request chips, the branch's name, the assistant's name, the status stamp, the branch and pull request button, and last the assistant's mark give way in that order, each icon naming itself under the pointer. In a window under 480 logical pixels high, which only a larger Interface size produces, a chat leaves out the worktree row under its composer, keeps the composer's options to one row by cutting their labels short, and scrolls as a whole if the composer still does not fit, so Send stays in reach; a setting's control drops under its title in a column too narrow for both. A docked question takes at most half the window and gives up its height before anything else, by layout rather than a fixed allowance, so what it gives up follows the title bar, header, tabs and rows really around it: the editor and Send stay whole under it, the last two lines of the conversation stay in sight above it, and in the smallest window at 150% its question and first answer still show whole.

Outside an open chat, Android phones show a floating bar of icon tabs (Projects, Terminal and Usage) over the foot of the page, with a round + button just above its right end on Projects. The + opens a menu upward with New folder, New project and, nearest the button, New session, which asks which project when there is more than one; the page has no separate New row. Tablets show the same menu from a labelled New button. Lists scroll beneath the bar, and a long press names each tab. The layout follows the window's size on both axes. From 600dp wide a navigation rail with labels replaces the bar. Sessions open from Projects and have no tab of their own. A phone chat keeps one 56dp header row so the transcript gets the height: back to projects, the open chat's title (which opens the session's other chats and New chat) over the session and project, a named view button that swaps the chat for Changes, Files or Terminal, and More for chat settings and the chat and session actions. Back from a workspace view returns to the chat. A window at least 600dp wide and 480dp high keeps the tablet session header with scrolling chat tabs and workspace tabs below it, as one glass surface; a shorter window, such as a phone on its side, keeps the one-row phone header so the transcript has the height. From 840dp wide, Changes, Files and Terminal open beside the chat instead of in its place. Reading surfaces (the transcript, the composer, Settings rows and the Projects list) keep a 720dp measure, centred, however wide the window. Touch controls preserve 48dp targets; content keeps clear of system bars and display cutouts, and headers and fields grow with enlarged text.

While the host is not connected, every Android page, the chat included, shows one slim line under its header: quiet for a first connection, in the error role once the link has failed, with the reason and when it tries again. Connected, it draws nothing.

## Elevation & Depth

Desktop requests an opaque native window; nothing behind the window shows through. Both themes are glass drawn inside that opaque window: it paints one flat `glass-backdrop`, with no gradient or glow; page canvases (the chat, a project's page, the session column) leave it showing; and the sidebar, title bars, tab rows and composer are translucent panes (`glass-pane`, `glass-composer`) over it, edged with a `glass-edge` hairline. Cards, code blocks, the terminal, menus and dialogs keep opaque roles. Every alpha blends over the app itself. Native OS decorations remain responsible for window controls where provided. Floating menus and dialogs retain their existing shadows.

Desktop motion is the status pulse and nothing decorative: a working mark steps a few times a second, and the usage refresh icon turns with it. What moves with the pulse, a running agent's or task's timer included, is drawn on a layer over the window that alone redraws as it steps. Settings › Appearance › Reduce motion holds the marks steady and stills the icon.

Android draws the same glass without compositor blur: the same flat backdrop and translucent panes (headers, the navigation rail and the composer) inside the app. The floating tab bar, which lists scroll beneath, takes the same frosted tone made opaque, so nothing shows through it. It retains native dialogs, ripples and navigation behavior, including predictive Back. System bars are clear and take their icons from the app's theme, in the app's own dialogs too; the launch window follows the system's night mode, since the saved theme cannot be read before launch. Pages change with two transitions only, a shared axis between a page and the one opened from it and a fade through between the destinations of the bar or rail, and with none while the system's Remove animations is on.

## Shapes

Desktop corners are named in `desktop/src/metrics.rs`: 6px for a control (buttons, icon buttons, fields, menu rows, tooltips), 7px for a tab and a row picked like one, 8px for a panel (cards, menus, code blocks, image tiles), 10px for a dialog, and 16px for the composer, the messages sent from it and the Usage sheet. Pills and circles are fully round. Controls come in two heights, 30px and a 28px compact one for toolbars and rows, which is also the side of an icon button; no pointer target is smaller than 24px on either side, though the glyph inside may be. Fine borders separate chrome without decorative rules. Android takes its corners from the theme's `Shapes` and keeps Material touch sizing, with a rounded composer and tonal active tabs.

## Components

**Project navigation.** Each project reveals its sessions. One sidebar row represents a session even when it contains several chats. The row restores that session's last active chat and aggregates working, waiting and unread activity. On desktop, drag a project's row to reorder it among its siblings; a line shows where it will land, and the order is saved on the host. Settled sessions collapse into a shelf pinned just above the sidebar footer, whose Terminal, Settings and Usage icons sit at its left edge; opened, the shelf grows upward to half the sidebar and scrolls past that.

**Chat tabs.** Each tab has an agent mark and title. Add creates another chat sharing the workspace. Close hides a tab without deleting history; Reopen on desktop and All chats on Android restore it. On phones the chat title's menu lists every chat, hidden ones included, and More holds Close chat. Tablet tabs keep inline close buttons. Explicit rename and confirmed delete remain separate actions. Desktop keeps the final visible chat open. Android can hide all tabs and shows a saved-chats state with reopen and create actions.

**Delegated agents.** While a chat's agents work, one quiet line above the composer says how many and what they are doing, with a single pulsing dot; opened, it lists each agent's task and elapsed time. A row, or Open on an agent's card, opens that agent as a temporary tab after the chat tabs (on phones, in place of the chat, listed in the title menu). The tab shows the task, the steps, the latest message or report, and the messages sent to it, over a box to message it. A message goes to the lead as an ordinary chat message that asks it to pass the words on; the chat shows it as the user's words under "To" and the agent's name, and the agent's tab under "Sent through the main chat". Agent tabs last only as long as the window and close without affecting the agent.

**New chat.** A new chat in a new worktree starts from a base branch or from a pull request's branch: the branch picker lists the project's open pull requests, by number and title, above its branches, and its search finds either, as Android's new session does.

**Composer.** A bordered glass pane, using Material on Android, holds the editor, attachments, agent options and send action. On desktop, attached images are 64px cover-cropped tiles above the editor, each with an overlaid remove button. A tile opens a full-window viewer over a black 75% backdrop (the `viewer` roles, which both themes keep dark) that fits the image, zooms by click, wheel or keyboard, pans by drag or arrow keys, steps through the draft's images, and closes on Escape or a click outside the image. With nothing typed after a turn was cut short, the send action becomes a filled play button that continues the chat. Empty-state text uses natural height and a clear gap above it. While another chat's metadata loads, its editor waits for the correct saved draft. On phones the composer is one row: attach, the message growing upward to six lines, the agent and model chip that opens the model picker, and send or stop.

**Host notices.** Warnings sit directly in the conversation with a small warning icon and muted, proportional text. They have no card, colored background, border, or heading. Keep the explanation selectable and allow it to wrap.

**Workspace tools.** Files, Changes, Browser and Terminal retain session scope across sibling chat switches. Agent history, options, approvals and drafts retain chat scope. Hosts without grouped-session support remain usable with singleton sessions and show an update message when adding a sibling chat.

**Settings.** A page of its own, not a dialog, on both clients, modelled on macOS System Settings. Sections: Appearance, Chats, Agents, Cleanup, Devices, Keyboard (desktop) and Updates, each with a one-line summary. Desktop covers the workspace below the title bar: a 248px glass navigation column (Back, the title, a search field, the sections with icons) beside a centred 680px reading column with the section's title, its summary and its settings in grouped glass cards, rows split by hairlines with the control on the right. Android opens a full-screen page: search, then the sections as rows with live summaries, each opening its own page; Back leaves a section, then clears a search, then closes. Search matches every word against a setting's section, title and description, shows the matches grouped by section and working in place (desktop counts them beside each section), and Escape or Back clears it before leaving. Both offer Dark and Light, side by side, or stacked when the names would not fit (a very narrow column, or enlarged text on a phone). On desktop a change that could not be saved says so under the setting it changed, in the danger ink; a save that fails anywhere else says so in the error banner.

**Usage.** The chart icon opens a 348px flyout just above the sidebar footer, without dimming the work behind it. Tabs switch between Limits, Cost and Tokens. Each limit compares what is left with the time left as two thin bars, and stays neutral until it needs attention: amber when spending ahead of pace, rose when spent. Pooled accounts nest under their total, and pressing a row opens its details and reset credit. Cost and Tokens show a total, small period bars and ranked models, in neutral shades. The maximize button or a model row opens the full page, a large panel holding filters, model prices, charts and breakdowns.

**Buttons and fields.** Primary actions use the indigo action role; secondary and ghost controls use tonal hover and focus feedback. Destructive actions use the danger role and explicit confirmation where data is removed. Search and text fields retain visible focus, the `selected-border` on their frame, and readable hints that end in an ellipsis. A field's frame and a secondary button's edge take the `control-border` role, which reads at 3:1 or better on every ground a control sits on; the quiet `border` stays for the hairlines between panes and around cards, menus and dialogs. Every field in a dialog has its name above its frame, so what it takes is still said once the hint gives way to what was typed; a press on the name puts the keyboard in the field. A choice that cannot be changed now, such as access in Plan mode, is drawn as its words alone, with no chevron and nothing that reacts. A button that is an icon alone is one build on desktop, 28px around a 16px icon or 24px around a smaller one, and always names itself under the pointer; on Android it carries a content description. A title or label that does not fit ends in an ellipsis and reads out in full under the pointer.

**Dialogs and menus.** Every desktop dialog is one shell at one of two widths, 450px for a question, a name or a short list and 480px for a form: a title, what it says or asks for, and its actions at the bottom right with the way out first and the main action last. It never outgrows the window; the part that can be long scrolls inside it. Every menu, picker and popover is one shell with 32px rows, kept inside the window. A dropdown hangs under its button, or over it where the window ends too soon below, so the button is never under its own menu.

**Keyboard.** On desktop the whole window is reachable without a pointer, and Settings › Keyboard lists every key. Window shortcuts switch chat tabs and sessions, jump to the next session waiting on you, stop the running turn, show the side pane and the terminal, open Settings and Usage, go to the message box and to the sidebar's find field, and change the Interface size. They are bound at the window, so the terminal and the text boxes keep every key they use themselves. Up and Down move through a menu, a picker, find results, the Settings sections and the landing page; Enter chooses the row or presses a dialog's main button; Escape closes what is on top. Tab and Shift+Tab stop on the fields and then the buttons of a dialog, on the controls of the open Settings section, and on the options under the message box; Tab in the message box goes on to its options whether or not it holds text, and indents only inside a fenced code block (between lines of three backticks). Ctrl/Cmd+W closes the chat or agent tab in front, as its close button does, and leaves the last chat open; the terminal keeps Ctrl+W, which deletes a word in a shell. The control Tab is on shows a 2px `selected-border` ring standing 3px off it, and Enter or Space presses it. Tab also stops on the controls of Usage, the Files and Changes panes, the browser pane's tabs and toolbar, the sidebar's Undo and a project's pause button, the chat and agent tabs' close buttons, Reopen and New chat, the header's pull requests and Link pull request with the panel it opens, each message's Copy (shown while the ring is on it), a queued message's Steer and remove, the terminal's tabs, their close buttons and New terminal, and the error banner's Reconnect and close; Up and Down move the ring through the Files list's rows. A control that is an icon alone shows its name under it while the ring is on it, as it does under the pointer, and a control or row whose label is cut short shows the whole of it there while the keyboard is on it. Shift+F10 or the Menu key opens the menu of the open session, or of the current project on the landing page, or, with the ring on a message's Copy, that message's menu, or, with the ring on a sidebar row, that row's menu. The sidebar's projects, folders and sessions are one Tab stop, on the row the keys were last on (else the open session's, else the first): Up, Down, Home and End move between the rows it shows, Left closes a row or goes to the row it is in, Right opens it or goes to its first row, Enter opens a session or closes and opens a project, and Tab goes on to that row's own buttons (+, pause, Settle, Bring back). The title bar's "xd" goes home to the landing page and its connection label (Local or Remote) pairs a host, switches to Remote or reconnects, each saying which under the pointer; both are Tab stops, as are New project, the find field, the Settled shelf's header and Show more, and the footer's Terminal, Settings and Usage. Ctrl/Cmd+Shift+T opens the Terminal page. An approval's Allow once, Allow for session and Deny are Tab stops; with the message box empty and focused, 1 to 3 take them, and Up goes onto Allow once, which Enter takes; nothing is approved unasked. Every control of a transcript card (the agents tray and its rows, a work log's toggle and steps, an activity or agent card's disclosure, Background, Stop and Open, an agent's row, a report line, Started by) is a Tab stop in the order they are drawn, named under the ring. While the integrated browser's page has the keyboard, Shift+Escape, Ctrl/Cmd+L, Ctrl/Cmd+K, Ctrl/Cmd+, and the window's other shortcuts that a page has no use for go to the window instead of the page, and Tab past the page's last control goes back to the address field. A project's menu also offers what its pause button does, Pause or Resume this project's chats. A project's or folder's menu offers Move to…, which turns the menu into the top level and every folder it can go in, so nothing needs a drag. Page Up, Page Down, Ctrl+Home and Ctrl+End (also Cmd+Up and Cmd+Down on macOS) scroll the reading pane: the chat, or the side pane the pointer last pressed in, and Usage or Settings while they are open. The terminal keeps Page Up and Page Down for its scrollback and Alt with an arrow for the program in it, so those keys step through sessions only outside the terminal. Actions that a row shows under the pointer are also in sight on the open row.

**Landing page.** Leaving a chat on desktop shows what needs attention, not a welcome: the sessions waiting on you first, then those at work, each one click away, with Continue for the last session and New session in the project just left. The welcome, "Create a project, then start a Codex or Claude session in it.", is the first run's alone.

**Errors and Undo.** A desktop error is a banner that stays until dismissed, with selectable text and a close button; when the host is out of reach it offers Reconnect. Settling a session, discarding a draft and removing a queued message each leave a short notice with Undo in the sidebar footer for thirty seconds, or until the next one replaces it, instead of asking first; its time stands still while the pointer is over it or the Tab ring is on its Undo. Where the sidebar is not drawn, the same notice sits above the message box, or at the foot of the session when there is none. A message's Copy says Copied for a moment once pressed, in its own place. Undo is a Tab stop, and Ctrl/Cmd+Z presses it while no text box or terminal has the keyboard; those keep Ctrl+Z for themselves. A conversation that did not load offers Try again, on both clients. Android reports in its own snackbar, never in a system toast.

**Pictures.** A picture in an Android chat opens full screen, where it zooms by pinch or double tap and pans by drag, as the desktop viewer does by click, wheel and drag.

**Wording.** Both clients use the same words for the same things. The container is a project; workspace is kept for a session's shared tools and for the host in Devices. An untitled chat is New chat, and the worktree choices are New worktree and Current worktree, in sentence case. A session that needs the user is Waiting on you; a delegated agent that does keeps Needs you. The opposite of Settle is Bring back. Every retry is Try again, an answer is sent with Send answer, and deleting a chat asks "Delete chat?" with Delete chat. Empty Changes says "No changes." and an empty folder "This folder is empty." Android's Settings › Devices offers Forget this host, which erases the pairing and asks first ("This device will need a new pairing code to connect again."); a failed connection that another attempt can get past offers Try again before it.

## Do's and Don'ts

### Do:

- Do use catalog display names for models and readable labels for known settings; preserve custom identifiers when no display name is available.
- Do use semantic ThemeColors or Material color roles in every theme.
- Do center the empty-state context and heading over the composer, allowing natural text height.
- Do preserve independent chat histories, drafts and agent settings when switching tabs.
- Do retain native window, keyboard, touch and system inset behavior.
- Do keep opaque reading surfaces beneath code and terminal output; prose may sit on the glass backdrop.
- Do take text sizes, corners and control heights from the named constants, and write desktop lengths in logical pixels so they follow the Interface size.
- Do give every icon-only button a name, every status its mark and word, and every action a way to reach it from the keyboard.
- Do build dialogs, menus and icon buttons on the shared shells.

### Don't:

- Don't restore the retired red rules, bone palette or card-catalog metaphor.
- Don't make closing a chat tab delete its conversation.
- Don't use fake Mac window controls on other platforms.
- Don't let the desktop or other windows show through the app.
- Don't depend on color alone to identify selection or status.
- Don't show an action only under the pointer, or dim a disabled control by opacity.
- Don't remove something at one click without Undo or a confirmation.
- Don't add motion beyond the status pulse on desktop or the two page transitions on Android, and none where motion is reduced.
- Don't scale desktop layouts into phone thumbnails; use native responsive structure.
