//! The message composer. A chat and a new chat type into the same box: the
//! draft's tiles, the editor, and a row of controls with the send button, then
//! a row under the box naming the project, worktree, and branch the message
//! goes to. A new chat can still change the worktree and branch there; an
//! open chat shows them as they are.

use super::*;
use crate::metrics::{
    CONTROL_HEIGHT, CONTROL_HEIGHT_COMPACT, RADIUS_COMPOSER, RADIUS_PANEL, TEXT_BODY, TEXT_CONTROL,
    TEXT_DISPLAY, TEXT_HEADING, TEXT_LABEL, line_px,
};
use image_preview::DraftOwner;

/// The shell's border and padding, and the editor's own padding, on each side.
const COMPOSER_INSET: f32 = 8.0 + 1.0 + 8.0;

/// How much of a composer's width its text has. Layout hands the editor the
/// space the column has, not the width of the box it is painted in, so a
/// message would otherwise wrap to the window instead of to the composer.
pub(super) fn composer_text_width(width: Pixels) -> Pixels {
    width - px(2.0 * COMPOSER_INSET)
}

/// The padding at each side of the page a composer is centred on.
const PAGE_INSET: f32 = 24.0;
/// The shell's border and padding on each side.
const SHELL_INSET: f32 = 1.0 + 8.0;

/// Notes how wide the page's column is, for the composer on it, and draws
/// again when that has changed. The composer's row of options needs its
/// width in pixels. The composer is as wide as the column up to a maximum,
/// and such a box is measured for its height at the column's width before
/// the maximum applies: where the options fit one row at that width and two
/// at the real one, as an account's name makes them, the second row hung out
/// of the box. Give this to the page's own column.
pub(super) fn composer_room(room: Rc<Cell<Option<Pixels>>>) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            let width = bounds.size.width;
            if room.get() != Some(width) {
                room.set(Some(width));
                window.request_animation_frame();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left(px(PAGE_INSET))
    .right(px(PAGE_INSET))
    .h_0()
}

/// The most of the window a docked ask takes before it scrolls: half the
/// window's height. Where that leaves too little for what is around it, the
/// ask gives up more: the composer shrinks into the room the title bar, the
/// session's header and tabs, the row under the composer and the least of
/// the conversation leave, so what it gives up follows what is really there.
pub(super) fn ask_max_height(window: f32) -> f32 {
    window * 0.5
}

/// The least of the conversation a docked ask leaves in sight: two lines of
/// the last message, over the Copy under it.
pub(super) const TRANSCRIPT_MIN_HEIGHT: f32 = 2.0 * MESSAGE_LINE_HEIGHT + COPY_ROW_HEIGHT;

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
    /// What waits on the user in this chat, docked at the top of the box
    /// above a hairline: a question, an approval, or a quota stop.
    pub ask: Option<gpui::AnyElement>,
    /// Room is short: the options keep to one row, cutting their labels
    /// short, and the editor starts a line high.
    pub compact: bool,
}

/// The docked ask's first line: the waiting ring, its title, and, when more
/// than one thing waits, which one this is, such as "1 / 2".
pub(super) fn ask_header(
    title: impl Into<SharedString>,
    counter: Option<String>,
    colors: ThemeColors,
) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .gap(px(10.0))
        // Centred on the title's first line, however many lines it takes.
        .child(
            div()
                .h(px(if short_window() { 19.0 } else { 22.0 }))
                .flex()
                .items_center()
                .child(waiting_ring(colors)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(text_px(TEXT_HEADING))
                .line_height(px(if short_window() { 19.0 } else { 22.0 }))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(colors.text))
                .child(super::wrapped_heading(title)),
        )
        .when_some(counter, |row, counter| {
            row.child(
                div()
                    .flex_none()
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .debug_selector(|| "composer-ask-counter".into())
                    .text_size(text_px(TEXT_LABEL))
                    .text_color(rgb(colors.faint))
                    .child(counter),
            )
        })
}

/// The room between the parts of a docked ask: less in a short window,
/// where its question and first answer must fit above the editor.
pub(super) fn ask_gap() -> f32 {
    if short_window() { 6.0 } else { 12.0 }
}

