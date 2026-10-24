---
name: xd
description: A card catalog for the agent sessions running across your projects.
colors:
  cabinet-graphite: "#121314"
  drawer-graphite: "#17181a"
  card-stock-dark: "#1e1f22"
  card-stock-raised: "#27292c"
  cabinet-seam: "#313338"
  pulled-card: "#2b2d31"
  bone: "#e8e5de"
  bone-ink: "#eceae5"
  pencil-gray: "#aaa9a3"
  faint-gray: "#8b8a85"
  bone-hover: "#ffffff"
  ink-blue-dark: "#93b5e9"
  ruled-red-dark: "#a8493c"
  signal-green-dark: "#72b783"
  signal-amber-dark: "#eaa93a"
  signal-blue-dark: "#86a9e4"
  danger-dark: "#ec7064"
  danger-surface-dark: "#3b201e"
  diff-added-dark: "#1b2b20"
  diff-removed-dark: "#33201e"
  selection-dark: "#93b5e955"
  backdrop-dark: "#000000a6"
  reading-room-white: "#f7f7f5"
  drawer-white: "#eeeeeb"
  card-stock-white: "#ffffff"
  card-stock-white-raised: "#e9e9e5"
  cabinet-seam-light: "#d7d6d0"
  pulled-card-light: "#e6e5e0"
  catalog-ink: "#1b1c1e"
  pencil-gray-light: "#56554f"
  faint-gray-light: "#64635d"
  ink-hover: "#3a3c40"
  ink-blue-light: "#2b5897"
  ruled-red-light: "#c2483b"
  signal-green-light: "#276b3c"
  signal-amber-light: "#8c5100"
  danger-light: "#b2342a"
  danger-surface-light: "#f8e2df"
  diff-added-light: "#e2f0e5"
  diff-removed-light: "#f8e4e1"
  selection-light: "#2b589740"
  backdrop-light: "#1b1c1e66"
typography:
  display:
    fontFamily: "Libre Franklin"
    fontSize: "22px"
    fontWeight: 600
  headline:
    fontFamily: "Libre Franklin"
    fontSize: "15px"
    fontWeight: 600
  title:
    fontFamily: "Libre Franklin"
    fontSize: "13px"
    fontWeight: 500
  body:
    fontFamily: "Libre Franklin"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: "23px"
  body-ui:
    fontFamily: "Libre Franklin"
    fontSize: "12.5px"
    fontWeight: 400
    lineHeight: "19px"
  label:
    fontFamily: "Libre Franklin"
    fontSize: "11.5px"
    fontWeight: 500
  stamp:
    fontFamily: "Libre Franklin"
    fontSize: "10px"
    fontWeight: 600
  mono:
    fontFamily: "JetBrains Mono"
    fontSize: "12.5px"
    fontWeight: 400
    lineHeight: "19px"
rounded:
  tab: "2px"
  label: "3px"
  control-sm: "4px"
  card: "5px"
  control: "6px"
  segmented: "7px"
  panel: "8px"
  card-android: "7dp"
  drawer-android: "10dp"
spacing:
  card-gap: "3px"
  drawer-inset: "5px"
  list-gutter: "7px"
  sm: "8px"
  field-inset: "10px"
  md: "12px"
  lg: "16px"
