use super::*;
use serde_json::json;

/// Between the heading and the composer, as an empty chat has them.
const HEADING_GAP: f32 = 28.0;

/// A new chat sent off to be made whose view was left before the host
/// answered. The chat is still made, and its first message sent, without
/// taking the user back to it; until then it shows in the sidebar.
#[derive(Clone)]
pub(super) struct StartingChat {
    pub token: u64,
    /// The host it was sent to.
    pub endpoint: ChatEndpoint,
    /// Where its message goes back to if it is not made.
    pub draft_key: String,
    pub folder_id: String,
    pub session: Option<String>,
    pub title: String,
    pub agent: AgentCli,
    pub model: Option<String>,
    pub choices: settings::NewChatChoices,
    pub worktree: Option<NewSessionWorktree>,
    pub remote: bool,
    pub prompt: String,
    pub attachments: Vec<Attachment>,
}

impl StartingChat {
    /// What the sidebar calls it: the message's first line, as the host
    /// will name it, unless it was given a name.
    pub(super) fn label(&self) -> String {
        draft_label(&self.title, &self.prompt)
    }
}

/// A draft or starting chat's name: the one given, or its message's first line.
pub(super) fn draft_label(title: &str, prompt: &str) -> String {
    let title = title.trim();
    if !is_default_chat_title(title) {
        return title.to_owned();
    }
    prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(80).collect())
        .unwrap_or_else(|| DEFAULT_CHAT_TITLE.to_owned())
}

/// Whether a saved new-chat draft holds anything to send.
pub(super) fn draft_has_content(draft: &Value) -> bool {
    draft["text"]
        .as_str()
        .is_some_and(|text| !text.trim().is_empty())
        || draft["has_attachments"].as_bool() == Some(true)
        || draft["attachments"]
            .as_array()
            .is_some_and(|attachments| !attachments.is_empty())
}