/// One of a question's choices in the docked ask: a faint number, which is
/// also its key, the answer, and an arrow on the row under the pointer or the
/// keyboard.
pub(super) fn ask_choice(
    index: usize,
    label: String,
    description: Option<String>,
    focused: bool,
    enabled: bool,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    let group: SharedString = format!("composer-ask-choice-{index}").into();
    div()
        .id(("composer-ask-choice", index))
        .debug_selector(move || format!("composer-ask-choice-{index}"))
        .group(group.clone())
        .w_full()
        .px(px(10.0))
        // In a short window a row is a line and a little, so the first
        // answer fits whole under the question.
        .py(px(if short_window() { 4.0 } else { 9.0 }))
        .flex()
        .items_start()
        .gap(px(14.0))
        .rounded(px(RADIUS_PANEL))
        .text_size(text_px(TEXT_BODY))
        .line_height(line_px(LINE_CONTROL, TEXT_BODY))
        // Waiting on the host, a choice reads as one that cannot be picked.
        .text_color(rgb(if enabled {
            colors.text
        } else {
            disabled_ink(colors)
        }))
        // The arrow keys' choice also takes the ring a menu's row takes,
        // which hover never draws.
        .when(enabled && focused, |row| {
            row.relative()
                .bg(rgb(colors.selected_surface))
                .child(key_row_ring(colors).rounded(px(RADIUS_PANEL)))
        })
        .when(enabled, |row| {
            row.cursor_pointer()
                .hover(|style| style.bg(rgb(colors.selected_surface)))
        })
        .child(
            div()
                .flex_none()
                .w(px(10.0))
                .text_size(text_px(TEXT_LABEL))
                .text_color(rgb(colors.faint))
                .child((index + 1).to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_wrap()
                .gap_x(px(6.0))
                .child(div().min_w_0().child(label))
                .when_some(description, |text, description| {
                    text.child(
                        div()
                            .min_w_0()
                            .text_size(text_px(TEXT_CONTROL))
                            .text_color(rgb(if enabled {
                                colors.muted
                            } else {
                                disabled_ink(colors)
                            }))
                            .child(format!("· {description}")),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .h(px(20.0))
                .flex()
                .items_center()
                .when(!enabled, |arrow| arrow.invisible())
                .when(enabled && !focused, |arrow| {
                    arrow
                        .invisible()
                        .group_hover(group, |style| style.visible())
                })
                .child(icon(ARROW_RIGHT_ICON, 14.0, colors.faint)),
        )
}

/// What an empty chat, or a new one, says above its composer.
pub(super) const EMPTY_CHAT: (&str, &str) = (
    "What are we working on?",
    "A new conversation in the same workspace.",
);

/// What a chat says in place of a conversation the host did not send. A
/// button under it asks again.
pub(super) const CHAT_NOT_LOADED: (&str, &str) = (
    "Conversation did not load",
    "The request to the host failed.",
);

/// The heading over an empty chat's composer: the project, a question, and a
/// line under it. A new chat shows the same one, as it did when it opened as
/// an empty chat.
pub(super) fn chat_heading(
    project: String,
    avatar: Option<Arc<Image>>,
    heading: &'static str,
    description: &'static str,
    colors: ThemeColors,
) -> gpui::Div {
    div()
        .w_full()
        .min_h(px(140.0))
        .py_3()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .text_center()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .text_size(text_px(TEXT_LABEL))
                .text_color(rgb(colors.muted))
                .child(workspace_icon(avatar, 14.0, colors.muted))
                .child(project),
        )
        .child(
            div()
                .text_size(text_px(TEXT_DISPLAY))
                .debug_selector(|| "empty-chat-heading".into())
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(colors.text))
                .child(heading),
        )
        .child(
            div()
                .text_size(text_px(TEXT_CONTROL))
                .debug_selector(|| "empty-chat-description".into())
                .text_color(rgb(colors.muted))
                .child(description),
        )
}

/// A chip in the row under the composer. One that opens a picker is a button
/// with a chevron. A settled one is a plain label: it has nothing to open, so
/// it neither looks pressable nor points at a menu.
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
        div()
            .id(id)
            .h(px(CONTROL_HEIGHT))
            .px_2()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_color(rgb(colors.muted))
    };
    chip.max_w(px(280.0))
        .min_w_0()
        .flex_shrink()
        .child(assistant_icon(icon_path, 13.0, colors.muted, colors))
        .child(truncating_label(label).min_w_0())
        .when(interactive, |chip| {
            chip.child(icon(CHEVRON_DOWN_ICON, 11.0, colors.muted))
        })
}

