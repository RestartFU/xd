//! Chat quotas: the host's default share of an account's usage limit that each
//! chat may use, and what a chat's composer shows once it has used its share.

use super::*;
use crate::metrics::{
    CONTROL_HEIGHT, RADIUS_CONTROL, TEXT_CAPTION, TEXT_CONTROL, TEXT_LABEL, TEXT_TITLE,
};
use xd_desktop::model::{parse_quota_percent, quota_points};

/// What the open chat's quota card holds between frames.
#[derive(Debug, Default)]
pub(super) struct QuotaCard {
    /// The stop the field was last filled for: the chat and the quota it
    /// stopped at. A new stop fills it again; a frame of the same one keeps
    /// what the user typed.
    pub(super) filled_for: Option<(String, Option<u8>)>,
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
        let broadcasts = self.chat_quota_broadcasts;
        if let Some(host) = self.active_host()
            && let Err(error) = host.chat_quota_settings(broadcasts)
        {
            self.model.connection_error = Some(error);
        }
    }

    pub(super) fn change_chat_quota_settings(
        &mut self,
        settings: ChatQuotaSettings,
        cx: &mut Context<Self>,
    ) {
        let broadcasts = self.chat_quota_broadcasts;
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.set_chat_quota_settings(settings, broadcasts));
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

    /// Docked in the composer, in place of the resume button: how much of
    /// its quota the chat used, and a new quota to go on with.
    pub(super) fn render_quota_card(
        &self,
        colors: ThemeColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.quota_card_visible() {
            return None;
        }
        let used = match self.model.quota {
            Some(quota) => div()
                .flex()
                .items_baseline()
                .gap(px(18.0))
                .debug_selector(|| "chat-quota-detail".into())
                .child(
                    div()
                        .flex_none()
                        .text_size(text_px(TEXT_TITLE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{}%", quota_points(quota.used))),
                )
                .child(div().min_w_0().text_color(rgb(colors.muted)).child(format!(
                    "used of the 5-hour limit, past its {}% quota",
                    quota.percent
                ))),
            None => div()
                .text_color(rgb(colors.muted))
                .debug_selector(|| "chat-quota-detail".into())
                .child("It used its share of its account's 5-hour limit."),
        };
        let waiting = self.quota_card.pending.is_some();
        let percent = parse_quota_percent(&self.quota_card.text);
        let can_continue = !waiting && self.model.connected && percent.is_some();
        let can_lift = !waiting && self.model.connected;
        let invalid = percent.is_none();
        let field = self.quota_input.read(cx).focus_handle(cx);
        let focused = field.is_focused(window);
        let keys = self.key_targets.clone();
        let field_border = if invalid {
            colors.danger
        } else if focused {
            colors.selected_border
        } else {
            colors.control_border
        };
        Some(
            div()
                .id("chat-quota-card")
                .debug_selector(|| "chat-quota-card".into())
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .text_size(text_px(TEXT_CONTROL))
                .text_color(rgb(colors.text))
                .child(
                    composer::ask_header("This chat reached its quota", None, colors)
                        .debug_selector(|| "chat-quota-sentence".into()),
                )
                .child(used)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(div().text_color(rgb(colors.muted)).child("New quota"))
                        .child(
                            div()
                                .id("chat-quota-input")
                                .w(px(70.0))
                                .h(px(CONTROL_HEIGHT))
                                .px(px(10.0))
                                .flex()
                                .items_center()
                                .gap_1()
                                .cursor_text()
                                .rounded(px(RADIUS_CONTROL))
                                .border_1()
                                .border_color(rgb(field_border))
                                .bg(rgb(colors.background))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.quota_input.read(cx).focus_handle(cx).focus(window);
                                    }),
                                )
                                .child(div().flex_1().min_w_0().child(self.quota_input.clone()))
                                .child(div().flex_none().text_color(rgb(colors.faint)).child("%"))
                                // Tab stops on it; Enter in it is Continue.
                                .child(keys.mark(KeyLayer::Page, KeyStop::Field(field))),
                        )
                        .child(
                            ui_button_when(
                                can_continue,
                                "chat-quota-continue",
                                ButtonKind::Primary,
                                colors,
                            )
                            .debug_selector(|| "chat-quota-continue".into())
                            .when_some(percent.filter(|_| can_continue), |button, percent| {
                                button.key_stop(
                                    &keys,
                                    KeyLayer::Page,
                                    "chat-quota-continue",
                                    colors,
                                    cx,
                                    move |this, _, cx| this.continue_with_quota(Some(percent), cx),
                                )
                            })
                            .child("Continue"),
                        )
                        .child(div().flex_1())
                        // No limit at all is the bigger step, so it asks for
                        // less attention than going on with a quota.
                        .child(
                            ui_button_when(can_lift, "chat-quota-lift", ButtonKind::Ghost, colors)
                                .debug_selector(|| "chat-quota-lift".into())
                                .when(can_lift, |button| {
                                    button.key_stop(
                                        &keys,
                                        KeyLayer::Page,
                                        "chat-quota-lift",
                                        colors,
                                        cx,
                                        |this, _, cx| this.continue_with_quota(None, cx),
                                    )
                                })
                                .child("Continue without a limit"),
                        ),
                )
                .when(invalid, |card| {
                    card.child(
                        div()
                            .debug_selector(|| "chat-quota-invalid".into())
                            .text_size(text_px(TEXT_LABEL))
                            .text_color(rgb(colors.danger))
                            .child("Enter a whole number from 1 to 100."),
                    )
                })
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
                .text_size(text_px(TEXT_CAPTION))
                .text_color(rgb(tone))
                .tooltip(pane_tooltip(
                    "How much of its account's 5-hour limit this chat has used, \
                     against its quota. An estimate: chats working on one \
                     account share each rise.",
                    colors,
                ))
                .child(format!(
                    "Quota {}% of {}%",
                    quota_points(quota.used),
                    quota.percent
                ))
                .into_any_element(),
        )
    }

    /// Tells the user about a chat that ended a turn or stopped, if they
    /// want to hear: "{title} finished", "{title} stopped at its quota".
    pub(super) fn notify_chat_ended(
        &mut self,
        chat_id: Option<String>,
        title: &str,
        ending: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.notifications {
            return;
        }
        #[cfg(target_os = "linux")]
        self.turn_notifier.show(
            notifications::chat_ended_body(title, ending),
            TurnNotificationTarget {
                endpoint: self.active_endpoint,
                chat_id,
            },
            cx.background_executor(),
        );
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (chat_id, cx);
            notify_turn_finished(title, ending);
        }
    }

    /// Notifies about each chat the tree newly shows stopped at its quota,
    /// such as one that reached it as its turn ended, unless its turn's end
    /// already said so or the user is looking at it.
    pub(super) fn notify_new_quota_stops(&mut self, cx: &mut Context<Self>) {
        let stopped = self
            .model
            .chats
            .iter()
            .filter(|chat| chat.quota_reached)
            .map(|chat| (chat.id.clone(), chat.title.clone()))
            .collect::<Vec<_>>();
        let now = stopped
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<HashSet<_>>();
        self.quota_stops_announced.retain(|id| now.contains(id));
        let Some(seen) = self.quota_stops_seen.replace(now) else {
            return;
        };
        for (chat_id, title) in stopped {
            if seen.contains(&chat_id)
                || !self.quota_stops_announced.insert(chat_id.clone())
                || self.chat_is_active(&chat_id)
            {
                continue;
            }
            let title = title.unwrap_or_else(|| "Background chat".to_owned());
            self.notify_chat_ended(Some(chat_id), &title, QUOTA_STOP_ENDING, cx);
        }
    }
}
