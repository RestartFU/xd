use super::*;
use crate::metrics::{
    CONTROL_HEIGHT, LINE_LABEL, RADIUS_CONTROL, RADIUS_PANEL, RADIUS_TAB, TEXT_CAPTION,
    TEXT_CONTROL, TEXT_DISPLAY, TEXT_LABEL, TEXT_TITLE,
};

/// Room for a theme name and the chosen dot.
const THEME_CARD_MIN_WIDTH: f32 = 136.0;
/// The gap between theme cards, `gap_4`.
const THEME_CARD_GAP: f32 = 16.0;
/// The settings page's navigation column, and the narrower one for a small
/// window.
const SETTINGS_NAV_WIDTH: f32 = 248.0;
const SETTINGS_NAV_NARROW_WIDTH: f32 = 208.0;
/// Below this window width the navigation column narrows.
const SETTINGS_NARROW_WINDOW: f32 = 900.0;
/// The reading column of a section, and its side padding.
const SETTINGS_COLUMN_WIDTH: f32 = 680.0;
const SETTINGS_COLUMN_PADDING: f32 = 40.0;
/// The padding inside a settings card, around a row's content.
const SETTINGS_CARD_PADDING: f32 = 14.0;
/// A section's row in the navigation column: a little taller than a menu
/// row, as a sidebar's rows are in System Settings, for an icon beside it.
const SETTINGS_NAV_ROW_HEIGHT: f32 = 34.0;
/// The title bar stays visible above the settings page.
const SETTINGS_TITLEBAR_HEIGHT: f32 = 38.0;

const MESSAGE_ICON: &str = "icons/message-square.svg";
const BOT_ICON: &str = "icons/bot.svg";
const KEYBOARD_ICON: &str = "icons/keyboard.svg";

/// What Settings says about xd's version where the host cannot install an
/// update itself: a source build, or the Windows app, whose host lives in WSL.
///
/// It used to say only that the update was manual, whatever "Check now" had
/// found, so checking appeared to do nothing.
fn manual_update_status(status: &SelfUpdateStatus, windows: bool) -> String {
    match status.latest.as_deref() {
        Some(latest) if status.available && windows => {
            format!("Update available: {latest}. Download the installer and run it to update.")
        }
        Some(latest) if status.available => {
            format!("Update available: {latest}. This installation requires a manual update.")
        }
        Some(_) => format!("xd {}. Up to date.", status.version),
        None => {
            "This installation requires a manual update. Check now to see if there is one.".into()
        }
    }
}

/// The installer of the newest build on this app's channel.
fn windows_installer_url(nightly: bool) -> &'static str {
    if nightly {
        "https://github.com/RestartFU/xd/releases/download/nightly/xd-nightly-windows-x86_64.msi"
    } else {
        "https://github.com/RestartFU/xd/releases/latest/download/xd-windows-x86_64.msi"
    }
}

/// How many theme cards sit in a row across `room`: all of them, or two to
/// a row, or one, whichever keeps every card wide enough for its name. With two
/// themes, both sit side by side unless the panel is very narrow.
fn theme_columns(room: f32) -> usize {
    [ThemePreset::ALL.len(), 2]
        .into_iter()
        .find(|&columns| theme_card_width(room, columns) >= THEME_CARD_MIN_WIDTH)
        .unwrap_or(1)
}

fn theme_card_width(room: f32, columns: usize) -> f32 {
    // A pixel short, so rounding never pushes the last card onto a new row.
    ((room - THEME_CARD_GAP * (columns - 1) as f32) / columns as f32).floor() - 1.0
}

/// The width of a section's reading column in a window `window_width` wide.
fn settings_column_width(window_width: f32) -> f32 {
    let nav = if window_width < SETTINGS_NARROW_WINDOW {
        SETTINGS_NAV_NARROW_WIDTH
    } else {
        SETTINGS_NAV_WIDTH
    };
    (window_width - nav - 2.0 * SETTINGS_COLUMN_PADDING).clamp(0.0, SETTINGS_COLUMN_WIDTH)
}

/// One page of settings in the navigation.
struct SettingsSection {
    name: &'static str,
    summary: &'static str,
    icon: &'static str,
}

/// The settings section that holds the assistant accounts.
pub(super) const AGENTS_SECTION: usize = 2;

const SETTINGS_SECTIONS: [SettingsSection; 7] = [
    SettingsSection {
        name: "Appearance",
        summary: "How xd looks.",
        icon: SLIDERS_ICON,
    },
    SettingsSection {
        name: "Chats",
        summary: "How chats start, run and are named.",
        icon: MESSAGE_ICON,
    },
    SettingsSection {
        name: "Agents",
        summary: "The assistants xd runs, and what they may do.",
        icon: BOT_ICON,
    },
    SettingsSection {
        name: "Cleanup",
        summary: "When finished sessions settle and go away.",
        icon: TRASH_ICON,
    },
    SettingsSection {
        name: "Devices",
        summary: "Reach this workspace from other machines.",
        icon: SWITCH_ICON,
    },
    SettingsSection {
        name: "Keyboard",
        summary: "Every shortcut, by where it works.",
        icon: KEYBOARD_ICON,
    },
    SettingsSection {
        name: "Updates",
        summary: "Keep xd current.",
        icon: DOWNLOAD_ICON,
    },
];

/// The icons the settings page adds, for the embedded asset source.
pub(super) fn settings_icon(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        MESSAGE_ICON => include_bytes!("../assets/icons/message-square.svg").as_slice(),
        BOT_ICON => include_bytes!("../assets/icons/bot.svg").as_slice(),
        KEYBOARD_ICON => include_bytes!("../assets/icons/keyboard.svg").as_slice(),
        _ => return None,
    })
}

/// One setting: what search matches it by, and how it draws.
pub(super) struct SettingRow {
    pub(super) title: SharedString,
    /// What search reads besides the title: the description shown under it,
    /// or text the row shows some other way, such as a shortcut's keys.
    pub(super) description: SharedString,
    element: gpui::AnyElement,
}

impl SettingRow {
    /// Adds words a row shows outside its description, such as a stepper's
    /// value, to what search reads.
    fn searchable(mut self, shown: impl std::fmt::Display) -> Self {
        self.description = format!("{} {shown}", self.description).into();
        self
    }

    /// A row drawn some other way than [`setting`], such as the theme cards.
    fn custom(
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        element: impl IntoElement,
    ) -> Self {
        Self {
            title: title.into(),
            description: description.into(),
            element: element.into_any_element(),
        }
    }
}

/// Rows that belong together on a section's page, under an optional label.
struct SettingGroup {
    label: Option<&'static str>,
    rows: Vec<SettingRow>,
}

fn group(label: &'static str, rows: Vec<SettingRow>) -> SettingGroup {
    SettingGroup {
        label: Some(label),
        rows,
    }
}

/// The words of a search, lowercased. Empty when nothing is being searched.
pub(super) fn settings_query_words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether a setting matches every word of a search, by its section's name,
/// its title or its description.
fn setting_matches(words: &[String], section: &str, title: &str, description: &str) -> bool {
    let text = format!("{section} {title} {description}").to_lowercase();
    words.iter().all(|word| text.contains(word.as_str()))
}

