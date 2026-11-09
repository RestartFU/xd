//! Assistant accounts in Settings, Agents: several sign-ins per assistant, the
//! order turns use them in, signing in without a terminal, and the notice a
//! chat shows when it moves to another account.
use super::*;
use serde_json::json;

pub(super) const CHEVRON_UP_ICON: &str = "icons/chevron-up.svg";
pub(super) const COPY_ICON: &str = "icons/copy.svg";
pub(super) const SWITCH_ICON: &str = "icons/arrow-left-right.svg";

/// Marks a transcript row that says a chat moved to another account.
const ACCOUNT_SWITCH_PREFIX: &str = "account_switch\n";
/// The assistants that can hold several accounts, in the order shown.
const ACCOUNT_PROVIDERS: [&str; 2] = ["codex", "claude"];

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(super) struct AuthAccount {
    pub(super) id: String,
    #[serde(default)]
    pub(super) default: bool,
    #[serde(default)]
    pub(super) pending: bool,
    #[serde(default)]
    pub(super) label: String,
    #[serde(default)]
    pub(super) plan: Option<String>,
    #[serde(default)]
    pub(super) state: String,
    #[serde(default)]
    pub(super) detail: Option<String>,
    #[serde(default)]
    pub(super) login_url: Option<String>,
    #[serde(default)]
    pub(super) device_code: Option<String>,
    #[serde(default)]
    pub(super) needs_input: bool,
    #[serde(default)]
    pub(super) in_use: bool,
    #[serde(default)]
    pub(super) busy: bool,
    #[serde(default)]
    pub(super) problem: Option<String>,
    #[serde(default)]
    pub(super) limited_until_ms: Option<i64>,
    #[serde(default)]
    pub(super) limited_reason: Option<String>,
    #[serde(default)]
    pub(super) windows: Vec<AccountWindow>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct AccountWindow {
    kind: String,
    #[serde(default)]
    label: String,
    used_percent: f64,
}

impl AuthProvider {
    /// The provider's accounts. A host from before accounts reports only the
    /// user's own sign-in, at the top level.
    pub(super) fn account_list(&self) -> Vec<AuthAccount> {
        if !self.accounts.is_empty() {
            return self.accounts.clone();
        }
        vec![AuthAccount {
            id: "default".into(),
            default: true,
            label: format!("{} sign-in", presentation::provider_name(&self.provider)),
            state: self.state.clone(),
            detail: self.detail.clone(),
            login_url: self.login_url.clone(),
            device_code: self.device_code.clone(),
            needs_input: self.needs_input,
            in_use: true,
            ..AuthAccount::default()
        }]
    }

    /// Whether the host can keep more than the user's own sign-in.
    fn holds_several(&self) -> bool {
        !self.accounts.is_empty()
    }

    fn signing_in(&self) -> Option<AuthAccount> {
        self.account_list()
            .into_iter()
            .find(|account| account.state == "signing-in")
    }

    fn busy(&self) -> bool {
        self.account_list().iter().any(|account| {
            account.busy || matches!(account.state.as_str(), "signing-in" | "signing-out")
        })
    }
}

/// The sentence of an account-switch row, if the row is one.
pub(super) fn switch_notice(message: &Message) -> Option<&str> {
    (message.role == "tool")
        .then(|| message.content.strip_prefix(ACCOUNT_SWITCH_PREFIX))
        .flatten()
}

/// An account switch in the conversation: quiet, like any host notice.
pub(super) fn render_switch_notice(sentence: &str, colors: ThemeColors) -> gpui::AnyElement {
    div()
        .w_full()
        .max_w(px(780.0))
        .py(px(4.0))
        .flex()
        .flex_row()
        .gap(px(10.0))
        .child(
            div()
                .mt(px(3.0))
                .flex_shrink_0()
                .child(icon(SWITCH_ICON, 14.0, colors.muted)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.0))
                .line_height(px(20.0))
                .text_color(rgb(colors.muted))
                .child(sentence.to_owned()),
        )
        .into_any_element()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// "2 h 5 min", "12 min", "3 d".
fn time_until(at_ms: i64, now_ms: i64) -> String {
    let minutes = ((at_ms - now_ms).max(0) + 59_999) / 60_000;
    match (minutes / (24 * 60), minutes / 60 % 24, minutes % 60) {
        (0, 0, minutes) => format!("{} min", minutes.max(1)),
        (0, hours, 0) => format!("{hours} h"),
        (0, hours, minutes) => format!("{hours} h {minutes} min"),
        (days, 0, _) => format!("{days} d"),
        (days, hours, _) => format!("{days} d {hours} h"),
    }
}

/// What an account row says about the account, and in which color.
fn account_status(account: &AuthAccount, now: i64, colors: ThemeColors) -> (String, u32, bool) {
    let detail = || account.detail.clone();
    match account.state.as_str() {
        "signing-in" => {
            return (
                detail().unwrap_or_else(|| "Signing in…".into()),
                colors.muted,
                false,
            );
        }
        "signing-out" => {
            return (
                detail().unwrap_or_else(|| "Signing out…".into()),
                colors.muted,
                false,
            );
        }
        "signed-out" => return ("Signed out".into(), colors.muted, false),
        "failed" => {
            return (
                detail().unwrap_or_else(|| "Sign-in failed".into()),
                colors.danger,
                false,
            );
        }
        _ => {}
    }
    match account.problem.as_deref() {
        Some("mismatch") => {
            return (
                "Now signed in to a different account, so it is not used".into(),
                colors.danger,
                false,
            );
        }
        Some(_) => {
            return (
                "Same login as an account above, so it is not used".into(),
                colors.danger,
                false,
            );
        }
        None => {}
    }
    if let Some(until) = account.limited_until_ms.filter(|until| *until > now) {
        let reason = account.limited_reason.as_deref().unwrap_or("usage limit");
        return (
            format!("Out of its {reason} · back in {}", time_until(until, now)),
            colors.danger,
            false,
        );
    }
    if matches!(account.state.as_str(), "checking" | "unknown" | "") {
        return ("Checking…".into(), colors.faint, false);
    }
    if account.in_use {
        return ("In use".into(), colors.working, true);
    }
    ("Ready".into(), colors.muted, false)
}

/// The session and weekly allowance, as what is left.
fn account_meters(account: &AuthAccount, colors: ThemeColors) -> Option<gpui::Div> {
    let left = |kind: &str| {
        account
            .windows
            .iter()
            .find(|window| window.kind == kind && !window.label.contains('·'))
            .map(|window| (100.0 - window.used_percent).clamp(0.0, 100.0).round() as i64)
    };
    let meters = [("5h", left("session")), ("Week", left("weekly"))]
        .into_iter()
        .filter_map(|(label, left)| left.map(|left| (label, left)))
        .collect::<Vec<_>>();
    if meters.is_empty() || account.state != "signed-in" {
        return None;
    }
    Some(
        div()
            .w(px(132.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .children(meters.into_iter().map(|(label, left)| {
                let tone = if left <= 0 {
                    colors.danger
                } else if left <= 15 {
                    colors.waiting
                } else {
                    colors.muted
                };
                div()
                    .h(px(15.0))
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .text_size(px(11.0))
                    .child(
                        div()
                            .w(px(30.0))
                            .flex_none()
                            .text_color(rgb(colors.faint))
                            .child(label),
                    )
                    .child(div().flex_1().min_w_0().child(usage_view::flyout_bar(
                        left as f64 / 100.0,
                        tone,
                        4.0,
                        colors,
                    )))
                    .child(
                        div()
                            .w(px(30.0))
                            .flex_none()
                            .text_right()
                            .text_color(rgb(if left <= 15 { tone } else { colors.muted }))
                            .child(format!("{left}%")),
                    )
            })),
    )
}

/// A small square button holding one icon. Without `enabled` it is drawn
/// faint and does nothing.
fn icon_button(
    id: impl Into<gpui::ElementId>,
    path: &'static str,
    enabled: bool,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    let button = div()
        .id(id)
        .size(px(26.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0));
    if enabled {
        button
            .cursor_pointer()
            .hover(|style| style.bg(rgb(colors.surface_high)))
            .child(icon(path, 14.0, colors.muted))
    } else {
        button.child(icon(path, 14.0, colors.border))
    }
}

impl XdDesktop {
    pub(super) fn request_agent_accounts(&mut self) {
        if let Some(host) = self.active_host()
            && let Err(error) = host.agent_auth()
        {
            self.model.connection_error = Some(error);
        }
    }

    fn account_action(
        &mut self,
        operation: &str,
        provider: &str,
        extra: Value,
        cx: &mut Context<Self>,
    ) {
        self.account_error = None;
        if let Some(host) = self.active_host()
            && let Err(error) = host.agent_account_action(operation, provider, extra)
        {
            self.account_error = Some(error);
        }
        cx.notify();
    }

    fn move_account(&mut self, provider: &str, index: usize, up: bool, cx: &mut Context<Self>) {
        let Some(found) = self
            .auth_providers
            .iter()
            .find(|candidate| candidate.provider == provider)
        else {
            return;
        };
        let mut order = found
            .account_list()
            .into_iter()
            .filter(|account| !account.pending)
            .map(|account| account.id)
            .collect::<Vec<_>>();
        let other = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1).filter(|next| *next < order.len())
        };
        let Some(other) = other else {
            return;
        };
        order.swap(index, other);
        self.account_action(
            "agent-accounts-order",
            provider,
            json!({"accounts": order}),
            cx,
        );
    }

    /// Settings, Agents: every account of each assistant, in the order turns
    /// use them, with signing in and out where the account is.
    pub(super) fn render_account_settings(
        &self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let providers = ACCOUNT_PROVIDERS
            .iter()
            .enumerate()
            .filter_map(|(index, id)| {
                self.auth_providers
                    .iter()
                    .find(|provider| provider.provider == *id)
                    .map(|provider| self.render_provider_accounts(index, provider, colors, cx))
            })
            .collect::<Vec<_>>();
        let loading = providers.is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(22.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(colors.muted))
                    .child(
                        "Each assistant can hold several sign-ins. New chats start on the first one \
                         with room left; a chat stays on its account until that account runs out.",
                    ),
            )
            .when(loading, |body| {
                body.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(colors.faint))
                        .child("Checking sign-ins…"),
                )
            })
            .when_some(self.account_error.clone(), |body, error| {
                body.child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(rgb(colors.danger))
                        .child(
                            div()
                                .mt(px(2.0))
                                .child(icon(usage_view::ALERT_ICON, 13.0, colors.danger)),
                        )
                        .child(error),
                )
            })
            .children(providers)
            .into_any_element()
    }

    fn render_provider_accounts(
        &self,
        provider_index: usize,
        provider: &AuthProvider,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = provider.provider.clone();
        let name = presentation::provider_name(&id).to_owned();
        let accounts = provider.account_list();
        let ranked = accounts.iter().filter(|account| !account.pending).count();
        let several = ranked > 1;
        let busy = provider.busy();
        let can_add = provider.holds_several() && !busy && !provider.adding;
        let now = now_ms();
        let mut rows = Vec::new();
        for (index, account) in accounts.iter().enumerate() {
            if account.pending {
                continue;
            }
            rows.push(self.render_account_row(
                provider_index * 100 + index,
                &id,
                index,
                ranked,
                account,
                busy,
                now,
                colors,
                cx,
            ));
        }
        let signing_in = provider.signing_in();
        let add_id = id.clone();
        let switch_id = id.clone();
        let auto_switch = provider.auto_switch;
        let mark = match id.as_str() {
            "claude" => CLAUDE_ICON,
            _ => CODEX_ICON,
        };
        div()
            .id(("account-provider", provider_index))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(assistant_icon(mark, 16.0, colors.text, colors))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(name.clone()),
                    )
                    .when(provider.holds_several(), |header| {
                        header.child(
                            ui_button(("add-account", provider_index), ButtonKind::Ghost, colors)
                                .when(!can_add, |button| {
                                    button.opacity(0.45).cursor_default()
                                })
                                .when(can_add, |button| {
                                    button.on_click(cx.listener(move |this, _, _, cx| {
                                        this.account_action("agent-auth-add", &add_id, json!({}), cx);
                                    }))
                                })
                                .child(icon(usage_view::PLUS_ICON, 14.0, colors.muted))
                                .child("Add account"),
                        )
                    }),
            )
            .child(
                div()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.surface))
                    .overflow_hidden()
                    .children(rows)
                    .when_some(signing_in, |panel, account| {
                        panel.child(self.render_sign_in(provider_index, &id, &account, colors, cx))
                    }),
            )
            .when_some(provider.notice.clone(), |block, notice| {
                block.child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(rgb(colors.muted))
                        .child(notice),
                )
            })
            .when(several, |block| {
                block.child(
                    div()
                        .id(("account-auto-switch", provider_index))
                        .mt(px(2.0))
                        .flex()
                        .items_center()
                        .gap_4()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.account_action(
                                "agent-accounts-auto-switch",
                                &switch_id,
                                json!({"enabled": !auto_switch}),
                                cx,
                            );
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .child("Switch accounts when one runs out"),
                                )
                                .child(
                                    div()
                                        .mt(px(4.0))
                                        .text_size(px(12.0))
                                        .line_height(px(18.0))
                                        .text_color(rgb(colors.muted))
                                        .child(if id == "codex" {
                                            "At a 5-hour, weekly, or monthly limit, the chat continues on the next account with room. Codex starts a new session there, given the chat's messages."
                                        } else {
                                            "At a 5-hour or weekly limit, the chat continues on the next account with room, in the same session."
                                        }),
                                ),
                        )
                        .child(settings_switch(colors, auto_switch)),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_account_row(
        &self,
        element: usize,
        provider: &str,
        index: usize,
        ranked: usize,
        account: &AuthAccount,
        busy: bool,
        now: i64,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let several = ranked > 1;
        let (status, tone, dot) = account_status(account, now, colors);
        let confirming =
            self.account_remove_confirm
                .as_ref()
                .is_some_and(|(confirm_provider, confirm_id)| {
                    confirm_provider == provider && confirm_id == &account.id
                });
        let mut facts = vec![];
        if let Some(plan) = &account.plan {
            facts.push(plan.clone());
        }
        if account.default {
            facts.push("CLI sign-in".to_owned());
        }
        let signed_out = matches!(account.state.as_str(), "signed-out" | "failed")
            || account.problem.as_deref() == Some("mismatch");
        let in_flight = matches!(account.state.as_str(), "signing-in" | "signing-out");
        let provider_owned = provider.to_owned();
        let account_id = account.id.clone();
        // A steady width keeps the meters of every row in one column.
        let mut actions = div()
            .flex_none()
            .min_w(px(if several { 150.0 } else { 90.0 }))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(2.0));
        if several && !confirming {
            let up_provider = provider_owned.clone();
            let down_provider = provider_owned.clone();
            actions =
                actions
                    .child(
                        icon_button(("account-up", element), CHEVRON_UP_ICON, index > 0, colors)
                            .when(index > 0, |button| {
                                button.on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_account(&up_provider, index, true, cx);
                                }))
                            }),
                    )
                    .child(
                        icon_button(
                            ("account-down", element),
                            CHEVRON_DOWN_ICON,
                            index + 1 < ranked,
                            colors,
                        )
                        .when(index + 1 < ranked, |button| {
                            button.on_click(cx.listener(move |this, _, _, cx| {
                                this.move_account(&down_provider, index, false, cx);
                            }))
                        }),
                    );
        }
        if confirming {
            let remove_provider = provider_owned.clone();
            let remove_id = account_id.clone();
            actions = actions
                .child(
                    ui_button(("account-keep", element), ButtonKind::Ghost, colors)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.account_remove_confirm = None;
                            cx.notify();
                        }))
                        .child("Keep"),
                )
                .child(
                    ui_button(
                        ("account-remove-confirm", element),
                        ButtonKind::Danger,
                        colors,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.account_remove_confirm = None;
                        this.account_action(
                            "agent-auth-logout",
                            &remove_provider,
                            json!({"account": remove_id}),
                            cx,
                        );
                    }))
                    .child("Remove"),
                );
        } else if !in_flight && !busy {
            if signed_out {
                let sign_in_provider = provider_owned.clone();
                let sign_in_id = account_id.clone();
                actions = actions.child(
                    ui_button(("account-sign-in", element), ButtonKind::Secondary, colors)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.account_action(
                                "agent-auth-start",
                                &sign_in_provider,
                                json!({"account": sign_in_id}),
                                cx,
                            );
                        }))
                        .child(if account.default {
                            "Sign in"
                        } else {
                            "Sign in again"
                        }),
                );
            }
            if account.default {
                if !signed_out {
                    let sign_out_provider = provider_owned.clone();
                    actions = actions.child(
                        ui_button(("account-sign-out", element), ButtonKind::Ghost, colors)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.account_action(
                                    "agent-auth-logout",
                                    &sign_out_provider,
                                    json!({}),
                                    cx,
                                );
                            }))
                            .child("Sign out"),
                    );
                }
            } else {
                let confirm_provider = provider_owned.clone();
                let confirm_id = account_id.clone();
                actions = actions.child(
                    ui_button(("account-remove", element), ButtonKind::Ghost, colors)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.account_remove_confirm =
                                Some((confirm_provider.clone(), confirm_id.clone()));
                            cx.notify();
                        }))
                        .child("Remove"),
                );
            }
        }
        div()
            .id(("account-row", element))
            .min_h(px(60.0))
            .px(px(14.0))
            .py(px(11.0))
            .flex()
            .items_center()
            .gap(px(14.0))
            .when(index > 0, |row| {
                row.border_t_1().border_color(rgb(colors.border))
            })
            .when(several, |row| {
                row.child(
                    div()
                        .w(px(12.0))
                        .flex_none()
                        .text_size(px(12.0))
                        .text_color(rgb(colors.faint))
                        .child((index + 1).to_string()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(px(13.0))
                                    .text_color(rgb(colors.text))
                                    .truncate()
                                    .child(account.label.clone()),
                            )
                            .when(!facts.is_empty(), |line| {
                                line.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(12.0))
                                        .text_color(rgb(colors.faint))
                                        .child(facts.join(" · ")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(px(12.0))
                            .line_height(px(17.0))
                            .when(dot, |line| {
                                line.child(
                                    div().flex_none().size(px(6.0)).rounded_full().bg(rgb(tone)),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(tone))
                                    .child(if confirming {
                                        "Signs it out and deletes its saved sign-in from this host."
                                            .to_owned()
                                    } else {
                                        status
                                    }),
                            ),
                    ),
            )
            .when_some(
                account_meters(account, colors).filter(|_| !confirming),
                |row, meters| row.child(meters),
            )
            .child(actions)
            .into_any_element()
    }

    /// The steps of a sign-in in progress, inside the assistant's panel.
    fn render_sign_in(
        &self,
        provider_index: usize,
        provider: &str,
        account: &AuthAccount,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let name = presentation::provider_name(provider);
        let title = if account.pending {
            format!("Add a {name} account")
        } else {
            format!("Sign in to {}", account.label)
        };
        let cancel_provider = provider.to_owned();
        let url = account.login_url.clone();
        let waiting = account.login_url.is_none() && account.device_code.is_none();
        div()
            .id(("account-sign-in-steps", provider_index))
            .border_t_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.background))
            .px(px(16.0))
            .py(px(16.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .when(account.pending, |heading| {
                        heading.child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(rgb(colors.muted))
                                .child(
                                    "Sign in with the account you want to add. If your browser is already \
                                     signed in to another one, switch accounts there or use a private window.",
                                ),
                        )
                    }),
            )
            .when(waiting, |steps| {
                steps.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(colors.faint))
                        .child(account.detail.clone().unwrap_or_else(|| "Starting sign-in…".into())),
                )
            })
            .when_some(account.device_code.clone(), |steps, code| {
                let copied = code.clone();
                steps.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            div()
                                .px(px(14.0))
                                .py(px(8.0))
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.surface))
                                .font_family(MONO)
                                .text_size(px(20.0))
                                .line_height(px(26.0))
                                .text_color(rgb(colors.text))
                                .child(code),
                        )
                        .child(
                            ui_button(("account-copy-code", provider_index), ButtonKind::Ghost, colors)
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                                }))
                                .child(icon(COPY_ICON, 14.0, colors.muted))
                                .child("Copy code"),
                        ),
                )
            })
            .when_some(url, |steps, url| {
                let opened = url.clone();
                let copied = url.clone();
                steps.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            ui_button(("account-open-page", provider_index), ButtonKind::Primary, colors)
                                .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&opened)))
                                .child(icon(EXTERNAL_LINK_ICON, 14.0, colors.accent_text))
                                .child("Open sign-in page"),
                        )
                        .child(
                            ui_button(("account-copy-link", provider_index), ButtonKind::Ghost, colors)
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                                }))
                                .child("Copy link"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(colors.faint))
                                .child(if account.device_code.is_some() {
                                    "Enter the code on that page."
                                } else if account.needs_input {
                                    "Then paste the code the page shows."
                                } else {
                                    "Waiting for you to finish in the browser…"
                                }),
                        ),
                )
            })
            .when(account.needs_input, |steps| {
                steps.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h(px(34.0))
                                .px(px(10.0))
                                .flex()
                                .items_center()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.surface))
                                .child(self.auth_input.clone()),
                        )
                        .child(
                            ui_button(("account-submit-code", provider_index), ButtonKind::Secondary, colors)
                                .on_click(cx.listener(|this, _, _, cx| this.submit_auth_input(cx)))
                                .child("Submit code"),
                        ),
                )
            })
            .child(
                div().flex().child(
                    ui_button(("account-cancel-sign-in", provider_index), ButtonKind::Ghost, colors)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.account_action("agent-auth-cancel", &cancel_provider, json!({}), cx);
                        }))
                        .child("Cancel"),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(value: Value) -> AuthProvider {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_host_from_before_accounts_shows_its_one_sign_in() {
        let old = provider(json!({"provider":"codex","state":"signed-in"}));
        let accounts = old.account_list();
        assert_eq!(accounts.len(), 1);
        assert!(accounts[0].default && accounts[0].in_use);
        assert_eq!(accounts[0].label, "Codex sign-in");
        assert!(!old.holds_several());
    }

    #[test]
    fn rows_say_what_each_account_is_doing() {
        let colors = ThemePreset::Dark.colors();
        let now = 1_000_000;
        let account = |value: Value| serde_json::from_value::<AuthAccount>(value).unwrap();
        let in_use = account(json!({"id":"default","state":"signed-in","in_use":true}));
        assert_eq!(
            account_status(&in_use, now, colors),
            ("In use".into(), colors.working, true)
        );
        let spent = account(json!({
            "id":"a","state":"signed-in","limited_until_ms": now + 2 * 3_600_000 + 5 * 60_000,
            "limited_reason":"5-hour limit"
        }));
        assert_eq!(
            account_status(&spent, now, colors).0,
            "Out of its 5-hour limit · back in 2 h 5 min"
        );
        let moved = account(json!({"id":"a","state":"signed-in","problem":"mismatch"}));
        assert_eq!(account_status(&moved, now, colors).1, colors.danger);
        let out = account(json!({"id":"a","state":"signed-out"}));
        assert_eq!(account_status(&out, now, colors).0, "Signed out");
    }

    #[test]
    fn switch_rows_are_notices_not_activity() {
        let notice = Message::new(
            Some(1),
            "tool",
            "account_switch\nSwitched to b@example.com.",
            None,
        );
        assert_eq!(switch_notice(&notice), Some("Switched to b@example.com."));
        assert!(!activity::is_plain_activity(&notice.content));
        let reply = Message::new(Some(2), "assistant", "account_switch\nno", None);
        assert_eq!(switch_notice(&reply), None);
    }
}