components:
  button-primary:
    backgroundColor: "{colors.bone}"
    textColor: "{colors.cabinet-graphite}"
    typography: "{typography.body-ui}"
    rounded: "{rounded.control}"
    padding: "0 12px"
    height: "30px"
  button-primary-hover:
    backgroundColor: "{colors.bone-hover}"
  button-secondary:
    backgroundColor: "{colors.card-stock-dark}"
    textColor: "{colors.bone-ink}"
    rounded: "{rounded.control}"
    padding: "0 12px"
    height: "30px"
  button-secondary-hover:
    backgroundColor: "{colors.card-stock-raised}"
  button-ghost:
    textColor: "{colors.pencil-gray}"
    rounded: "{rounded.control}"
    padding: "0 12px"
    height: "30px"
  button-ghost-hover:
    backgroundColor: "{colors.card-stock-raised}"
    textColor: "{colors.bone-ink}"
  button-ghost-active:
    backgroundColor: "{colors.pulled-card}"
    textColor: "{colors.bone-ink}"
  button-danger:
    textColor: "{colors.danger-dark}"
    rounded: "{rounded.control}"
    padding: "0 12px"
    height: "30px"
  button-danger-hover:
    backgroundColor: "{colors.danger-surface-dark}"
  session-card:
    backgroundColor: "{colors.card-stock-dark}"
    textColor: "{colors.bone-ink}"
    typography: "{typography.title}"
    rounded: "{rounded.card}"
    padding: "0 12px"
    height: "56px"
  session-card-hover:
    backgroundColor: "{colors.card-stock-raised}"
  session-card-selected:
    backgroundColor: "{colors.pulled-card}"
  drawer-closed:
    textColor: "{colors.bone-ink}"
    typography: "{typography.title}"
    rounded: "{rounded.card}"
    height: "34px"
  drawer-open:
    backgroundColor: "{colors.cabinet-graphite}"
    rounded: "{rounded.panel}"
    padding: "5px"
  label-frame:
    textColor: "{colors.bone-ink}"
    rounded: "{rounded.label}"
    padding: "0 7px"
    height: "22px"
  stamp:
    textColor: "{colors.signal-amber-dark}"
    typography: "{typography.stamp}"
    rounded: "{rounded.tab}"
    padding: "0 5px"
    height: "18px"
  find-field:
    backgroundColor: "{colors.cabinet-graphite}"
    typography: "{typography.body-ui}"
    rounded: "{rounded.control}"
    padding: "0 8px"
    height: "32px"
  catalog-sidebar:
    backgroundColor: "{colors.drawer-graphite}"
    width: "288px"
  title-bar:
    backgroundColor: "{colors.drawer-graphite}"
    height: "38px"
  session-header:
    backgroundColor: "{colors.cabinet-graphite}"
    typography: "{typography.headline}"
    height: "52px"
  pane-tab-active:
    backgroundColor: "{colors.pulled-card}"
    textColor: "{colors.bone-ink}"
    rounded: "{rounded.control}"
    height: "24px"
  popover:
    backgroundColor: "{colors.card-stock-dark}"
    rounded: "{rounded.panel}"
    padding: "6px"
    width: "300px"
---

# Design System: xd

## Overview

**Creative North Star: "The Card Catalog"**

xd is a library card catalog for agent sessions. Projects are drawers, sessions are index cards, and a small metal signal tab clipped to the top edge of a card says what its agent is doing. One catalog sidebar on the left holds every drawer; only the current project's drawer is pulled out to show its cards, and the rest stay one line tall and show just their tabs. The centre belongs to the session's transcript, and the right dock holds Changes, Files, Terminal and Browser.

The material is a cool graphite cabinet with bone card-stock ink in Dark, and white card stock with near-black ink in Light. Each card has one thin red ruled line under its title, the only decorative mark in the system. Colour is mostly reserved for state: green, amber and blue live on the signal tabs and the words beside them, and the chrome around them stays neutral so the agent's output stays the content. Density is that of a working tool: 12.5 to 13px UI text, 30px controls, 56px cards.

Android follows the same world inside Material 3. Material owns the structure (navigation bar, dialogs, extended FAB, ripples, touch targets). The brand comes through as complete colour roles, Libre Franklin across the type scale, and the same drawer, card, rule and tab shapes.

**Key Characteristics:**
- Two themes only, Dark and Light. Saved names of retired themes (warm, nord, dracula) load as Dark.
- Session state shows through the tab's shape as well as its hue, so it still reads in grayscale.
- The current drawer is the only one that lifts. The current project's name sits in an outlined label frame.
- Every session card has one fixed height and baseline (56px on desktop).
- Flat, tonal surfaces. Only floating popovers and menus get a soft shadow.
- Drawn 24-grid stroke icons with a 2px stroke, tinted to the text around them.

## Colors

A neutral graphite and bone palette with one red ruled line, ink-blue links, and three signal hues used only for session state. Each role has a Dark value and a Light value. Desktop reads them from `ThemeColors` in `desktop/src/theme.rs`, and Android maps the same values onto Material roles in `minimalColors` and `XdSignals`.

### Primary
- **Bone** (Dark) / **Catalog Ink** (Light): the fill of the primary action, with cabinet graphite or white text on it. Bone also outlines the selected card and the label frame in Dark, and catalog ink does both in Light. On Android this is `primary`.
- **Ink Blue** (`ink-blue-dark` / `ink-blue-light`): links, focus rings and the caret. Text selection is ink blue at reduced alpha. On Android this is `tertiary` and `XdSignals.link`.

### Secondary
- **Ruled Red** (`ruled-red-dark` / `ruled-red-light`): the 1px line under every card title, and nothing else. It is not an error colour.