/// Every shortcut xd has, in the groups Settings › Keyboard shows them in:
/// what each does, and its keys as this platform names them. A key the
/// terminal or a text box uses for itself stays with it while that has the
/// keyboard, which the Terminal group says.
pub(super) fn keyboard_shortcuts() -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    // Ctrl on Linux and Windows, Cmd on macOS.
    let m = super::MOD_KEY;
    let paste = if cfg!(target_os = "macos") {
        "Cmd V"
    } else {
        "Ctrl Shift V"
    };
    let ends = if cfg!(target_os = "macos") {
        "Ctrl Home / Ctrl End, or Cmd Up / Cmd Down"
    } else {
        "Ctrl Home / Ctrl End"
    };
    vec![
        (
            "Window",
            vec![
                ("Find a session or project", format!("{m} K")),
                ("Next / previous session", "Alt Down / Alt Up".into()),
                ("Next session waiting on you", "Alt Shift Down".into()),
                (
                    "Next / previous chat tab",
                    "Ctrl Tab / Ctrl Shift Tab".into(),
                ),
                ("New chat tab", format!("{m} T")),
                ("Close the chat or agent tab in front", format!("{m} W")),
                ("Go to the message box", format!("{m} L / Shift Escape")),
                (
                    "Stop the running turn, or the agent whose tab is in front",
                    format!("{m} ."),
                ),
                ("Show or hide the side pane", format!("{m} B")),
                ("Show or hide the terminal under the chat", "Ctrl `".into()),
                (
                    "Narrow / widen the sidebar",
                    format!("{m} Alt Left / {m} Alt Right"),
                ),
                (
                    "Widen / narrow the side pane",
                    format!("{m} Alt Shift Left / {m} Alt Shift Right"),
                ),
                (
                    "Taller / shorter terminal under the chat",
                    format!("{m} Alt Up / {m} Alt Down"),
                ),
                ("The Terminal page", format!("{m} Shift T")),
                ("Settings", format!("{m} ,")),
                ("Usage", format!("{m} U")),
                ("Larger / smaller interface", format!("{m} = / {m} -")),
                ("Interface size back to 100 %", format!("{m} 0")),
                ("Copy the selected text", format!("{m} C")),
                (
                    "Scroll the chat, or the side pane you last clicked in, by a page",
                    "Page Up / Page Down".into(),
                ),
                ("Scroll to the top / bottom", ends.into()),
                ("Undo the last settle, discard or removal", format!("{m} Z")),
                (
                    "Close a dialog, menu, banner or notice, clear a search, or leave the chat",
                    "Escape".into(),
                ),
            ],
        ),
        (
            "Menus and dialogs",
            vec![
                (
                    "Move through a menu, a picker, find results or the landing page",
                    "Up / Down".into(),
                ),
                (
                    "Choose the row, or press the dialog's main button",
                    "Enter".into(),
                ),
                (
                    "Next / previous field or button of a dialog",
                    "Tab / Shift Tab".into(),
                ),
                (
                    "Press the button, or flip the switch, that Tab is on",
                    "Enter or Space".into(),
                ),
                (
                    "Open the menu of the open session, of the current project on the landing page, or of the sidebar row the ring is on",
                    "Shift F10 / Menu".into(),
                ),
                (
                    "In the sidebar's projects and sessions, which Tab reaches as one: to the row above or below, the first or the last",
                    "Up / Down, Home / End".into(),
                ),
                (
                    "Close or open a project, folder or session's agents, or go to the row it is in or the first in it",
                    "Left / Right".into(),
                ),
                (
                    "Open the session the sidebar's ring is on, or close or open its project",
                    "Enter".into(),
                ),
                (
                    "Through the tabs' close buttons, the header's pull requests, each message's Copy and pictures, the queue, the background tasks, an agent tab's Background and Stop, and the terminal's tabs",
                    "Tab / Shift Tab".into(),
                ),
            ],
        ),
        (
            "Settings",
            vec![
                ("Next / previous section", "Down / Up".into()),
                (
                    "Through the search field and the controls of the open section",
                    "Tab / Shift Tab".into(),
                ),
                (
                    "Flip a switch, press a button, or open a dropdown",
                    "Enter or Space".into(),
                ),
                (
                    "Move through an open dropdown, and choose",
                    "Up / Down, Enter".into(),
                ),
            ],
        ),
        (
            "Chats",
            vec![
                ("Send, or queue while the agent works", "Enter".into()),
                ("Redirect the agent while it works", format!("{m} Enter")),
                ("New line", "Shift Enter".into()),
                (
                    "From the message box to the options under it, and through them",
                    "Tab / Shift Tab".into(),
                ),
                ("Open the option Tab is on", "Enter or Space".into()),
                ("Back to the message box from its options", "Escape".into()),
                (
                    "From the message box back through its attachments: each picture, which opens in the viewer, and its Remove",
                    "Shift Tab".into(),
                ),
                (
                    "Remove the last attachment, with the caret at the very start of the message box",
                    "Backspace".into(),
                ),
                ("Indent, inside a ``` code block", "Tab".into()),
                (
                    "Copy a message, from the menu of the message whose Copy Tab is on",
                    "Shift F10 / Menu".into(),
                ),
                ("Copy / cut / paste", format!("{m} C / {m} X / {m} V")),
                ("Select all", format!("{m} A")),
                ("Undo / redo", format!("{m} Z / {m} Shift Z or Ctrl Y")),
                ("Pick an answer to a question", "1 to 9".into()),
                ("Move between a question's answers", "Up / Down".into()),
            ],
        ),
        (
            "Terminal",
            vec![
                ("Interrupt command", "Ctrl C".into()),
                ("Copy selection", "Ctrl Shift C".into()),
                ("Paste", paste.into()),
                ("Clear terminal", "Ctrl L".into()),
                ("Search command history", "Ctrl R".into()),
                ("Scroll back and forward", "Page Up / Page Down".into()),
                (
                    "Every other Ctrl and letter goes to the shell, so Ctrl K, L, B, T and W do not reach xd from here",
                    "Ctrl A to Z".into(),
                ),
                (
                    "Alt with an arrow goes to the program too, so these do not change session from here",
                    "Alt Up / Alt Down / Alt Shift Down".into(),
                ),
            ],
        ),
        (
            "Files",
            vec![
                ("Save the file", format!("{m} S")),
                ("Undo / redo", format!("{m} Z / {m} Shift Z or Ctrl Y")),
                ("Indent", "Tab".into()),
            ],
        ),
        (
            "Usage",
            vec![
                ("Show cost / tokens / limits", "C / T / L".into()),
                (
                    "Last day / 7 days / 30 days / 90 days",
                    format!("{m} Shift 1 to 4"),
                ),
            ],
        ),
        (
            "Image viewer",
            vec![
                ("Previous / next image", "Left / Right".into()),
                ("Zoom in / out", "+ / -".into()),
                ("Fit the window, or actual size", "Enter or Space".into()),
                ("Reset zoom", "0".into()),
                ("Move around a zoomed image", "Arrow keys".into()),
                ("Close", "Escape".into()),
            ],
        ),
    ]
}

/// How a saved naming model the host does not offer is named.
fn unlisted_model_label(id: &str) -> String {
    format!("{id} (not offered by this host)")
}

/// Why a setting was not saved, in the danger ink under its words.
fn setting_failure_line(error: SharedString, colors: ThemeColors) -> gpui::Div {
    div()
        .debug_selector(|| "setting-failure".into())
        .mt(px(4.0))
        .flex()
        .items_start()
        .gap(px(6.0))
        .text_size(text_px(TEXT_LABEL))
        .line_height(px(LINE_LABEL))
        .text_color(rgb(colors.danger))
        .child(
            div()
                .h(px(17.0))
                .flex()
                .items_center()
                .child(icon(ALERT_ICON, 12.0, colors.danger)),
        )
        .child(div().min_w_0().child(error))
}

/// The row a setting draws: its title and description on the left, with the
/// caller adding the control on the right, and under them why its last save
/// failed, when it did.
fn settings_row(
    id: impl Into<gpui::ElementId>,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    let description = description.into();
    let id = id.into();
    let failure = super::setting_failure(&id);
    div()
        .id(id)
        .w_full()
        .min_h(px(52.0))
        .px(px(SETTINGS_CARD_PADDING))
        .py(px(11.0))
        .flex()
        // In a column too narrow for both, as the smallest window's is at
        // the largest Interface sizes, the control goes under the words
        // instead of squeezing them to a letter a line.
        .flex_wrap()
        .items_center()
        .gap_x_4()
        .gap_y_2()
        .child(
            div()
                .flex_1()
                .min_w(px(SETTINGS_ROW_TEXT_MIN))
                .child(
                    div()
                        .text_size(text_px(TEXT_CONTROL))
                        .text_color(rgb(colors.text))
                        .child(title.into()),
                )
                .when(!description.is_empty(), |body| {
                    body.child(
                        div()
                            .mt(px(3.0))
                            .text_size(text_px(TEXT_LABEL))
                            .line_height(px(LINE_LABEL))
                            .text_color(rgb(colors.muted))
                            .child(description),
                    )
                })
                .when_some(failure, |body, error| {
                    body.child(setting_failure_line(error, colors))
                }),
        )
}

/// A setting drawn as a row, with `build` adding its control.
fn setting(
    id: impl Into<gpui::ElementId>,
    title: &'static str,
    description: impl Into<SharedString>,
    colors: ThemeColors,
    build: impl FnOnce(gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div>,
) -> SettingRow {
    let description = description.into();
    SettingRow {
        title: title.into(),
        description: description.clone(),
        element: build(settings_row(id, title, description, colors)).into_any_element(),
    }
}

/// Settings that belong together, in one glass card with hairlines between.
fn settings_card(rows: Vec<SettingRow>, colors: ThemeColors) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(RADIUS_PANEL))
        .overflow_hidden()
        .border_1()
        .border_color(crate::glass::edge(colors))
        .bg(crate::glass::pane(colors))
        .children(rows.into_iter().enumerate().map(|(index, row)| {
            div()
                .w_full()
                .when(index > 0, |row| {
                    row.border_t_1().border_color(crate::glass::edge(colors))
                })
                .child(row.element)
        }))
}

