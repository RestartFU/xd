use super::*;

impl XdDesktop {
    pub(super) fn render_new_chat(
        &mut self,
        colors: ThemeColors,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
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
        let ready = (!new_worktree || !self.chat_create_sources_loading)
            && !self.chat_create_submitting
            && (!self.new_chat_prompt.trim().is_empty() || !self.new_chat_attachments.is_empty())
            && (shared
                || (!self.chat_create_worktrees_loading && self.chat_create_worktree.is_some()));
        let menu = self.new_chat_menu;
        let mut rows = Vec::new();
        match menu {
            Some("model") => {
                for agent in [AgentCli::Codex, AgentCli::Claude] {
                    rows.push(
                        div()
                            .px_3()
                            .py_2()
                            .text_xs()
                            .text_color(rgb(colors.muted))
                            .child(agent.label())
                            .into_any_element(),
                    );
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
                            .justify_start()
                            .on_click(cx.listener(move |this, _, _, cx| {
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
                            .justify_start()
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
                            .child(label)
                            .into_any_element(),
                    );
                }
            }
            Some("branch") => {
                if new_worktree {
                    rows.push(
                        ui_button("draft-remote", ButtonKind::Ghost, colors)
                            .w_full()
                            .justify_start()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_chat_remote = !this.new_chat_remote;
                                cx.notify();
                            }))
                            .child(if self.new_chat_remote {
                                "Start from remote · On"
                            } else {
                                "Start from remote · Off"
                            })
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
                        if !name.to_lowercase().contains(&self.new_chat_query) {
                            continue;
                        }
                        rows.push(
                            ui_button(("draft-base", index), ButtonKind::Ghost, colors)
                                .w_full()
                                .justify_start()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.chat_create_worktree = Some(NewSessionWorktree::Branch {
                                        reference: branch.reference.clone(),
                                        oid: branch.oid.clone(),
                                    });
                                    this.new_chat_menu = None;
                                    cx.notify();
                                }))
                                .child(name)
                                .into_any_element(),
                        );
                    }
                } else {
                    for (index, worktree) in self.chat_create_worktrees.iter().enumerate() {
                        let path = worktree.path.clone();
                        rows.push(
                            ui_button(("draft-checkout", index), ButtonKind::Ghost, colors)
                                .w_full()
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
        let popup = menu.map(|_| {
            gpui::deferred(
                div()
                    .id("draft-picker")
                    .occlude()
                    .absolute()
                    .bottom(px(42.0))
                    .left_0()
                    .w(px(300.0))
                    .max_h(px(320.0))
                    .overflow_y_scroll()
                    .p_1()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.surface))
                    .child(div().p_2().child(self.new_chat_search.clone()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(rows),
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
                                    .min_h(px(70.0))
                                    .max_h(px(200.0))
                                    .overflow_y_scroll()
                                    .text_color(rgb(colors.text))
                                    .child(self.new_chat_input.clone()),
                            )
                            .child(
                                div()
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(self.draft_picker_button(
                                        "model",
                                        model_label,
                                        colors,
                                        cx,
                                    ))
                                    .child(div().flex_1())
                                    .child(
                                        ui_button(
                                            "draft-send",
                                            if ready {
                                                ButtonKind::Primary
                                            } else {
                                                ButtonKind::Secondary
                                            },
                                            colors,
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if ready {
                                                this.save_minimal_chat_create(cx);
                                            }
                                        }))
                                        .child(
                                            if self.chat_create_submitting {
                                                "Setting up…"
                                            } else {
                                                "Send"
                                            },
                                        ),
                                    )
                                    .children(popup),
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
                                .child(self.draft_picker_button("branch", branch_label, colors, cx))
                            })
                            .when(shared, |row| row.child("Shared session worktree"))
                            .child(div().flex_1())
                            .child(
                                ui_button("draft-cancel", ButtonKind::Ghost, colors)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.chat_create_submitting {
                                            this.cancel_chat_create(cx);
                                        }
                                    }))
                                    .child("Cancel"),
                            ),
                    ),
            )
            .into_any_element()
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
                let focus = this.new_chat_search.read(cx).focus_handle(cx);
                window.focus(&focus);
                this.new_chat_menu = if this.new_chat_menu == Some(menu) {
                    None
                } else {
                    Some(menu)
                };
                cx.notify();
            }
        }))
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
