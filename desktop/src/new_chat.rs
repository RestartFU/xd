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
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            )
            .with_priority(1)
        });
        self.new_chat_input
            .update(cx, |editor, _| editor.set_wrap_width(px(620.0)));
        let folder = self
            .creating_chat_folder
            .as_ref()
            .and_then(|id| self.model.folders.iter().find(|f| &f.id == id))
            .map(|f| f.name.clone())
            .unwrap_or_default();
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
            .justify_center()
            .px_6()
            .bg(rgb(colors.background))
            .child(
                div()
                    .w_full()
                    .max_w(px(660.0))
                    .mb_6()
                    .text_size(px(22.0))
                    .text_color(rgb(colors.text))
                    .child("What would you like to build?"),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(660.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .p_3()
                            .rounded(px(16.0))
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.composer))
                            .children(self.new_chat_attachments.iter().enumerate().map(
                                |(index, attachment)| {
                                    ui_button(
                                        ("draft-attachment", index),
                                        ButtonKind::Secondary,
                                        colors,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.chat_create_submitting {
                                            this.new_chat_attachments.remove(index);
                                            cx.notify();
                                        }
                                    }))
                                    .child(format!("{} · Remove", attachment.name))
                                },
                            ))
                            .child(
                                div()
                                    .id("draft-composer")
                                    .track_scroll(&self.new_chat_input.read(cx).scroll_handle())
                                    .track_focus(&self.new_chat_input.read(cx).focus_handle(cx))
                                    .min_h(px(70.0))
                                    .max_h(px(200.0))
                                    .overflow_y_scroll()
                                    .text_color(rgb(colors.text))
                                    .when(!self.chat_create_submitting, |editor| {
                                        editor.child(self.new_chat_input.clone())
                                    })
                                    .when(self.chat_create_submitting, |editor| {
                                        editor.child(
                                            div()
                                                .text_color(rgb(colors.muted))
                                                .child("Setting up…"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        ui_button("draft-attach", ButtonKind::Ghost, colors)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.pick_new_chat_attachments(window, cx)
                                            }))
                                            .child(icon(PAPERCLIP_ICON, 15.0, colors.muted)),
                                    )
                                    .child(
                                        div()
                                            .relative()
                                            .child(self.draft_picker_button(
                                                "model",
                                                model_label,
                                                colors,
                                                cx,
                                            ))
                                            .children(if menu == Some("model") {
                                                popup.take()
                                            } else {
                                                None
                                            }),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .id("draft-send")
                                            .size(px(32.0))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_full()
                                            .bg(rgb(if ready {
                                                colors.accent
                                            } else {
                                                colors.surface_high
                                            }))
                                            .when(ready, |button| {
                                                button.cursor_pointer().hover(|style| {
                                                    style.bg(rgb(colors.accent_hover))
                                                })
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if ready {
                                                    this.save_minimal_chat_create(cx);
                                                }
                                            }))
                                            .child(icon(
                                                SEND_ICON,
                                                16.0,
                                                if ready {
                                                    colors.accent_text
                                                } else {
                                                    colors.faint
                                                },
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.0))
                            .text_color(rgb(colors.muted))
                            .child(div().px_2().child(folder))
                            .when(!shared, |row| {
                                row.child(
                                    div()
                                        .relative()
                                        .child(
                                            self.draft_picker_button(
                                                "workspace",
                                                if new_worktree {
                                                    "New Worktree"
                                                } else {
                                                    "Current Worktree"
                                                }
                                                .into(),
                                                colors,
                                                cx,
                                            ),
                                        )
                                        .children(if menu == Some("workspace") {
                                            popup.take()
                                        } else {
                                            None
                                        }),
                                )
                                .child(
                                    div()
                                        .relative()
                                        .child(self.draft_picker_button(
                                            "branch",
                                            branch_label,
                                            colors,
                                            cx,
                                        ))
                                        .children(if menu == Some("branch") {
                                            popup.take()
                                        } else {
                                            None
                                        }),
                                )
                            })
                            .when(shared, |row| row.child("Shared session worktree")),
                    ),
            )
            .into_any_element()
    }

    fn pick_new_chat_attachments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn draft_picker_button(
        &self,
        menu: &'static str,
        label: String,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        ui_button(
            SharedString::from(format!("draft-{menu}")),
            ButtonKind::Ghost,
            colors,
        )
        .max_w(px(280.0))
        .on_click(cx.listener(move |this, _, window, cx| {
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
        .child(assistant_icon(
            match menu {
                "model" => agent_icon(self.minimal_new_session_agent),
                "branch" => GIT_BRANCH_ICON,
                _ => FOLDER_ICON,
            },
            13.0,
            colors.muted,
            colors,
        ))
        .child(div().truncate().child(label))
        .child(icon(CHEVRON_DOWN_ICON, 11.0, colors.muted))
    }

    pub(super) fn default_new_chat_base(&self) -> Option<NewSessionWorktree> {
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
            let (id, text, attachments) = desktop.first_chat_prompt.as_ref().unwrap();
            assert_eq!(id, "image-chat");
            assert!(text.is_empty());
            assert_eq!(attachments.len(), 1);
        });
    }
}
