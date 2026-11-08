//! The message composer. A chat and a new chat type into the same box: the
//! draft's tiles, the editor, and a row of controls with the send button, then
//! a row under the box naming the project, worktree, and branch the message
//! goes to. A new chat can still change the worktree and branch there; an
//! open chat shows them as they are.

use super::*;
use image_preview::DraftOwner;

/// The shell's border and padding, and the editor's own padding, on each side.
const COMPOSER_INSET: f32 = 8.0 + 1.0 + 8.0;

/// How much of a composer's width its text has. Layout hands the editor the
/// space the column has, not the width of the box it is painted in, so a
/// message would otherwise wrap to the window instead of to the composer.
pub(super) fn composer_text_width(width: Pixels) -> Pixels {
    width - px(2.0 * COMPOSER_INSET)
}

/// What differs between the composers. Everything else is drawn the same.
pub(super) struct ComposerParts {
    pub owner: DraftOwner,
    pub width: Pixels,
    /// What the editor box shows: the editor, or a line in its place.
    pub editor: gpui::AnyElement,
    /// After the attach button: the model and the other options.
    pub controls: Vec<gpui::AnyElement>,
    /// At the right end: the context meter, queue, stop, and send.
    pub actions: Vec<gpui::AnyElement>,
    /// Whether the draft's tiles show: not once they have been sent.
    pub show_attachments: bool,
}

/// A chip in the row under the composer. One that opens a picker reacts to
/// the pointer like a button; a settled one looks the same but does not.
pub(super) fn context_chip(
    id: impl Into<gpui::ElementId>,
    icon_path: &'static str,
    label: String,
    interactive: bool,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    let chip = if interactive {
        ui_button(id, ButtonKind::Ghost, colors)
    } else {
        ui_button_shape(id).text_color(rgb(colors.muted))
    };
    chip.max_w(px(280.0))
        .min_w_0()
        .child(assistant_icon(icon_path, 13.0, colors.muted, colors))
        .child(div().min_w_0().truncate().child(label))
        .child(icon(CHEVRON_DOWN_ICON, 11.0, colors.muted))
}

/// The row under the composer, starting with the project's name.
pub(super) fn context_row(project: String, colors: ThemeColors) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .text_size(px(12.0))
        .text_color(rgb(colors.muted))
        .child(div().px_2().child(project))
}

/// The round send button at the end of the composer's controls.
pub(super) fn composer_send_button(
    id: &'static str,
    enabled: bool,
    icon_path: &'static str,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .size(px(32.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(rgb(if enabled {
            colors.accent
        } else {
            colors.surface_high
        }))
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(rgb(colors.accent_hover)))
        })
        .child(icon(
            icon_path,
            16.0,
            if enabled {
                colors.accent_text
            } else {
                colors.faint
            },
        ))
}

/// A chip among the composer's controls: an optional icon, a label, and a
/// chevron for the menu it opens.
pub(super) fn composer_option(
    id: impl Into<gpui::ElementId>,
    leading: Option<gpui::AnyElement>,
    label: String,
    max_width: f32,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    ui_button(id, ButtonKind::Ghost, colors)
        .h(px(28.0))
        .px(px(8.0))
        .gap(px(5.0))
        .text_size(px(12.0))
        .max_w(px(max_width))
        .min_w_0()
        .children(leading)
        .child(div().min_w_0().truncate().child(label))
        .child(icon(CHEVRON_DOWN_ICON, 11.0, colors.faint))
}