### Tertiary (session signals)
- **Signal Green** (`signal-green-dark` / `signal-green-light`): Working. Used for the solid tab, the "Working" label and its animated dots, and the connected dot in the phone header.
- **Signal Amber** (`signal-amber-dark` / `signal-amber-light`): Waiting on you, meaning an approval or question is waiting for the user. It covers the taller solid tab, the "Waiting on you" stamp and the 1.5px border of the interaction card.
- **Signal Blue** (`signal-blue-dark` / `ink-blue-light`): Done, meaning a finished turn the user has not opened yet. It is drawn as a hollow tab. In Light the done colour and the link colour are the same ink blue.
- **Danger** (`danger-*`, `danger-surface-*`): destructive buttons, failed tool runs (border and stamp), error message cards and the disconnected dot. Danger is not a session state.

### Neutral
- **Cabinet Graphite** / **Reading-Room White**: the window ground, the chat column, the inside of the open drawer, and the Find field.
- **Drawer Graphite** / **Drawer White**: the catalog sidebar and the title bar.
- **Card Stock** / **Card Stock White**: session cards, secondary buttons, popovers, activity and message cards, code blocks.
- **Raised Card Stock**: the hover fill on cards, ghost buttons and drawer rows.
- **Pulled Card**: the selected card, the active pane tab and active ghost buttons.
- **Cabinet Seam**: every 1px border and divider.
- **Bone Ink** / **Catalog Ink**: primary text. **Pencil Gray**: secondary text, card metadata and inactive ghost labels. **Faint Gray**: counts, hints, timestamps, chevrons and the Idle label. Never body copy.
- **Diff Added / Diff Removed**: line backgrounds in the Changes pane.
- **Backdrop**: the modal scrim behind dialogs.

### Named Rules
**The One Amber Rule.** Amber means one thing: an agent is waiting on the user. No warning, badge, highlight or brand mark may use it.

**The Shape-Before-Hue Rule.** A signal always has its own shape as well as its own colour. Working is a solid flag, Waiting is a taller solid flag, Done is a hollow flag, and Idle has no tab at all. A new signal surface must stay legible in grayscale.

**The Four States Rule.** A session can be Idle, Working, Waiting or Done. Nothing else. There is no failed or queued session state, and cards never show one. A failed tool run is stamped in the transcript instead.

## Typography

