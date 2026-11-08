use super::*;

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
                for (index, (label, new)) in [("Current Worktree", false), ("New Worktree", true)]
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
                                                this.minimal_new_session_agent = agent;
                                                this.new_chat_model = None;
                                            }
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
        let folder = self
            .creating_chat_folder
            .as_ref()
            .and_then(|id| self.model.folders.iter().find(|f| &f.id == id))
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let starting = self.chat_create_submitting;
        let editor = if starting {
            // The message went up into the conversation above.
            div()
                .text_color(rgb(colors.faint))
                .child("Message xd...")
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
                controls: vec![model],
                actions: vec![send],
                show_attachments: !starting,
            },
            colors,
            window,
            cx,
        );
        let context = composer::context_row(folder, colors)
            .debug_selector(|| "new-chat-context".into())
            .when(!shared, |row| {
                let workspace = composer::context_chip(
                    "draft-workspace",
                    FOLDER_ICON,
                    if new_worktree {
                        "New Worktree"
                    } else {
                        "Current Worktree"
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
            .bg(rgb(colors.background))
            .when(!starting, |view| {
                view.child(
                    div()
                        .w_full()
                        .max_w(width)
                        .mb_6()
                        .text_size(px(22.0))
                        .text_color(rgb(colors.text))
                        .child("What would you like to build?"),
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
        if choices == self.new_chat_remembered {
            return;
        }
        self.settings
            .new_chat_choices
            .insert(self.new_chat_choices_key(&folder_id), choices.clone());
        self.new_chat_remembered = choices;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn disconnect_and_endpoint_switch_release_chat_creation(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            desktop.chat_create_submitting = true;
            desktop.new_chat_submitted_prompt = Some("Build a footer".into());
            assert!(desktop.switch_active_endpoint(ChatEndpoint::Remote, cx));
            assert!(!desktop.chat_create_submitting);
            assert!(desktop.new_chat_submitted_prompt.is_none());
            desktop.creating_chat_folder = Some("project".into());
            desktop.new_chat_prompt = "Keep this prompt".into();
            desktop.chat_create_submitting = true;
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
                assert!(!desktop.new_chat_remote);
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
            assert_eq!(desktop.new_chat_status(), "Creating a worktree from dev");
            assert!(desktop.anything_working());
        });

        desktop.update(cx, |desktop, cx| {
            desktop.handle_reply(
                RequestKind::NewChat {
                    folder_id: "atlas".into(),
                    title: "New Chat".into(),
                    workdir: None,
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
                desktop.chat_create_submitting = true;
                // Not answered yet: nothing else opens.
                desktop.open_minimal_session(
                    "atlas".into(),
                    "review".into(),
                    AgentCli::Claude,
                    window,
                    cx,
                );
                assert!(desktop.creating_chat_folder.is_some());
                assert_ne!(desktop.model.selected_chat.as_deref(), Some("review"));

                desktop.chat_create_title = "New Chat".into();
                desktop.new_chat_submitted_prompt = Some("First".into());
                desktop.handle_reply(
                    RequestKind::NewChat {
                        folder_id: "atlas".into(),
                        title: "New Chat".into(),
                        workdir: None,
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
                desktop.new_chat_started = Some("made".into());
                desktop.begin_chat_create("atlas".into(), window, cx);
                assert!(!desktop.chat_create_submitting);
                assert!(desktop.new_chat_started.is_none());
                assert_eq!(desktop.creating_chat_folder.as_deref(), Some("atlas"));
                // Its first message, and the first chat's, both wait to be sent.
                desktop.chat_create_title = "New Chat".into();
                desktop.new_chat_submitted_prompt = Some("Second".into());
                desktop.chat_create_submitting = true;
                desktop.handle_reply(
                    RequestKind::NewChat {
                        folder_id: "atlas".into(),
                        title: "New Chat".into(),
                        workdir: None,
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
                desktop.handle_reply(
                    RequestKind::NewChat {
                        folder_id: "atlas".into(),
                        title: "New Chat".into(),
                        workdir: None,
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
            desktop.new_chat_submitted_prompt = Some(String::new());
            desktop.new_chat_submitted_attachments = desktop.new_chat_attachments.clone();
            desktop.handle_reply(
                RequestKind::NewChat {
                    folder_id: "project".into(),
                    title: "New Chat".into(),
                    workdir: None,
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
