use super::*;

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
/// The title bar stays visible above the settings page.
const SETTINGS_TITLEBAR_HEIGHT: f32 = 38.0;

const MESSAGE_ICON: &str = "icons/message-square.svg";
const BOT_ICON: &str = "icons/bot.svg";
const KEYBOARD_ICON: &str = "icons/keyboard.svg";

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
        summary: "Shortcuts for chats and the terminal.",
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

/// The row a setting draws: its title and description on the left, with the
/// caller adding the control on the right.
fn settings_row(
    id: impl Into<gpui::ElementId>,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    let description = description.into();
    div()
        .id(id)
        .w_full()
        .min_h(px(52.0))
        .px(px(SETTINGS_CARD_PADDING))
        .py(px(11.0))
        .flex()
        .items_center()
        .gap_4()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(colors.text))
                        .child(title.into()),
                )
                .when(!description.is_empty(), |body| {
                    body.child(
                        div()
                            .mt(px(3.0))
                            .text_size(px(12.0))
                            .line_height(px(17.0))
                            .text_color(rgb(colors.muted))
                            .child(description),
                    )
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
        .rounded(px(10.0))
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
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(colors.muted))
        .child(label.into())
}

/// A minus or plus button beside a number, dimmed at a limit.
fn settings_step_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    ui_button(id, ButtonKind::Secondary, colors)
        .w(px(30.0))
        .px_0()
        .when(!enabled, |button| button.opacity(0.4))
        .child(label)
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
                .text_size(px(13.0))
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

    pub(super) fn toggle_chat_title_model_menu(
        &mut self,
        backend: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.chat_title_model_menu =
            (self.chat_title_model_menu != Some(backend)).then_some(backend);
        cx.notify();
    }

    fn choose_chat_title_model(&mut self, backend: &str, model: String, cx: &mut Context<Self>) {
        self.chat_title_model_menu = None;
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
        let Some(settings) = self.chat_title_settings.clone() else {
            return Vec::new();
        };
        let mut rows = vec![setting(
            "workspace-settings-chat-titles",
            "Generate chat titles",
            "Name a new chat from its first message, using its naming model.",
            colors,
            |row| {
                row.cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_chat_titles(cx)))
                    .child(settings_switch(colors, settings.enabled))
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
            let label = selected.as_deref().map_or_else(
                || "Choose a model".to_owned(),
                |selected| {
                    let name = catalog
                        .and_then(|catalog| {
                            catalog.models.iter().find(|model| model.id == selected)
                        })
                        .map(|model| model.name.as_str());
                    presentation::model_name(selected, name)
                },
            );
            let open = self.chat_title_model_menu == Some(backend);
            let models = catalog.map_or(&[][..], |catalog| catalog.models.as_slice());
            let menu = open.then(|| {
                div()
                    .px(px(8.0))
                    .pb(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .children(models.iter().enumerate().map(|(index, model)| {
                        let id = model.id.clone();
                        div()
                            .id((row, index + 1))
                            .h(px(32.0))
                            .px(px(10.0))
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .text_size(px(13.0))
                            .text_color(rgb(colors.text))
                            .when(selected.as_deref() == Some(model.id.as_str()), |choice| {
                                choice.bg(rgb(colors.selected_surface))
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.choose_chat_title_model(backend, id.clone(), cx)
                            }))
                            .child(presentation::model_name(&model.id, Some(&model.name)))
                    }))
            });
            rows.push(SettingRow::custom(
                title,
                description,
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(
                        settings_row(row, title, description, colors).child(
                            ui_button((row, 0_usize), ButtonKind::Secondary, colors)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_chat_title_model_menu(backend, cx)
                                }))
                                .child(label)
                                .child(icon(CHEVRON_DOWN_ICON, 12.0, colors.muted)),
                        ),
                    )
                    .children(menu),
            ));
        }
        rows
    }

    /// The connected host's settling rules, or nothing while it has not said
    /// what they are: an older host does not have any.
    fn auto_settle_rows(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Vec<SettingRow> {
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
                    row.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_auto_settle(
                                AutoSettleRules {
                                    on_merge: !rules.on_merge,
                                    ..rules
                                },
                                cx,
                            );
                        }))
                        .child(settings_switch(colors, rules.on_merge))
                },
            ),
            setting(
                "workspace-settings-settle-inactive",
                "Settle inactive sessions",
                "Sessions with no activity for a while settle on their own. Any new activity un-settles them.",
                colors,
                |row| {
                    row.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
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
                        }))
                        .child(settings_switch(colors, rules.after_days.is_some()))
                },
            ),
        ];
        let Some(days) = rules.after_days else {
            return rows;
        };
        let step = |id: &'static str, label: &'static str, to: i64, cx: &mut Context<Self>| {
            let allowed =
                (AutoSettleRules::MIN_AFTER_DAYS..=AutoSettleRules::MAX_AFTER_DAYS).contains(&to);
            settings_step_button(id, label, allowed, colors).when(allowed, |button| {
                button.on_click(cx.listener(move |this, _, _, cx| {
                    this.change_auto_settle(
                        AutoSettleRules {
                            after_days: Some(to),
                            ..rules
                        },
                        cx,
                    );
                }))
            })
        };
        let fewer = step("settle-days-fewer", "\u{2212}", days - 1, cx);
        let more = step("settle-days-more", "+", days + 1, cx);
        rows.push(setting(
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
        ));
        rows.push(setting(
            "workspace-settings-settle-keep-pr",
            "Keep sessions with a pull request",
            "Inactivity does not settle sessions with a pull request. Merge and close still do.",
            colors,
            |row| {
                row.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change_auto_settle(
                            AutoSettleRules {
                                keep_pull_requests: !rules.keep_pull_requests,
                                ..rules
                            },
                            cx,
                        );
                    }))
                    .child(settings_switch(colors, rules.keep_pull_requests))
            },
        ));
        rows
    }

    /// The connected host's deletion rule, or nothing while it has not said
    /// what it is: an older host does not have one.
    fn auto_delete_rows(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Vec<SettingRow> {
        let Some(rules) = self.auto_delete else {
            return Vec::new();
        };
        let mut rows = vec![setting(
            "workspace-settings-delete-inactive",
            "Delete inactive sessions",
            "Delete sessions with no activity for a while, with the worktree xd made for them. Branches and their commits are kept.",
            colors,
            |row| {
                row.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change_auto_delete(
                            AutoDeleteRules {
                                after_days: match rules.after_days {
                                    Some(_) => None,
                                    None => Some(AutoDeleteRules::DEFAULT_AFTER_DAYS),
                                },
                            },
                            cx,
                        );
                    }))
                    .child(settings_switch(colors, rules.after_days.is_some()))
            },
        )];
        let Some(days) = rules.after_days else {
            return rows;
        };
        let step = |id: &'static str, label: &'static str, longer: bool, cx: &mut Context<Self>| {
            let to = AutoDeleteRules::step(days, longer);
            settings_step_button(id, label, to.is_some(), colors).when_some(to, |button, to| {
                button.on_click(cx.listener(move |this, _, _, cx| {
                    this.change_auto_delete(
                        AutoDeleteRules {
                            after_days: Some(to),
                        },
                        cx,
                    );
                }))
            })
        };
        let fewer = step("delete-days-fewer", "\u{2212}", false, cx);
        let more = step("delete-days-more", "+", true, cx);
        rows.push(setting(
            "workspace-settings-delete-days",
            "Days before deleting",
            "A worktree with uncommitted changes keeps its session until you commit or clean it up.",
            colors,
            |row| row.child(settings_stepper(fewer, days.to_string(), more, 28.0, colors)),
        ));
        rows
    }

    /// The connected host's default chat quota, on a host that has quotas
    /// and once it has said what the quota is.
    pub(super) fn chat_quota_rows(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<SettingRow> {
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
                row.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change_chat_quota_settings(
                            ChatQuotaSettings {
                                percent: match settings.percent {
                                    Some(_) => None,
                                    None => Some(ChatQuotaSettings::DEFAULT_PERCENT),
                                },
                            },
                            cx,
                        );
                    }))
                    .child(settings_switch(colors, settings.percent.is_some()))
            },
        )];
        let Some(percent) = settings.percent else {
            return rows;
        };
        let step = |id: &'static str, label: &'static str, larger: bool, cx: &mut Context<Self>| {
            let to = ChatQuotaSettings::step(percent, larger);
            settings_step_button(id, label, to.is_some(), colors).when_some(to, |button, to| {
                button.on_click(cx.listener(move |this, _, _, cx| {
                    this.change_chat_quota_settings(ChatQuotaSettings { percent: Some(to) }, cx);
                }))
            })
        };
        let fewer = step("chat-quota-smaller", "\u{2212}", false, cx);
        let more = step("chat-quota-larger", "+", true, cx);
        rows.push(setting(
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
        ));
        rows
    }

    /// A switch row for one of this device's own settings, saved on change.
    fn local_switch(
        id: &'static str,
        title: &'static str,
        description: &'static str,
        enabled: bool,
        toggle: fn(&mut AppSettings),
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> SettingRow {
        setting(id, title, description, colors, |row| {
            row.cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    toggle(&mut this.settings);
                    if let Err(error) = this.settings.save() {
                        this.model.connection_error = Some(error);
                    }
                    cx.notify();
                }))
                .child(settings_switch(colors, enabled))
        })
    }

    /// The theme cards, as one setting.
    fn theme_row(&self, column: f32, colors: ThemeColors, cx: &mut Context<Self>) -> SettingRow {
        let room = column - 2.0 * SETTINGS_CARD_PADDING;
        let card_width = theme_card_width(room, theme_columns(room));
        let cards = ThemePreset::ALL
            .into_iter()
            .enumerate()
            .map(|(index, theme)| {
                let swatch = theme.colors();
                let selected = self.settings.theme == theme;
                div()
                    .id(("minimal-theme-choice", index))
                    .flex_none()
                    .w(px(card_width))
                    .rounded(px(10.0))
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.choose_minimal_theme(theme, cx);
                        this.minimal_theme_open = true;
                        cx.notify();
                    }))
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
                                    .rounded(px(5.0))
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
                                            .rounded(px(7.0))
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
                            .text_size(px(13.0))
                            .text_color(rgb(colors.text))
                            .child(theme.label())
                            .when(selected, |label| {
                                label.child(icon(CHECK_ICON, 14.0, colors.accent_ink))
                            }),
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
                    div().child(div().text_size(px(13.0)).child("Theme")).child(
                        div()
                            .mt(px(3.0))
                            .text_size(px(12.0))
                            .text_color(rgb(colors.muted))
                            .child("Dark or Light. Both are frosted glass."),
                    ),
                )
                // Side by side when there is room, else one to a row, so the
                // cards never clip their names.
                .child(div().flex().flex_wrap().gap_4().children(cards)),
        )
    }

    /// The update status of xd itself, with what can be done about it.
    fn update_row(&self, colors: ThemeColors, cx: &mut Context<Self>) -> SettingRow {
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
                        "This installation requires a manual update.".into()
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
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.request_app_update("check", cx)
                                    }))
                                    .child("Check now"),
                            )
                        })
                        .when(!busy && !installed && available, |actions| {
                            actions.child(
                                ui_button("install-app-update", ButtonKind::Primary, colors)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.install_app_update(cx)),
                                    )
                                    .child("Install"),
                            )
                        })
                        .when(!busy && installed, |actions| {
                            actions.child(
                                ui_button("restart-app-update", ButtonKind::Primary, colors)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.flush_draft_before_navigation() {
                                            this.request_app_update("restart", cx);
                                        }
                                    }))
                                    .child("Restart to update"),
                            )
                        }),
                )
            },
        )
    }

    /// Building and installing a branch, pull request or commit.
    fn source_build_row(&self, colors: ThemeColors, cx: &mut Context<Self>) -> SettingRow {
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
                        .child(div().text_size(px(13.0)).child("Build from source"))
                        .child(
                            div()
                                .mt(px(3.0))
                                .text_size(px(12.0))
                                .line_height(px(17.0))
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
                                .flex_1()
                                .min_w_0()
                                .h(px(32.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded(px(8.0))
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.surface))
                                .text_size(px(12.5))
                                .child(self.source_build_input.clone()),
                        )
                        .child(
                            ui_button(
                                "workspace-settings-source-build",
                                ButtonKind::Secondary,
                                colors,
                            )
                            .when(!can_build, |button| button.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_build {
                                    this.start_source_build(cx);
                                }
                            }))
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
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(run) = &this.source_build_run {
                                        run.cancel();
                                    }
                                    this.source_build_panel.stopping = true;
                                    cx.notify();
                                }))
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
                            .text_size(px(12.0))
                            .line_height(px(17.0))
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
                            .p_3()
                            .rounded(px(8.0))
                            .bg(rgb(colors.surface))
                            .font_family(MONO)
                            .text_size(px(11.5))
                            .line_height(px(17.0))
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
        prefix: &'static str,
        shortcuts: &[(&'static str, &'static str)],
        colors: ThemeColors,
    ) -> Vec<SettingRow> {
        shortcuts
            .iter()
            .enumerate()
            .map(|(index, &(label, keys))| {
                // The keys are searchable, though they show as a chip rather
                // than a description.
                SettingRow::custom(
                    label,
                    keys,
                    settings_row((prefix, index), label, "", colors).child(
                        div()
                            .flex_none()
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .border_1()
                            .border_color(rgb(colors.border))
                            .font_family(MONO)
                            .text_size(px(11.5))
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
        match section {
            0 => vec![SettingGroup {
                label: None,
                rows: vec![self.theme_row(column, colors, cx)],
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
                            colors,
                            cx,
                        ),
                        Self::local_switch(
                            "workspace-settings-notifications",
                            "Notify when a chat finishes",
                            "A chat that finishes in the background sends a notification.",
                            self.settings.notifications,
                            |settings| settings.notifications = !settings.notifications,
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
                        "Sign-ins for each assistant. New chats start on the first one with room left.",
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
                            row.cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_minimal_all_permissions(cx)
                                }))
                                .child(settings_switch(colors, self.settings.allow_all_permissions))
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
            5 => {
                let copy = if cfg!(target_os = "macos") {
                    "Cmd C / Cmd V"
                } else {
                    "Ctrl C / Ctrl V"
                };
                let redirect = if cfg!(target_os = "macos") {
                    "Cmd Enter"
                } else {
                    "Ctrl Enter"
                };
                let paste = if cfg!(target_os = "macos") {
                    "Cmd V"
                } else {
                    "Ctrl Shift V"
                };
                vec![
                    group(
                        "Chats",
                        Self::shortcut_rows(
                            "workspace-settings-chat-key",
                            &[
                                ("Send, or queue while the agent works", "Enter"),
                                ("Redirect the agent while it works", redirect),
                                ("New line", "Shift Enter"),
                                ("Copy / paste", copy),
                            ],
                            colors,
                        ),
                    ),
                    group(
                        "Terminal",
                        Self::shortcut_rows(
                            "workspace-settings-terminal-key",
                            &[
                                ("Interrupt command", "Ctrl C"),
                                ("Copy selection", "Ctrl Shift C"),
                                ("Paste", paste),
                                ("Clear terminal", "Ctrl L"),
                                ("Search command history", "Ctrl R"),
                                ("Close dialog", "Escape"),
                            ],
                            colors,
                        ),
                    ),
                ]
            }
            _ => vec![
                group("xd", vec![self.update_row(colors, cx)]),
                group("Development", vec![self.source_build_row(colors, cx)]),
            ],
        }
    }

    /// This device's connection, pairing, and remote hosts.
    fn device_groups(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Vec<SettingGroup> {
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
                        .text_size(px(12.0))
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
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.minimal_theme_open = false;
                                this.share_panel = Some(SharePanel {
                                    loading: true,
                                    ..Default::default()
                                });
                                if let Some(host) = &this.host {
                                    let _ = host.peer_pairing();
                                }
                                cx.notify();
                            }))
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
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.minimal_theme_open = false;
                                this.devices_panel = Some(DevicesPanel {
                                    loading: true,
                                    ..Default::default()
                                });
                                this.request_devices();
                                cx.notify();
                            }))
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
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.minimal_theme_open = false;
                            this.open_remote(window, cx);
                        }))
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
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.minimal_theme_open = false;
                            this.disconnect_remote_runtime(cx);
                        }))
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

    /// Opens a section, leaving any search.
    fn open_settings_section(&mut self, index: usize, cx: &mut Context<Self>) {
        self.settings_section = index;
        if index == AGENTS_SECTION {
            self.request_agent_accounts();
        }
        self.clear_settings_search(cx);
        cx.notify();
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
        let window_width = f32::from(window.viewport_size().width);
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

        let navigation =
            div()
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
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_minimal_theme_popup(window, cx)
                        }))
                        .child(icon(CHEVRON_LEFT_ICON, 14.0, colors.muted))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(rgb(colors.muted))
                                .child("Back"),
                        ),
                )
                .child(
                    div()
                        .mt(px(14.0))
                        .px(px(6.0))
                        .text_size(px(20.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(colors.text))
                        .child("Settings"),
                )
                .child(
                    div()
                        .mt(px(14.0))
                        .h(px(32.0))
                        .px(px(9.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(crate::glass::edge(colors))
                        .bg(rgb(colors.surface))
                        .text_size(px(12.5))
                        .child(icon(SEARCH_ICON, 13.0, colors.faint))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(self.settings_search_input.clone()),
                        )
                        .when(searching, |field| {
                            field.child(
                                div()
                                    .id("workspace-settings-search-clear")
                                    .flex_none()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.clear_settings_search(cx);
                                        cx.notify();
                                    }))
                                    .child(icon(CLOSE_ICON, 12.0, colors.faint)),
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
                            .h(px(34.0))
                            .px(px(8.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .rounded(px(7.0))
                            .text_size(px(13.0))
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
                            .when(selected, |item| item.bg(rgb(colors.selected_surface)))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_settings_section(index, cx)
                            }))
                            .child(icon(
                                entry.icon,
                                15.0,
                                if selected { colors.text } else { colors.muted },
                            ))
                            .child(div().flex_1().min_w_0().truncate().child(entry.name))
                            .when_some(matches, |item, count| {
                                item.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.5))
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
                    .text_size(px(28.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Search"),
            );
            if found.is_empty() {
                page = page.child(
                    div()
                        .mt(px(10.0))
                        .text_size(px(13.0))
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
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(colors.muted))
                            .cursor_pointer()
                            .hover(|style| style.text_color(rgb(colors.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_settings_section(index, cx)
                            }))
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
                        .text_size(px(28.0))
                        .font_weight(FontWeight::MEDIUM)
                        .child(entry.name),
                )
                .child(
                    div()
                        .mt(px(6.0))
                        .text_size(px(13.0))
                        .line_height(px(20.0))
                        .text_color(rgb(colors.muted))
                        .child(entry.summary),
                );
            let groups = self.settings_groups(section, column, colors, cx);
            if groups.iter().all(|group| group.rows.is_empty()) {
                page = page.child(
                    div()
                        .mt(px(28.0))
                        .text_size(px(13.0))
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

        let content = div()
            .id((
                "workspace-settings-content",
                if searching { usize::MAX } else { section },
            ))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .overflow_y_scroll()
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
            // it takes a click.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(navigation)
            .child(content)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
