use std::time::Duration;

use gpui::{
    App, Application, Bounds, Context, FocusHandle, KeyDownEvent, ListAlignment, ListState, Render,
    Timer, Window, WindowBounds, WindowOptions, div, list, prelude::*, px, rgb, size,
};
use serde_json::Value;
use xd_desktop::{
    daemon::{DaemonHandle, DaemonUpdate, RequestKind},
    model::{AppModel, Message},
};

const BG: u32 = 0x111318;
const SURFACE: u32 = 0x191c22;
const SURFACE_HIGH: u32 = 0x232730;
const BORDER: u32 = 0x303641;
const TEXT: u32 = 0xe8eaf0;
const MUTED: u32 = 0x969daa;
const ACCENT: u32 = 0x6b8cff;

struct XdDesktop {
    model: AppModel,
    daemon: Option<DaemonHandle>,
    transcript: ListState,
    composer_focus: FocusHandle,
    composer: String,
    draft_generation: u64,
    draft_dirty: bool,
    sending: bool,
    pending_send: Option<String>,
}

impl XdDesktop {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut desktop = Self {
            model: AppModel {
                draft_revision: -1,
                ..Default::default()
            },
            daemon: None,
            transcript: ListState::new(0, ListAlignment::Bottom, px(700.0)),
            composer_focus: cx.focus_handle(),
            composer: String::new(),
            draft_generation: 0,
            draft_dirty: false,
            sending: false,
            pending_send: None,
        };
        desktop.connect(cx);
        desktop
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        match DaemonHandle::connect_discovered() {
            Ok((daemon, updates)) => {
                self.daemon = Some(daemon);
                self.model.connection_error = None;
                cx.spawn(async move |this, cx| {
                    while let Ok(update) = updates.recv().await {
                        if this
                            .update(cx, |this, cx| this.handle_daemon_update(update, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => {
                self.model.connected = false;
                self.model.connection_error = Some(error.to_string());
            }
        }
    }

    fn handle_daemon_update(&mut self, update: DaemonUpdate, cx: &mut Context<Self>) {
        match update {
            DaemonUpdate::Connected { .. } => {
                self.model.connected = true;
                self.model.connection_error = None;
                self.request_tree();
            }
            DaemonUpdate::Disconnected { message } => {
                self.model.connected = false;
                self.model.connection_error = Some(message);
                self.sending = false;
                self.restore_pending_send();
            }
            DaemonUpdate::Reply { kind, body } => self.handle_reply(kind, body, cx),
            DaemonUpdate::Event { name, body } => self.handle_event(&name, Value::Object(body), cx),
        }
        cx.notify();
    }

    fn handle_reply(
        &mut self,
        kind: RequestKind,
        body: serde_json::Map<String, Value>,
        cx: &mut Context<Self>,
    ) {
        let value = Value::Object(body);
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            self.model.connection_error = Some(
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("The xd daemon rejected the request.")
                    .to_owned(),
            );
            if matches!(kind, RequestKind::Send { .. }) {
                self.sending = false;
                self.restore_pending_send();
            }
            return;
        }

        match kind {
            RequestKind::Tree => {
                if let Err(error) = self.model.apply_tree(&value) {
                    self.model.connection_error = Some(format!("Invalid tree response: {error}"));
                    return;
                }
                if self.model.selected_chat.is_none() {
                    if let Some(chat_id) = self.model.chats.first().map(|chat| chat.id.clone()) {
                        self.select_chat(chat_id, cx);
                    }
                }
            }
            RequestKind::Chat { chat_id } if self.chat_is_active(&chat_id) => {
                self.model.apply_chat(&value);
                if !self.draft_dirty {
                    self.composer.clone_from(&self.model.draft);
                }
            }
            RequestKind::Messages { chat_id } if self.chat_is_active(&chat_id) => {
                if let Err(error) = self.model.apply_messages(&value) {
                    self.model.connection_error =
                        Some(format!("Invalid transcript response: {error}"));
                    return;
                }
                if !self.model.working {
                    self.model.live_text.clear();
                    self.model.live_activity.clear();
                }
                self.transcript.reset(self.model.display_message_count());
            }
            RequestKind::Send { chat_id, text } if self.chat_is_active(&chat_id) => {
                self.sending = false;
                self.pending_send = None;
                let queued = value
                    .get("queued")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !queued {
                    self.model.working = true;
                    self.request_messages(&chat_id);
                }
                if self.composer.is_empty() {
                    self.draft_dirty = false;
                }
                let _ = text;
            }
            RequestKind::SetDraft { chat_id, text } if self.chat_is_active(&chat_id) => {
                self.model.apply_draft_snapshot(&value);
                if self.composer == text {
                    self.draft_dirty = false;
                }
            }
            _ => {}
        }
    }

    fn handle_event(&mut self, name: &str, body: Value, cx: &mut Context<Self>) {
        match name {
            "tree" => self.request_tree(),
            "changed" if self.event_is_active(&body) => {
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_chat(&chat_id);
                }
            }
            "draft" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
                if !self.draft_dirty {
                    self.composer.clone_from(&self.model.draft);
                }
            }
            "turn-started" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
                self.sync_transcript_count(false);
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_messages(&chat_id);
                    self.request_chat(&chat_id);
                }
            }
            "text" | "tool" if self.event_is_active(&body) => {
                let old_count = self.model.display_message_count();
                self.model.apply_event(name, &body);
                let new_count = self.model.display_message_count();
                if new_count > old_count {
                    self.transcript
                        .splice(old_count..old_count, new_count - old_count);
                } else if new_count > 0 {
                    self.transcript.splice(new_count - 1..new_count, 1);
                }
            }
            "turn-finished" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_messages(&chat_id);
                    self.request_chat(&chat_id);
                }
            }
            "queued" if self.event_is_active(&body) => self.model.apply_event(name, &body),
            _ => {}
        }
        cx.notify();
    }

    fn request_tree(&mut self) {
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.tree() {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn request_chat(&mut self, chat_id: &str) {
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.chat(chat_id) {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn request_messages(&mut self, chat_id: &str) {
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.messages(chat_id) {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn select_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        if self.model.selected_chat.as_deref() == Some(chat_id.as_str()) {
            return;
        }
        self.model.select_chat(chat_id.clone());
        self.composer.clear();
        self.draft_dirty = false;
        self.pending_send = None;
        self.sending = false;
        self.transcript.reset(0);
        self.request_chat(&chat_id);
        self.request_messages(&chat_id);
        cx.notify();
    }

    fn send_composer(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let text = self.composer.trim().to_owned();
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let Some(daemon) = &self.daemon else {
            self.model.connection_error = Some("xd-dev is not connected to a daemon.".into());
            return;
        };
        if let Err(error) = daemon.send_message(&chat_id, &text) {
            self.model.connection_error = Some(error);
            return;
        }

        self.sending = true;
        self.pending_send = Some(text);
        self.composer.clear();
        self.draft_dirty = true;
        self.draft_generation = self.draft_generation.saturating_add(1);
        let _ = daemon.set_draft(&chat_id, "");
        cx.notify();
    }

    fn restore_pending_send(&mut self) {
        let Some(text) = self.pending_send.take() else {
            return;
        };
        if self.composer.is_empty() {
            self.composer = text;
        } else {
            self.composer = format!("{text}\n{}", self.composer);
        }
        self.draft_dirty = true;
    }

    fn edit_composer(&mut self, edit: impl FnOnce(&mut String), cx: &mut Context<Self>) {
        edit(&mut self.composer);
        self.draft_dirty = true;
        self.draft_generation = self.draft_generation.saturating_add(1);
        let generation = self.draft_generation;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(250)).await;
            let _ = this.update(cx, |this, _cx| {
                if this.draft_generation == generation {
                    this.sync_draft();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn sync_draft(&mut self) {
        if !self.draft_dirty {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.set_draft(&chat_id, &self.composer) {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn on_composer_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.key == "enter" && !keystroke.modifiers.shift {
            cx.stop_propagation();
            self.send_composer(cx);
            return;
        }
        if keystroke.key == "enter" {
            cx.stop_propagation();
            self.edit_composer(|text| text.push('\n'), cx);
            return;
        }
        if keystroke.key == "backspace" {
            cx.stop_propagation();
            self.edit_composer(
                |text| {
                    text.pop();
                },
                cx,
            );
            return;
        }
        if (keystroke.modifiers.platform || keystroke.modifiers.control)
            && keystroke.key.eq_ignore_ascii_case("v")
        {
            cx.stop_propagation();
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                self.edit_composer(|composer| composer.push_str(&text), cx);
            }
            return;
        }
        if keystroke.modifiers.platform || keystroke.modifiers.control {
            return;
        }
        if let Some(text) = &keystroke.key_char {
            if !text.chars().any(char::is_control) {
                cx.stop_propagation();
                self.edit_composer(|composer| composer.push_str(text), cx);
            }
        }
    }

    fn event_is_active(&self, body: &Value) -> bool {
        body.get("chat").and_then(Value::as_str) == self.model.selected_chat.as_deref()
    }

    fn chat_is_active(&self, chat_id: &str) -> bool {
        self.model.selected_chat.as_deref() == Some(chat_id)
    }

    fn sync_transcript_count(&self, reset: bool) {
        let count = self.model.display_message_count();
        if reset {
            self.transcript.reset(count);
            return;
        }
        let old_count = self.transcript.item_count();
        if count > old_count {
            self.transcript
                .splice(old_count..old_count, count - old_count);
        } else if count < old_count {
            self.transcript.splice(count..old_count, 0);
        }
    }

    fn message_row(message: &Message) -> impl IntoElement {
        let is_user = message.role == "user";
        let is_tool = message.role == "tool";
        let label = message
            .label
            .clone()
            .unwrap_or_else(|| match message.role.as_str() {
                "user" => "You".into(),
                "assistant" => "Assistant".into(),
                "tool" => "Activity".into(),
                role => role.to_owned(),
            });

        div().w_full().px_6().py_2().child(
            div()
                .w_full()
                .max_w(px(920.0))
                .mx_auto()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(rgb(if is_user { 0x3c4b78 } else { BORDER }))
                .bg(rgb(if is_user {
                    0x202944
                } else if is_tool {
                    0x171a20
                } else {
                    SURFACE
                }))
                .text_color(rgb(TEXT))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(if is_user { 0xaec0ff } else { MUTED }))
                        .mb_2()
                        .child(label),
                )
                .child(
                    div()
                        .text_sm()
                        .line_height(px(21.0))
                        .child(message.content.clone()),
                ),
        )
    }
}

impl Render for XdDesktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let messages = self.model.display_messages();
        let queue_count = self.model.queue.len();
        let working = self.model.working;
        let selected = self.model.selected_summary().cloned();
        let status_text = if self.model.connected {
            "connected"
        } else {
            "offline"
        };
        let status_color = if self.model.connected {
            0x92d5a5
        } else {
            0xe49a9a
        };

        let mut tree_rows = Vec::new();
        let mut chat_row_index = 0_usize;
        for folder in self.model.folders.clone() {
            let indent = if folder.parent.is_some() { 22.0 } else { 12.0 };
            tree_rows.push(
                div()
                    .px_3()
                    .ml(px(indent))
                    .pt_2()
                    .pb_1()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child(format!("▾  {}", folder.name))
                    .into_any_element(),
            );
            for chat in self
                .model
                .chats
                .iter()
                .filter(|chat| chat.folder == folder.id)
                .cloned()
            {
                let chat_id = chat.id.clone();
                let row_id = chat_row_index;
                chat_row_index += 1;
                let is_selected = self.model.selected_chat.as_deref() == Some(chat.id.as_str());
                let title = chat.title.unwrap_or_else(|| "New Chat".into());
                tree_rows.push(
                    div()
                        .id(("chat", row_id))
                        .mx_2()
                        .ml(px(indent + 10.0))
                        .mb_1()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(if is_selected { SURFACE_HIGH } else { SURFACE }))
                        .text_color(rgb(if is_selected { TEXT } else { MUTED }))
                        .text_sm()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.select_chat(chat_id.clone(), cx)
                            }),
                        )
                        .child(if chat.working {
                            format!("●  {title}")
                        } else {
                            format!("   {title}")
                        })
                        .into_any_element(),
                );
            }
        }

        let sidebar = div()
            .w(px(280.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(SURFACE))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(58.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().text_lg().text_color(rgb(TEXT)).child("xd-dev"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(status_color))
                            .child(status_text),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("WORKSPACES"),
            )
            .child(
                div()
                    .id("workspace-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(tree_rows),
            );

        let title = selected
            .as_ref()
            .and_then(|chat| chat.title.clone())
            .unwrap_or_else(|| "Select a chat".into());
        let context = selected
            .as_ref()
            .map(|chat| chat.backend.clone())
            .unwrap_or_else(|| "xd daemon".into());
        let header = div()
            .h(px(58.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .px_5()
            .bg(rgb(SURFACE))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_sm().text_color(rgb(TEXT)).child(title))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(context)),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(rgb(if working { 0x26354d } else { SURFACE_HIGH }))
                    .text_xs()
                    .text_color(rgb(if working { 0xaec0ff } else { MUTED }))
                    .child(if working { "Working…" } else { "Ready" }),
            );

        let transcript = list(self.transcript.clone(), move |index, _window, _cx| {
            Self::message_row(&messages[index]).into_any_element()
        })
        .size_full();

        let composer_text = if self.composer.is_empty() {
            "Message xd…".to_owned()
        } else {
            self.composer.clone()
        };
        let composer_color = if self.composer.is_empty() {
            MUTED
        } else {
            TEXT
        };
        let composer_focus = self.composer_focus.clone();
        let can_send = !self.composer.trim().is_empty()
            && self.model.selected_chat.is_some()
            && self.model.connected
            && !self.sending;
        let send_label = if self.sending {
            "Sending…"
        } else if working {
            "Queue"
        } else {
            "Send"
        };

        let composer = div()
            .flex_shrink_0()
            .px_5()
            .pt_2()
            .pb_4()
            .bg(rgb(BG))
            .when_some(self.model.connection_error.clone(), |element, error| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(920.0))
                        .mx_auto()
                        .mb_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(0x382126))
                        .text_xs()
                        .text_color(rgb(0xefb1b1))
                        .child(error),
                )
            })
            .when(queue_count > 0, |element| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(920.0))
                        .mx_auto()
                        .mb_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(0x24212f))
                        .text_xs()
                        .text_color(rgb(0xc8b6e8))
                        .child(format!(
                            "{queue_count} queued message{}",
                            if queue_count == 1 { "" } else { "s" }
                        )),
                )
            })
            .child(
                div()
                    .id("composer")
                    .track_focus(&composer_focus)
                    .w_full()
                    .max_w(px(920.0))
                    .mx_auto()
                    .min_h(px(74.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(if composer_focus.is_focused(window) {
                        ACCENT
                    } else {
                        BORDER
                    }))
                    .bg(rgb(SURFACE))
                    .on_click(cx.listener(|this, _, window, _| window.focus(&this.composer_focus)))
                    .on_key_down(cx.listener(Self::on_composer_key_down))
                    .child(
                        div()
                            .flex_1()
                            .whitespace_normal()
                            .text_sm()
                            .line_height(px(21.0))
                            .text_color(rgb(composer_color))
                            .child(composer_text),
                    )
                    .child(
                        div()
                            .id("send")
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .bg(rgb(if can_send { ACCENT } else { SURFACE_HIGH }))
                            .text_sm()
                            .text_color(rgb(if can_send { 0xffffff } else { MUTED }))
                            .when(can_send, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x7b98ff)))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_send {
                                    this.send_composer(cx);
                                }
                            }))
                            .child(send_label),
                    ),
            );

        div()
            .size_full()
            .flex()
            .bg(rgb(BG))
            .font_family("Inter")
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(transcript))
                    .child(composer),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1180.0), px(780.0)), cx);
        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(XdDesktop::new),
        )
        .expect("open xd GPUI window");
        cx.activate(true);
    });
}