/// The row under the composer, starting with the project's name.
pub(super) fn context_row(project: String, colors: ThemeColors) -> gpui::Div {
    div()
        // One line, as wide as what it holds and no wider than its place:
        // the chips give way by cutting their labels short. It used to wrap,
        // and at Interface sizes where its chips' widths are fractions of a
        // pixel the row came out a hair narrower than their sum, so the last
        // chip went to a second line under the window's edge.
        .min_w_0()
        .max_w_full()
        .flex()
        .items_center()
        .gap_2()
        .text_size(text_px(TEXT_LABEL))
        .text_color(rgb(colors.muted))
        .child(div().flex_none().px_2().child(project))
}

/// The side of the round buttons at the end of the composer's controls.
const COMPOSER_ROUND_BUTTON: f32 = 32.0;

/// A round button at the end of the composer's controls, Send or Stop. It is
/// an icon alone, so `label` names it under the pointer.
fn composer_round_button(
    id: &'static str,
    label: impl Into<SharedString>,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .tooltip(pane_tooltip(label, colors))
        .size(px(COMPOSER_ROUND_BUTTON))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
}

/// Stop, beside Send while a turn runs: the quiet fill, warming to the
/// danger tint under the pointer.
pub(super) fn composer_stop_button(
    id: &'static str,
    label: impl Into<SharedString>,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    composer_round_button(id, label, colors)
        .bg(rgb(colors.surface_high))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(colors.danger_surface)))
        .child(icon(STOP_ICON, 13.0, colors.text))
}

/// The round send button at the end of the composer's controls.
pub(super) fn composer_send_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    icon_path: &'static str,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    composer_round_button(id, label, colors)
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
        .h(px(CONTROL_HEIGHT_COMPACT))
        .px(px(8.0))
        .gap(px(5.0))
        .text_size(text_px(TEXT_LABEL))
        .max_w(px(max_width))
        .min_w_0()
        .children(leading)
        .child(truncating_label(label).min_w_0())
        .child(icon(CHEVRON_DOWN_ICON, 11.0, colors.faint))
}