/// A small label above a card.
fn settings_group_label(label: impl Into<SharedString>, colors: ThemeColors) -> gpui::Div {
    div()
        .mt(px(28.0))
        .mb(px(8.0))
        .px(px(2.0))
        .text_size(text_px(TEXT_LABEL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(colors.muted))
        .child(label.into())
}

/// A minus or plus button beside a number. At a limit it is drawn as a
/// button that cannot be pressed, and Tab passes over it.
fn settings_step_button(
    keys: &KeyTargets,
    id: &'static str,
    label: &'static str,
    colors: ThemeColors,
    cx: &Context<XdDesktop>,
    run: Option<impl Fn(&mut XdDesktop, &mut Window, &mut Context<XdDesktop>) + 'static>,
) -> gpui::Stateful<gpui::Div> {
    let button = ui_button_when(run.is_some(), id, ButtonKind::Secondary, colors)
        .w(px(CONTROL_HEIGHT))
        .px_0()
        .child(label);
    match run {
        Some(run) => button.key_stop(keys, KeyLayer::Overlay, id, colors, cx, run),
        None => button,
    }
}

/// The least width a setting's title and description keep beside its
/// control.
const SETTINGS_ROW_TEXT_MIN: f32 = 150.0;

/// How a section of Settings' list is named as a Tab stop: this, then its
/// place in the list.
const SECTION_STOP: &str = "workspace-settings-section-";

/// The corners of the ring around a section of Settings' list: a tab's,
/// moved out.
const SECTION_RING_RADIUS: f32 = RADIUS_TAB + crate::FOCUS_RING_GAP;

/// The list the arrow keys are in while a Settings dropdown is open.
const SETTINGS_MENU: &str = "settings-menu";
/// The Interface size dropdown, among the page's dropdowns.
const INTERFACE_SIZE_MENU: &str = "interface-size";
/// The Interface size row, for a failure to save it.
pub(super) const INTERFACE_SIZE_SETTING: &str = "workspace-settings-interface-size";
const INTERFACE_SIZE_DESCRIPTION: &str = "Scales everything in the window: text, controls, spacing and the terminal. Pages in the browser keep their own size.";

/// A setting that is a switch. A click anywhere on its row flips it; Tab
/// stops on the switch itself, which Space and Enter flip.
pub(super) fn key_switch(
    row: gpui::Stateful<gpui::Div>,
    keys: &KeyTargets,
    id: impl Into<SharedString>,
    on: bool,
    colors: ThemeColors,
    cx: &Context<XdDesktop>,
    run: impl Fn(&mut XdDesktop, &mut Window, &mut Context<XdDesktop>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let run: KeyRun = Rc::new(run);
    let id = id.into();
    let ringed = keys.stop_on(&id);
    row.cursor_pointer()
        .on_click(cx.listener({
            let run = run.clone();
            move |this, _, window, cx| run(this, window, cx)
        }))
        .child(
            div()
                .relative()
                .flex_none()
                .child(settings_switch(colors, on))
                .child(keys.mark(KeyLayer::Overlay, KeyStop::Control(id, run)))
                .when(ringed, |switch| {
                    switch.child(focus_ring(colors, PILL_RING_RADIUS))
                }),
        )
}

/// A number between a minus and a plus button.
fn settings_stepper(
    fewer: gpui::Stateful<gpui::Div>,
    value: String,
    more: gpui::Stateful<gpui::Div>,
    width: f32,
    colors: ThemeColors,
) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .child(fewer)
        .child(
            div()
                .w(px(width))
                .flex()
                .justify_center()
                .text_size(text_px(TEXT_CONTROL))
                .text_color(rgb(colors.text))
                .child(value),
        )
        .child(more)
}

/// The assistants a naming model can be chosen for, each with its row's element
/// id, title, and description. The host titles chats and names new worktrees
/// with the same low-cost model.
const CHAT_TITLE_BACKENDS: [(&str, &str, &str, &str); 2] = [
    (
        "codex",
        "workspace-settings-chat-title-codex",
        "Codex naming model",
        "Titles Codex chats and names the worktrees new sessions get.",
    ),
    (
        "claude",
        "workspace-settings-chat-title-claude",
        "Claude naming model",
        "Titles Claude chats and names the worktrees new sessions get.",
    ),
];

/// The host's chat title settings. They live on the host because that is where
/// titles are generated, for messages sent from any client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ChatTitleSettings {
    pub(super) enabled: bool,
    models: HashMap<String, String>,
}

impl ChatTitleSettings {
    pub(super) fn from_reply(reply: &Value) -> Option<Self> {
        Some(Self {
            enabled: reply.get("enabled")?.as_bool()?,
            models: reply
                .get("models")?
                .as_object()?
                .iter()
                .filter_map(|(backend, model)| Some((backend.clone(), model.as_str()?.to_owned())))
                .collect(),
        })
    }

    pub(super) fn model(&self, backend: &str) -> Option<&str> {
        self.models.get(backend).map(String::as_str)
    }
}

impl XdDesktop {
    pub(super) fn request_chat_title_settings(&mut self) {
        if let Some(host) = self.active_host()
            && let Err(error) = host.chat_title_settings()
        {
            self.model.connection_error = Some(error);
        }
    }