impl XdDesktop {
    pub(super) fn render_composer(
        &mut self,
        parts: ComposerParts,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let ComposerParts {
            owner,
            width,
            editor,
            controls,
            actions,
            show_attachments,
        } = parts;
        let input = match owner {
            DraftOwner::Chat => self.composer_input.clone(),
            DraftOwner::NewChat => self.new_chat_input.clone(),
        };
        input.update(cx, |editor, _| {
            editor.set_wrap_width(composer_text_width(width))
        });
        let focus = input.read(cx).focus_handle(cx);
        let scroll = input.read(cx).scroll_handle();
        let attachments = self
            .render_composer_attachments(owner, colors, window, cx)
            .filter(|_| show_attachments);
        let (shell_id, editor_id, attach_id) = match owner {
            DraftOwner::Chat => ("native-composer-shell", "native-composer", "native-attach"),
            DraftOwner::NewChat => ("draft-composer-shell", "draft-composer", "draft-attach"),
        };
        div()
            .id(shell_id)
            .debug_selector(move || match owner {
                DraftOwner::Chat => "chat-composer".into(),
                DraftOwner::NewChat => "new-chat-composer".into(),
            })
            .w_full()
            .max_w(width)
            .pt(px(10.0))
            .pb(px(8.0))
            .px(px(8.0))
            .flex()
            .flex_col()
            .gap_1()
            .rounded(px(16.0))
            .border_1()
            .border_color(rgb(if focus.is_focused(window) {
                colors.selected_border
            } else {
                colors.border
            }))
            .bg(rgb(colors.composer))
            .shadow_md()
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(rgb(colors.accent))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                let focus = this.draft_editor_focus(owner, cx);
                window.focus(&focus);
            }))
            .children(attachments)
            .child(
                div()
                    .id(editor_id)
                    .debug_selector(move || editor_id.into())
                    .min_h(px(48.0))
                    .max_h(px(200.0))
                    .px(px(8.0))
                    .py_1()
                    .text_size(px(14.0))
                    .text_color(rgb(colors.text))
                    // A message past the tenth line scrolls inside the
                    // composer; without this it paints over the chat.
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .track_focus(&focus)
                    .child(editor),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id(attach_id)
                            .debug_selector(move || attach_id.into())
                            .size(px(28.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .tooltip(move |_, cx| {
                                cx.new(|_| PaneTooltip {
                                    label: "Attach files",
                                    colors,
                                })
                                .into()
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                match owner {
                                    DraftOwner::Chat => this.pick_attachments(window, cx),
                                    DraftOwner::NewChat => {
                                        this.pick_new_chat_attachments(window, cx)
                                    }
                                }
                            }))
                            .child(icon(PAPERCLIP_ICON, 15.0, colors.muted)),
                    )
                    .children(controls)
                    .child(div().flex_1())
                    .children(actions),
            )
    }

    /// The open chat's project, worktree, and branch, as the row under a new
    /// chat's composer shows them, except that a chat's worktree is settled.
    pub(super) fn render_chat_context_row(&self, colors: ThemeColors) -> Option<gpui::Div> {
        let chat = self
            .model
            .selected_chat
            .as_deref()
            .and_then(|id| self.model.chats.iter().find(|chat| chat.id == id))?;
        let project = self
            .model
            .folders
            .iter()
            .find(|folder| folder.id == chat.folder)
            .map(|folder| folder.name.clone())?;
        let (worktree, branch) = chat_worktree_labels(
            &self.model.worktrees,
            self.model.new_worktree,
            chat.branch.as_deref(),
        );
        let fixed = |chip: gpui::Stateful<gpui::Div>| {
            chip.tooltip(move |_, cx| {
                cx.new(|_| PaneTooltip {
                    label: "Set when the chat started",
                    colors,
                })
                .into()
            })
        };
        Some(
            context_row(project, colors)
                .debug_selector(|| "chat-context".into())
                .child(fixed(
                    context_chip(
                        "chat-context-workspace",
                        FOLDER_ICON,
                        worktree.into(),
                        false,
                        colors,
                    )
                    .debug_selector(|| "chat-context-workspace".into()),
                ))
                .child(fixed(
                    context_chip(
                        "chat-context-branch",
                        GIT_BRANCH_ICON,
                        branch,
                        false,
                        colors,
                    )
                    .debug_selector(|| "chat-context-branch".into()),
                )),
        )
    }
}

/// How the row under a chat's composer names its worktree and branch, in the
/// words a new chat's pickers use: a chat in a linked worktree, or about to
/// make one, was started in a new worktree.
pub(super) fn chat_worktree_labels(
    worktrees: &[Worktree],
    new_worktree: bool,
    chat_branch: Option<&str>,
) -> (&'static str, String) {
    let current = worktrees.iter().find(|worktree| worktree.current);
    let linked = current.is_some_and(|worktree| !worktree.main);
    let worktree = if new_worktree || linked {
        "New Worktree"
    } else {
        "Current Worktree"
    };
    let branch = match current {
        Some(Worktree {
            branch: Some(branch),
            ..
        }) => branch.clone(),
        Some(Worktree { detached: true, .. }) => "Detached HEAD".into(),
        _ => chat_branch.map_or_else(|| "Project directory".into(), str::to_owned),
    };
    (worktree, branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree(main: bool, current: bool, branch: Option<&str>) -> Worktree {
        Worktree {
            path: if main { "/repo" } else { "/worktrees/repo-1" }.into(),
            branch: branch.map(str::to_owned),
            detached: branch.is_none(),
            main,
            current,
        }
    }

    #[test]
    fn a_chat_names_its_worktree_and_branch_the_way_a_new_chat_does() {
        let main = [worktree(true, true, Some("main"))];
        assert_eq!(
            chat_worktree_labels(&main, false, None),
            ("Current Worktree", "main".into())
        );
        let linked = [
            worktree(true, false, Some("main")),
            worktree(false, true, Some("xd/footer-1a2b")),
        ];
        assert_eq!(
            chat_worktree_labels(&linked, false, Some("main")),
            ("New Worktree", "xd/footer-1a2b".into())
        );
        // Its worktree is made with the first message.
        assert_eq!(chat_worktree_labels(&main, true, None).0, "New Worktree");
        let detached = [worktree(true, true, None)];
        assert_eq!(
            chat_worktree_labels(&detached, false, None).1,
            "Detached HEAD"
        );
        // Not a repository: the chat runs in the project directory.
        assert_eq!(
            chat_worktree_labels(&[], false, None),
            ("Current Worktree", "Project directory".into())
        );
        assert_eq!(chat_worktree_labels(&[], false, Some("dev")).1, "dev");
    }
}