/// A composer option that cannot be changed now, such as access in Plan
/// mode: its words alone, as a settled chip in the row under the composer
/// is, with nothing that looks pressable or points at a menu. `why` says
/// what holds it, under the pointer.
pub(super) fn composer_option_locked(
    id: impl Into<gpui::ElementId>,
    label: String,
    why: &'static str,
    max_width: f32,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(CONTROL_HEIGHT_COMPACT))
        .px(px(8.0))
        .flex_none()
        .flex()
        .items_center()
        .max_w(px(max_width))
        .min_w_0()
        .text_size(text_px(TEXT_LABEL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(colors.muted))
        .tooltip(move |_, cx| {
            cx.new(|_| ClearTooltip {
                label: why.into(),
                colors,
            })
            .into()
        })
        .child(truncating_label(label).min_w_0())
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
            ask,
            compact,
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
        let keys = self.key_targets.clone();
        let attachments = self
            .render_composer_attachments(owner, colors, window, cx)
            .filter(|_| show_attachments);
        let (shell_id, editor_id, attach_id) = match owner {
            DraftOwner::Chat => ("native-composer-shell", "native-composer", "native-attach"),
            DraftOwner::NewChat => ("draft-composer-shell", "draft-composer", "draft-attach"),
        };
        // A docked ask gives up its height before the editor and Send do:
        // the composer shrinks into the chat's room, and the ask scrolls
        // inside what is left.
        let docked = ask.is_some();
        div()
            .id(shell_id)
            .when(docked, |shell| shell.flex_shrink().min_h_0())
            .debug_selector(move || match owner {
                DraftOwner::Chat => "chat-composer".into(),
                DraftOwner::NewChat => "new-chat-composer".into(),
            })
            .w_full()
            .max_w(width)
            .pt(px(if compact { 4.0 } else { 10.0 }))
            .pb(px(if compact { 4.0 } else { 8.0 }))
            .px(px(8.0))
            .flex()
            .flex_col()
            .gap(px(if compact { 2.0 } else { 4.0 }))
            .rounded(px(RADIUS_COMPOSER))
            .border_1()
            // At rest it has the control edge every field has.
            .border_color(field_border(focus.is_focused(window), colors))
            .bg(crate::glass::composer(colors))
            .shadow_md()
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(rgb(colors.accent))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                let focus = this.draft_editor_focus(owner, cx);
                window.focus(&focus);
            }))
            .when_some(ask, |shell, ask| {
                // The shell is padded 10 at the top and 8 at the sides; the
                // section sits 16 from the top and 18 from the sides. A tall
                // one scrolls inside itself so the editor stays in view.
                shell
                    .child(
                        div()
                            .id("composer-ask")
                            .debug_selector(|| "composer-ask".into())
                            .max_h(px(ask_max_height(crate::logical(
                                window.viewport_size().height,
                            ))))
                            .flex_shrink()
                            .min_h_0()
                            .overflow_y_scroll()
                            .thumbed("composer-ask", colors)
                            // Choice rows reach into its padding; patches and
                            // commands scroll inside their own boxes.
                            .overflow_x_hidden()
                            .pt(px(if compact { 0.0 } else { 6.0 }))
                            .px(px(10.0))
                            .pb(px(if compact { 2.0 } else { 6.0 }))
                            // Its buttons and fields keep the focus they take,
                            // rather than the box handing it to the editor.
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .child(ask),
                    )
                    .child(
                        div()
                            .flex_none()
                            .mx(px(10.0))
                            .mb(px(if compact { 4.0 } else { 6.0 }))
                            .h(px(1.0))
                            .bg(rgb(colors.border)),
                    )
            })
            .children(attachments)
            .child(
                div()
                    .id(editor_id)
                    .debug_selector(move || editor_id.into())
                    .min_h(px(if compact { 28.0 } else { 48.0 }))
                    .max_h(px(200.0))
                    .px(px(8.0))
                    .py_1()
                    .text_size(text_px(TEXT_BODY))
                    .text_color(rgb(colors.text))
                    // A message past the tenth line scrolls inside the
                    // composer; without this it paints over the chat.
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .track_focus(&focus)
                    .child(editor),
            )
            // Tab goes from the editor through the options and back.
            .child(keys.mark(KeyLayer::Page, KeyStop::Field(focus.clone())))
            .child(
                // The options wrap among themselves when the composer is
                // narrow; the send action stays at the bottom right, level
                // with their last row.
                div()
                    .flex()
                    .items_end()
                    .gap_1()
                    // In pixels where the page is wider than the box, so the
                    // options wrap the same when the box is measured as when
                    // it is drawn; see [`composer_room`]. Where the page is
                    // the narrower, the box is measured at its real width.
                    .when(
                        self.composer_room.get().is_some_and(|room| room > width),
                        |row| row.w(width - px(2.0 * SHELL_INSET)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .py(px(if compact { 0.0 } else { 2.0 }))
                            .flex()
                            .when(!compact, |options| options.flex_wrap())
                            .when(compact, |options| options.overflow_hidden())
                            .items_center()
                            .gap_1()
                            .child(
                                icon_button(attach_id, "Attach files", PAPERCLIP_ICON, colors)
                                    .glyph(15.0)
                                    .build()
                                    .debug_selector(move || attach_id.into())
                                    .key_stop_named(
                                        &keys,
                                        KeyLayer::Page,
                                        attach_id,
                                        "Attach files",
                                        colors,
                                        cx,
                                        move |this, window, cx| {
                                            cx.stop_propagation();
                                            match owner {
                                                DraftOwner::Chat => {
                                                    this.pick_attachments(window, cx)
                                                }
                                                DraftOwner::NewChat => {
                                                    this.pick_new_chat_attachments(window, cx)
                                                }
                                            }
                                        },
                                    ),
                            )
                            .children(controls),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_1()
                            .children(actions),
                    ),
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
                cx.new(|_| ClearTooltip {
                    label: "Set when the chat started".into(),
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
        "New worktree"
    } else {
        "Current worktree"
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
            ("Current worktree", "main".into())
        );
        let linked = [
            worktree(true, false, Some("main")),
            worktree(false, true, Some("xd/footer-1a2b")),
        ];
        assert_eq!(
            chat_worktree_labels(&linked, false, Some("main")),
            ("New worktree", "xd/footer-1a2b".into())
        );
        // Its worktree is made with the first message.
        assert_eq!(chat_worktree_labels(&main, true, None).0, "New worktree");
        let detached = [worktree(true, true, None)];
        assert_eq!(
            chat_worktree_labels(&detached, false, None).1,
            "Detached HEAD"
        );
        // Not a repository: the chat runs in the project directory.
        assert_eq!(
            chat_worktree_labels(&[], false, None),
            ("Current worktree", "Project directory".into())
        );
        assert_eq!(chat_worktree_labels(&[], false, Some("dev")).1, "dev");
    }
}