    fn toggle_chat_titles(&mut self, cx: &mut Context<Self>) {
        let Some(enabled) = self
            .chat_title_settings
            .as_ref()
            .map(|settings| !settings.enabled)
        else {
            return;
        };
        let result = match self.active_host() {
            Some(host) => host.set_chat_titles_enabled(enabled),
            None => return,
        };
        match result {
            Ok(()) => {
                if let Some(settings) = &mut self.chat_title_settings {
                    settings.enabled = enabled;
                }
            }
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    /// Opens one of the page's dropdowns, or closes it if it is the one open.
    pub(super) fn toggle_settings_menu(&mut self, menu: &'static str, cx: &mut Context<Self>) {
        self.settings_menu = (self.settings_menu != Some(menu)).then_some(menu);
        cx.notify();
    }

    fn choose_chat_title_model(&mut self, backend: &str, model: String, cx: &mut Context<Self>) {
        self.settings_menu = None;
        let result = match self.active_host() {
            Some(host) => host.set_chat_title_model(backend, &model),
            None => return,
        };
        match result {
            Ok(()) => {
                if let Some(settings) = &mut self.chat_title_settings {
                    settings.models.insert(backend.to_owned(), model);
                }
            }
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    /// The chat title switch and a naming model row per assistant, once the
    /// host has said what they are.
    fn chat_title_rows(&self, colors: ThemeColors, cx: &mut Context<Self>) -> Vec<SettingRow> {
        let keys = self.key_targets.clone();
        let Some(settings) = self.chat_title_settings.clone() else {
            return Vec::new();
        };
        let mut rows = vec![setting(
            "workspace-settings-chat-titles",
            "Generate chat titles",
            "Name a new chat from its first message, using its naming model.",
            colors,
            |row| {
                key_switch(
                    row,
                    &keys,
                    "workspace-settings-chat-titles",
                    settings.enabled,
                    colors,
                    cx,
                    |this, _, cx| this.toggle_chat_titles(cx),
                )
            },
        )];
        // Shown with titles off too: new worktrees are still named with these.
        for (backend, row, title, description) in CHAT_TITLE_BACKENDS {
            let catalog = self
                .model
                .agent_backends
                .iter()
                .find(|known| known.id == backend);
            let selected = settings.model(backend).map(str::to_owned);
            let models = catalog.map_or(&[][..], |catalog| catalog.models.as_slice());
            // A saved model the host no longer lists keeps its place: named
            // for what it is, ticked, and still the one in force.
            let unlisted = selected
                .clone()
                .filter(|selected| !models.iter().any(|model| &model.id == selected));
            let label = match (&selected, &unlisted) {
                (_, Some(unlisted)) => unlisted_model_label(unlisted),
                (Some(selected), None) => {
                    let name = models
                        .iter()
                        .find(|model| &model.id == selected)
                        .map(|model| model.name.as_str());
                    presentation::model_name(selected, name)
                }
                (None, None) => "Choose a model".to_owned(),
            };
            let kept = unlisted.map(|id| {
                let label = unlisted_model_label(&id);
                let choose: KeyRun = Rc::new(move |this, _, cx| {
                    this.choose_chat_title_model(backend, id.clone(), cx)
                });
                (label, true, choose)
            });
            let choices = kept
                .into_iter()
                .chain(models.iter().map(|model| {
                    let id = model.id.clone();
                    let choose: KeyRun = Rc::new(move |this, _, cx| {
                        this.choose_chat_title_model(backend, id.clone(), cx)
                    });
                    (
                        presentation::model_name(&model.id, Some(&model.name)),
                        selected.as_deref() == Some(model.id.as_str()),
                        choose,
                    )
                }))
                .collect();
            rows.push(self.dropdown_row(
                row,
                backend,
                title,
                description,
                label,
                220.0,
                choices,
                colors,
                cx,
            ));
        }
        rows
    }

    /// A setting chosen from a short list: a button that names the choice in
    /// force and opens the list under it, or over it where the window ends
    /// too soon under it. `menu` names the list among the page's dropdowns,
    /// one of which is open at a time.
    #[allow(clippy::too_many_arguments)]
    fn dropdown_row(
        &self,
        row: &'static str,
        menu: &'static str,
        title: &'static str,
        description: &'static str,
        label: String,
        width: f32,
        choices: Vec<(String, bool, KeyRun)>,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> SettingRow {
        let keys = self.key_targets.clone();
        let list = (self.settings_menu == Some(menu)).then(|| {
            menu_under(
                gpui::Corner::TopRight,
                CONTROL_HEIGHT,
                menu_shell(colors)
                    .id((row, usize::MAX))
                    .debug_selector(|| "settings-menu".into())
                    .min_w(px(width))
                    .max_h(px(menu_under_max_height(CONTROL_HEIGHT, 320.0)))
                    .overflow_y_scroll()
                    .track_scroll(&self.key_scroll)
                    .children(choices.into_iter().enumerate().map(
                        |(index, (name, chosen, choose))| {
                            menu_row((row, index + 1), colors)
                                .key_row(
                                    &keys,
                                    KeyLayer::Menu,
                                    SETTINGS_MENU,
                                    colors.surface_high,
                                    cx,
                                    move |this, window, cx| choose(this, window, cx),
                                )
                                .key_child(&keys, index)
                                .child(div().flex_1().min_w_0().child(name))
                                .child(menu_check(chosen, colors))
                        },
                    ))
                    // After the rows, which the keys scroll to by their place.
                    .child(scroll_thumb(self.key_scroll.clone(), colors)),
            )
        });
        SettingRow::custom(
            title,
            description,
            settings_row(row, title, description, colors).child(
                div()
                    .relative()
                    .flex_none()
                    .child(
                        ui_button((row, 0_usize), ButtonKind::Secondary, colors)
                            .debug_selector(move || row.into())
                            // The press that opens the menu is not one
                            // outside it, which would close it again.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .key_stop(
                                &keys,
                                KeyLayer::Overlay,
                                row,
                                colors,
                                cx,
                                move |this, _, cx| this.toggle_settings_menu(menu, cx),
                            )
                            .child(label)
                            .child(icon(CHEVRON_DOWN_ICON, 12.0, colors.muted)),
                    )
                    .children(list),
            ),
        )
    }

    /// The Interface size, as a dropdown of the sizes xd offers.
    fn interface_size_row(&self, colors: ThemeColors, cx: &mut Context<Self>) -> SettingRow {
        let chosen = self.settings.interface_size;
        let name = |size: u16| format!("{size} %");
        let choices = crate::metrics::INTERFACE_SIZES
            .into_iter()
            .map(|size| {
                let choose: KeyRun = Rc::new(move |this, window, cx| {
                    this.settings_menu = None;
                    this.set_interface_size(size, window, cx);
                });
                (name(size), size == chosen, choose)
            })
            .collect();
        self.dropdown_row(
            INTERFACE_SIZE_SETTING,
            INTERFACE_SIZE_MENU,
            "Interface size",
            INTERFACE_SIZE_DESCRIPTION,
            name(chosen),
            120.0,
            choices,
            colors,
            cx,
        )
        // Search finds it by its sizes and by the words people look for.
        .searchable("90 % 100 % 115 % 130 % 150 % zoom scale text size larger smaller")
    }

    /// The connected host's settling rules, or nothing while it has not said
    /// what they are: an older host does not have any.
    fn auto_settle_rows(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Vec<SettingRow> {
        let keys = self.key_targets.clone();
        let Some(rules) = self.auto_settle else {
            return Vec::new();
        };
        let mut rows = vec![
            setting(
                "workspace-settings-settle-merged",
                "Settle merged sessions",
                "Settle a session when its pull request merges. A closed pull request settles it while either rule here is on.",
                colors,
                |row| {
                    key_switch(
                        row,
                        &keys,
                        "workspace-settings-settle-merged",
                        rules.on_merge,
                        colors,
                        cx,
                        move |this, _, cx| {
                            this.change_auto_settle(
                                AutoSettleRules {
                                    on_merge: !rules.on_merge,
                                    ..rules
                                },
                                cx,
                            );
                        },
                    )
                },
            ),
            setting(
                "workspace-settings-settle-inactive",
                "Settle inactive sessions",
                "Sessions with no activity for a while settle on their own. Any new activity brings them back.",
                colors,
                |row| {
                    key_switch(
                        row,
                        &keys,
                        "workspace-settings-settle-inactive",
                        rules.after_days.is_some(),
                        colors,
                        cx,
                        move |this, _, cx| {
                            this.change_auto_settle(
                                AutoSettleRules {
                                    after_days: match rules.after_days {
                                        Some(_) => None,
                                        None => Some(AutoSettleRules::DEFAULT_AFTER_DAYS),
                                    },
                                    ..rules
                                },
                                cx,
                            );
                        },
                    )
                },
            ),
        ];
        let Some(days) = rules.after_days else {
            return rows;
        };
        let step = |id: &'static str, label: &'static str, to: i64, cx: &mut Context<Self>| {
            let allowed =
                (AutoSettleRules::MIN_AFTER_DAYS..=AutoSettleRules::MAX_AFTER_DAYS).contains(&to);
            settings_step_button(
                &keys,
                id,
                label,
                colors,
                cx,
                allowed.then_some(
                    move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                        this.change_auto_settle(
                            AutoSettleRules {
                                after_days: Some(to),
                                ..rules
                            },
                            cx,
                        );
                    },
                ),
            )
        };
        let fewer = step("settle-days-fewer", "\u{2212}", days - 1, cx);
        let more = step("settle-days-more", "+", days + 1, cx);
        rows.push(
            setting(
                "workspace-settings-settle-days",
                "Days before settling",
                "How long a session sits without activity before it settles.",
                colors,
                |row| {
                    row.child(settings_stepper(
                        fewer,
                        days.to_string(),
                        more,
                        28.0,
                        colors,
                    ))
                },
            )
            .searchable(days),
        );
        rows.push(setting(
            "workspace-settings-settle-keep-pr",
            "Keep sessions with a pull request",
            "Inactivity does not settle sessions with a pull request. Merge and close still do.",
            colors,
            |row| {
                key_switch(
                    row,
                    &keys,
                    "workspace-settings-settle-keep-pr",
                    rules.keep_pull_requests,
                    colors,
                    cx,
                    move |this, _, cx| {
                        this.change_auto_settle(
                            AutoSettleRules {
                                keep_pull_requests: !rules.keep_pull_requests,
                                ..rules
                            },
                            cx,
                        );
                    },
                )
            },
        ));
        rows
    }

    /// The connected host's deletion rule, or nothing while it has not said
    /// what it is: an older host does not have one.
    fn auto_delete_rows(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Vec<SettingRow> {
        let keys = self.key_targets.clone();
        let Some(rules) = self.auto_delete else {
            return Vec::new();
        };
        let mut rows = vec![setting(
            "workspace-settings-delete-inactive",
            "Delete inactive sessions",
            "Delete sessions with no activity for a while, with the worktree xd made for them. Branches and their commits are kept.",
            colors,
            |row| {
                key_switch(
                    row,
                    &keys,
                    "workspace-settings-delete-inactive",
                    rules.after_days.is_some(),
                    colors,
                    cx,
                    move |this, _, cx| {
                        this.change_auto_delete(
                            AutoDeleteRules {
                                after_days: match rules.after_days {
                                    Some(_) => None,
                                    None => Some(AutoDeleteRules::DEFAULT_AFTER_DAYS),
                                },
                            },
                            cx,
                        );
                    },
                )
            },
        )];
        let Some(days) = rules.after_days else {
            return rows;
        };
        let step = |id: &'static str, label: &'static str, longer: bool, cx: &mut Context<Self>| {
            let to = AutoDeleteRules::step(days, longer);
            settings_step_button(
                &keys,
                id,
                label,
                colors,
                cx,
                to.map(|to| {
                    move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                        this.change_auto_delete(
                            AutoDeleteRules {
                                after_days: Some(to),
                            },
                            cx,
                        );
                    }
                }),
            )
        };
        let fewer = step("delete-days-fewer", "\u{2212}", false, cx);
        let more = step("delete-days-more", "+", true, cx);
        rows.push(setting(
            "workspace-settings-delete-days",
            "Days before deleting",
            "A worktree with uncommitted changes keeps its session until you commit or clean it up.",
            colors,
            |row| row.child(settings_stepper(fewer, days.to_string(), more, 28.0, colors)),
        )
        .searchable(days));
        rows
    }

    /// The connected host's default chat quota, on a host that has quotas
    /// and once it has said what the quota is.
    pub(super) fn chat_quota_rows(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<SettingRow> {
        let keys = self.key_targets.clone();
        let Some(settings) = self.chat_quota.filter(|_| self.model.chat_quotas_supported) else {
            return Vec::new();
        };
        let mut rows = vec![setting(
            "workspace-settings-chat-quota",
            "Limit each chat's usage",
            match settings.percent {
                Some(percent) => format!(
                    "Limit each chat to {percent}% of its account's 5-hour usage limit. A chat that reaches it stops and asks before going on."
                ),
                None => "Limit each chat to a share of its account's 5-hour usage limit. A chat that reaches it stops and asks before going on.".to_owned(),
            },
            colors,
            |row| {
                key_switch(row, &keys, "workspace-settings-chat-quota", settings.percent.is_some(), colors, cx, move |this, _, cx| {
                        this.change_chat_quota_settings(
                            ChatQuotaSettings {
                                percent: match settings.percent {
                                    Some(_) => None,
                                    None => Some(ChatQuotaSettings::DEFAULT_PERCENT),
                                },
                            },
                            cx,
                        );
                    })
            },
        )];
        let Some(percent) = settings.percent else {
            return rows;
        };
        let step = |id: &'static str, label: &'static str, larger: bool, cx: &mut Context<Self>| {
            let to = ChatQuotaSettings::step(percent, larger);
            settings_step_button(
                &keys,
                id,
                label,
                colors,
                cx,
                to.map(|to| {
                    move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                        this.change_chat_quota_settings(
                            ChatQuotaSettings { percent: Some(to) },
                            cx,
                        );
                    }
                }),
            )
        };
        let fewer = step("chat-quota-smaller", "\u{2212}", false, cx);
        let more = step("chat-quota-larger", "+", true, cx);
        rows.push(
            setting(
                "workspace-settings-chat-quota-percent",
                "Quota per chat",
                "A chat's own quota, chosen when it stops, takes the place of this one.",
                colors,
                |row| {
                    row.child(settings_stepper(
                        fewer,
                        format!("{percent}%"),
                        more,
                        40.0,
                        colors,
                    ))
                },
            )
            .searchable(format_args!("{percent}%")),
        );
        rows
    }

    /// A switch row for one of this device's own settings, saved on change.
    #[allow(clippy::too_many_arguments)]
    fn local_switch(
        id: &'static str,
        title: &'static str,
        description: &'static str,
        enabled: bool,
        toggle: fn(&mut AppSettings),
        keys: &KeyTargets,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> SettingRow {
        setting(id, title, description, colors, |row| {
            key_switch(
                row.debug_selector(move || id.into()),
                keys,
                id,
                enabled,
                colors,
                cx,
                move |this, _, cx| {
                    toggle(&mut this.settings);
                    super::set_reduce_motion(this.settings.reduce_motion);
                    this.save_setting(id);
                    cx.notify();
                },
            )
        })
    }

    /// The theme cards, as one setting.
    fn theme_row(&self, column: f32, colors: ThemeColors, cx: &mut Context<Self>) -> SettingRow {
        let keys = self.key_targets.clone();
        // The card's own hairlines stay one pixel of the window each, which
        // is more than a logical pixel under 100 %: left out, the two theme
        // cards came out a fraction too wide for one row at 90 %.
        let room = column - 2.0 * SETTINGS_CARD_PADDING - 2.0 * logical(gpui::px(1.0));
        let card_width = theme_card_width(room, theme_columns(room));
        let cards = ThemePreset::ALL
            .into_iter()
            .enumerate()
            .map(|(index, theme)| {
                let swatch = theme.colors();
                let selected = self.settings.theme == theme;
                let stop: SharedString = format!("minimal-theme-choice-{index}").into();
                let choose: KeyRun = Rc::new(move |this, _, cx| {
                    this.choose_minimal_theme(theme, cx);
                    this.minimal_theme_open = true;
                    cx.notify();
                });
                let card = div()
                    .id(("minimal-theme-choice", index))
                    .w(px(card_width))
                    .rounded(px(RADIUS_PANEL))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(if selected {
                        colors.selected_border
                    } else {
                        colors.border
                    }))
                    .bg(rgb(colors.surface))
                    .cursor_pointer()
                    .hover(|style| style.border_color(rgb(colors.selected_border)))
                    .child(
                        div()
                            .h(px(104.0))
                            .p_3()
                            .flex()
                            .gap(px(10.0))
                            .bg(crate::glass::window(swatch))
                            .child(
                                div()
                                    .w(px(28.0))
                                    .h_full()
                                    .rounded(px(RADIUS_CONTROL))
                                    .bg(crate::glass::pane(swatch)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .pt_3()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    // The accent, so the previews show more than a ground.
                                    .child(
                                        div()
                                            .w(px(72.0))
                                            .h(px(5.0))
                                            .rounded_full()
                                            .bg(rgb(swatch.accent)),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(4.0))
                                            .rounded_full()
                                            .bg(rgb(swatch.border)),
                                    )
                                    .child(
                                        div()
                                            .w(px(56.0))
                                            .h(px(4.0))
                                            .rounded_full()
                                            .bg(rgb(swatch.border)),
                                    )
                                    .child(
                                        div()
                                            .mt_2()
                                            .w_full()
                                            .h(px(22.0))
                                            .rounded(px(RADIUS_TAB))
                                            .border_1()
                                            .border_color(crate::glass::edge(swatch))
                                            .bg(crate::glass::composer(swatch)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .h(px(40.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(text_px(TEXT_CONTROL))
                            .text_color(rgb(colors.text))
                            .child(theme.label())
                            .when(selected, |label| {
                                label.child(icon(CHECK_ICON, 14.0, colors.accent_ink))
                            }),
                    );
                // The card clips to its corners, so the ring goes around it.
                div()
                    .id(("minimal-theme-choice-stop", index))
                    .relative()
                    .flex_none()
                    .child(card)
                    .key_stop_ring(
                        &keys,
                        KeyLayer::Overlay,
                        stop,
                        13.0,
                        colors,
                        cx,
                        move |this, window, cx| choose(this, window, cx),
                    )
            });
        SettingRow::custom(
            "Theme",
            "Dark or Light. Both are frosted glass.",
            div()
                .p(px(SETTINGS_CARD_PADDING))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .child(div().text_size(text_px(TEXT_CONTROL)).child("Theme"))
                        .child(
                            div()
                                .mt(px(3.0))
                                .text_size(text_px(TEXT_LABEL))
                                .text_color(rgb(colors.muted))
                                .child("Dark or Light. Both are frosted glass."),
                        )
                        .children(
                            super::setting_failure(&THEME_SETTING.into())
                                .map(|error| setting_failure_line(error, colors)),
                        ),
                )
                // Side by side when there is room, else one to a row, so the
                // cards never clip their names.
                .child(div().flex().flex_wrap().gap_4().children(cards)),
        )
    }

    /// The version of each assistant's command-line tool the host runs, or
    /// why there is none to show.
    pub(super) fn assistant_version_rows(&self, colors: ThemeColors) -> Vec<SettingRow> {
        let row = |id: String, title: String, description: String| SettingRow {
            title: title.clone().into(),
            description: description.clone().into(),
            element: settings_row(SharedString::from(id), title, description, colors)
                .into_any_element(),
        };
        if self.cli_versions.is_empty() {
            let description = match (&self.cli_versions_error, self.cli_versions_loading) {
                (Some(error), _) => format!("Could not read the versions. {error}"),
                (None, true) => "Checking the assistants' versions…".to_owned(),
                (None, false) => "The host has not said which versions it runs.".to_owned(),
            };
            return vec![row(
                "workspace-settings-cli-versions".into(),
                "Assistant versions".into(),
                description,
            )];
        }
        let mut rows = self
            .cli_versions
            .iter()
            .map(|cli| {
                let name = if cli.display_name.is_empty() {
                    cli.provider.clone()
                } else {
                    cli.display_name.clone()
                };
                let version = cli.version.as_deref().unwrap_or("not installed");
                let description = match cli.state.as_str() {
                    "checking" => format!("{version}. Checking for an update…"),
                    "updating" => format!("{version}. Updating…"),
                    "updated" => format!("Updated to {version}."),
                    "failed" => format!(
                        "{version}. The update failed: {}",
                        cli.detail.as_deref().unwrap_or("no reason given")
                    ),
                    _ => match &cli.latest {
                        Some(latest) if cli.version.as_deref() != Some(latest) => {
                            format!("{version}. {latest} is available.")
                        }
                        _ => format!("{version}. Up to date."),
                    },
                };
                row(
                    format!("workspace-settings-cli-{}", cli.provider),
                    name,
                    description,
                )
            })
            .collect::<Vec<_>>();
        if let Some(error) = &self.cli_versions_error {
            rows.push(row(
                "workspace-settings-cli-versions".into(),
                "Assistant versions".into(),
                format!("Could not refresh the versions. {error}"),
            ));
        }
        rows
    }

    /// The update status of xd itself, with what can be done about it.
    fn update_row(&self, colors: ThemeColors, cx: &mut Context<Self>) -> SettingRow {
        let keys = self.key_targets.clone();
        let panel = self.self_update_panel.as_ref();
        let status: String = panel
            .map(|panel| {
                if let Some(error) = &panel.error {
                    return error.clone();
                }
                match panel.status.as_ref() {
                    Some(status) if status.state == "installed" => {
                        "Installed. Restart xd to finish updating.".into()
                    }
                    Some(status) if status.state == "installing" => "Installing update…".into(),
                    Some(status) if status.state == "checking" => "Checking for updates…".into(),
                    Some(status) if !status.supported => {
                        manual_update_status(status, cfg!(target_os = "windows"))
                    }
                    Some(status) if status.available => format!(
                        "Update available: {}",
                        status.latest.as_deref().unwrap_or("new version")
                    ),
                    Some(status) if status.latest.is_none() => {
                        format!("xd {}. Not checked yet.", status.version)
                    }
                    Some(status) => format!("xd {}. Up to date.", status.version),
                    None => "Connecting to the local update service…".into(),
                }
            })
            .unwrap_or_else(|| "Check for an update to xd.".into());
        let busy = panel.is_some_and(|panel| panel.busy);
        let state = |wanted: &str| {
            panel.is_some_and(|panel| {
                panel
                    .status
                    .as_ref()
                    .is_some_and(|status| status.state == wanted)
            })
        };
        let installed = state("installed");
        let available = panel.is_some_and(|panel| {
            panel
                .status
                .as_ref()
                .is_some_and(|status| status.supported && status.available)
        });
        // Windows updates by running the newer installer over the old one.
        let download = cfg!(target_os = "windows")
            && panel.is_some_and(|panel| {
                panel.error.is_none()
                    && panel
                        .status
                        .as_ref()
                        .is_some_and(|status| !status.supported && status.available)
            });
        setting(
            "workspace-settings-update",
            "xd updates",
            status,
            colors,
            |row| {
                row.child(
                    div()
                        .flex_none()
                        .flex()
                        .gap_2()
                        .when(!busy && !installed, |actions| {
                            actions.child(
                                ui_button("check-app-update", ButtonKind::Secondary, colors)
                                    .key_stop(
                                        &keys,
                                        KeyLayer::Overlay,
                                        "check-app-update",
                                        colors,
                                        cx,
                                        |this, _, cx| this.request_app_update("check", cx),
                                    )
                                    .child("Check now"),
                            )
                        })
                        .when(!busy && download, |actions| {
                            actions.child(
                                ui_button("download-app-update", ButtonKind::Primary, colors)
                                    .key_stop(
                                        &keys,
                                        KeyLayer::Overlay,
                                        "download-app-update",
                                        colors,
                                        cx,
                                        |_, _, cx| {
                                            cx.open_url(windows_installer_url(
                                                xd_desktop::channel::nightly(),
                                            ))
                                        },
                                    )
                                    .child("Download installer"),
                            )
                        })
                        .when(!busy && !installed && available, |actions| {
                            actions.child(
                                ui_button("install-app-update", ButtonKind::Primary, colors)
                                    .key_stop(
                                        &keys,
                                        KeyLayer::Overlay,
                                        "install-app-update",
                                        colors,
                                        cx,
                                        |this, _, cx| this.install_app_update(cx),
                                    )
                                    .child("Install"),
                            )
                        })
                        .when(!busy && installed, |actions| {
                            actions.child(
                                ui_button("restart-app-update", ButtonKind::Primary, colors)
                                    .key_stop(
                                        &keys,
                                        KeyLayer::Overlay,
                                        "restart-app-update",
                                        colors,
                                        cx,
                                        |this, _, cx| {
                                            if this.flush_draft_before_navigation() {
                                                this.request_app_update("restart", cx);
                                            }
                                        },
                                    )
                                    .child("Restart to update"),
                            )
                        }),
                )
            },
        )
    }

    /// Building and installing a branch, pull request or commit.
    fn source_build_row(&self, colors: ThemeColors, cx: &mut Context<Self>) -> SettingRow {
        let keys = self.key_targets.clone();
        let running = self.source_build_panel.running;
        let can_build =
            source_build::supported() && self.source_build_panel.target.is_some() && !running;
        let description = if source_build::supported() {
            "Build and install a branch, pull request, or commit."
        } else {
            "Source builds are available in an installed nightly or dev build."
        };
        SettingRow::custom(
            "Build from source",
            description,
            div()
                .p(px(SETTINGS_CARD_PADDING))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .child(
                            div()
                                .text_size(text_px(TEXT_CONTROL))
                                .child("Build from source"),
                        )
                        .child(
                            div()
                                .mt(px(3.0))
                                .text_size(text_px(TEXT_LABEL))
                                .line_height(px(LINE_LABEL))
                                .text_color(rgb(colors.muted))
                                .child(description),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_w_0()
                                .h(px(CONTROL_HEIGHT))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded(px(RADIUS_CONTROL))
                                .border_1()
                                .border_color(self.field_frame(&self.source_build_input, cx))
                                .bg(rgb(colors.surface))
                                .text_size(text_px(TEXT_CONTROL))
                                .child(self.source_build_input.clone())
                                .child(keys.mark(
                                    KeyLayer::Overlay,
                                    KeyStop::Field(
                                        self.source_build_input.read(cx).focus_handle(cx),
                                    ),
                                )),
                        )
                        .child(
                            ui_button_when(
                                can_build,
                                "workspace-settings-source-build",
                                ButtonKind::Secondary,
                                colors,
                            )
                            .when(can_build, |button| {
                                button.key_stop(
                                    &keys,
                                    KeyLayer::Overlay,
                                    "workspace-settings-source-build",
                                    colors,
                                    cx,
                                    |this, _, cx| this.start_source_build(cx),
                                )
                            })
                            .child(if running {
                                "Building…"
                            } else {
                                "Build and install"
                            }),
                        )
                        .when(running, |actions| {
                            actions.child(
                                ui_button(
                                    "workspace-settings-source-stop",
                                    ButtonKind::Ghost,
                                    colors,
                                )
                                .key_stop(
                                    &keys,
                                    KeyLayer::Overlay,
                                    "workspace-settings-source-stop",
                                    colors,
                                    cx,
                                    |this, _, cx| {
                                        if let Some(run) = &this.source_build_run {
                                            run.cancel();
                                        }
                                        this.source_build_panel.stopping = true;
                                        cx.notify();
                                    },
                                )
                                .child(
                                    if self.source_build_panel.stopping {
                                        "Stopping…"
                                    } else {
                                        "Stop"
                                    },
                                ),
                            )
                        }),
                )
                .when_some(self.source_build_panel.message.clone(), |body, message| {
                    body.child(
                        div()
                            .text_size(text_px(TEXT_LABEL))
                            .line_height(px(LINE_LABEL))
                            .text_color(rgb(colors.muted))
                            .child(message),
                    )
                })
                .when(!self.source_build_panel.output.is_empty(), |body| {
                    body.child(
                        div()
                            .id("workspace-settings-build-output")
                            .max_h(px(180.0))
                            .overflow_y_scroll()
                            .thumbed("workspace-settings-build-output", colors)
                            .p_3()
                            .rounded(px(RADIUS_PANEL))
                            .bg(rgb(colors.surface))
                            .font_family(MONO)
                            .text_size(text_px(TEXT_LABEL))
                            .line_height(px(LINE_LABEL))
                            .child(
                                self.source_build_panel
                                    .output
                                    .iter()
                                    .cloned()
                                    .collect::<String>(),
                            ),
                    )
                }),
        )
    }

    /// Keyboard shortcuts as rows with the keys on the right.
    fn shortcut_rows(
        prefix: (&'static str, usize),
        shortcuts: &[(&'static str, String)],
        colors: ThemeColors,
    ) -> Vec<SettingRow> {
        shortcuts
            .iter()
            .enumerate()
            .map(|(index, (label, keys))| {
                let (label, keys) = (*label, SharedString::from(keys.clone()));
                // The keys are searchable, though they show as a chip rather
                // than a description.
                SettingRow::custom(
                    label,
                    keys.clone(),
                    settings_row(
                        gpui::ElementId::NamedInteger(
                            prefix.0.into(),
                            (prefix.1 * 100 + index) as u64,
                        ),
                        label,
                        "",
                        colors,
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(RADIUS_CONTROL))
                            .border_1()
                            .border_color(rgb(colors.border))
                            .font_family(MONO)
                            .text_size(text_px(TEXT_LABEL))
                            .text_color(rgb(colors.muted))
                            .child(keys),
                    ),
                )
            })
            .collect()
    }

    /// Everything one section holds, in groups.
    fn settings_groups(
        &mut self,
        section: usize,
        column: f32,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<SettingGroup> {
        let keys = self.key_targets.clone();
        match section {
            0 => vec![SettingGroup {
                label: None,
                rows: vec![
                    self.theme_row(column, colors, cx),
                    self.interface_size_row(colors, cx),
                    Self::local_switch(
                        "workspace-settings-reduce-motion",
                        "Reduce motion",
                        "Status marks hold steady instead of pulsing, and the usage refresh icon does not turn.",
                        self.settings.reduce_motion,
                        |settings| settings.reduce_motion = !settings.reduce_motion,
                        &keys,
                        colors,
                        cx,
                    ),
                ],
            }],
            1 => {
                let mut groups = Vec::new();
                let titles = self.chat_title_rows(colors, cx);
                if !titles.is_empty() {
                    groups.push(group("Titles and names", titles));
                }
                groups.push(group(
                    "While chats run",
                    vec![
                        Self::local_switch(
                            "workspace-settings-auto-resume",
                            "Resume chats after a restart",
                            "Continue turns a restart cut off. Agents start working without asking.",
                            self.settings.auto_resume_chats,
                            |settings| settings.auto_resume_chats = !settings.auto_resume_chats,
                            &keys,
                            colors,
                            cx,
                        ),
                        Self::local_switch(
                            "workspace-settings-notifications",
                            "Notify when a chat finishes",
                            "A chat that finishes in the background sends a notification.",
                            self.settings.notifications,
                            |settings| settings.notifications = !settings.notifications,
                            &keys,
                            colors,
                            cx,
                        ),
                    ],
                ));
                let quota = self.chat_quota_rows(colors, cx);
                if !quota.is_empty() {
                    groups.push(group("Usage", quota));
                }
                groups
            }
            AGENTS_SECTION => vec![
                group(
                    "Accounts",
                    vec![SettingRow::custom(
                        "Accounts",
                        format!(
                            "{} Codex Claude Code sign in",
                            super::accounts_view::ACCOUNTS_EXPLANATION
                        ),
                        div()
                            .p(px(SETTINGS_CARD_PADDING))
                            .child(self.render_account_settings(colors, cx)),
                    )],
                ),
                group(
                    "Permissions",
                    vec![setting(
                        "minimal-all-permissions",
                        "Skip permission prompts",
                        "New chats pass the agent's unrestricted flag: no approvals or sandbox where the CLI supports it.",
                        colors,
                        |row| {
                            key_switch(
                                row,
                                &keys,
                                "minimal-all-permissions",
                                self.settings.allow_all_permissions,
                                colors,
                                cx,
                                |this, _, cx| this.toggle_minimal_all_permissions(cx),
                            )
                        },
                    )],
                ),
            ],
            3 => {
                let mut groups = Vec::new();
                let settle = self.auto_settle_rows(colors, cx);
                if !settle.is_empty() {
                    groups.push(group("Settling", settle));
                }
                let delete = self.auto_delete_rows(colors, cx);
                if !delete.is_empty() {
                    groups.push(group("Deleting", delete));
                }
                groups
            }
            4 => self.device_groups(colors, cx),
            5 => keyboard_shortcuts()
                .into_iter()
                .enumerate()
                .map(|(index, (label, shortcuts))| {
                    group(
                        label,
                        Self::shortcut_rows(("workspace-settings-key", index), &shortcuts, colors),
                    )
                })
                .collect(),
            _ => vec![
                group("xd", vec![self.update_row(colors, cx)]),
                group("Assistants", self.assistant_version_rows(colors)),
                group("Development", vec![self.source_build_row(colors, cx)]),
            ],
        }
    }

    /// This device's connection, pairing, and remote hosts.
    fn device_groups(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Vec<SettingGroup> {
        let keys = self.key_targets.clone();
        let local = self.active_endpoint == ChatEndpoint::Local;
        let mut this_device = vec![setting(
            "workspace-settings-connection",
            "This device",
            "Sessions keep running on the host when the window closes.",
            colors,
            |row| {
                row.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(text_px(TEXT_LABEL))
                        .text_color(rgb(if self.model.connected {
                            colors.working
                        } else {
                            colors.muted
                        }))
                        .child(div().size(px(6.0)).rounded_full().bg(rgb(
                            if self.model.connected {
                                colors.working
                            } else {
                                colors.muted
                            },
                        )))
                        .child(if local { "Local" } else { "Remote" }),
                )
            },
        )];
        if local {
            this_device.push(setting(
                "workspace-settings-pair",
                "Pair another device",
                "Connect another desktop or the Android app to this host.",
                colors,
                |row| {
                    row.child(
                        ui_button("pair-device", ButtonKind::Secondary, colors)
                            .key_stop(
                                &keys,
                                KeyLayer::Overlay,
                                "pair-device",
                                colors,
                                cx,
                                |this, _, cx| {
                                    this.leave_settings(cx);
                                    this.share_panel = Some(SharePanel {
                                        loading: true,
                                        ..Default::default()
                                    });
                                    if let Some(host) = &this.host {
                                        let _ = host.peer_pairing();
                                    }
                                    cx.notify();
                                },
                            )
                            .child("Pair a device"),
                    )
                },
            ));
            this_device.push(setting(
                "workspace-settings-devices",
                "Paired devices",
                "The devices that can connect to this host.",
                colors,
                |row| {
                    row.child(
                        ui_button("paired-devices", ButtonKind::Secondary, colors)
                            .key_stop(
                                &keys,
                                KeyLayer::Overlay,
                                "paired-devices",
                                colors,
                                cx,
                                |this, _, cx| {
                                    this.leave_settings(cx);
                                    this.devices_panel = Some(DevicesPanel {
                                        loading: true,
                                        ..Default::default()
                                    });
                                    this.request_devices();
                                    cx.notify();
                                },
                            )
                            .child("Manage"),
                    )
                },
            ));
        }
        let mut remote = vec![setting(
            "workspace-settings-remote",
            "Connect to another host",
            "Pair with a host using the code it shows.",
            colors,
            |row| {
                row.child(
                    ui_button("workspace-settings-connect", ButtonKind::Secondary, colors)
                        .key_stop(
                            &keys,
                            KeyLayer::Overlay,
                            "workspace-settings-connect",
                            colors,
                            cx,
                            |this, window, cx| {
                                this.leave_settings(cx);
                                this.open_remote(window, cx);
                            },
                        )
                        .child("Connect"),
                )
            },
        )];
        if !local {
            remote.push(setting(
                "workspace-settings-disconnect",
                "Return to this device",
                "Keep remote sessions running and switch to the local host.",
                colors,
                |row| {
                    row.child(
                        ui_button(
                            "workspace-settings-disconnect-remote",
                            ButtonKind::Secondary,
                            colors,
                        )
                        .key_stop(
                            &keys,
                            KeyLayer::Overlay,
                            "workspace-settings-disconnect-remote",
                            colors,
                            cx,
                            |this, _, cx| {
                                this.leave_settings(cx);
                                this.disconnect_remote_runtime(cx);
                            },
                        )
                        .child("Disconnect"),
                    )
                },
            ));
        }
        vec![
            group("This device", this_device),
            group("Other hosts", remote),
        ]
    }

    /// The settings that match every word of a search, by section, in the
    /// navigation's order. Sections without a match are left out.
    pub(super) fn settings_search_results(
        &mut self,
        words: &[String],
        column: f32,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<(usize, Vec<SettingRow>)> {
        (0..SETTINGS_SECTIONS.len())
            .filter_map(|index| {
                let name = SETTINGS_SECTIONS[index].name;
                let rows = self
                    .settings_groups(index, column, colors, cx)
                    .into_iter()
                    .flat_map(|group| group.rows)
                    .filter(|row| setting_matches(words, name, &row.title, &row.description))
                    .collect::<Vec<_>>();
                (!rows.is_empty()).then_some((index, rows))
            })
            .collect()
    }

    /// Up and Down in Settings: the section after the open one, or before
    /// it, going round. A search, a dialog over the page, Usage, or a text
    /// field of the open section keeps the keys. Says whether the section
    /// changed.
    pub(super) fn step_settings_section(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        if !self.minimal_theme_open
            || self.usage_open
            || self.dialog_open()
            || !self.settings_query.trim().is_empty()
        {
            return false;
        }
        // From the page itself, a control Tab is on, or the search field.
        let search = self.settings_search_input.read(cx).focus_handle(cx);
        let at_home = [&self.minimal_popup_focus, &self.stop_focus, &search]
            .into_iter()
            .any(|focus| self.frame_focus.as_ref() == Some(focus));
        if !at_home {
            return false;
        }
        let next = stepped_section(self.settings_section, SETTINGS_SECTIONS.len(), down);
        self.open_settings_section(next, cx);
        // The ring on the section list goes along to the section opened,
        // which is a Tab stop only from the next frame on.
        if self
            .key_stop
            .as_ref()
            .is_some_and(|id| id.starts_with(SECTION_STOP))
        {
            self.key_stop = Some(format!("{SECTION_STOP}{next}").into());
            self.key_stop_scrolling = true;
        }
        true
    }

    /// Opens a section, leaving any search.
    fn open_settings_section(&mut self, index: usize, cx: &mut Context<Self>) {
        self.settings_menu = None;
        self.settings_section = index;
        if index == AGENTS_SECTION {
            self.request_agent_accounts();
        }
        self.clear_settings_search(cx);
        cx.notify();
    }

    /// Leaves Settings for something it hands over to, such as pairing, with
    /// no search left behind for the next visit.
    pub(super) fn leave_settings(&mut self, cx: &mut Context<Self>) {
        self.minimal_theme_open = false;
        self.clear_settings_search(cx);
    }

    /// Empties the settings search field.
    pub(super) fn clear_settings_search(&mut self, cx: &mut Context<Self>) {
        self.settings_query.clear();
        self.settings_search_input
            .update(cx, |input, cx| input.set_text("", cx));
    }

    /// The settings page: the sections and search on the left, the open
    /// section, or what the search found, on the right. It covers the
    /// workspace below the title bar.
    pub(super) fn render_workspace_settings(
        &mut self,
        colors: ThemeColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if !self.minimal_theme_open {
            self.settings_menu = None;
        }
        // Each setting's row looks for a failure of its own as it is drawn.
        super::SETTING_FAILURE.with(|failure| failure.replace(self.settings_failure.clone()));
        let window_width = logical(window.viewport_size().width);
        let narrow = window_width < SETTINGS_NARROW_WINDOW;
        let column = settings_column_width(window_width);
        let section = self.settings_section.min(SETTINGS_SECTIONS.len() - 1);
        let minimal_popup_focus = self.minimal_popup_focus.clone();
        let words = settings_query_words(&self.settings_query);
        let searching = !words.is_empty();

        // While searching, every section is gathered and filtered, so the
        // navigation can say where the matches are.
        let found = if searching {
            self.settings_search_results(&words, column, colors, cx)
        } else {
            Vec::new()
        };

        let navigation = div()
            .id("workspace-settings-navigation")
            .w(px(if narrow {
                SETTINGS_NAV_NARROW_WIDTH
            } else {
                SETTINGS_NAV_WIDTH
            }))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .px(px(12.0))
            .py(px(12.0))
            .bg(crate::glass::pane(colors))
            .border_r_1()
            .border_color(crate::glass::edge(colors))
            .child(
                ui_button("workspace-settings-close", ButtonKind::Ghost, colors)
                    .debug_selector(|| "workspace-settings-back".into())
                    .w(px(84.0))
                    .px(px(6.0))
                    .gap(px(4.0))
                    .key_stop(
                        &self.key_targets,
                        KeyLayer::Overlay,
                        "workspace-settings-close",
                        colors,
                        cx,
                        |this, window, cx| this.close_minimal_theme_popup(window, cx),
                    )
                    .child(icon(CHEVRON_LEFT_ICON, 14.0, colors.muted))
                    .child(
                        div()
                            .text_size(text_px(TEXT_CONTROL))
                            .text_color(rgb(colors.muted))
                            .child("Back"),
                    ),
            )
            .child(
                div()
                    .mt(px(14.0))
                    .px(px(6.0))
                    .text_size(text_px(TEXT_TITLE))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(colors.text))
                    .child("Settings"),
            )
            .child(
                div()
                    .relative()
                    .mt(px(14.0))
                    .h(px(CONTROL_HEIGHT))
                    .px(px(9.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(RADIUS_CONTROL))
                    .child(self.key_targets.mark(
                        KeyLayer::Overlay,
                        KeyStop::Field(self.settings_search_input.read(cx).focus_handle(cx)),
                    ))
                    .border_1()
                    .border_color(
                        if self.frame_focus.as_ref()
                            == Some(&self.settings_search_input.read(cx).focus_handle(cx))
                        {
                            rgb(colors.selected_border)
                        } else {
                            crate::glass::edge(colors)
                        },
                    )
                    .bg(rgb(colors.surface))
                    .text_size(text_px(TEXT_CONTROL))
                    .child(icon(SEARCH_ICON, 13.0, colors.faint))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.settings_search_input.clone()),
                    )
                    .when(searching, |field| {
                        field.child(
                            icon_button(
                                "workspace-settings-search-clear",
                                "Clear the search",
                                CLOSE_ICON,
                                colors,
                            )
                            .size(24.0)
                            .build()
                            .mr(px(-5.0))
                            .key_stop_named(
                                &self.key_targets,
                                KeyLayer::Overlay,
                                "workspace-settings-search-clear",
                                "Clear the search",
                                colors,
                                cx,
                                |this, window, cx| {
                                    this.leave_key_stop(window);
                                    this.clear_settings_search(cx);
                                    cx.notify();
                                },
                            ),
                        )
                    }),
            )
            .child(div().mt(px(12.0)).flex().flex_col().gap(px(2.0)).children(
                SETTINGS_SECTIONS.iter().enumerate().map(|(index, entry)| {
                    let matches = found
                        .iter()
                        .find(|(found, _)| *found == index)
                        .map(|(_, rows)| rows.len());
                    let selected = !searching && section == index;
                    let quiet = searching && matches.is_none();
                    div()
                        .id(("workspace-settings-section", index))
                        .h(px(SETTINGS_NAV_ROW_HEIGHT))
                        .px(px(8.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .rounded(px(RADIUS_TAB))
                        .text_size(text_px(TEXT_CONTROL))
                        .font_weight(if selected {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(rgb(if selected {
                            colors.text
                        } else if quiet {
                            colors.faint
                        } else {
                            colors.muted
                        }))
                        .when(selected, |item| {
                            item.bg(rgb(colors.selected_surface))
                                .child(super::selection_marker(super::MarkerEdge::Leading, colors))
                        })
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(colors.surface_high)))
                        // Tab reaches the section open, and Up and Down move
                        // from it to the others.
                        .key_stop_roving(
                            selected,
                            &self.key_targets,
                            KeyLayer::Overlay,
                            format!("{SECTION_STOP}{index}"),
                            SECTION_RING_RADIUS,
                            colors,
                            cx,
                            move |this, _, cx| this.open_settings_section(index, cx),
                        )
                        .child(icon(
                            entry.icon,
                            15.0,
                            if selected { colors.text } else { colors.muted },
                        ))
                        .child(truncating_label(entry.name).flex_1().min_w_0())
                        .when_some(matches, |item, count| {
                            item.child(
                                div()
                                    .flex_none()
                                    .text_size(text_px(TEXT_CAPTION))
                                    .text_color(rgb(colors.faint))
                                    .child(count.to_string()),
                            )
                        })
                }),
            ));

        let mut page = div()
            .w(px(column))
            .mx_auto()
            .pt(px(36.0))
            .pb(px(56.0))
            .flex()
            .flex_col()
            .text_color(rgb(colors.text));
        if searching {
            page = page.child(
                div()
                    .text_size(text_px(TEXT_DISPLAY))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Search"),
            );
            if found.is_empty() {
                page = page.child(
                    div()
                        .mt(px(10.0))
                        .text_size(text_px(TEXT_CONTROL))
                        .text_color(rgb(colors.muted))
                        .child(format!(
                            "No settings match \u{201c}{}\u{201d}.",
                            self.settings_query.trim()
                        )),
                );
            }
            for (index, rows) in found {
                let entry = &SETTINGS_SECTIONS[index];
                page = page
                    .child(
                        div()
                            .id(("workspace-settings-result-section", index))
                            .mt(px(28.0))
                            .mb(px(8.0))
                            .px(px(2.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_size(text_px(TEXT_CONTROL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(colors.muted))
                            .cursor_pointer()
                            .hover(|style| style.text_color(rgb(colors.text)))
                            .key_stop(
                                &self.key_targets,
                                KeyLayer::Overlay,
                                format!("workspace-settings-result-section-{index}"),
                                colors,
                                cx,
                                move |this, window, cx| {
                                    this.leave_key_stop(window);
                                    this.open_settings_section(index, cx)
                                },
                            )
                            .child(icon(entry.icon, 14.0, colors.muted))
                            .child(entry.name),
                    )
                    .child(settings_card(rows, colors));
            }
        } else {
            let entry = &SETTINGS_SECTIONS[section];
            page = page
                .child(
                    div()
                        .text_size(text_px(TEXT_DISPLAY))
                        .font_weight(FontWeight::MEDIUM)
                        .child(entry.name),
                )
                .child(
                    div()
                        .mt(px(6.0))
                        .text_size(text_px(TEXT_CONTROL))
                        .line_height(px(LINE_CONTROL))
                        .text_color(rgb(colors.muted))
                        .child(entry.summary),
                );
            let groups = self.settings_groups(section, column, colors, cx);
            if groups.iter().all(|group| group.rows.is_empty()) {
                page = page.child(
                    div()
                        .mt(px(28.0))
                        .text_size(text_px(TEXT_CONTROL))
                        .text_color(rgb(colors.muted))
                        .child("This host does not offer these settings."),
                );
            }
            for group in groups.into_iter().filter(|group| !group.rows.is_empty()) {
                page = match group.label {
                    Some(label) => page.child(settings_group_label(label, colors)),
                    None => page.child(div().h(px(28.0))),
                }
                .child(settings_card(group.rows, colors));
            }
        }

        // Each section, and a search, starts at its top.
        let content_key = if searching { usize::MAX } else { section };
        if self.settings_scroll_key != Some(content_key) {
            self.settings_scroll_key = Some(content_key);
            self.settings_scroll.set_offset(point(px(0.0), px(0.0)));
        }
        let content = div()
            .id(("workspace-settings-content", content_key))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .overflow_y_scroll()
            .scroll_thumb(&self.settings_scroll, colors)
            .child(page);

        div()
            .id("workbench-settings")
            .occlude()
            .absolute()
            .top(px(SETTINGS_TITLEBAR_HEIGHT))
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .bg(crate::glass::window(colors))
            .track_focus(&minimal_popup_focus)
            // The page is the whole window below the title bar: nothing behind
            // it takes a click. A press outside an open dropdown closes it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    if this.settings_menu.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(navigation)
            .child(content)
            .into_any_element()
    }
}

/// The section after `at`, or before it, going round at either end.
fn stepped_section(at: usize, count: usize, down: bool) -> usize {
    let at = at.min(count - 1);
    if down {
        (at + 1) % count
    } else {
        (at + count - 1) % count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_sections_step_round_at_both_ends() {
        let count = SETTINGS_SECTIONS.len();
        assert_eq!(stepped_section(0, count, true), 1);
        assert_eq!(stepped_section(count - 1, count, true), 0);
        assert_eq!(stepped_section(0, count, false), count - 1);
        assert_eq!(stepped_section(3, count, false), 2);
        // A stale index past the end steps from the last section.
        assert_eq!(stepped_section(count + 4, count, true), 0);
    }

    #[test]
    fn checking_for_an_update_shows_what_was_found_even_where_installing_is_manual() {
        let status = |latest: Option<&str>, available: bool| SelfUpdateStatus {
            version: "0.2.17".into(),
            latest: latest.map(str::to_owned),
            available,
            supported: false,
            ..Default::default()
        };
        assert_eq!(
            manual_update_status(&status(None, false), true),
            "This installation requires a manual update. Check now to see if there is one."
        );
        assert_eq!(
            manual_update_status(&status(Some("0.2.17"), false), true),
            "xd 0.2.17. Up to date."
        );
        assert_eq!(
            manual_update_status(&status(Some("0.2.18"), true), true),
            "Update available: 0.2.18. Download the installer and run it to update."
        );
        // A source build has no installer to offer.
        assert_eq!(
            manual_update_status(&status(Some("0.2.18"), true), false),
            "Update available: 0.2.18. This installation requires a manual update."
        );
    }

    #[test]
    fn each_channel_downloads_its_own_installer() {
        assert!(windows_installer_url(false).ends_with("/latest/download/xd-windows-x86_64.msi"));
        assert!(
            windows_installer_url(true)
                .ends_with("/download/nightly/xd-nightly-windows-x86_64.msi")
        );
    }

    #[test]
    fn theme_cards_sit_side_by_side_unless_their_names_would_clip() {
        // The widest column, the column at the window's minimum width, and a
        // narrow one.
        assert_eq!(theme_columns(SETTINGS_COLUMN_WIDTH - 28.0), 2);
        assert_eq!(theme_columns(settings_column_width(760.0) - 28.0), 2);
        assert_eq!(theme_columns(240.0), 1);
        for room in [240.0, 300.0, 412.0, 560.0, 652.0] {
            let columns = theme_columns(room);
            let width = theme_card_width(room, columns);
            assert!(width >= THEME_CARD_MIN_WIDTH, "{room}: {width}");
            // The row fits in the room, so it never wraps early.
            let row = width * columns as f32 + THEME_CARD_GAP * (columns - 1) as f32;
            assert!(row <= room, "{room}: {row}");
        }
    }

    #[test]
    fn the_reading_column_fits_beside_the_navigation_at_every_window_width() {
        assert_eq!(settings_column_width(1600.0), SETTINGS_COLUMN_WIDTH);
        // At the minimum window width the navigation narrows to leave room.
        let minimum = settings_column_width(760.0);
        assert_eq!(minimum, 760.0 - SETTINGS_NAV_NARROW_WIDTH - 80.0);
        assert!(minimum >= 2.0 * THEME_CARD_MIN_WIDTH + THEME_CARD_GAP + 28.0);
    }

    #[test]
    fn search_matches_every_word_in_any_order_and_case() {
        let words = settings_query_words("  Settle   MERGED ");
        assert_eq!(words, ["settle", "merged"]);
        assert!(setting_matches(
            &words,
            "Cleanup",
            "Settle merged sessions",
            ""
        ));
        assert!(setting_matches(
            &settings_query_words("merged settle"),
            "Cleanup",
            "Settle merged sessions",
            ""
        ));
        // The section's name and the description count too.
        assert!(setting_matches(
            &settings_query_words("cleanup worktree"),
            "Cleanup",
            "Delete inactive sessions",
            "Delete sessions with the worktree xd made for them."
        ));
        assert!(!setting_matches(
            &settings_query_words("settle theme"),
            "Cleanup",
            "Settle merged sessions",
            ""
        ));
        assert!(settings_query_words("   ").is_empty());
    }

    #[test]
    fn every_section_has_a_name_summary_and_bundled_icon() {
        for entry in &SETTINGS_SECTIONS {
            assert!(!entry.name.is_empty() && entry.summary.ends_with('.'));
            assert!(entry.icon.starts_with("icons/"));
        }
        assert_eq!(SETTINGS_SECTIONS[AGENTS_SECTION].name, "Agents");
        for path in [MESSAGE_ICON, BOT_ICON, KEYBOARD_ICON] {
            assert!(settings_icon(path).is_some_and(|svg| svg.starts_with(b"<?xml")));
        }
    }
}