**UI Font:** Libre Franklin (static Regular, Medium, SemiBold, Bold and Italic, embedded in the desktop binary; on Android a variable font set to the same four weights)
**Mono Font:** JetBrains Mono (bundled; never the host's `monospace` alias)

**Character:** Libre Franklin is a plain, sturdy grotesque that reads like typewritten catalog cards. It carries every piece of chrome. JetBrains Mono appears only where the text is machine text.

### Hierarchy
- **Display** (600, 22px): the project name on the project home, set inside its label frame. Markdown H1 in the transcript is 22px Bold, H2 is 19px and H3 is 16px.
- **Headline** (600, 15px): the session title in the 52px session header.
- **Title** (500 to 600, 13px): card titles (500), drawer names (500 when closed, 600 when current), the "Projects" header and popover row labels.
- **Body** (400, 14px / 23px): agent and user messages in the chat column. The column is capped at 860px, assistant messages at 820px and user messages at 720px.
- **Body UI** (400 to 500, 12.5px / 19px): buttons (500), the Find field, empty states and hints.
- **Label** (500, 11.5px): card metadata (branch, state), message role labels (600, faint) and settings group labels (600, faint, sentence case).
- **Stamp** (600, 10px, uppercase): the rubber-stamp status frame. On Android it uses `labelSmall` with 0.6sp tracking.
- **Mono** (400, 12.5px / 19px): code blocks, tool output, the terminal, branch names in lists, and drawer session counts (11px, faint).

On Android the Material 3 type scale is kept and set in Libre Franklin. Headline-small and every title role are SemiBold.

### Named Rules
**The Machine Text Rule.** JetBrains Mono is for text a machine wrote or reads: code, terminal, tool output, branch names, counts and agent wire names. Prose and chrome are always Libre Franklin.

## Layout

The desktop window has three regions. At the top is a 38px title bar (drawer graphite, 1px seam below) with the xd mark, a project / session breadcrumb, a Browser toggle and the connection state. On the left is the 288px catalog sidebar (`CATALOG_WIDTH`; the direction contract said 296px, and the build is what this records). The centre is the session. On macOS the title bar is inset 86px for the traffic lights, and on other platforms 16px.

The catalog stacks a 44px "Projects" header with an add button, then the Find field (32px, 10px side margins), then the drawer list (7px side gutter, 3px between drawers), then a footer with a 1px top seam and 10px vertical padding. The footer holds the primary New session button, a Terminal ghost button and a Settings ghost button. A closed drawer is one 34px row: a chevron, the name, a mono count, and on the right its non-idle signal tabs (at most six) followed by an add button. The open drawer insets its 56px cards by 5px, 3px apart.

In the centre, a 52px session header holds the title, agent and branch chips (24px, 1px seam border), the state stamp, and a segmented control for Changes / Files / Terminal. The chat column below it is centred and capped at 860px, with the composer at the bottom. The right dock opens beside the chat and can be resized.

Spacing is small and irregular rather than a strict scale: 3, 5, 7 and 10px for catalog joinery, and 8, 12 and 16px for general padding.

On Android, a 56px product header with the mark, the title, a connection dot and Settings sits above a 52px context bar. The catalog is the Projects destination, and a Material navigation bar switches between Projects, Sessions and Terminal. All touch targets are at least 48dp.

## Elevation & Depth

Surfaces are flat and depth is tonal. The cabinet ground sits lowest, the drawer and title bar sit one step above it, cards sit on card stock, and hover or selection brings in raised or pulled card stock. The open drawer "lifts" by contrast, not by shadow: it is a well with an 8px radius and a 1px seam that is filled with the cabinet ground colour, so its cards stand out against it. Floating layers (the settings popover, context menus and pickers) are the only places a shadow appears.

### Shadow Vocabulary
- **Floating layer** (`box-shadow: 0 10px 15px -3px rgba(0,0,0,0.1), 0 4px 6px -4px rgba(0,0,0,0.1)`, GPUI `shadow_lg`): popovers, menus and pickers that float over the shell.
- **Android**: Material 3 tonal elevation and the extended FAB's default shadow. Nothing custom.

### Named Rules
**The Lift-By-Contrast Rule.** In-flow surfaces never cast shadows. A drawer, card or panel shows that it is raised through fill and seam alone.

## Shapes

Corners are small and practical, like card stock and steel. Signal tabs and stamps round only by 2px, and signal tabs round only their top corners so they look clipped to the card's edge. The label frame is 3px with a 1.5px outline in the primary colour. Cards are 5px. Buttons, the Find field and pane tabs are 6px. The segmented control is 7px. Panels, the open drawer, popovers and message cards are 8px. Borders are 1px cabinet seam by default. A 1.5px border marks the label frame, the hollow Done tab and the amber interaction card.

Android scales the same shapes for touch: cards 7dp, the open drawer 10dp, theme choices and chat panels 8dp, the label frame 3dp. Material components keep their own shapes.

Icons are drawn on a 24 grid with a 2px round-capped, round-joined stroke and no fill, and are tinted with the surrounding text colour (desktop `assets/icons/*.svg`, Android `res/drawable/ic_*.xml`). Agent logos are the exception: they keep their own marks.

## Components

### Buttons
Calm and compact. There is one vocabulary (`ui_button` with `ButtonKind`) for the shell, popovers and dialogs.
- **Shape:** 6px radius, 30px tall, 12px horizontal padding, 12.5px Medium, 8px gap to an icon.
- **Primary:** a bone fill with cabinet graphite text in Dark, and an ink fill with white text in Light. On hover it brightens to white (Dark) or ink hover (Light). Use one per region, for example New session.
- **Secondary:** card stock with a 1px seam border and primary text. Hover moves to raised card stock.
- **Ghost:** no fill and pencil gray text. Hover moves to raised card stock with primary text. When the button's panel is open, it takes the pulled-card fill.
- **Danger:** a 1px danger border and danger text. Hover fills with the danger surface.

### Signal Tab (signature)
A metal flag clipped to the top-right of a card, 12px in from the edge and 16px wide, with 2px top corners.
- **Working:** solid green, 9px tall, rising 4px above the card.
- **Waiting:** solid amber, 12px tall, rising 7px above the card.
- **Done:** hollow, with a 1.5px blue outline and no bottom edge, 9px tall, filled with the cabinet ground.
- **Idle:** no tab.
- **Drawer tabs:** a closed drawer shows its sessions' tabs as 10px-wide miniatures, 3px apart and bottom-aligned. Working and Done are 5px tall, and Waiting is 8px.

### Session Card (signature)
- **Corner Style:** 5px (7dp on Android).
- **Background:** card stock. Hover uses raised card stock. A selected card uses pulled card stock with a 1px primary-colour border.
- **Structure:** a 28px title line (13px Medium, with 26px right clearance for the tab), then a 1px ruled-red line, then a 25px metadata line: agent logo at 12px, branch, a faint "·", and the state label in its signal colour (Medium when not Idle, with animated dots when Working).
- **Height:** fixed at 56px (`CARD_HEIGHT`; the direction contract said 54px). Cards are draggable to reorder within their drawer.
- **Android:** the agent logo moves to the title row, the branch is set in mono, and the Waiting state shows as a stamp instead of plain text.

### Drawer
- **Closed:** a 34px row with a 5px hover radius: chevron, name (13px Medium), mono count, signal tabs and an add button.
- **Current:** the name sits in the **label frame** (22px tall, 7px padding, 1.5px primary-colour outline, 3px radius) and is set SemiBold. The drawer becomes the 8px well with 5px padding, filled with the cabinet ground.

### Stamp
A rubber-stamp status in small uppercase: a 1px border in the state's colour, 2px radius, 18px tall, 10px SemiBold. It is used for the session state in the header and on project home rows, for "Waiting on you" on interactions, and for a failed tool run's status in danger red. It is a status mark, never a section heading.

### Inputs / Fields
- **Find:** 32px tall with a 6px radius, a 1px seam border, the cabinet ground fill, a 13px faint search icon and 12.5px text. It filters drawers and cards live, and Esc clears it. It deliberately has no keyboard shortcut, so that terminals keep Ctrl-K.
- **Composer:** pinned to the foot of the chat column, inside the same 860px measure.

### Navigation
- **Desktop:** the catalog is the primary navigation (project, then session). The pane switch in the session header is a segmented control (2px inset, 7px radius, 1px seam) of 24px ghost buttons. The active segment takes the pulled-card fill and primary text. Selecting the active pane again returns to Chat.
- **Android:** a Material `NavigationBar` on the surface colour, with a `secondaryContainer` indicator, for Projects, Sessions and Terminal, using drawn stroke icons.

### Cards / Containers (transcript)
- **Activity card:** card stock, 6px radius and a 1px seam, in the 860px measure, with 7px vertical padding. A chevron toggles its detail, which opens into a well with a 5px radius filled with the cabinet ground, 12px mono text and a 260px maximum height. A failed run turns the border danger red and adds a danger stamp.
- **Message card:** user messages sit on card stock with an 8px radius, a 1px seam and 16×12px padding, under an 11.5px SemiBold faint role label. Assistant messages sit directly on the ground. Errors use a danger border on the danger surface.
- **Interaction (approval or question):** card stock, 8px radius, a 1.5px amber border, 13px text and a "Waiting on you" stamp.

### Settings Popover
It opens from the catalog footer, anchored 56px from the bottom and 10px from the left: 300px wide, 6px padding, 8px radius, a 1px seam, card stock and the floating-layer shadow. The Appearance group offers exactly two theme choices, Dark and Light. Each choice is a 34px row with a 6px radius, a miniature card swatch (20×14px, 3px radius, with a ruled line) and a primary-colour border when selected. On Android, `MinimalSettingsDialog` is a Material `AlertDialog` that offers the same two choices as 48dp rows with an 8dp radius and a 1.5dp primary-colour border when chosen.

## Do's and Don'ts

### Do:
- **Do** take every colour from the `ThemeColors` roles (desktop) or `minimalColors` / `XdSignals` (Android), and define each new role for both Dark and Light.
- **Do** give every new session-state surface a shape difference as well as a hue difference, following the signal tab.
- **Do** keep session cards at one fixed height (56px) with the 1px ruled-red line under the title.
- **Do** keep primary actions to one per region: a bone fill with ink text in Dark, an ink fill with white text in Light.
- **Do** stamp a failed tool run in the transcript in danger red, and leave the session card's state alone.
- **Do** use drawn 24-grid, 2px stroke icons tinted to the surrounding text.
- **Do** let Material 3 own the structure on Android, and carry the brand through colour roles, Libre Franklin and the catalog shapes.

### Don't:
- **Don't** use signal amber for anything except an agent waiting on the user.
- **Don't** add a failed, queued or error state to session cards or signal tabs.
- **Don't** add a third theme or per-user accent colours. There are exactly two themes, Dark and Light.
- **Don't** bind a keyboard shortcut to Find in the catalog. Ctrl-K belongs to the terminals.
- **Don't** cast shadows from in-flow surfaces. Only floating popovers and menus get the floating-layer shadow.
- **Don't** use the ruled red for errors or emphasis. It is only the card's rule line.
- **Don't** set prose or chrome in JetBrains Mono, or fall back to the host's `monospace`.
- **Don't** use text glyphs (←, ✕, +, a letter in a box) as icons.
- **Don't** add glow, gradients or decorative motion. The only standing animation is the Working dots.