impl XdDesktop {
    pub(super) fn render_new_chat(
        &mut self,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.chat_create_submitting
            && self
                .new_chat_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        {
            window.focus(&self.minimal_popup_focus);
        }
        let shared = self.creating_chat_session.is_some();
        let new_worktree = matches!(
            self.chat_create_worktree,
            Some(NewSessionWorktree::New | NewSessionWorktree::Branch { .. })
        );
        let branch_label = match &self.chat_create_worktree {
            Some(NewSessionWorktree::Branch { reference, .. }) => reference
                .strip_prefix("refs/heads/")
                .or_else(|| reference.strip_prefix("refs/remotes/"))
                .unwrap_or(reference)
                .to_owned(),
            Some(NewSessionWorktree::Existing(path)) => self
                .chat_create_worktrees
                .iter()
                .find(|w| &w.path == path)
                .and_then(|w| w.branch.clone())
                .unwrap_or_else(|| "Project directory".into()),
            _ => "Choose base branch".into(),
        };
        let backend = self.minimal_new_session_agent.protocol_name();
        let model_label = self
            .new_chat_model
            .as_ref()
            .and_then(|id| {
                self.model
                    .agent_backends
                    .iter()
                    .find(|b| b.id == backend)
                    .and_then(|b| b.models.iter().find(|m| &m.id == id))
            })
            .map(|m| m.name.clone())
            .unwrap_or_else(|| format!("{} · Default", self.minimal_new_session_agent.label()));
        let ready = self.new_chat_reading == 0
            && (!new_worktree || !self.chat_create_sources_loading)
            && !self.chat_create_submitting
            && (!self.new_chat_prompt.trim().is_empty() || !self.new_chat_attachments.is_empty())
            && (shared
                || (!self.chat_create_worktrees_loading && self.chat_create_worktree.is_some()));
        let menu = self.new_chat_menu;
        let mut rows = Vec::new();
        match menu {
            Some(option @ ("effort" | "access" | "mode")) => {
                let choices: Vec<(String, String)> = match option {
                    "effort" => self
                        .model
                        .agent_backends
                        .iter()
                        .find(|b| b.id == self.minimal_new_session_agent.protocol_name())
                        .map(|b| {
                            b.efforts
                                .iter()
                                .map(|e| (e.clone(), presentation::effort_name(e).to_owned()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    "access" => ["read-only", "edit", "full"]
                        .into_iter()
                        .map(|a| (a.to_owned(), presentation::access_name(a).to_owned()))
                        .collect(),
                    _ => vec![
                        ("false".into(), "Build".into()),
                        ("true".into(), "Plan".into()),
                    ],
                };
                for (index, (value, label)) in choices.into_iter().enumerate() {
                    if !label.to_lowercase().contains(&self.new_chat_query) {
                        continue;
                    }
                    rows.push(
                        ui_button(("draft-option-choice", index), ButtonKind::Ghost, colors)
                            .w_full()
                            .justify_start()
                            .child(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                match option {
                                    "effort" => {
                                        this.new_chat_remembered.effort = Some(value.clone())
                                    }
                                    "access" => {
                                        this.new_chat_remembered.access = Some(value.clone())
                                    }
                                    _ => this.new_chat_remembered.plan = Some(value == "true"),
                                }
                                this.new_chat_menu = None;
                                this.remember_new_chat_choices();
                                window.focus(&this.new_chat_input.read(cx).focus_handle(cx));
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Some("model") => {
                for agent in [AgentCli::Codex, AgentCli::Claude] {
                    if self.new_chat_query.is_empty() && agent != self.minimal_new_session_agent {
                        continue;
                    }
                    if !self.new_chat_query.is_empty() {
                        rows.push(
                            div()
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(rgb(colors.muted))
                                .flex()
                                .items_center()
                                .gap_2()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(assistant_icon(
                                    agent_icon(agent),
                                    14.0,
                                    colors.muted,
                                    colors,
                                ))
                                .child(agent.label())
                                .into_any_element(),
                        );
                    }
                    let catalog = self
                        .model
                        .agent_backends
                        .iter()
                        .find(|b| b.id == agent.protocol_name());
                    let choices = std::iter::once((None, "Default model".to_owned())).chain(
                        catalog
                            .into_iter()
                            .flat_map(|b| b.models.iter())
                            .map(|m| (Some(m.id.clone()), m.name.clone())),
                    );
                    for (index, (id, name)) in choices.enumerate() {
                        if !name.to_lowercase().contains(&self.new_chat_query) {
                            continue;
                        }
                        rows.push(
                            ui_button(
                                (
                                    if agent == AgentCli::Codex {
                                        "draft-codex-model"
                                    } else {
                                        "draft-claude-model"
                                    },
                                    index,
                                ),
                                ButtonKind::Ghost,
                                colors,
                            )
                            .w_full()
                            .h(px(34.0))
                            .rounded(px(6.0))
                            .text_size(px(13.0))
                            .text_color(rgb(colors.text))
                            .justify_start()
                            .when(
                                self.minimal_new_session_agent == agent
                                    && self.new_chat_model == id,
                                |row| row.bg(rgb(colors.surface_high)),
                            )
                            .child(div().size(px(14.0)).when(
                                self.minimal_new_session_agent == agent
                                    && self.new_chat_model == id,
                                |mark| mark.child(icon(CHECK_ICON, 13.0, colors.text)),
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.focus(&this.new_chat_input.read(cx).focus_handle(cx));
                                if this.minimal_new_session_agent != agent {
                                    this.new_chat_remembered.effort = None;
                                }
                                this.minimal_new_session_agent = agent;
                                this.new_chat_model = id.clone();
                                this.new_chat_menu = None;
                                this.remember_new_chat_choices();
                                cx.notify();
                            }))
                            .child(name)
                            .into_any_element(),
                        );
                    }
                }
            }
            Some("workspace") => {
                for (index, (label, new)) in [("Current worktree", false), ("New worktree", true)]
                    .into_iter()
                    .enumerate()
                {
                    if new && !self.chat_create_can_new_worktree {
                        continue;
                    }
                    rows.push(
                        ui_button(("draft-workspace-choice", index), ButtonKind::Ghost, colors)
                            .w_full()
                            .h(px(58.0))
                            .rounded(px(6.0))
                            .text_color(rgb(colors.text))
                            .justify_start()
                            .child(icon(
                                if new { GIT_BRANCH_ICON } else { FOLDER_ICON },
                                16.0,
                                colors.muted,
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.new_chat_worktree_picked = Some(new);
                                this.chat_create_worktree = if new {
                                    this.default_new_chat_base()
                                } else {
                                    this.chat_create_worktrees
                                        .iter()
                                        .find(|w| w.current)
                                        .or_else(|| this.chat_create_worktrees.first())
                                        .map(|w| NewSessionWorktree::Existing(w.path.clone()))
                                };
                                this.new_chat_menu = None;
                                this.remember_new_chat_choices();
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .items_start()
                                    .gap_1()
                                    .child(div().text_size(px(13.0)).child(label))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(colors.muted))
                                            .child(if new {
                                                "Create an isolated branch and checkout"
                                            } else {
                                                "Use the project's existing checkout"
                                            }),
                                    ),
                            )
                            .child(div().size(px(14.0)).when(new == new_worktree, |mark| {
                                mark.child(icon(CHECK_ICON, 13.0, colors.text))
                            }))
                            .into_any_element(),
                    );
                }
            }
            Some("branch") => {
                if new_worktree {
                    rows.push(
                        ui_button("draft-remote", ButtonKind::Ghost, colors)
                            .w_full()
                            .h(px(34.0))
                            .rounded(px(6.0))
                            .text_size(px(13.0))
                            .text_color(rgb(colors.text))
                            .justify_start()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_chat_remote = !this.new_chat_remote;
                                this.remember_new_chat_choices();
                                cx.notify();
                            }))
                            .child(div().flex_1().child("Start from remote"))
                            .child(
                                div()
                                    .w(px(28.0))
                                    .h(px(16.0))
                                    .p(px(2.0))
                                    .rounded_full()
                                    .bg(rgb(if self.new_chat_remote {
                                        colors.accent
                                    } else {
                                        colors.surface_high
                                    }))
                                    .child(
                                        div()
                                            .size(px(12.0))
                                            .rounded_full()
                                            .when(self.new_chat_remote, |knob| knob.ml(px(12.0)))
                                            .bg(rgb(if self.new_chat_remote {
                                                colors.accent_text
                                            } else {
                                                colors.muted
                                            })),
                                    ),
                            )
                            .into_any_element(),
                    );
                    rows.push(
                        div()
                            .px_3()
                            .py_2()
                            .text_xs()
                            .text_color(rgb(colors.muted))
                            .child("Base branch")
                            .into_any_element(),
                    );
                    for (index, branch) in self.chat_create_branches.iter().enumerate() {
                        let branch = branch.clone();
                        let name = branch.name.clone();
                        let selected = matches!(&self.chat_create_worktree, Some(NewSessionWorktree::Branch { reference, .. }) if reference == &branch.reference);
                        if !name.to_lowercase().contains(&self.new_chat_query) {
                            continue;
                        }
                        rows.push(
                            ui_button(("draft-base", index), ButtonKind::Ghost, colors)
                                .w_full()
                                .h(px(34.0))
                                .rounded(px(6.0))
                                .text_size(px(13.0))
                                .text_color(rgb(colors.text))
                                .justify_start()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.chat_create_worktree = Some(NewSessionWorktree::Branch {
                                        reference: branch.reference.clone(),
                                        oid: branch.oid.clone(),
                                    });
                                    this.new_chat_menu = None;
                                    this.remember_new_chat_choices();
                                    cx.notify();
                                }))
                                .child(icon(GIT_BRANCH_ICON, 13.0, colors.muted))
                                .child(div().flex_1().truncate().child(name))
                                .child(div().size(px(14.0)).when(selected, |mark| {
                                    mark.child(icon(CHECK_ICON, 13.0, colors.text))
                                }))
                                .into_any_element(),
                        );
                    }
                } else {
                    for (index, worktree) in self.chat_create_worktrees.iter().enumerate() {
                        let path = worktree.path.clone();
                        rows.push(
                            ui_button(("draft-checkout", index), ButtonKind::Ghost, colors)
                                .w_full()
                                .h(px(34.0))
                                .rounded(px(6.0))
                                .text_size(px(13.0))
                                .text_color(rgb(colors.text))
                                .justify_start()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.chat_create_worktree =
                                        Some(NewSessionWorktree::Existing(path.clone()));
                                    this.new_chat_menu = None;
                                    this.remember_new_chat_choices();
                                    cx.notify();
                                }))
                                .child(
                                    worktree
                                        .branch
                                        .clone()
                                        .unwrap_or_else(|| worktree.path.clone()),
                                )
                                .into_any_element(),
                        );
                    }
                }
                if self.chat_create_sources_loading {
                    rows.push(
                        div()
                            .p_3()
                            .text_sm()
                            .child("Loading branches…")
                            .into_any_element(),
                    );
                }
            }
            _ => {}
        }
        if menu.is_some() && rows.is_empty() {
            rows.push(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(rgb(colors.muted))
                    .child("No matches")
                    .into_any_element(),
            );
        }
        let mut popup = menu.map(|kind| {
            gpui::deferred(
                div()
                    .id("draft-picker")
                    .occlude()
                    .absolute()
                    .bottom(px(38.0))
                    .left_0()
                    .w(px(if kind == "workspace" { 280.0 } else { 320.0 }))
                    .flex()
                    .flex_col()
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.surface))
                    .shadow_lg()
                    .when(kind != "workspace", |picker| {
                        picker.child(
                            div()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .border_b_1()
                                .border_color(rgb(colors.border))
                                .child(icon(SEARCH_ICON, 14.0, colors.muted))
                                .child(div().flex_1().child(self.new_chat_search.clone())),
                        )
                    })
                    .when(kind == "model", |picker| {
                        picker.child(
                            div().p_1().flex().gap_1().children(
                                [AgentCli::Codex, AgentCli::Claude]
                                    .into_iter()
                                    .map(|agent| {
                                        ui_button(
                                            if agent == AgentCli::Codex {
                                                "draft-provider-codex"
                                            } else {
                                                "draft-provider-claude"
                                            },
                                            ButtonKind::Ghost,
                                            colors,
                                        )
                                        .flex_1()
                                        .h(px(30.0))
                                        .text_color(rgb(colors.text))
                                        .when(agent == self.minimal_new_session_agent, |tab| {
                                            tab.bg(rgb(colors.surface_high))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.minimal_new_session_agent != agent {
                                                this.new_chat_remembered.effort = None;
                                                this.minimal_new_session_agent = agent;
                                                this.new_chat_model = None;
                                            }
                                            this.remember_new_chat_choices();
                                            this.new_chat_query.clear();
                                            this.new_chat_search.update(cx, |input, cx| {
                                                input.set_text(String::new(), cx)
                                            });
                                            cx.notify();
                                        }))
                                        .child(assistant_icon(
                                            agent_icon(agent),
                                            14.0,
                                            colors.muted,
                                            colors,
                                        ))
                                        .child(agent.label())
                                    }),
                            ),
                        )
                    })
                    .child(
                        div()
                            .id("draft-picker-options")
                            .max_h(px(280.0))
                            .overflow_y_scroll()
                            .p_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .children(rows),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // The model picker opens inside the composer, which would
                    // take the focus from the search field to the editor.
                    .on_click(|_, _, cx| cx.stop_propagation()),
            )
            .with_priority(1)
        });
        let project = self
            .creating_chat_folder
            .as_ref()
            .and_then(|id| self.model.folders.iter().find(|f| &f.id == id));
        let avatar = self.owner_avatar(project.and_then(|f| f.owner.as_deref()));
        let folder = project.map(|f| f.name.clone()).unwrap_or_default();
        let starting = self.chat_create_submitting;
        let editor = if starting {
            // The message went up into the conversation above.
            div()
                .text_color(rgb(colors.faint))
                .child(editor::MESSAGE_PLACEHOLDER)
                .into_any_element()
        } else {
            self.new_chat_input.clone().into_any_element()
        };
        let model_icon = assistant_icon(
            agent_icon(self.minimal_new_session_agent),
            13.0,
            colors.muted,
            colors,
        )
        .into_any_element();
        let model = div()
            .relative()
            .child(self.draft_picker_toggle(
                "model",
                composer::composer_option(
                    "draft-model",
                    Some(model_icon),
                    model_label,
                    280.0,
                    colors,
                ),
                cx,
            ))
            .children(if menu == Some("model") {
                popup.take()
            } else {
                None
            })
            .into_any_element();
        let mut controls = vec![model];
        let supports_effort = self
            .model
            .agent_backends
            .iter()
            .find(|b| b.id == self.minimal_new_session_agent.protocol_name())
            .is_some_and(|b| !b.efforts.is_empty());
        let plan = self.new_chat_remembered.plan.unwrap_or(false);
        let options = [
            (
                "effort",
                self.new_chat_remembered
                    .effort
                    .as_deref()
                    .map(|e| format!("{} effort", presentation::effort_name(e)))
                    .unwrap_or("Default effort".into()),
            ),
            (
                "access",
                self.new_chat_remembered
                    .access
                    .as_deref()
                    .map(|a| presentation::access_name(a).to_owned())
                    .unwrap_or("Default access".into()),
            ),
            ("mode", if plan { "Plan" } else { "Build" }.to_owned()),
        ];
        for (option, label) in options {
            if option == "effort" && !supports_effort {
                continue;
            }
            let label = if option == "access" && plan {
                "Read only".to_owned()
            } else {
                label
            };
            let chip = composer::composer_option(
                SharedString::from(format!("draft-{option}")),
                None,
                label,
                160.0,
                colors,
            );
            let chip = if option == "access" && plan {
                chip
            } else {
                self.draft_picker_toggle(option, chip, cx)
            };
            controls.push(
                div()
                    .relative()
                    .child(chip)
                    .children(if menu == Some(option) {
                        popup.take()
                    } else {
                        None
                    })
                    .into_any_element(),
            );
        }
        let send = composer::composer_send_button("draft-send", ready, SEND_ICON, colors)
            .on_click(cx.listener(move |this, _, _, cx| {
                if ready {
                    this.save_minimal_chat_create(cx);
                }
            }))
            .into_any_element();
        // Starting, it takes the open chat's width and place, which is
        // where the chat will show when it opens.
        let width = px(if starting { 736.0 } else { 660.0 });
        let composer = self.render_composer(
            composer::ComposerParts {
                owner: image_preview::DraftOwner::NewChat,
                width,
                editor,
                controls,
                actions: vec![send],
                show_attachments: !starting,
                ask: None,
            },
            colors,
            window,
            cx,
        );
        let context = composer::context_row(folder.clone(), colors)
            .debug_selector(|| "new-chat-context".into())
            .when(!shared, |row| {
                let workspace = composer::context_chip(
                    "draft-workspace",
                    FOLDER_ICON,
                    if new_worktree {
                        "New worktree"
                    } else {
                        "Current worktree"
                    }
                    .into(),
                    true,
                    colors,
                );
                let branch = composer::context_chip(
                    "draft-branch",
                    GIT_BRANCH_ICON,
                    branch_label,
                    true,
                    colors,
                );
                row.child(
                    div()
                        .relative()
                        .child(self.draft_picker_toggle("workspace", workspace, cx))
                        .children(if menu == Some("workspace") {
                            popup.take()
                        } else {
                            None
                        }),
                )
                .child(
                    div()
                        .relative()
                        .child(self.draft_picker_toggle("branch", branch, cx))
                        .children(if menu == Some("branch") {
                            popup.take()
                        } else {
                            None
                        }),
                )
            })
            .when(shared, |row| row.child("Shared session worktree"));
        div()
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.attach_new_chat_paths(paths.paths().to_vec(), cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.new_chat_menu.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .when(!starting, |view| view.justify_center())
            .px_6()
            .bg(crate::glass::canvas())
            .when(!starting, |view| {
                let (heading, description) = composer::EMPTY_CHAT;
                view.child(
                    composer::chat_heading(folder, avatar, heading, description, colors)
                        .max_w(width)
                        .mb(px(HEADING_GAP)),
                )
            })
            .when(starting, |view| {
                view.child(self.render_new_chat_starting(width, colors, window))
            })
            .child(composer.mb_2())
            .child(
                context
                    .w_full()
                    .max_w(width)
                    .when(starting, |row| row.mb(px(10.0))),
            )
            .into_any_element()
    }

    /// What the conversation will open with, while the chat is set up: the
    /// message as sent, and what is being done before it can be answered.
    fn render_new_chat_starting(
        &self,
        width: Pixels,
        colors: ThemeColors,
        window: &Window,
    ) -> gpui::AnyElement {
        let prompt = self.new_chat_submitted_prompt.clone().unwrap_or_default();
        let side = (64.0 * window.scale_factor()).ceil() as u32;
        let images = self
            .new_chat_submitted_attachments
            .iter()
            .filter(|attachment| attachment.preview.is_some())
            .map(|attachment| {
                let thumbnail = match self.composer_thumbnails.get(&(attachment.id, side)) {
                    Some(image_preview::ThumbnailState::Ready(image)) => Some(image.clone()),
                    _ => None,
                };
                div()
                    .size(px(64.0))
                    .flex_none()
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .bg(rgb(colors.surface_high))
                    .children(thumbnail.map(|image| {
                        gpui::img(image)
                            .size_full()
                            .object_fit(gpui::ObjectFit::Fill)
                    }))
            })
            .collect::<Vec<_>>();
        let files = self
            .new_chat_submitted_attachments
            .iter()
            .filter(|attachment| attachment.preview.is_none())
            .map(|attachment| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(rgb(colors.muted))
                    .child(icon(FILE_ICON, 13.0, colors.muted))
                    .child(attachment.name.clone())
            })
            .collect::<Vec<_>>();
        div()
            .id("new-chat-starting")
            .flex_1()
            .min_h_0()
            .w_full()
            .max_w(width)
            .pt(px(24.0))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap_2()
                    .when(!images.is_empty(), |column| {
                        column.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .justify_end()
                                .gap_2()
                                .children(images),
                        )
                    })
                    .when(!prompt.trim().is_empty() || !files.is_empty(), |column| {
                        column.child(
                            div()
                                .debug_selector(|| "new-chat-sent".into())
                                .max_w(px(600.0))
                                .px(px(14.0))
                                .py(px(9.0))
                                .rounded(px(18.0))
                                .bg(rgb(colors.surface_high))
                                .flex()
                                .flex_col()
                                .gap_2()
                                .text_size(px(14.0))
                                .text_color(rgb(colors.text))
                                .when(!prompt.trim().is_empty(), |bubble| bubble.child(prompt))
                                .children(files),
                        )
                    }),
            )
            .child(
                div()
                    .debug_selector(|| "new-chat-status".into())
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(2.0))
                    .text_size(px(12.5))
                    .text_color(rgb(colors.muted))
                    .child(
                        div()
                            .size(px(7.0))
                            .rounded_full()
                            .bg(rgb(colors.working))
                            .opacity(pulse_opacity()),
                    )
                    .child(self.new_chat_status())
                    .child(working_dots(0, colors.muted)),
            )
            .into_any_element()
    }

    /// What starting the chat is waiting on, in the words of its pickers.
    pub(super) fn new_chat_status(&self) -> String {
        if self.new_chat_started.is_some() {
            return "Opening the chat".into();
        }
        if self.creating_chat_session.is_some() {
            return "Starting the chat".into();
        }
        match &self.chat_create_worktree {
            Some(NewSessionWorktree::New) => "Creating a worktree".into(),
            Some(NewSessionWorktree::Branch { reference, .. }) => {
                let branch = reference
                    .strip_prefix("refs/heads/")
                    .or_else(|| reference.strip_prefix("refs/remotes/"))
                    .unwrap_or(reference);
                if self.new_chat_remote {
                    format!("Creating a worktree from the latest {branch}")
                } else {
                    format!("Creating a worktree from {branch}")
                }
            }
            Some(NewSessionWorktree::PullRequest { number, .. }) => {
                format!("Checking out pull request #{number}")
            }
            Some(NewSessionWorktree::Existing(_)) | None => "Starting the chat".into(),
        }
    }

    pub(super) fn pick_new_chat_attachments(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chat_create_submitting {
            return;
        }
        let generation = self.new_chat_generation;
        let connection = self.attachment_connection();
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| match chosen.await {
            Ok(Ok(Some(paths))) => {
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.new_chat_generation == generation
                        && this.attachment_connection() == connection
                        && this.creating_chat_folder.is_some()
                    {
                        window.focus(&this.new_chat_input.read(cx).focus_handle(cx));
                        this.attach_new_chat_paths(paths, cx);
                    }
                });
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.model.connection_error = Some(format!(
                        "Could not open file picker: {error}. Drop files onto the chat instead."
                    ));
                    cx.notify();
                });
            }
            _ => {}
        })
        .detach();
    }

    fn attach_new_chat_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.chat_create_submitting || self.creating_chat_folder.is_none() {
            return;
        }
        let draft = &self.new_chat_attachments;
        let room = DraftRoom {
            slots: MAX_ATTACHMENTS.saturating_sub(draft.len()),
            images: MAX_IMAGE_ATTACHMENTS
                .saturating_sub(draft.iter().filter(|a| a.preview.is_some()).count()),
            bytes: MAX_TOTAL_ATTACHMENT_BYTES.saturating_sub(draft.iter().map(|a| a.bytes).sum()),
        };
        let generation = self.new_chat_generation;
        let connection = self.attachment_connection();
        let read = cx.background_spawn(async move { read_attachments(paths, room) });
        self.new_chat_reading += 1;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_new_chat_file_read(generation, connection, read, cx);
            });
        })
        .detach();
    }

    fn finish_new_chat_file_read(
        &mut self,
        generation: u64,
        connection: (ChatEndpoint, u64, u64),
        read: Vec<Result<Attachment, String>>,
        cx: &mut Context<Self>,
    ) {
        if self.new_chat_generation != generation {
            return;
        }
        self.new_chat_reading = self.new_chat_reading.saturating_sub(1);
        if self.attachment_connection() != connection || self.creating_chat_folder.is_none() {
            cx.notify();
            return;
        }
        for attachment in read {
            match attachment.and_then(|a| {
                attachment_fits(&self.new_chat_attachments, &a)?;
                Ok(a)
            }) {
                Ok(a) => self.new_chat_attachments.push(a),
                Err(error) => self.model.connection_error = Some(error),
            }
        }
        self.persist_new_chat_draft();
        cx.notify();
    }

    pub(super) fn attach_new_chat_image(
        &mut self,
        format: gpui::ImageFormat,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        if self.chat_create_submitting {
            return;
        }
        let result = if format != gpui::ImageFormat::Png {
            Err("Paste a PNG image into the chat.".to_owned())
        } else if bytes.len() > MAX_ATTACHMENT_BYTES {
            Err("That image is larger than 10 MiB.".to_owned())
        } else {
            Attachment::from_png(
                format!("paste-{}.png", self.new_chat_attachments.len() + 1),
                bytes,
            )
            .and_then(|attachment| {
                attachment_fits(&self.new_chat_attachments, &attachment)?;
                self.new_chat_attachments.push(attachment);
                Ok(())
            })
        };
        if let Err(error) = result {
            self.model.connection_error = Some(error);
        }
        self.persist_new_chat_draft();
        cx.notify();
    }

    /// A chip that opens one of the new chat's pickers, or closes it again.
    fn draft_picker_toggle(
        &self,
        menu: &'static str,
        chip: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        chip.on_click(cx.listener(move |this, _, window, cx| {
            // The composer behind it would take the focus back to the editor.
            cx.stop_propagation();
            if !this.chat_create_submitting {
                this.new_chat_query.clear();
                this.new_chat_search
                    .update(cx, |input, cx| input.set_text(String::new(), cx));
                if menu != "workspace" {
                    let focus = this.new_chat_search.read(cx).focus_handle(cx);
                    window.focus(&focus);
                }
                this.new_chat_menu = if this.new_chat_menu == Some(menu) {
                    None
                } else {
                    Some(menu)
                };
                cx.notify();
            }
        }))
    }

    fn new_chat_draft_key(&self) -> Option<String> {
        let folder = self.creating_chat_folder.as_ref()?;
        Some(format!(
            "{}/{}",
            self.new_chat_choices_key(folder),
            self.creating_chat_session.as_deref().unwrap_or("new")
        ))
    }

    pub(super) fn schedule_new_chat_draft(&mut self, cx: &mut Context<Self>) {
        self.new_chat_save_generation += 1;
        let generation = self.new_chat_save_generation;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(250)).await;
            let _ = this.update(cx, |this, _| {
                if this.new_chat_save_generation == generation {
                    this.persist_new_chat_draft();
                }
            });
        })
        .detach();
    }

    pub(super) fn persist_new_chat_draft(&mut self) {
        self.new_chat_save_generation += 1;
        if self.chat_create_submitting {
            return;
        }
        let Some(key) = self.new_chat_draft_key() else {
            return;
        };
        let mut choices = self.new_chat_remembered.clone();
        choices.agent = Some(self.minimal_new_session_agent.protocol_name().to_owned());
        choices.model = self.new_chat_model.clone();
        let text = self.new_chat_prompt.clone();
        let attachments = self.new_chat_attachments.clone();
        self.write_new_chat_draft(key, &text, &attachments, &choices);
    }

    /// Saves a new-chat draft under its key, whichever view is open.
    fn write_new_chat_draft(
        &mut self,
        key: String,
        text: &str,
        attachments: &[Attachment],
        choices: &settings::NewChatChoices,
    ) {
        let ids = json!(attachments.iter().map(|a| a.id).collect::<Vec<_>>());
        let changed = self
            .settings
            .new_chat_drafts
            .get(&key)
            .is_none_or(|d| d["ids"] != ids);
        let payload = (changed || cfg!(test)).then(|| {
            json!(
                attachments
                    .iter()
                    .map(|a| json!({"name": a.name, "mime": a.mime, "data": a.data}))
                    .collect::<Vec<_>>()
            )
        });
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&ids.to_string(), &mut hash);
        let attachment_file = format!("attachments-{:016x}.json", std::hash::Hasher::finish(&hash));
        let mut draft = json!({"text": text, "choices": choices,
            "has_attachments": !attachments.is_empty(), "ids": ids, "attachment_file": attachment_file});
        if cfg!(test) {
            draft["attachments"] = payload.clone().unwrap_or(json!([]));
        }
        if draft_has_content(&draft) {
            self.new_chat_draft_keys.insert(key.clone());
        } else {
            self.new_chat_draft_keys.remove(&key);
        }
        self.new_chat_drafts_written.insert(key.clone());
        self.settings
            .new_chat_drafts
            .insert(key.clone(), draft.clone());
        if !cfg!(test)
            && let Err(error) = new_chat_store::save(key, draft, payload)
        {
            self.model.connection_error =
                Some(format!("Could not save the new-chat draft: {error}"));
        }
    }

    /// The draft saved under a key: its text, files and choices.
    fn read_new_chat_draft(&mut self, key: &str) -> Option<Value> {
        if cfg!(test) {
            return self.settings.new_chat_drafts.get(key).cloned();
        }
        match new_chat_store::read(key.to_owned()) {
            Ok(draft) => Some(draft),
            Err(error) => {
                self.model.connection_error =
                    Some(format!("Could not restore the new-chat draft: {error}"));
                None
            }
        }
    }

    pub(super) fn restore_new_chat_draft(&mut self, cx: &mut Context<Self>) {
        let loaded = self
            .new_chat_draft_key()
            .and_then(|key| self.read_new_chat_draft(&key));
        let draft = loaded.as_ref();
        self.new_chat_prompt = draft
            .and_then(|d| d["text"].as_str())
            .unwrap_or_default()
            .to_owned();
        self.new_chat_attachments = draft
            .and_then(|d| d["attachments"].as_array())
            .map(|a| a.iter().filter_map(Attachment::from_value).collect())
            .unwrap_or_default();
        if let Some(choices) = draft
            .and_then(|d| d.get("choices"))
            .and_then(|v| serde_json::from_value::<settings::NewChatChoices>(v.clone()).ok())
        {
            if let Some(agent) = choices
                .agent
                .as_deref()
                .and_then(AgentCli::from_protocol_name)
            {
                self.minimal_new_session_agent = agent;
            }
            // A saved draft must follow the same catalog validation as remembered choices.
            self.new_chat_model = choices.model.clone().filter(|model| {
                self.model
                    .agent_backends
                    .iter()
                    .find(|backend| backend.id == self.minimal_new_session_agent.protocol_name())
                    .is_some_and(|backend| {
                        backend
                            .models
                            .iter()
                            .any(|candidate| &candidate.id == model)
                    })
            });
            self.new_chat_remote = choices.start_from_remote;
            self.new_chat_remembered = choices;
        }
        let text = self.new_chat_prompt.clone();
        self.new_chat_input
            .update(cx, |input, cx| input.set_text(text, cx));
    }

    /// Waits for every draft write asked for so far, whichever drafts.
    pub(super) fn flush_new_chat_draft(&mut self) {
        if !cfg!(test)
            && let Err(error) = new_chat_store::flush()
        {
            self.model.connection_error = Some(error);
        }
    }

    pub(super) fn clear_new_chat_draft(&mut self) {
        if let Some(key) = self.new_chat_draft_key() {
            self.new_chat_draft_keys.remove(&key);
            self.new_chat_drafts_written.insert(key.clone());
            self.settings.new_chat_drafts.remove(&key);
            if !cfg!(test)
                && let Err(error) = new_chat_store::save(key, json!({}), Some(json!([])))
            {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn new_chat_choices_key(&self, folder_id: &str) -> String {
        format!("{}/project/{folder_id}", self.current_connection_key())
    }

    /// Start a project's new chat the way its last one was set up. The agent,
    /// model, and remote start apply now; the worktree and base branch once
    /// the host has said which still exist.
    pub(super) fn restore_new_chat_choices(&mut self, folder_id: &str) {
        let remembered = self
            .settings
            .new_chat_choices
            .get(&self.new_chat_choices_key(folder_id))
            .cloned()
            .unwrap_or_default();
        let agent = remembered
            .agent
            .as_deref()
            .and_then(AgentCli::from_protocol_name)
            .filter(|agent| matches!(agent, AgentCli::Codex | AgentCli::Claude));
        if let Some(agent) = agent {
            self.minimal_new_session_agent = agent;
        }
        // A model the agent no longer offers would be refused on send.
        self.new_chat_model = remembered.model.clone().filter(|model| {
            agent.is_some()
                && self
                    .model
                    .agent_backends
                    .iter()
                    .find(|backend| backend.id == self.minimal_new_session_agent.protocol_name())
                    .is_some_and(|backend| backend.models.iter().any(|m| &m.id == model))
        });
        self.new_chat_remote = remembered.start_from_remote;
        self.new_chat_remembered = remembered;
        self.new_chat_worktree_picked = None;
    }

    /// Keep what the new chat is set up with for the project's next one.
    pub(super) fn remember_new_chat_choices(&mut self) {
        let Some(folder_id) = self.creating_chat_folder.clone() else {
            return;
        };
        let mut choices = self.new_chat_remembered.clone();
        choices.agent = Some(self.minimal_new_session_agent.protocol_name().to_owned());
        choices.model = self.new_chat_model.clone();
        // A chat joining a shared session takes that session's worktree, and
        // says nothing about where the project's next chat should run.
        if self.creating_chat_session.is_none() {
            match &self.chat_create_worktree {
                Some(NewSessionWorktree::New) => {
                    choices.new_worktree = true;
                    choices.start_from_remote = self.new_chat_remote;
                }
                Some(NewSessionWorktree::Branch { reference, .. }) => {
                    choices.new_worktree = true;
                    choices.base = Some(reference.clone());
                    choices.start_from_remote = self.new_chat_remote;
                }
                Some(NewSessionWorktree::Existing(path)) => {
                    choices.new_worktree = false;
                    choices.checkout = Some(path.clone());
                }
                // The worktrees have not arrived yet to pick a checkout or
                // base from, but the mode was picked.
                Some(NewSessionWorktree::PullRequest { .. }) | None => {
                    if let Some(new_worktree) = self.new_chat_worktree_picked {
                        choices.new_worktree = new_worktree;
                        if !new_worktree {
                            // The project's current checkout, whichever that is.
                            choices.checkout = None;
                        }
                    }
                }
            }
        }
        if self
            .settings
            .new_chat_choices
            .get(&self.new_chat_choices_key(&folder_id))
            == Some(&choices)
        {
            return;
        }
        self.settings
            .new_chat_choices
            .insert(self.new_chat_choices_key(&folder_id), choices.clone());
        self.new_chat_remembered = choices;
        self.persist_new_chat_draft();
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
    }

    pub(super) fn default_new_chat_base(&self) -> Option<NewSessionWorktree> {
        // The base the project's last new worktree branched from, while it
        // is still there.
        if let Some(branch) = self.new_chat_remembered.base.as_ref().and_then(|base| {
            self.chat_create_branches
                .iter()
                .find(|branch| &branch.reference == base)
        }) {
            return Some(NewSessionWorktree::Branch {
                reference: branch.reference.clone(),
                oid: branch.oid.clone(),
            });
        }
        let current = self
            .chat_create_worktrees
            .iter()
            .find(|w| w.current)
            .and_then(|w| w.branch.as_deref());
        self.chat_create_branches
            .iter()
            .find(|b| Some(b.name.as_str()) == self.new_chat_default_branch.as_deref())
            .or_else(|| {
                self.new_chat_default_branch.as_ref().and_then(|name| {
                    self.chat_create_branches
                        .iter()
                        .find(|b| b.reference == format!("refs/remotes/origin/{name}"))
                })
            })
            .or_else(|| {
                self.chat_create_branches
                    .iter()
                    .find(|b| Some(b.name.as_str()) == current)
            })
            .map(|b| NewSessionWorktree::Branch {
                reference: b.reference.clone(),
                oid: b.oid.clone(),
            })
            .or(Some(NewSessionWorktree::New))
    }
}

impl XdDesktop {
    /// Leaves the new-chat view keeping what it held: an unsent draft stays
    /// saved for the project's next new chat, and a chat already sent off
    /// goes on starting, shown in the sidebar.
    pub(super) fn leave_chat_create(&mut self) {
        if self.creating_chat_folder.is_none() {
            return;
        }
        if self.chat_create_in_flight() {
            self.detach_chat_create();
        } else {
            self.persist_new_chat_draft();
        }
    }

    /// The view's chat, sent off and not answered yet, carries on without it.
    fn detach_chat_create(&mut self) {
        let (Some(folder_id), Some(token), Some(draft_key)) = (
            self.creating_chat_folder.clone(),
            self.chat_create_token,
            self.new_chat_draft_key(),
        ) else {
            return;
        };
        self.starting_chats.push(StartingChat {
            token,
            endpoint: self.active_endpoint,
            draft_key,
            folder_id,
            session: self.creating_chat_session.clone(),
            title: self.chat_create_title.clone(),
            agent: self.minimal_new_session_agent,
            model: self.new_chat_model.clone(),
            choices: self.new_chat_remembered.clone(),
            worktree: self.chat_create_worktree.clone(),
            remote: self.new_chat_remote,
            prompt: self.new_chat_submitted_prompt.clone().unwrap_or_default(),
            attachments: self.new_chat_submitted_attachments.clone(),
        });
        // Its draft was emptied when it was sent, so the project's next new
        // chat starts empty. The message comes back if the chat is not made.
        self.chat_create_submitting = false;
        self.chat_create_token = None;
        self.new_chat_submitted_prompt = None;
        self.new_chat_submitted_attachments.clear();
        self.new_chat_prompt.clear();
        self.new_chat_attachments.clear();
    }

    /// Goes back to a chat that is starting, to watch it open.
    pub(super) fn open_starting_chat(
        &mut self,
        token: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chat_create_token == Some(token) {
            return;
        }
        if !self
            .starting_chats
            .iter()
            .any(|chat| chat.token == token && chat.endpoint == self.active_endpoint)
            || !self.flush_draft_before_navigation()
        {
            return;
        }
        self.send_first_messages_aside(None);
        if self.creating_chat_folder.is_some() {
            self.cancel_chat_create(cx);
        }
        self.pending_minimal_session = None;
        let Some(index) = self
            .starting_chats
            .iter()
            .position(|chat| chat.token == token)
        else {
            return;
        };
        let chat = self.starting_chats.remove(index);
        if self.browser_is_open() {
            self.close_browser(cx);
        }
        self.browser_restore_focus = false;
        self.new_chat_generation += 1;
        self.new_chat_reading = 0;
        self.new_chat_query.clear();
        self.new_chat_menu = None;
        self.new_chat_default_branch = None;
        self.creating_chat_folder = Some(chat.folder_id);
        self.creating_chat_session = chat.session;
        self.chat_create_title = chat.title.clone();
        self.chat_create_input
            .update(cx, |input, cx| input.set_text(chat.title, cx));
        self.minimal_new_session_agent = chat.agent;
        self.new_chat_model = chat.model;
        self.new_chat_remembered = chat.choices;
        self.new_chat_remote = chat.remote;
        self.new_chat_worktree_picked = None;
        self.chat_create_worktree = chat.worktree;
        self.chat_create_worktrees.clear();
        self.chat_create_worktrees_loading = false;
        self.chat_create_can_new_worktree = false;
        self.chat_create_branches.clear();
        self.chat_create_pull_requests.clear();
        self.chat_create_sources_loading = false;
        self.chat_create_sources_error = None;
        self.new_chat_prompt = chat.prompt.clone();
        self.new_chat_attachments = chat.attachments.clone();
        let text = chat.prompt.clone();
        self.new_chat_input
            .update(cx, |input, cx| input.set_text(text, cx));
        self.new_chat_submitted_prompt = Some(chat.prompt);
        self.new_chat_submitted_attachments = chat.attachments;
        self.new_chat_started = None;
        self.chat_create_token = Some(chat.token);
        self.chat_create_submitting = true;
        window.focus(&self.minimal_popup_focus);
        cx.notify();
    }

    /// Handles the host's answer for a chat whose view was left. Returns
    /// whether the reply was one.
    pub(super) fn handle_starting_chat_reply(
        &mut self,
        endpoint: ChatEndpoint,
        kind: &RequestKind,
        value: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let RequestKind::NewChat { token, .. } = kind else {
            return false;
        };
        let Some(index) = self
            .starting_chats
            .iter()
            .position(|chat| chat.token == *token && chat.endpoint == endpoint)
        else {
            return false;
        };
        let chat = self.starting_chats.remove(index);
        let chat_id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|_| value.get("ok").and_then(Value::as_bool) == Some(true));
        let Some(chat_id) = chat_id else {
            let error = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("The host returned no chat id.");
            self.endpoint_model_mut(endpoint).connection_error = Some(format!(
                "Could not start “{}”: {error} Its message is back in the project's new chat.",
                chat.label()
            ));
            self.return_starting_chat(chat, cx);
            return true;
        };
        // As the view would have: the message waits in the chat's draft,
        // and goes out at once rather than when the chat is next opened.
        if let Some(host) = self.endpoint_host(endpoint)
            && let Err(error) = host.set_draft(chat_id, &chat.prompt, Some(&chat.attachments), None)
        {
            self.endpoint_model_mut(endpoint).connection_error = Some(error);
        }
        self.first_chat_prompts.insert(
            chat_id.to_owned(),
            FirstChatPrompt {
                endpoint,
                text: chat.prompt,
                attachments: chat.attachments,
            },
        );
        self.send_first_message_aside(chat_id);
        if endpoint == self.active_endpoint {
            self.request_tree();
        }
        cx.notify();
        true
    }

    /// Chats still starting on a host whose connection closed: their answer
    /// will not come, so their messages go back to their drafts.
    pub(super) fn release_starting_chats(
        &mut self,
        endpoint: ChatEndpoint,
        cx: &mut Context<Self>,
    ) {
        let (released, kept) = std::mem::take(&mut self.starting_chats)
            .into_iter()
            .partition::<Vec<_>, _>(|chat| chat.endpoint == endpoint);
        self.starting_chats = kept;
        for chat in released {
            self.return_starting_chat(chat, cx);
        }
    }

    /// A chat that was not made: its message goes back to the new chat it
    /// came from, beside anything written there since.
    fn return_starting_chat(&mut self, chat: StartingChat, cx: &mut Context<Self>) {
        let open = self.new_chat_draft_key().as_deref() == Some(chat.draft_key.as_str())
            && !self.chat_create_submitting;
        if open {
            self.new_chat_prompt = join_drafts(&chat.prompt, &self.new_chat_prompt);
            let mut attachments = chat.attachments;
            merge_attachments(
                &mut attachments,
                std::mem::take(&mut self.new_chat_attachments),
            );
            self.new_chat_attachments = attachments;
            let text = self.new_chat_prompt.clone();
            self.new_chat_input
                .update(cx, |input, cx| input.set_text(text, cx));
            self.persist_new_chat_draft();
            return;
        }
        let saved = self.read_new_chat_draft(&chat.draft_key);
        let saved_text = saved
            .as_ref()
            .and_then(|draft| draft["text"].as_str())
            .unwrap_or_default();
        let text = join_drafts(&chat.prompt, saved_text);
        let mut attachments = chat.attachments;
        merge_attachments(
            &mut attachments,
            saved
                .as_ref()
                .and_then(|draft| draft["attachments"].as_array())
                .map(|saved| saved.iter().filter_map(Attachment::from_value).collect())
                .unwrap_or_default(),
        );
        let choices = saved
            .as_ref()
            .and_then(|draft| draft.get("choices"))
            .and_then(|choices| serde_json::from_value(choices.clone()).ok())
            .unwrap_or(chat.choices);
        self.write_new_chat_draft(chat.draft_key, &text, &attachments, &choices);
    }

    /// The view's chat was not made: its message is editable again, with
    /// anything that came back to the same draft meanwhile, and saved.
    pub(super) fn reopen_failed_chat_create(&mut self, cx: &mut Context<Self>) {
        self.chat_create_submitting = false;
        self.chat_create_token = None;
        self.new_chat_submitted_prompt = None;
        self.new_chat_submitted_attachments.clear();
        // Gone back to from the sidebar while it started, the view has no
        // checkouts or branches to pick another from: ask for them again.
        if self.creating_chat_session.is_none()
            && self.chat_create_worktrees.is_empty()
            && !self.chat_create_worktrees_loading
            && let Some(folder_id) = self.creating_chat_folder.clone()
            && let Some(host) = self.active_host().cloned()
        {
            self.chat_create_worktrees_loading = true;
            self.chat_create_sources_loading = true;
            self.chat_create_sources_error = None;
            if let Err(error) = host.folder_settings(&folder_id) {
                self.chat_create_worktrees_loading = false;
                self.model.connection_error = Some(error);
            }
            if let Err(error) = host.new_chat_sources(&folder_id) {
                self.chat_create_sources_loading = false;
                self.chat_create_sources_error = Some(error);
            }
        }
        if let Some(key) = self.new_chat_draft_key()
            && let Some(saved) = self.read_new_chat_draft(&key).filter(draft_has_content)
        {
            self.new_chat_prompt = join_drafts(
                &self.new_chat_prompt,
                saved["text"].as_str().unwrap_or_default(),
            );
            merge_attachments(
                &mut self.new_chat_attachments,
                saved["attachments"]
                    .as_array()
                    .map(|saved| saved.iter().filter_map(Attachment::from_value).collect())
                    .unwrap_or_default(),
            );
            let text = self.new_chat_prompt.clone();
            self.new_chat_input
                .update(cx, |input, cx| input.set_text(text, cx));
        }
        self.persist_new_chat_draft();
    }

    /// Closing xd: messages still on their way back to their drafts, which
    /// were emptied when they were sent, so none is lost if no chat is made.
    pub(super) fn keep_starting_messages(&mut self, cx: &mut Context<Self>) {
        if self.chat_create_in_flight() {
            self.detach_chat_create();
            // Its draft holds whatever was written there since, not the
            // emptied view: the message goes back beside that.
            self.creating_chat_folder = None;
            self.creating_chat_session = None;
        }
        for chat in std::mem::take(&mut self.starting_chats) {
            self.return_starting_chat(chat, cx);
        }
    }

    /// Learns which projects have drafts saved from before, for the sidebar.
    pub(super) fn load_new_chat_draft_keys(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let listed = cx.background_spawn(async move { new_chat_store::list() });
        cx.spawn(async move |this, cx| {
            let keys = listed.await;
            let _ = this.update(cx, |this, cx| {
                // Drafts saved or emptied this run already know their state.
                for (key, draft) in keys {
                    if !this.new_chat_drafts_written.contains(&key) {
                        this.new_chat_draft_keys.insert(key.clone());
                        this.settings.new_chat_drafts.insert(key, draft);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether a project has a new chat being written, saved or open.
    pub(super) fn project_draft(&self, folder_id: &str) -> Option<String> {
        let key = format!("{}/new", self.new_chat_choices_key(folder_id));
        let open = self.creating_chat_folder.as_deref() == Some(folder_id)
            && self.creating_chat_session.is_none()
            && !self.chat_create_submitting
            && self.new_chat_started.is_none();
        if open {
            return Some(draft_label("", &self.new_chat_prompt));
        }
        if !self.new_chat_draft_keys.contains(&key) {
            return None;
        }
        let text = self
            .settings
            .new_chat_drafts
            .get(&key)
            .and_then(|draft| draft["text"].as_str())
            .unwrap_or_default();
        Some(draft_label("", text))
    }

    /// Throws a project's new-chat draft away.
    pub(super) fn discard_project_draft(&mut self, folder_id: &str, cx: &mut Context<Self>) {
        let key = format!("{}/new", self.new_chat_choices_key(folder_id));
        if self.new_chat_draft_key().as_deref() == Some(key.as_str())
            && !self.chat_create_submitting
        {
            self.new_chat_save_generation += 1;
            self.new_chat_prompt.clear();
            self.new_chat_attachments.clear();
            self.new_chat_input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
        }
        let choices = self
            .settings
            .new_chat_drafts
            .get(&key)
            .and_then(|draft| draft.get("choices"))
            .and_then(|choices| serde_json::from_value(choices.clone()).ok())
            .unwrap_or_default();
        self.write_new_chat_draft(key, "", &[], &choices);
        cx.notify();
    }
}

/// Two drafts' texts as one, the first above.
fn join_drafts(first: &str, second: &str) -> String {
    match (first.trim().is_empty(), second.trim().is_empty()) {
        (_, true) => first.to_owned(),
        (true, false) => second.to_owned(),
        (false, false) if first.trim() == second.trim() => first.to_owned(),
        (false, false) => format!("{}\n\n{}", first.trim_end(), second.trim_start()),
    }
}

/// Adds the files not attached already. All of them, even past the limits:
/// none is dropped unseen, and sending asks for any over them to go first.
fn merge_attachments(into: &mut Vec<Attachment>, more: Vec<Attachment>) {
    for attachment in more {
        if !into.iter().any(|existing| existing.id == attachment.id) {
            into.push(attachment);
        }
    }
}

impl XdDesktop {
    /// The session header while a new chat is open: what the chat will be,
    /// rather than the chat that was open before.
    pub(super) fn render_new_chat_toolbar(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let project = self
            .creating_chat_folder
            .as_ref()
            .and_then(|id| self.model.folders.iter().find(|folder| &folder.id == id))
            .map(|folder| folder.name.clone());
        let session = self
            .creating_chat_session
            .as_ref()
            .and_then(|id| self.model.sessions.iter().find(|session| &session.id == id))
            .map(|session| session.title.clone());
        let starting = self.chat_create_submitting;
        let title = if starting {
            draft_label(
                &self.chat_create_title,
                self.new_chat_submitted_prompt
                    .as_deref()
                    .unwrap_or_default(),
            )
        } else {
            DEFAULT_CHAT_TITLE.to_owned()
        };
        let agent = self.minimal_new_session_agent;
        let chip = || {
            div()
                .h(px(22.0))
                .px(px(8.0))
                .min_w_0()
                .flex_shrink()
                .flex()
                .items_center()
                .gap(px(6.0))
                .rounded_full()
                .bg(rgb(colors.surface_high))
                .text_size(px(11.5))
                .text_color(rgb(colors.muted))
        };
        div()
            .id("new-chat-toolbar")
            .debug_selector(|| "new-chat-toolbar".into())
            .h(px(browser::SESSION_HEADER_HEIGHT))
            .w_full()
            .flex_none()
            .px(px(20.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .bg(rgb(colors.chrome))
            .child(
                div()
                    .min_w_0()
                    .flex_shrink()
                    .truncate()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(colors.text))
                    .child(title),
            )
            .child(
                chip()
                    .flex_none()
                    .child(assistant_icon(
                        agent_icon(agent),
                        13.0,
                        colors.muted,
                        colors,
                    ))
                    .child(presentation::provider_name(agent.protocol_name())),
            )
            .when_some(session.or(project), |toolbar, place| {
                toolbar.child(
                    chip()
                        .child(
                            svg()
                                .path(FOLDER_ICON)
                                .size(px(12.0))
                                .flex_none()
                                .text_color(rgb(colors.muted)),
                        )
                        .child(div().min_w_0().truncate().child(place)),
                )
            })
            .child(div().flex_1())
            .child(if starting {
                stamp("Starting", colors.working)
            } else {
                stamp("Draft", colors.muted)
            })
            .child(
                div().ml_2().flex_none().child(
                    pane_icon_button(
                        "toggle-side-pane",
                        "Toggle side pane",
                        SIDE_LAYOUT_ICON,
                        self.browser_is_open(),
                        colors,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_browser(cx))),
                ),
            )
            .into_any_element()
    }

    /// A project's draft chat and the chats starting in it, at the top of its
    /// sessions in the sidebar.
    pub(super) fn render_draft_chat_rows(
        &mut self,
        colors: ThemeColors,
        project_id: &str,
        project_index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        let in_view = self.creating_chat_folder.as_deref() == Some(project_id);
        // The view's own chat, sent off and not open yet.
        if in_view && (self.chat_create_submitting || self.new_chat_started.is_some()) {
            let label = draft_label(
                &self.chat_create_title,
                self.new_chat_submitted_prompt
                    .as_deref()
                    .unwrap_or_default(),
            );
            rows.push(self.render_starting_row(
                colors,
                ("starting-chat-open", project_index),
                label,
                self.minimal_new_session_agent,
                true,
                None,
                cx,
            ));
        }
        let starting = self
            .starting_chats
            .iter()
            .filter(|chat| chat.endpoint == self.active_endpoint && chat.folder_id == project_id)
            .map(|chat| (chat.token, chat.label(), chat.agent))
            .collect::<Vec<_>>();
        for (token, label, agent) in starting {
            rows.push(self.render_starting_row(
                colors,
                ("starting-chat", token as usize),
                label,
                agent,
                false,
                Some(token),
                cx,
            ));
        }
        if let Some(label) = self.project_draft(project_id) {
            let selected = in_view
                && self.creating_chat_session.is_none()
                && !self.chat_create_submitting
                && self.new_chat_started.is_none();
            let open_project = project_id.to_owned();
            let discard_project = project_id.to_owned();
            let agent = if selected {
                self.minimal_new_session_agent
            } else {
                self.settings
                    .new_chat_drafts
                    .get(&format!("{}/new", self.new_chat_choices_key(project_id)))
                    .and_then(|draft| draft["choices"]["agent"].as_str())
                    .and_then(AgentCli::from_protocol_name)
                    .unwrap_or(self.minimal_new_session_agent)
            };
            rows.push(
                div()
                    .id(("draft-chat", project_index))
                    .debug_selector(|| "draft-chat-row".into())
                    .group("catalog-session")
                    .relative()
                    .w_full()
                    .h(px(CARD_HEIGHT))
                    .flex_none()
                    .pl(px(10.0))
                    .pr(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(px(7.0))
                    .when(selected, |row| row.bg(rgb(colors.selected_surface)))
                    .cursor_pointer()
                    .when(!selected, |row| {
                        row.hover(|style| style.bg(rgb(colors.surface_high)))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.creating_chat_folder.as_deref() != Some(open_project.as_str())
                            || this.creating_chat_session.is_some()
                            || this.chat_create_submitting
                            || this.new_chat_started.is_some()
                        {
                            this.begin_chat_create(open_project.clone(), window, cx);
                        }
                    }))
                    .child(assistant_icon(
                        agent_icon(agent),
                        13.0,
                        if selected { colors.text } else { colors.faint },
                        colors,
                    ))
                    .child(
                        truncating_label(label)
                            .flex_1()
                            .text_size(px(13.0))
                            .text_color(rgb(if selected { colors.text } else { colors.muted }))
                            .when(selected, |title| title.font_weight(FontWeight::MEDIUM)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(rgb(colors.faint))
                            .child("Draft"),
                    )
                    .child(
                        session_row_action(
                            ("discard-draft-chat", project_index),
                            "Discard draft",
                            TRASH_ICON,
                            selected,
                            colors,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.discard_project_draft(&discard_project, cx);
                        })),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    #[allow(clippy::too_many_arguments)]
    fn render_starting_row(
        &mut self,
        colors: ThemeColors,
        id: impl Into<gpui::ElementId>,
        label: String,
        agent: AgentCli,
        selected: bool,
        token: Option<u64>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .id(id)
            .debug_selector(|| "starting-chat-row".into())
            .w_full()
            .h(px(CARD_HEIGHT))
            .flex_none()
            .pl(px(10.0))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(7.0))
            .when(selected, |row| row.bg(rgb(colors.selected_surface)))
            .cursor_pointer()
            .when(!selected, |row| {
                row.hover(|style| style.bg(rgb(colors.surface_high)))
            })
            .when_some(token, |row, token| {
                row.on_click(cx.listener(move |this, _, window, cx| {
                    this.open_starting_chat(token, window, cx);
                }))
            })
            .child(assistant_icon(
                agent_icon(agent),
                13.0,
                if selected { colors.text } else { colors.faint },
                colors,
            ))
            .child(
                truncating_label(label)
                    .flex_1()
                    .text_size(px(13.0))
                    .text_color(rgb(if selected { colors.text } else { colors.muted }))
                    .when(selected, |title| title.font_weight(FontWeight::MEDIUM)),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(11.0))
                    .text_color(rgb(colors.working))
                    .child(
                        div()
                            .size(px(7.0))
                            .rounded_full()
                            .bg(rgb(colors.working))
                            .opacity(pulse_opacity()),
                    )
                    .child("Starting"),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn new_chat_drafts_restore_text_files_and_keep_destinations_separate(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            desktop.creating_chat_folder = Some("project".into());
            desktop.new_chat_prompt = "Unsent message".into();
            desktop.new_chat_attachments =
                vec![Attachment::from_file("notes.txt", b"notes".to_vec())];
            desktop.new_chat_remembered.effort = Some("high".into());
            desktop.persist_new_chat_draft();
            desktop.creating_chat_session = Some("session".into());
            desktop.restore_new_chat_draft(cx);
            assert!(desktop.new_chat_prompt.is_empty());
            assert!(desktop.new_chat_attachments.is_empty());
            desktop.creating_chat_session = None;
            desktop.restore_new_chat_draft(cx);
            assert_eq!(desktop.new_chat_prompt, "Unsent message");
            assert_eq!(desktop.new_chat_attachments[0].name, "notes.txt");
            assert_eq!(desktop.new_chat_remembered.effort.as_deref(), Some("high"));
            desktop.clear_new_chat_draft();
            desktop.restore_new_chat_draft(cx);
            assert!(desktop.new_chat_prompt.is_empty());
            assert!(desktop.new_chat_attachments.is_empty());
        });
    }

    #[gpui::test]
    fn disconnect_and_endpoint_switch_release_chat_creation(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            desktop.chat_create_submitting = true;
            desktop.chat_create_token = Some(1);
            desktop.new_chat_submitted_prompt = Some("Build a footer".into());
            assert!(desktop.switch_active_endpoint(ChatEndpoint::Remote, cx));
            assert!(!desktop.chat_create_submitting);
            assert!(desktop.new_chat_submitted_prompt.is_none());
            desktop.creating_chat_folder = Some("project".into());
            desktop.new_chat_prompt = "Keep this prompt".into();
            desktop.chat_create_submitting = true;
            desktop.chat_create_token = Some(1);
            desktop.new_chat_submitted_prompt = Some(desktop.new_chat_prompt.clone());
            let generation = desktop.remote_generation;
            desktop.handle_remote_update(
                HostUpdate::Disconnected {
                    message: "test disconnect".into(),
                },
                generation,
                cx,
            );
            assert!(!desktop.chat_create_submitting);
            assert!(desktop.new_chat_submitted_prompt.is_none());
            assert_eq!(desktop.new_chat_prompt, "Keep this prompt");
            assert_eq!(desktop.creating_chat_folder.as_deref(), Some("project"));
        });
    }

    #[gpui::test]
    fn reconnect_discards_file_results_without_leaving_send_blocked(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            desktop.creating_chat_folder = Some("project".into());
            desktop.new_chat_reading = 1;
            let connection = desktop.attachment_connection();
            desktop.connection_generation += 1;
            desktop.finish_new_chat_file_read(
                desktop.new_chat_generation,
                connection,
                vec![Err("stale read must be discarded".into())],
                cx,
            );
            assert_eq!(desktop.new_chat_reading, 0);
            assert!(desktop.model.connection_error.is_none());
            desktop.new_chat_generation += 1;
            desktop.new_chat_reading = 1;
            desktop.finish_new_chat_file_read(
                desktop.new_chat_generation - 1,
                connection,
                vec![],
                cx,
            );
            assert_eq!(
                desktop.new_chat_reading, 1,
                "an old draft cannot consume a new draft's read count"
            );
        });
    }

    #[gpui::test]
    fn draft_defaults_to_repository_base_and_failed_creation_keeps_prompt(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            desktop.new_chat_default_branch = Some("main".into());
            desktop.chat_create_branches = serde_json::from_value(serde_json::json!([
                {"name":"main", "reference":"refs/heads/main", "oid":"abc"},
                {"name":"feature", "reference":"refs/heads/feature", "oid":"def"}
            ])).unwrap();
            assert!(matches!(desktop.default_new_chat_base(), Some(NewSessionWorktree::Branch {reference, ..}) if reference == "refs/heads/main"));
            desktop.chat_create_branches[0].name = "origin/main".into();
            desktop.chat_create_branches[0].reference = "refs/remotes/origin/main".into();
            assert!(matches!(desktop.default_new_chat_base(), Some(NewSessionWorktree::Branch {reference, ..}) if reference == "refs/remotes/origin/main"));
            desktop.creating_chat_folder = Some("project".into());
            desktop.new_chat_prompt = "Build a footer".into();
            desktop.chat_create_worktrees_loading = false;
            desktop.chat_create_worktree = Some(NewSessionWorktree::New);
            desktop.chat_create_sources_loading = true;
            desktop.save_minimal_chat_create(cx);
            assert!(desktop.model.connection_error.is_none(), "Enter must wait for base branches");
            desktop.chat_create_sources_loading = false;
            desktop.save_minimal_chat_create(cx);
            assert!(!desktop.chat_create_submitting);
            assert_eq!(desktop.new_chat_prompt, "Build a footer");
            assert_eq!(desktop.creating_chat_folder.as_deref(), Some("project"));
            assert!(desktop.model.chats.is_empty());
        });
    }
}

#[cfg(test)]
mod choice_tests {
    use super::*;

    fn reply(
        desktop: &mut XdDesktop,
        kind: RequestKind,
        body: serde_json::Value,
        cx: &mut Context<XdDesktop>,
    ) {
        desktop.handle_reply(kind, body.as_object().unwrap().clone(), None, cx);
    }

    fn folder_settings(desktop: &mut XdDesktop, folder: &str, cx: &mut Context<XdDesktop>) {
        reply(
            desktop,
            RequestKind::FolderSettings {
                folder_id: folder.into(),
            },
            serde_json::json!({
                "ok": true,
                "effective_workdir": "/repo",
                "can_create_worktree": true,
                "worktrees": [
                    {"path": "/repo", "branch": "main", "detached": false, "main": true, "current": true},
                    {"path": "/worktrees/repo-1", "branch": "spike", "detached": false, "main": false, "current": false}
                ]
            }),
            cx,
        );
    }

    fn sources(desktop: &mut XdDesktop, folder: &str, cx: &mut Context<XdDesktop>) {
        sources_with_default(desktop, folder, "main", cx);
    }

    fn sources_with_default(
        desktop: &mut XdDesktop,
        folder: &str,
        default_branch: &str,
        cx: &mut Context<XdDesktop>,
    ) {
        reply(
            desktop,
            RequestKind::NewChatSources {
                folder_id: folder.into(),
            },
            serde_json::json!({
                "ok": true,
                "default_branch": default_branch,
                "branches": [
                    {"name": "main", "reference": "refs/heads/main", "oid": "aaa"},
                    {"name": "origin/dev", "reference": "refs/remotes/origin/dev", "oid": "bbb"}
                ]
            }),
            cx,
        );
    }

    fn dev() -> Option<NewSessionWorktree> {
        Some(NewSessionWorktree::Branch {
            reference: "refs/remotes/origin/dev".into(),
            oid: "bbb".into(),
        })
    }

    #[gpui::test]
    fn a_projects_next_new_chat_starts_the_way_its_last_one_was_set_up(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.model.agent_backends = vec![xd_desktop::model::AgentBackend {
                    id: "claude".into(),
                    name: "Claude".into(),
                    default_model: "claude-sonnet-5".into(),
                    models: vec![xd_desktop::model::AgentModel {
                        id: "claude-opus-5".into(),
                        name: "Opus 5".into(),
                    }],
                    efforts: Vec::new(),
                }];
                desktop.begin_chat_create("project".into(), window, cx);
                folder_settings(desktop, "project", cx);
                sources(desktop, "project", cx);
                assert_eq!(desktop.minimal_new_session_agent, AgentCli::Codex);
                assert!(matches!(
                    desktop.chat_create_worktree,
                    Some(NewSessionWorktree::Existing(ref path)) if path == "/repo"
                ));

                // The picks, as the pickers make them.
                desktop.minimal_new_session_agent = AgentCli::Claude;
                desktop.new_chat_model = Some("claude-opus-5".into());
                desktop.remember_new_chat_choices();
                desktop.chat_create_worktree = dev();
                desktop.new_chat_remote = true;
                desktop.remember_new_chat_choices();
                desktop.cancel_chat_create(cx);

                desktop.begin_chat_create("project".into(), window, cx);
                assert_eq!(desktop.minimal_new_session_agent, AgentCli::Claude);
                assert_eq!(desktop.new_chat_model.as_deref(), Some("claude-opus-5"));
                assert!(desktop.new_chat_remote);
                // The branches may arrive before the worktrees, or after.
                sources(desktop, "project", cx);
                folder_settings(desktop, "project", cx);
                assert_eq!(desktop.chat_create_worktree, dev());
                desktop.cancel_chat_create(cx);

                desktop.begin_chat_create("project".into(), window, cx);
                folder_settings(desktop, "project", cx);
                assert_eq!(desktop.chat_create_worktree, Some(NewSessionWorktree::New));
                sources(desktop, "project", cx);
                assert_eq!(desktop.chat_create_worktree, dev());

                // A base that is gone, with a default branch that is not
                // among the branches either, falls back to the current
                // checkout's branch whichever reply comes first.
                desktop.new_chat_remembered.base = Some("refs/heads/gone".into());
                for sources_first in [false, true] {
                    desktop.chat_create_worktree = Some(NewSessionWorktree::New);
                    desktop.chat_create_worktrees.clear();
                    desktop.chat_create_branches.clear();
                    desktop.chat_create_sources_loading = true;
                    if sources_first {
                        sources_with_default(desktop, "project", "trunk", cx);
                        folder_settings(desktop, "project", cx);
                    } else {
                        folder_settings(desktop, "project", cx);
                        sources_with_default(desktop, "project", "trunk", cx);
                    }
                    assert_eq!(
                        desktop.chat_create_worktree,
                        Some(NewSessionWorktree::Branch {
                            reference: "refs/heads/main".into(),
                            oid: "aaa".into(),
                        }),
                        "sources first: {sources_first}"
                    );
                }
                desktop.new_chat_remembered.base = Some("refs/remotes/origin/dev".into());
                desktop.cancel_chat_create(cx);

                // Current Worktree picked before the worktrees arrive stays
                // picked, and is what the next new chat starts with.
                desktop.begin_chat_create("project".into(), window, cx);
                // Even over a checkout remembered from before.
                desktop.new_chat_remembered.checkout = Some("/worktrees/repo-1".into());
                desktop.new_chat_worktree_picked = Some(false);
                desktop.chat_create_worktree = None;
                desktop.remember_new_chat_choices();
                folder_settings(desktop, "project", cx);
                sources(desktop, "project", cx);
                assert!(matches!(
                    desktop.chat_create_worktree,
                    Some(NewSessionWorktree::Existing(ref path)) if path == "/repo"
                ));
                desktop.cancel_chat_create(cx);
                desktop.begin_chat_create("project".into(), window, cx);
                assert!(!desktop.new_chat_remembered.new_worktree);
                assert!(desktop.new_chat_remembered.checkout.is_none());
                // And New Worktree with a base picked early keeps that base.
                sources(desktop, "project", cx);
                desktop.new_chat_worktree_picked = Some(true);
                desktop.chat_create_worktree = Some(NewSessionWorktree::Branch {
                    reference: "refs/heads/main".into(),
                    oid: "aaa".into(),
                });
                folder_settings(desktop, "project", cx);
                assert_eq!(
                    desktop.chat_create_worktree,
                    Some(NewSessionWorktree::Branch {
                        reference: "refs/heads/main".into(),
                        oid: "aaa".into(),
                    })
                );
                desktop.chat_create_worktree = dev();
                desktop.remember_new_chat_choices();

                // Back to a checkout of the project.
                desktop.chat_create_worktree =
                    Some(NewSessionWorktree::Existing("/worktrees/repo-1".into()));
                desktop.remember_new_chat_choices();
                desktop.cancel_chat_create(cx);
                desktop.begin_chat_create("project".into(), window, cx);
                folder_settings(desktop, "project", cx);
                sources(desktop, "project", cx);
                assert!(matches!(
                    desktop.chat_create_worktree,
                    Some(NewSessionWorktree::Existing(ref path)) if path == "/worktrees/repo-1"
                ));
                desktop.cancel_chat_create(cx);

                // Another project keeps its own, and a model the agent no
                // longer offers is not carried over.
                desktop.begin_chat_create("other".into(), window, cx);
                assert!(desktop.new_chat_remote);
                assert!(desktop.new_chat_model.is_none());
                folder_settings(desktop, "other", cx);
                assert!(matches!(
                    desktop.chat_create_worktree,
                    Some(NewSessionWorktree::Existing(ref path)) if path == "/repo"
                ));
                desktop.cancel_chat_create(cx);
                desktop.model.agent_backends.clear();
                desktop.begin_chat_create("project".into(), window, cx);
                assert_eq!(desktop.minimal_new_session_agent, AgentCli::Claude);
                assert!(desktop.new_chat_model.is_none());
            });
        });
    }
}

