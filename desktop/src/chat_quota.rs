//! Chat quotas: the host's default share of an account's usage limit that each
//! chat may use, and the card a chat shows once it has used its share.

use super::*;
use xd_desktop::model::{parse_quota_percent, quota_points};

/// What the open chat's quota card holds between frames.
#[derive(Debug, Default)]
pub(super) struct QuotaCard {
    /// The stop the field was last filled for: the chat and the quota it
    /// stopped at. A new stop fills it again; a frame of the same one keeps
    /// what the user typed.
    filled_for: Option<(String, Option<u8>)>,
    /// What the new quota field holds.
    pub(super) text: String,
    /// The chat a `set-chat-quota` is on its way for, so its buttons wait.
    pub(super) pending: Option<String>,
}

pub(super) fn new_quota_input(cx: &mut Context<XdDesktop>) -> Entity<ComposerInput> {
    let input = cx.new(|cx| {
        let mut input = ComposerInput::new(cx, "");
        input.set_compact(true);
        input
    });
    cx.subscribe(&input, |this, _, event, cx| match event {
        ComposerEvent::Changed(text) => {
            this.quota_card.text = text.clone();
            cx.notify();
        }
        ComposerEvent::Submit => {
            if let Some(percent) = parse_quota_percent(&this.quota_card.text) {
                this.continue_with_quota(Some(percent), cx);
            }
        }
        ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
    })
    .detach();
    input
}

impl XdDesktop {
    /// Asks for the host's default quota, on a host that has quotas.
    pub(super) fn request_chat_quota_settings(&mut self) {
        if !self.model.chat_quotas_supported {
            return;
        }
        if let Some(host) = self.active_host()
            && let Err(error) = host.chat_quota_settings()
        {
            self.model.connection_error = Some(error);
        }
    }

    pub(super) fn change_chat_quota_settings(
        &mut self,
        settings: ChatQuotaSettings,
        cx: &mut Context<Self>,
    ) {
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.set_chat_quota_settings(settings));
        match result {
            Ok(()) => self.chat_quota = Some(settings),
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    /// The open chat stopped at its quota and nothing runs in it.
    pub(super) fn quota_card_visible(&self) -> bool {
        self.model.selected_chat.is_some() && self.model.quota_reached && !self.model.working
    }

    /// Fills the new quota field when the open chat has newly stopped at its
    /// quota: double the last one, up to the whole limit.
    pub(super) fn sync_quota_input(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if !self.model.quota_reached {
            self.quota_card.filled_for = None;
            return;
        }
        let last = self.model.quota.map(|quota| quota.percent);
        let stop = (chat_id, last);
        if self.quota_card.filled_for.as_ref() == Some(&stop) {
            return;
        }
        let next = ChatQuota::next_percent(
            last.or(self.chat_quota.and_then(|settings| settings.percent))
                .unwrap_or(ChatQuotaSettings::DEFAULT_PERCENT),
        )
        .to_string();
        self.quota_input
            .update(cx, |input, cx| input.set_text(next.clone(), cx));
        self.quota_card.text = next;
        self.quota_card.filled_for = Some(stop);
    }

    /// Gives the open chat a new quota, or none, and so lets it go on.
    pub(super) fn continue_with_quota(&mut self, percent: Option<u8>, cx: &mut Context<Self>) {
        if self.quota_card.pending.is_some() {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.set_chat_quota(&chat_id, percent));
        match result {
            Ok(()) => self.quota_card.pending = Some(chat_id),
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    /// The host answered a `set-chat-quota`, either way: the chat's state
    /// says whether it went on.
    pub(super) fn chat_quota_set(&mut self, chat_id: &str) {
        if self.quota_card.pending.as_deref() == Some(chat_id) {
            self.quota_card.pending = None;
        }
        if self.chat_is_active(chat_id) {
            self.request_chat(chat_id);
        }
    }

    /// In place of the resume button: how much of its quota the chat used,
    /// and a new quota to go on with.
    pub(super) fn render_quota_card(
        &self,
        colors: ThemeColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.quota_card_visible() {
            return None;
        }
        let sentence = match self.model.quota {
            Some(quota) => format!(
                "This chat used its quota: {}% of {}% of the 5-hour limit.",
                quota_points(quota.used),
                quota.percent
            ),
            None => "This chat used its quota of the 5-hour limit.".to_owned(),
        };
        let waiting = self.quota_card.pending.is_some();
        let percent = parse_quota_percent(&self.quota_card.text);
        let can_continue = !waiting && self.model.connected && percent.is_some();
        let can_lift = !waiting && self.model.connected;
        let focused = self
            .quota_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        Some(
            div()
                .id("chat-quota-card")
                .debug_selector(|| "chat-quota-card".into())
                .w_full()
                .px_4()
                .py_3()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.surface))
                .text_size(px(13.0))
                .text_color(rgb(colors.text))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .debug_selector(|| "chat-quota-sentence".into())
                                .child(sentence),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(colors.muted))
                                .child("Messages you send wait in the queue until it goes on."),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgb(colors.muted))
                                .child("New quota"),
                        )
                        .child(
                            div()
                                .id("chat-quota-input")
                                .w(px(52.0))
                                .h(px(30.0))
                                .px(px(8.0))
                                .flex()
                                .items_center()
                                .cursor_text()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(rgb(if focused {
                                    colors.selected_border
                                } else {
                                    colors.border
                                }))
                                .bg(rgb(colors.background))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.quota_input.read(cx).focus_handle(cx).focus(window);
                                    }),
                                )
                                .child(div().w_full().child(self.quota_input.clone())),
                        )
                        .child(
                            div()
                                .mr_2()
                                .text_size(px(12.5))
                                .text_color(rgb(colors.muted))
                                .child("%"),
                        )
                        .child(
                            ui_button("chat-quota-continue", ButtonKind::Primary, colors)
                                .debug_selector(|| "chat-quota-continue".into())
                                .when(!can_continue, |button| button.opacity(0.5))
                                .when_some(percent.filter(|_| can_continue), |button, percent| {
                                    button.on_click(cx.listener(move |this, _, _, cx| {
                                        this.continue_with_quota(Some(percent), cx)
                                    }))
                                })
                                .child("Continue"),
                        )
                        .child(
                            ui_button("chat-quota-lift", ButtonKind::Secondary, colors)
                                .debug_selector(|| "chat-quota-lift".into())
                                .when(!can_lift, |button| button.opacity(0.5))
                                .when(can_lift, |button| {
                                    button.on_click(cx.listener(|this, _, _, cx| {
                                        this.continue_with_quota(None, cx)
                                    }))
                                })
                                .child("Continue without a limit"),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Beside the context meter while a quota applies: how much of it the
    /// chat has used.
    pub(super) fn render_quota_meter(&self, colors: ThemeColors) -> Option<gpui::AnyElement> {
        let quota = self.model.quota.filter(|_| !self.model.quota_reached)?;
        let share = quota.used / f64::from(quota.percent);
        let tone = if share >= 0.75 {
            colors.waiting
        } else {
            colors.faint
        };
        Some(
            div()
                .id("native-quota-meter")
                .debug_selector(|| "native-quota-meter".into())
                .flex_none()
                .mr_2()
                .text_size(px(11.0))
                .text_color(rgb(tone))
                .tooltip(move |_, cx| {
                    cx.new(|_| PaneTooltip {
                        label: "This chat's quota of its account's 5-hour limit",
                        colors,
                    })
                    .into()
                })
                .child(format!(
                    "Quota {} / {}%",
                    quota_points(quota.used),
                    quota.percent
                ))
                .into_any_element(),
        )
    }
}