#[cfg(test)]
mod start_tests {
    use super::*;

    #[gpui::test]
    fn opening_a_new_chat_closes_the_browser(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        let open_browser = |desktop: &Entity<XdDesktop>, cx: &mut gpui::VisualTestContext| {
            desktop.update(cx, |desktop, cx| {
                desktop.set_browser_open(true);
                cx.notify();
            });
            cx.run_until_parked();
            desktop.update(cx, |desktop, cx| {
                let pane = desktop.browser_panes[&desktop.browser_scope_key()].clone();
                assert!(pane.read(cx).is_visible());
                pane
            })
        };
        let assert_closed = |desktop: &Entity<XdDesktop>,
                             pane: &Entity<BrowserPane>,
                             cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| {
                let desktop = desktop.read(cx);
                assert!(desktop.creating_chat_folder.is_some());
                assert!(!desktop.browser_is_open());
                assert!(!pane.read(cx).is_visible());
                assert!(
                    desktop
                        .new_chat_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window),
                    "the message box keeps the focus"
                );
            });
        };
        desktop.update(cx, |desktop, cx| {
            desktop.active_endpoint = ChatEndpoint::Local;
            desktop.settings.browser_sessions.clear();
            desktop.model = crate::tests::grouped_workspace_fixture();
            desktop.model.select_chat("review");
            desktop.minimal_route = MinimalRoute::Cli {
                project_id: "atlas".into(),
                chat_id: "review".into(),
                agent: AgentCli::Claude,
            };
            cx.notify();
        });

        // From a project's new chat button.
        let pane = open_browser(&desktop, cx);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.begin_chat_create("atlas".into(), window, cx)
            })
        });
        assert_closed(&desktop, &pane, cx);
        // Leaving the new chat does not bring it back.
        desktop.update(cx, |desktop, cx| desktop.cancel_chat_create(cx));
        cx.run_until_parked();
        desktop.read_with(cx, |desktop, _| assert!(!desktop.browser_is_open()));

        // From a session's new chat button.
        let pane = open_browser(&desktop, cx);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.begin_session_chat_create(window, cx)
            })
        });
        assert_closed(&desktop, &pane, cx);

        // A browser closed by its toggle just before, with no frame drawn in
        // between, does not take the focus from the message box either.
        desktop.update(cx, |desktop, cx| desktop.cancel_chat_create(cx));
        let pane = open_browser(&desktop, cx);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.toggle_browser(cx);
                desktop.begin_chat_create("atlas".into(), window, cx)
            })
        });
        assert_closed(&desktop, &pane, cx);
    }

    #[gpui::test]
    fn a_new_chat_opens_under_the_empty_chat_heading(cx: &mut gpui::TestAppContext) {
        install_embedded_fonts(cx.text_system()).unwrap();
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            desktop.model = crate::tests::grouped_workspace_fixture();
            desktop.creating_chat_folder = Some("atlas".into());
            cx.notify();
        });
        cx.simulate_resize(size(px(1200.0), px(800.0)));
        cx.run_until_parked();

        let heading = cx
            .debug_bounds("empty-chat-heading")
            .expect("the heading shows");
        let description = cx.debug_bounds("empty-chat-description").unwrap();
        let composer = cx.debug_bounds("new-chat-composer").unwrap();
        assert!(heading.bottom() <= description.top() && description.bottom() < composer.top());
        assert!((heading.center().x - composer.center().x).abs() < px(1.0));
    }

    #[gpui::test]
    fn a_new_chat_has_its_own_header_and_sidebar_rows(cx: &mut gpui::TestAppContext) {
        install_embedded_fonts(cx.text_system()).unwrap();
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                sent_off(desktop);
                desktop.model.select_chat("review");
                desktop.minimal_route = MinimalRoute::Cli {
                    project_id: "atlas".into(),
                    chat_id: "review".into(),
                    agent: AgentCli::Claude,
                };
                // Left while it starts, then a new chat with a draft.
                desktop.begin_chat_create("atlas".into(), window, cx);
                desktop.new_chat_prompt = "Then the header".into();
                desktop.persist_new_chat_draft();
                cx.notify();
            })
        });
        cx.simulate_resize(size(px(1200.0), px(800.0)));
        cx.run_until_parked();
        // The chat open before does not head the new one.
        let header = cx
            .debug_bounds("new-chat-toolbar")
            .expect("the new chat's header shows");
        let composer = cx.debug_bounds("new-chat-composer").unwrap();
        assert!(header.bottom() <= composer.top());
        let draft = cx.debug_bounds("draft-chat-row").expect("the draft shows");
        let starting = cx
            .debug_bounds("starting-chat-row")
            .expect("the starting chat shows");
        assert!(starting.bottom() <= draft.top());
        assert!(draft.right() < header.left() + px(1.0));
        desktop.read_with(cx, |desktop, _| {
            assert_eq!(
                desktop.project_draft("atlas").as_deref(),
                Some("Then the header")
            );
        });
    }

    #[gpui::test]
    fn a_new_chat_shows_its_message_until_the_chat_opens(cx: &mut gpui::TestAppContext) {
        install_embedded_fonts(cx.text_system()).unwrap();
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            desktop.model = crate::tests::grouped_workspace_fixture();
            desktop.creating_chat_folder = Some("atlas".into());
            desktop.chat_create_title = "New Chat".into();
            desktop.new_chat_prompt = "Fix the footer links".into();
            desktop.new_chat_submitted_prompt = Some("Fix the footer links".into());
            desktop.chat_create_worktree = Some(NewSessionWorktree::Branch {
                reference: "refs/heads/dev".into(),
                oid: "abc".into(),
            });
            desktop.chat_create_submitting = true;
            desktop.chat_create_token = Some(1);
            cx.notify();
        });
        cx.simulate_resize(size(px(1200.0), px(800.0)));
        cx.run_until_parked();

        let sent = cx.debug_bounds("new-chat-sent").expect("the message shows");
        let status = cx
            .debug_bounds("new-chat-status")
            .expect("the status shows");
        let composer = cx.debug_bounds("new-chat-composer").unwrap();
        // In the conversation above the composer, as the chat will show it.
        assert!(sent.bottom() <= status.top() && status.bottom() < composer.top());
        assert!(sent.right() > composer.center().x);
        desktop.read_with(cx, |desktop, _| {
            assert_eq!(
                desktop.new_chat_status(),
                "Creating a worktree from the latest dev"
            );
            assert!(desktop.anything_working());
        });

        desktop.update(cx, |desktop, cx| {
            desktop.handle_reply(
                RequestKind::NewChat {
                    folder_id: "atlas".into(),
                    title: "New Chat".into(),
                    workdir: None,
                    token: 1,
                },
                serde_json::json!({"ok": true, "id": "implementation"})
                    .as_object()
                    .unwrap()
                    .clone(),
                None,
                cx,
            );
            // Still on screen until the chat it made opens in its place.
            assert_eq!(desktop.creating_chat_folder.as_deref(), Some("atlas"));
            assert!(desktop.chat_create_submitting);
            assert_eq!(desktop.new_chat_status(), "Opening the chat");
            let prompt = &desktop.first_chat_prompts["implementation"];
            assert_eq!(prompt.text, "Fix the footer links");
            assert_eq!(prompt.endpoint, ChatEndpoint::Local);

            // Nothing waits on it any more: another chat can open, or
            // another new chat start.
            assert!(!desktop.chat_create_in_flight());

            desktop.reconcile_minimal_navigation(cx);
            assert!(desktop.creating_chat_folder.is_none());
            assert!(!desktop.chat_create_submitting);
            assert!(desktop.new_chat_started.is_none());
            assert_eq!(
                desktop.model.selected_chat.as_deref(),
                Some("implementation")
            );
        });
    }

    #[gpui::test]
    fn another_chat_opens_while_a_created_one_waits_for_the_tree(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.model = crate::tests::grouped_workspace_fixture();
                desktop.creating_chat_folder = Some("atlas".into());
                desktop.chat_create_title = "New Chat".into();
                desktop.new_chat_submitted_prompt = Some("First".into());
                desktop.chat_create_submitting = true;
                desktop.chat_create_token = Some(1);
                desktop.handle_reply(
                    RequestKind::NewChat {
                        folder_id: "atlas".into(),
                        title: "New Chat".into(),
                        workdir: None,
                        token: 1,
                    },
                    serde_json::json!({"ok": true, "id": "implementation"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    None,
                    cx,
                );
                assert!(desktop.new_chat_started.is_some());
                desktop.open_minimal_session(
                    "atlas".into(),
                    "review".into(),
                    AgentCli::Claude,
                    window,
                    cx,
                );
                assert!(desktop.creating_chat_folder.is_none());
                assert!(!desktop.chat_create_submitting);
                assert_eq!(desktop.model.selected_chat.as_deref(), Some("review"));
                // The tree that would have opened the created chat does not
                // take the reader away from the one picked.
                desktop.reconcile_minimal_navigation(cx);
                assert_eq!(desktop.model.selected_chat.as_deref(), Some("review"));

                // And a new chat can start while one waits.
                desktop.creating_chat_folder = Some("atlas".into());
                desktop.chat_create_submitting = true;
                desktop.chat_create_token = Some(1);
                desktop.new_chat_started = Some("made".into());
                desktop.begin_chat_create("atlas".into(), window, cx);
                assert!(!desktop.chat_create_submitting);
                assert!(desktop.new_chat_started.is_none());
                assert_eq!(desktop.creating_chat_folder.as_deref(), Some("atlas"));
                // Its first message, and the first chat's, both wait to be sent.
                desktop.chat_create_title = "New Chat".into();
                desktop.new_chat_submitted_prompt = Some("Second".into());
                desktop.chat_create_submitting = true;
                desktop.chat_create_token = Some(1);
                desktop.handle_reply(
                    RequestKind::NewChat {
                        folder_id: "atlas".into(),
                        title: "New Chat".into(),
                        workdir: None,
                        token: 1,
                    },
                    serde_json::json!({"ok": true, "id": "second"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    None,
                    cx,
                );
                assert_eq!(desktop.first_chat_prompts["implementation"].text, "First");
                assert_eq!(desktop.first_chat_prompts["second"].text, "Second");
            });
        });
    }

    fn reply(
        desktop: &mut XdDesktop,
        kind: RequestKind,
        body: serde_json::Value,
        cx: &mut Context<XdDesktop>,
    ) {
        desktop.handle_reply(kind, body.as_object().unwrap().clone(), None, cx);
    }

    /// The view of a new chat sent off as "Fix the footer" with token 1, not
    /// answered yet.
    fn sent_off(desktop: &mut XdDesktop) {
        desktop.model = crate::tests::grouped_workspace_fixture();
        desktop.creating_chat_folder = Some("atlas".into());
        desktop.chat_create_title = "New Chat".into();
        desktop.new_chat_prompt = "Fix the footer".into();
        desktop.new_chat_submitted_prompt = Some("Fix the footer".into());
        desktop.chat_create_worktree = Some(NewSessionWorktree::New);
        desktop.chat_create_submitting = true;
        desktop.chat_create_token = Some(1);
        desktop.new_chat_tokens = 1;
    }

    fn made(token: u64) -> RequestKind {
        RequestKind::NewChat {
            folder_id: "atlas".into(),
            title: "New Chat".into(),
            workdir: None,
            token,
        }
    }

    #[gpui::test]
    fn a_chat_left_while_it_starts_is_made_without_taking_the_reader_back(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                sent_off(desktop);
                // Another chat opens while it starts.
                desktop.open_minimal_session(
                    "atlas".into(),
                    "review".into(),
                    AgentCli::Claude,
                    window,
                    cx,
                );
                assert!(desktop.creating_chat_folder.is_none());
                assert!(!desktop.chat_create_submitting);
                assert_eq!(desktop.model.selected_chat.as_deref(), Some("review"));
                assert_eq!(desktop.starting_chats.len(), 1);
                assert_eq!(desktop.starting_chats[0].label(), "Fix the footer");

                // So does another new chat in the same project, empty.
                desktop.begin_chat_create("atlas".into(), window, cx);
                assert_eq!(desktop.creating_chat_folder.as_deref(), Some("atlas"));
                assert!(!desktop.chat_create_submitting);
                assert!(desktop.new_chat_prompt.is_empty());
                desktop.cancel_chat_create(cx);

                reply(
                    desktop,
                    made(1),
                    serde_json::json!({"ok": true, "id": "footer"}),
                    cx,
                );
                assert!(desktop.starting_chats.is_empty());
                // Its first message goes out (here, with no host, it waits
                // for one) and the chat picked stays open.
                assert_eq!(desktop.first_chat_prompts["footer"].text, "Fix the footer");
                assert!(desktop.pending_minimal_session.is_none());
                desktop.reconcile_minimal_navigation(cx);
                assert_eq!(desktop.model.selected_chat.as_deref(), Some("review"));
            });
        });
    }

    #[gpui::test]
    fn a_chat_that_is_not_made_puts_its_message_back_beside_a_newer_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                sent_off(desktop);
                desktop.begin_chat_create("atlas".into(), window, cx);
                desktop.new_chat_prompt = "Then the header".into();
                desktop.persist_new_chat_draft();
                reply(
                    desktop,
                    made(1),
                    serde_json::json!({"ok": false, "error": "No such branch."}),
                    cx,
                );
                assert!(desktop.starting_chats.is_empty());
                assert_eq!(desktop.new_chat_prompt, "Fix the footer\n\nThen the header");
                assert!(
                    desktop
                        .model
                        .connection_error
                        .as_deref()
                        .is_some_and(|error| error.contains("No such branch."))
                );

                // Left for another chat, it goes back to the saved draft.
                desktop.new_chat_prompt = "Only this".into();
                desktop.persist_new_chat_draft();
                desktop.new_chat_tokens = 1;
                desktop.cancel_chat_create(cx);
                sent_off(desktop);
                desktop.chat_create_token = Some(2);
                desktop.open_minimal_session(
                    "atlas".into(),
                    "review".into(),
                    AgentCli::Claude,
                    window,
                    cx,
                );
                reply(
                    desktop,
                    made(2),
                    serde_json::json!({"ok": false, "error": "Nope."}),
                    cx,
                );
                let key = format!("{}/new", desktop.new_chat_choices_key("atlas"));
                assert_eq!(
                    desktop.settings.new_chat_drafts[&key]["text"],
                    "Fix the footer\n\nOnly this"
                );
                assert_eq!(
                    desktop.project_draft("atlas").as_deref(),
                    Some("Fix the footer")
                );
                assert_eq!(desktop.model.selected_chat.as_deref(), Some("review"));
            });
        });
    }

    #[gpui::test]
    fn a_starting_chat_can_be_opened_again_and_opens_once_made(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                sent_off(desktop);
                desktop.open_minimal_session(
                    "atlas".into(),
                    "review".into(),
                    AgentCli::Claude,
                    window,
                    cx,
                );
                desktop.open_starting_chat(1, window, cx);
                assert!(desktop.starting_chats.is_empty());
                assert_eq!(desktop.creating_chat_folder.as_deref(), Some("atlas"));
                assert!(desktop.chat_create_submitting);
                assert_eq!(desktop.chat_create_token, Some(1));
                assert_eq!(
                    desktop.new_chat_submitted_prompt.as_deref(),
                    Some("Fix the footer")
                );
                assert_eq!(desktop.new_chat_status(), "Creating a worktree");
                reply(
                    desktop,
                    made(1),
                    serde_json::json!({"ok": true, "id": "footer"}),
                    cx,
                );
                assert_eq!(desktop.new_chat_started.as_deref(), Some("footer"));
                assert_eq!(
                    desktop.pending_minimal_session,
                    Some(("atlas".into(), "footer".into()))
                );
                // Another new chat before the tree arrives: the made one does
                // not open over it.
                desktop.begin_chat_create("atlas".into(), window, cx);
                assert!(desktop.pending_minimal_session.is_none());
                assert_eq!(desktop.first_chat_prompts["footer"].text, "Fix the footer");
                desktop.reconcile_minimal_navigation(cx);
                assert_eq!(desktop.creating_chat_folder.as_deref(), Some("atlas"));
                assert_ne!(desktop.model.selected_chat.as_deref(), Some("footer"));
            });
        });
    }

    #[gpui::test]
    fn closing_xd_while_watching_a_starting_chat_keeps_the_newer_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                sent_off(desktop);
                desktop.begin_chat_create("atlas".into(), window, cx);
                desktop.new_chat_prompt = "Then the header".into();
                desktop.persist_new_chat_draft();
                // Back to watch the first one start, then xd closes.
                desktop.open_starting_chat(1, window, cx);
                desktop.keep_starting_messages(cx);
                let key = format!("{}/new", desktop.new_chat_choices_key("atlas"));
                assert_eq!(
                    desktop.settings.new_chat_drafts[&key]["text"],
                    "Fix the footer\n\nThen the header"
                );
                assert!(desktop.starting_chats.is_empty());
            });
        });
    }

    #[cfg(unix)]
    #[gpui::test]
    fn a_starting_chat_that_fails_after_reopening_lists_its_checkouts_again(
        cx: &mut gpui::TestAppContext,
    ) {
        use std::io::{BufRead, BufReader, Write};
        let (client, server_end) = std::os::unix::net::UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = server_end;
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut asked = Vec::new();
            while !(asked.iter().any(|op: &String| op == "folder-settings")
                && asked.iter().any(|op| op == "new-chat-sources"))
            {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if request["folder"] == "atlas" {
                    asked.push(request["op"].as_str().unwrap_or_default().to_owned());
                }
                if let Some(id) = request["_xd_request"].as_u64() {
                    writeln!(stream, "{{\"ok\":false,\"_xd_request\":{id}}}").unwrap();
                }
            }
            asked
        });
        let (host, _updates) = HostHandle::connect_io(
            client.try_clone().unwrap(),
            client,
            std::env::temp_dir().join("xd-reopened-sources-identity"),
        );
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.host = Some(host);
                sent_off(desktop);
                desktop.open_minimal_session(
                    "atlas".into(),
                    "review".into(),
                    AgentCli::Claude,
                    window,
                    cx,
                );
                desktop.open_starting_chat(1, window, cx);
                assert!(desktop.chat_create_worktrees.is_empty());
                reply(
                    desktop,
                    made(1),
                    serde_json::json!({"ok": false, "error": "No such branch."}),
                    cx,
                );
                assert!(!desktop.chat_create_submitting);
                assert!(desktop.chat_create_worktrees_loading);
                assert!(desktop.chat_create_sources_loading);
            });
        });
        let asked = server.join().unwrap();
        assert!(asked.iter().any(|op| op == "folder-settings"), "{asked:?}");
        assert!(asked.iter().any(|op| op == "new-chat-sources"), "{asked:?}");
    }

    #[gpui::test]
    fn a_disconnect_returns_starting_chats_to_their_drafts(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                sent_off(desktop);
                desktop.open_minimal_session(
                    "atlas".into(),
                    "review".into(),
                    AgentCli::Claude,
                    window,
                    cx,
                );
                let generation = desktop.connection_generation;
                desktop.handle_host_update(
                    HostUpdate::Disconnected {
                        message: "test disconnect".into(),
                    },
                    generation,
                    cx,
                );
                assert!(desktop.starting_chats.is_empty());
                assert_eq!(
                    desktop.project_draft("atlas").as_deref(),
                    Some("Fix the footer")
                );
            });
        });
    }

    /// A chat is created, then another one is picked, before the created
    /// chat opens or, once it has, before its state arrives.
    #[cfg(unix)]
    #[derive(Clone, Copy)]
    enum Leave {
        /// For another chat, before the created one opens.
        BeforeOpening,
        /// For another chat, once it has opened but before its state arrives.
        BeforeItsState,
        /// For somewhere that is not a chat, such as the terminal, before its
        /// state arrives.
        ForNoChat,
    }

    #[cfg(unix)]
    fn leave_a_created_chat(cx: &mut gpui::TestAppContext, leave: Leave) {
        use std::io::{BufRead, BufReader, Write};
        let (client, server_end) = std::os::unix::net::UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = server_end;
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut seen = Vec::new();
            while seen.len() < 2 {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                let op = request["op"].as_str().unwrap_or_default().to_owned();
                if matches!(op.as_str(), "set-draft" | "send") {
                    seen.push((
                        op,
                        request["chat"].as_str().unwrap_or_default().to_owned(),
                        request["text"].as_str().unwrap_or_default().to_owned(),
                    ));
                }
                if let Some(id) = request["_xd_request"].as_u64() {
                    writeln!(stream, "{{\"ok\":true,\"_xd_request\":{id}}}").unwrap();
                }
            }
            seen
        });
        let (host, _updates) = HostHandle::connect_io(
            client.try_clone().unwrap(),
            client,
            std::env::temp_dir().join("xd-first-aside-identity"),
        );

        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.host = Some(host);
                desktop.model = crate::tests::grouped_workspace_fixture();
                desktop.creating_chat_folder = Some("atlas".into());
                desktop.chat_create_title = "New Chat".into();
                desktop.new_chat_submitted_prompt = Some("Fix the footer".into());
                desktop.chat_create_submitting = true;
                desktop.chat_create_token = Some(1);
                desktop.handle_reply(
                    RequestKind::NewChat {
                        folder_id: "atlas".into(),
                        title: "New Chat".into(),
                        workdir: None,
                        token: 1,
                    },
                    serde_json::json!({"ok": true, "id": "implementation"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    None,
                    cx,
                );
                if !matches!(leave, Leave::BeforeOpening) {
                    desktop.reconcile_minimal_navigation(cx);
                    assert_eq!(
                        desktop.model.selected_chat.as_deref(),
                        Some("implementation")
                    );
                    assert!(desktop.first_chat_prompts.contains_key("implementation"));
                }
                if matches!(leave, Leave::ForNoChat) {
                    // Whatever route did it, the chat is no longer the open one
                    // when its state arrives.
                    desktop.model.selected_chat = None;
                    desktop.handle_reply(
                        RequestKind::Chat {
                            chat_id: "implementation".into(),
                        },
                        serde_json::json!({"ok": true}).as_object().unwrap().clone(),
                        None,
                        cx,
                    );
                } else {
                    desktop.open_minimal_session(
                        "atlas".into(),
                        "review".into(),
                        AgentCli::Claude,
                        window,
                        cx,
                    );
                }
                assert!(desktop.first_chat_prompts.is_empty());
            });
        });
        let seen = server.join().unwrap();
        let seen = seen
            .iter()
            .map(|(op, chat, text)| (op.as_str(), chat.as_str(), text.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            seen,
            [
                ("set-draft", "implementation", "Fix the footer"),
                ("send", "implementation", "Fix the footer"),
            ]
        );
    }

    #[cfg(unix)]
    #[gpui::test]
    fn a_created_chat_left_before_it_opens_still_sends_its_first_message(
        cx: &mut gpui::TestAppContext,
    ) {
        leave_a_created_chat(cx, Leave::BeforeOpening);
    }

    #[cfg(unix)]
    #[gpui::test]
    fn a_created_chat_left_before_its_state_arrives_still_sends_its_first_message(
        cx: &mut gpui::TestAppContext,
    ) {
        leave_a_created_chat(cx, Leave::BeforeItsState);
    }

    #[cfg(unix)]
    #[gpui::test]
    fn a_created_chat_left_for_no_chat_still_sends_its_first_message(
        cx: &mut gpui::TestAppContext,
    ) {
        leave_a_created_chat(cx, Leave::ForNoChat);
    }

    #[cfg(unix)]
    #[gpui::test]
    fn a_waiting_first_message_goes_only_to_its_chats_host(cx: &mut gpui::TestAppContext) {
        use std::io::{BufRead, BufReader, Write};
        let (client, server_end) = std::os::unix::net::UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = server_end;
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    return None;
                }
                let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if let Some(id) = request["_xd_request"].as_u64() {
                    writeln!(stream, "{{\"ok\":true,\"_xd_request\":{id}}}").unwrap();
                }
                if request["op"] == "send" {
                    return request["chat"].as_str().map(str::to_owned);
                }
            }
        });
        let (host, _updates) = HostHandle::connect_io(
            client.try_clone().unwrap(),
            client,
            std::env::temp_dir().join("xd-first-host-identity"),
        );
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, _| {
            desktop.host = Some(host);
            for (chat, endpoint) in [
                ("on-remote", ChatEndpoint::Remote),
                ("on-local", ChatEndpoint::Local),
            ] {
                desktop.first_chat_prompts.insert(
                    chat.into(),
                    FirstChatPrompt {
                        endpoint,
                        text: "First".into(),
                        attachments: Vec::new(),
                    },
                );
            }
            desktop.send_first_messages_aside(None);
            // The remote chat's host is not connected: its message waits.
            assert!(desktop.first_chat_prompts.contains_key("on-remote"));
            assert!(!desktop.first_chat_prompts.contains_key("on-local"));
        });
        assert_eq!(server.join().unwrap().as_deref(), Some("on-local"));
    }

    #[gpui::test]
    fn a_created_chat_is_not_offered_again_after_a_disconnect(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            assert!(desktop.switch_active_endpoint(ChatEndpoint::Remote, cx));
            desktop.creating_chat_folder = Some("project".into());
            desktop.chat_create_submitting = true;
            desktop.chat_create_token = Some(1);
            desktop.new_chat_started = Some("made".into());
            let generation = desktop.remote_generation;
            desktop.handle_remote_update(
                HostUpdate::Disconnected {
                    message: "test disconnect".into(),
                },
                generation,
                cx,
            );
            assert!(!desktop.chat_create_submitting);
            assert!(desktop.new_chat_started.is_none());
            assert!(desktop.creating_chat_folder.is_none());
        });
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    #[gpui::test]
    fn first_message_carries_images_through_chat_creation(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgba8(1, 1)
                .write_to(&mut png, image::ImageFormat::Png)
                .unwrap();
            desktop.attach_new_chat_image(gpui::ImageFormat::Png, png.into_inner(), cx);
            assert_eq!(desktop.new_chat_attachments.len(), 1);
            desktop.creating_chat_folder = Some("project".into());
            desktop.chat_create_title = "New Chat".into();
            desktop.chat_create_submitting = true;
            desktop.chat_create_token = Some(1);
            desktop.new_chat_submitted_prompt = Some(String::new());
            desktop.new_chat_submitted_attachments = desktop.new_chat_attachments.clone();
            desktop.handle_reply(
                RequestKind::NewChat {
                    folder_id: "project".into(),
                    title: "New Chat".into(),
                    workdir: None,
                    token: 1,
                },
                serde_json::json!({"ok":true,"id":"image-chat"})
                    .as_object()
                    .unwrap()
                    .clone(),
                None,
                cx,
            );
            let prompt = &desktop.first_chat_prompts["image-chat"];
            let (text, attachments) = (&prompt.text, &prompt.attachments);
            assert!(text.is_empty());
            assert_eq!(attachments.len(), 1);
        });
    }
}
