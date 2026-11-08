//! Usage: a flyout over the sidebar's chart icon showing subscription limits,
//! cost, and tokens, and a full page for cost and tokens in detail. The full page
//! follows T3 Code (MIT, see `data/licenses/t3code-LICENSE`).

use std::{cell::Cell, collections::BTreeMap, rc::Rc, time::Duration};

use gpui::{
    Animation, AnimationExt, AnyElement, MouseMoveEvent, PathBuilder, Rgba, Subscription,
    Transformation, anchored, deferred, percentage, relative,
};
use serde_json::{Map, json};
use xd_desktop::usage::{
    Bucket, Chart, ChartMetric, ChartPoint, LimitAccount, LimitPool, Merged, ModelTotals,
    PROVIDER_ORDER, Pace, Period, PeriodTotals, PoolWindow, ResetCredits, Summary, VIEW_HEIGHT,
    VIEW_WIDTH, build_chart, collect_accounts, collect_notices, collect_pools, format_count,
    format_percent, format_tokens, format_usd, merge, parse_limit_accounts, parse_price_cell,
    provider_label, rank_providers, sort_models_by_tokens, source_messages,
};

use super::*;
use crate::input::{ComposerEvent, ComposerInput};

pub(super) const USAGE_ICON: &str = "icons/chart-no-axes-column.svg";
pub(super) const REFRESH_ICON: &str = "icons/refresh.svg";
pub(super) const INFO_ICON: &str = "icons/info.svg";
pub(super) const SLIDERS_ICON: &str = "icons/sliders-horizontal.svg";
pub(super) const ROTATE_CCW_ICON: &str = "icons/rotate-ccw.svg";
pub(super) const PLUS_ICON: &str = "icons/plus.svg";
pub(super) const TICKET_ICON: &str = "icons/ticket.svg";
pub(super) const ALERT_ICON: &str = "icons/circle-alert.svg";

const WINDOW_OPTIONS: [(u32, &str); 4] = [
    (1, "Past 24h"),
    (7, "7 days"),
    (30, "30 days"),
    (90, "90 days"),
];
const METRIC_OPTIONS: [(&str, &str); 3] =
    [("cost", "Cost"), ("tokens", "Tokens"), ("limits", "Limits")];
const PLOT_HEIGHT: f32 = 224.0;
const FLYOUT_METRICS: [(&str, &str); 3] =
    [("limits", "Limits"), ("cost", "Cost"), ("tokens", "Tokens")];
const FLYOUT_WIDTH: f32 = 348.0;
/// Lines the flyout up with the sidebar footer's icons, just above them.
const FLYOUT_LEFT: f32 = 8.0;
const FLYOUT_BOTTOM: f32 = 46.0;
const FLYOUT_BARS_HEIGHT: f32 = 44.0;
const CLAUDE_COLOR: u32 = 0xd97757;
const PRICE_FIELDS: [(&str, bool); 4] = [
    ("Input", false),
    ("Output", false),
    ("Cache read", true),
    ("Cache write", true),
];

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn local_time_zone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
}

fn endpoint_index(endpoint: ChatEndpoint) -> usize {
    match endpoint {
        ChatEndpoint::Local => 0,
        ChatEndpoint::Remote => 1,
    }
}

fn endpoint_at(index: usize) -> ChatEndpoint {
    if index == 0 {
        ChatEndpoint::Local
    } else {
        ChatEndpoint::Remote
    }
}

fn environment_label(index: usize) -> &'static str {
    if index == 0 { "Local" } else { "Remote" }
}

fn provider_color(provider: &str, colors: ThemeColors) -> u32 {
    if provider == "claude" {
        CLAUDE_COLOR
    } else {
        colors.text
    }
}

fn provider_icon(provider: &str) -> &'static str {
    if provider == "claude" {
        CLAUDE_ICON
    } else {
        CODEX_ICON
    }
}

/// `percent` of `over` on `base`, the page's only way of varying ink.
fn ink(over: u32, base: u32, percent: u32) -> u32 {
    let channel = |shift: u32| {
        let top = (over >> shift) & 0xff;
        let bottom = (base >> shift) & 0xff;
        (top * percent + bottom * (100 - percent)) / 100
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

fn with_alpha(color: u32, alpha: f32) -> Rgba {
    rgba((color << 8) | ((alpha.clamp(0.0, 1.0) * 255.0) as u32))
}

#[derive(Default)]
struct UsageEnvironment {
    summary: Option<Summary>,
    /// `(days, now_ms)` of the answer held in `summary`.
    summary_window: Option<(u32, i64)>,
    requested_window: Option<(u32, i64)>,
    summary_error: Option<String>,
    summary_loading: bool,
    summary_generation: u64,
    accounts: Option<Vec<LimitAccount>>,
    limits_error: Option<String>,
    limits_loading: bool,
    limits_generation: u64,
    prices: Option<Value>,
}

/// What the open confirmation will spend: this account's credit, on this host.
#[derive(Clone, PartialEq)]
struct ResetConfirm {
    environment: usize,
    account_key: String,
    identity: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum UsageMenu {
    Environment,
    Provider,
}

#[derive(Clone, Copy, PartialEq)]
struct ChartHover {
    chart: &'static str,
    index: usize,
    x: f32,
    y: f32,
}

struct PriceRow {
    id: u64,
    model: String,
    is_new: bool,
    removed: bool,
    alias: String,
    values: [String; 4],
    initial_alias: String,
    initial_values: [String; 4],
    model_input: Option<Entity<ComposerInput>>,
    alias_input: Entity<ComposerInput>,
    cell_inputs: [Entity<ComposerInput>; 4],
    _subscriptions: Vec<Subscription>,
}

impl PriceRow {
    fn changed(&self) -> bool {
        self.is_new
            || self.removed
            || self.alias.trim() != self.initial_alias
            || self
                .values
                .iter()
                .zip(&self.initial_values)
                .any(|(value, initial)| value.trim() != initial)
    }

    /// Why this row cannot be saved, or the price it would save.
    fn validate(&self) -> Result<Option<Value>, &'static str> {
        if self.removed {
            return Ok(None);
        }
        let model = self.model.trim();
        if model.is_empty() {
            return Err("Enter a model ID.");
        }
        let alias = self.alias.trim();
        if alias == model {
            return Err("A model cannot map to itself.");
        }
        if !alias.is_empty() {
            return Ok(None);
        }
        let mut rates = [None; 4];
        for (index, value) in self.values.iter().enumerate() {
            rates[index] = parse_price_cell(value).ok_or("Enter valid prices.")?;
        }
        if rates[0].is_none() || rates[1].is_none() {
            return Err("Enter input and output prices.");
        }
        Ok(Some(json!({
            "input": rates[0],
            "output": rates[1],
            "cache_read": rates[2],
            "cache_write": rates[3],
        })))
    }
}

struct PriceDialog {
    rows: Vec<PriceRow>,
    /// A model to open with, once the saved prices have arrived.
    pending_model: Option<String>,
    /// Identifies this dialog, so a closed one's late answers are ignored.
    id: u64,
    /// Hosts that still owe an answer to the current save.
    saving: Vec<usize>,
    error: Option<String>,
}

pub(super) struct UsagePage {
    environments: [UsageEnvironment; 2],
    hidden_environments: Vec<usize>,
    breakdown_by_time: bool,
    menu: Option<UsageMenu>,
    model: Option<String>,
    hover: Option<ChartHover>,
    segment: Option<String>,
    confirm_reset: Option<ResetConfirm>,
    /// The host a reset is waiting on.
    reset_busy: Option<usize>,
    /// Unanswered attempts by account. Retrying an account reuses its key, so a
    /// reset whose answer was lost cannot spend a second credit.
    reset_attempts: HashMap<String, String>,
    reset_status: Option<String>,
    price: Option<PriceDialog>,
    limits_now: i64,
    /// Cost and tokens in detail, instead of the flyout.
    full_page: bool,
    focus: FocusHandle,
    generation: u64,
    next_row: u64,
    overview_bounds: Rc<Cell<Bounds<Pixels>>>,
    model_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl UsagePage {
    pub(super) fn new(focus: FocusHandle) -> Self {
        Self {
            environments: Default::default(),
            hidden_environments: Vec::new(),
            breakdown_by_time: false,
            menu: None,
            model: None,
            hover: None,
            segment: None,
            confirm_reset: None,
            reset_busy: None,
            reset_attempts: HashMap::new(),
            reset_status: None,
            price: None,
            limits_now: now_ms(),
            full_page: false,
            focus,
            generation: 0,
            next_row: 0,
            overview_bounds: Rc::new(Cell::new(Bounds::default())),
            model_bounds: Rc::new(Cell::new(Bounds::default())),
        }
    }
}

/// A tooltip with a headline and quieter detail lines.
struct UsageTooltip {
    lines: Vec<String>,
    colors: ThemeColors,
}

impl Render for UsageTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors;
        div()
            .max_w(px(400.0))
            .px_2()
            .py_1()
            .font_family(UI_FONT)
            .flex()
            .flex_col()
            .gap(px(2.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface_high))
            .text_size(px(12.0))
            .children(self.lines.iter().enumerate().map(|(index, line)| {
                div()
                    .text_color(rgb(if index == 0 {
                        colors.text
                    } else {
                        colors.muted
                    }))
                    .child(line.clone())
            }))
    }
}

fn usage_tooltip(
    lines: Vec<String>,
    colors: ThemeColors,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    move |_, cx| {
        let lines = lines.clone();
        cx.new(|_| UsageTooltip { lines, colors }).into()
    }
}

fn section_title(label: impl Into<SharedString>, colors: ThemeColors) -> gpui::Div {
    div()
        .text_size(px(13.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(colors.text))
        .child(label.into())
}

fn small_muted(label: impl Into<SharedString>, colors: ThemeColors) -> gpui::Div {
    div()
        .text_size(px(12.0))
        .text_color(rgb(colors.muted))
        .child(label.into())
}

fn figures(element: gpui::Div, loading: bool) -> gpui::Div {
    element.when(loading, |element| element.opacity(0.4))
}

fn segmented_shell() -> gpui::Div {
    div().flex().flex_none().items_center().gap(px(2.0))
}

fn segment_button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    active: bool,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(26.0))
        .px(px(9.0))
        .flex()
        .items_center()
        .rounded(px(7.0))
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .text_color(rgb(if active { colors.text } else { colors.muted }))
        .when(active, |button| {
            button.bg(rgb(ink(colors.text, colors.chrome, 14)))
        })
        .when(!active, |button| {
            button.hover(|style| style.bg(rgb(ink(colors.text, colors.chrome, 7))))
        })
        .child(label)
}

fn menu_row(
    id: impl Into<gpui::ElementId>,
    checked: bool,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(28.0))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(6.0))
        .text_size(px(12.5))
        .text_color(rgb(colors.text))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(colors.surface_high)))
        .child(div().size(px(14.0)).flex_none().when(checked, |slot| {
            slot.child(icon(CHECK_ICON, 14.0, colors.text))
        }))
}

fn menu_shell(colors: ThemeColors) -> gpui::Div {
    div()
        .occlude()
        .absolute()
        .top(px(30.0))
        .left_0()
        .min_w(px(220.0))
        .p_1()
        .flex()
        .flex_col()
        .rounded(px(8.0))
        .border_1()
        .border_color(rgb(colors.border))
        .bg(rgb(colors.chrome))
        .shadow_lg()
}

fn metric_cell(
    label: &'static str,
    value: String,
    loading: bool,
    colors: ThemeColors,
) -> gpui::Div {
    div()
        .min_w(px(128.0))
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(small_muted(label, colors))
        .child(figures(
            div()
                .text_size(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(colors.text))
                .child(value),
            loading,
        ))
}

/// One limit in the flyout: its name, then what is left against the time left.
struct LimitRow {
    label: SharedString,
    /// An account under its pooled total.
    nested: bool,
    remaining: i64,
    time_left: Option<i64>,
    pace: Option<Pace>,
    /// When it resets, or what the next reset restores.
    detail: Option<String>,
    credits: u64,
}

/// Neutral until the limit needs attention: spent, or running out early.
fn limit_tone(remaining: i64, pace: Option<Pace>, colors: ThemeColors) -> u32 {
    if remaining <= 0 {
        colors.danger
    } else if pace == Some(Pace::Ahead) {
        colors.waiting
    } else {
        colors.muted
    }
}

fn flyout_track(colors: ThemeColors) -> u32 {
    ink(colors.text, colors.chrome, 14)
}

fn flyout_bar(share: f64, tone: u32, height: f32, colors: ThemeColors) -> gpui::Div {
    div()
        .w_full()
        .h(px(height))
        .rounded(px(height / 2.0))
        .bg(rgb(flyout_track(colors)))
        .child(
            div()
                .h_full()
                .w(relative((share as f32).clamp(0.0, 1.0)))
                .rounded(px(height / 2.0))
                .bg(rgb(tone)),
        )
}

fn limit_measure(
    label: &'static str,
    share: i64,
    value: String,
    tone: u32,
    value_color: u32,
    colors: ThemeColors,
) -> gpui::Div {
    div()
        .h(px(16.0))
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(28.0))
                .flex_none()
                .text_size(px(11.5))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(flyout_bar(share as f64 / 100.0, tone, 5.0, colors)),
        )
        .child(
            div()
                .min_w(px(52.0))
                .flex_none()
                .text_right()
                .text_color(rgb(value_color))
                .child(value),
        )
}

fn limit_row(row: LimitRow, colors: ThemeColors) -> gpui::Div {
    let tone = limit_tone(row.remaining, row.pace, colors);
    let clock = row.detail.map(|detail| detail.replace("Resets in ", ""));
    let timed = row.time_left.is_some();
    div()
        .px_2()
        .py(px(6.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .rounded(px(6.0))
        .text_size(px(12.0))
        .text_color(rgb(colors.muted))
        .child(
            div()
                .h(px(20.0))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .when(row.nested, |label| label.pl_3())
                        .when(!row.nested, |label| {
                            label.text_size(px(13.0)).text_color(rgb(colors.text))
                        })
                        .child(row.label),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when(row.pace == Some(Pace::Ahead), |line| {
                            line.child(
                                div()
                                    .text_color(rgb(colors.waiting))
                                    .child(Pace::Ahead.short_label()),
                            )
                        })
                        .when(row.credits > 0, |line| {
                            line.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_color(rgb(colors.text))
                                    .child(icon(TICKET_ICON, 12.0, colors.muted))
                                    .child(row.credits.to_string()),
                            )
                        }),
                ),
        )
        .child(limit_measure(
            "Left",
            row.remaining,
            format!("{}%", row.remaining),
            tone,
            if row.remaining <= 0 {
                colors.danger
            } else {
                colors.text
            },
            colors,
        ))
        .when_some(row.time_left, |block, left| {
            block.child(limit_measure(
                "Time",
                left,
                clock.clone().unwrap_or_default(),
                ink(colors.text, colors.chrome, 45),
                colors.muted,
                colors,
            ))
        })
        // Without a clock to compare against, only the words are left to show.
        .when(!timed, |block| {
            block.children(clock.clone().map(|clock| div().pl(px(36.0)).child(clock)))
        })
}

/// Providers in the flyout differ by shade, so color stays reserved for status.
fn flyout_provider_shade(provider: &str, colors: ThemeColors) -> u32 {
    ink(
        colors.text,
        colors.chrome,
        if provider == "claude" { 78 } else { 42 },
    )
}

fn metric_amount(metric: ChartMetric, cost: f64, tokens: u64) -> f64 {
    match metric {
        ChartMetric::Cost => cost,
        ChartMetric::Tokens => tokens as f64,
    }
}

/// One column per period, stacked by provider, with the figures in its tooltip.
fn flyout_period_bars(
    periods: &[Period],
    totals: &[PeriodTotals],
    metric: ChartMetric,
    providers: &[String],
    colors: ThemeColors,
) -> gpui::Div {
    let amount = |key: &str, provider: &str| {
        totals
            .iter()
            .find(|held| held.key == key)
            .and_then(|held| held.by_provider.get(provider))
            .map_or(0.0, |(cost, tokens)| metric_amount(metric, *cost, *tokens))
    };
    let total = |key: &str| {
        providers
            .iter()
            .map(|provider| amount(key, provider))
            .sum::<f64>()
    };
    let peak = periods
        .iter()
        .map(|period| total(&period.key))
        .fold(0.0_f64, f64::max);
    div()
        .h(px(FLYOUT_BARS_HEIGHT))
        .flex()
        .items_end()
        // Ninety columns have no room to spare for gaps.
        .gap(px(if periods.len() > 45 { 1.0 } else { 2.0 }))
        .children(periods.iter().enumerate().map(|(index, period)| {
            let segments: Vec<(u32, f32)> = providers
                .iter()
                .rev()
                .filter_map(|provider| {
                    let value = amount(&period.key, provider);
                    (value > 0.0 && peak > 0.0).then(|| {
                        (
                            flyout_provider_shade(provider, colors),
                            ((value / peak) as f32 * FLYOUT_BARS_HEIGHT).max(2.0),
                        )
                    })
                })
                .collect();
            let empty = segments.is_empty();
            div()
                .id(("usage-period-bar", index))
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .gap(px(1.0))
                .tooltip(usage_tooltip(
                    vec![period.tooltip.clone(), metric.format(total(&period.key))],
                    colors,
                ))
                .when(empty, |column| {
                    column.child(
                        div()
                            .h(px(2.0))
                            .rounded(px(1.0))
                            .bg(rgb(flyout_track(colors))),
                    )
                })
                .children(
                    segments
                        .into_iter()
                        .map(|(color, height)| div().h(px(height)).rounded(px(1.0)).bg(rgb(color))),
                )
        }))
}

fn flyout_heading(
    icon_path: Option<&'static str>,
    label: impl Into<SharedString>,
    colors: ThemeColors,
) -> gpui::Div {
    div()
        .h(px(26.0))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(colors.muted))
        .children(icon_path.map(|path| assistant_icon(path, 14.0, colors.muted, colors)))
        .child(label.into())
}

struct ShareSegment {
    label: &'static str,
    value: f64,
    color: u32,
}

fn token_segments(
    tokens: &xd_desktop::usage::TokenTotals,
    colors: ThemeColors,
) -> Vec<ShareSegment> {
    let shade = |percent| ink(colors.text, colors.background, percent);
    vec![
        ShareSegment {
            label: "Input",
            value: tokens.uncached_input as f64,
            color: shade(60),
        },
        ShareSegment {
            label: "Cache read",
            value: tokens.cached_input as f64,
            color: shade(30),
        },
        ShareSegment {
            label: "Cache write",
            value: tokens.cache_creation as f64,
            color: shade(72),
        },
        ShareSegment {
            label: "Output",
            value: tokens.output as f64,
            color: shade(100),
        },
    ]
}

fn cost_segments(cost: &xd_desktop::usage::CategoryCost, colors: ThemeColors) -> Vec<ShareSegment> {
    let shade = |percent| ink(colors.text, colors.background, percent);
    vec![
        ShareSegment {
            label: "Input",
            value: cost.input,
            color: shade(60),
        },
        ShareSegment {
            label: "Cache read",
            value: cost.cache_read,
            color: shade(30),
        },
        ShareSegment {
            label: "Cache write",
            value: cost.cache_write,
            color: shade(72),
        },
        ShareSegment {
            label: "Output",
            value: cost.output,
            color: shade(100),
        },
        ShareSegment {
            label: "Other",
            value: if cost.unsplit >= 0.005 {
                cost.unsplit
            } else {
                0.0
            },
            color: shade(44),
        },
    ]
}

fn speed_segments(cost: &xd_desktop::usage::SpeedCost, colors: ThemeColors) -> Vec<ShareSegment> {
    let shade = |percent| ink(colors.text, colors.background, percent);
    vec![
        ShareSegment {
            label: "Standard",
            value: cost.standard,
            color: shade(34),
        },
        ShareSegment {
            label: "Fast",
            value: cost.fast,
            color: shade(66),
        },
        ShareSegment {
            label: "Ultrafast",
            value: cost.ultrafast,
            color: shade(100),
        },
    ]
}

fn share_bar(
    id: &'static str,
    label: &'static str,
    segments: Vec<ShareSegment>,
    format: fn(f64) -> String,
    premium: Option<f64>,
    colors: ThemeColors,
) -> Option<AnyElement> {
    let visible: Vec<ShareSegment> = segments
        .into_iter()
        .filter(|segment| segment.value > 0.0)
        .collect();
    let total: f64 = visible.iter().map(|segment| segment.value).sum();
    if total <= 0.0 {
        return None;
    }
    let bar = visible.iter().enumerate().map(|(index, segment)| {
        div()
            .id((id, index))
            .h_full()
            .min_w(px(4.0))
            .flex_basis(relative((segment.value / total) as f32))
            .flex_grow()
            .rounded(px(2.0))
            .bg(rgb(segment.color))
            .tooltip(usage_tooltip(
                vec![format!(
                    "{} · {} · {}",
                    segment.label,
                    format(segment.value),
                    format_percent(segment.value / total)
                )],
                colors,
            ))
    });
    let legend = visible.iter().map(|segment| {
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(12.0))
            .child(div().size(px(8.0)).rounded(px(2.0)).bg(rgb(segment.color)))
            .child(div().text_color(rgb(colors.muted)).child(segment.label))
            .child(
                div()
                    .text_color(rgb(colors.text))
                    .child(format(segment.value)),
            )
    });
    Some(
        div()
            .min_w(px(280.0))
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap_3()
                    .child(section_title(label, colors))
                    .when_some(premium, |row, premium| {
                        row.child(
                            div()
                                .flex()
                                .gap_1()
                                .text_size(px(12.0))
                                .text_color(rgb(colors.muted))
                                .child("Premium")
                                .child(
                                    div()
                                        .text_color(rgb(colors.text))
                                        .child(format_usd(premium)),
                                ),
                        )
                    }),
            )
            .child(div().h(px(8.0)).flex().gap(px(2.0)).children(bar))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x_4()
                    .gap_y_1()
                    .children(legend),
            )
            .into_any_element(),
    )
}

fn account_credits(account: &LimitAccount) -> u64 {
    account
        .reset_credits
        .as_ref()
        .map_or(0, |credits| credits.available_count)
}

fn format_tokens_value(value: f64) -> String {
    format_tokens(value)
}

impl XdDesktop {
    fn usage_connected(&self) -> Vec<usize> {
        [0, 1]
            .into_iter()
            .filter(|index| self.endpoint_host(endpoint_at(*index)).is_some())
            .collect()
    }

    fn usage_selected(&self) -> Vec<usize> {
        self.usage_connected()
            .into_iter()
            .filter(|index| !self.usage.hidden_environments.contains(index))
            .collect()
    }

    fn usage_showing_limits(&self) -> bool {
        self.settings.usage_metric == "limits"
    }

    fn usage_chart_metric(&self) -> ChartMetric {
        if self.settings.usage_metric == "tokens" {
            ChartMetric::Tokens
        } else {
            ChartMetric::Cost
        }
    }

    pub(super) fn toggle_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.usage_open {
            self.close_usage(window, cx);
        } else {
            self.open_usage(window, cx);
        }
    }

    fn open_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.usage_open = true;
        self.usage.limits_now = now_ms();
        self.focus_minimal_popup(self.usage.focus.clone(), window, cx);
        self.usage_read_current(false);
        cx.notify();
    }

    pub(super) fn close_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.usage_open = false;
        self.usage.full_page = false;
        self.usage.menu = None;
        self.usage.model = None;
        self.usage.hover = None;
        self.usage.segment = None;
        self.usage.confirm_reset = None;
        self.usage.price = None;
        self.restore_minimal_popup_focus(window);
        cx.notify();
    }

    /// Escape closes the innermost layer first. `false` means Usage is not open.
    pub(super) fn dismiss_usage_layer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.usage_open {
            return false;
        }
        let page = &mut self.usage;
        if page.confirm_reset.take().is_some()
            || page.segment.take().is_some()
            || page.menu.take().is_some()
        {
            cx.notify();
            return true;
        }
        if page.price.take().is_some() {
            window.focus(&page.focus);
            cx.notify();
            return true;
        }
        if page.model.take().is_some() {
            cx.notify();
            return true;
        }
        if std::mem::take(&mut page.full_page) {
            cx.notify();
            return true;
        }
        self.close_usage(window, cx);
        true
    }

    fn usage_read_current(&mut self, refresh: bool) {
        if self.usage_showing_limits() {
            self.usage_read_limits();
        } else {
            self.usage_read_summary(refresh);
        }
    }

    fn usage_read_summary(&mut self, refresh: bool) {
        let days = self.settings.usage_window_days;
        let zone = local_time_zone();
        let now = now_ms();
        for index in self.usage_selected() {
            self.usage.generation += 1;
            let generation = self.usage.generation;
            let result = match self.endpoint_host(endpoint_at(index)) {
                Some(host) => host.usage_summary(days, &zone, now, refresh, generation),
                None => continue,
            };
            let environment = &mut self.usage.environments[index];
            environment.summary_generation = generation;
            environment.requested_window = Some((days, now));
            environment.summary_loading = result.is_ok();
            environment.summary_error = result.err();
        }
    }

    fn usage_read_limits(&mut self) {
        let zone = local_time_zone();
        for index in self.usage_selected() {
            self.usage.generation += 1;
            let generation = self.usage.generation;
            let result = match self.endpoint_host(endpoint_at(index)) {
                Some(host) => host.usage_limits(&zone, generation),
                None => continue,
            };
            let environment = &mut self.usage.environments[index];
            environment.limits_generation = generation;
            environment.limits_loading = result.is_ok();
            environment.limits_error = result.err();
        }
    }

    /// Takes the page's own traffic off a host's update stream. A dropped
    /// connection also ends whatever that host still owed the page.
    pub(super) fn usage_host_update(
        &mut self,
        endpoint: ChatEndpoint,
        update: &HostUpdate,
        cx: &mut Context<Self>,
    ) -> bool {
        let index = endpoint_index(endpoint);
        let handled = match update {
            HostUpdate::Disconnected { .. } => {
                self.usage_forget_host(endpoint, cx);
                false
            }
            HostUpdate::Connected { .. } if endpoint == ChatEndpoint::Local => {
                self.usage_host_connected(endpoint);
                false
            }
            HostUpdate::Reply { kind, body, .. } => self.usage_reply(index, kind, body, cx),
            HostUpdate::Event { name, body, .. } => self.usage_event(index, name, body),
            _ => false,
        };
        if handled {
            cx.notify();
        }
        handled
    }

    /// That slot may come back as another host or login, so nothing it reported
    /// can be shown or acted on, and nothing it still owes can be waited for.
    pub(super) fn usage_forget_host(&mut self, endpoint: ChatEndpoint, cx: &mut Context<Self>) {
        let index = endpoint_index(endpoint);
        self.usage.environments[index] = UsageEnvironment::default();
        if let Some(dialog) = &mut self.usage.price
            && dialog.saving.contains(&index)
        {
            dialog.saving.retain(|host| *host != index);
            dialog.error = Some(format!(
                "{} disconnected before saving finished.",
                environment_label(index)
            ));
        }
        if self
            .usage
            .confirm_reset
            .as_ref()
            .is_some_and(|confirm| confirm.environment == index)
        {
            self.usage.confirm_reset = None;
        }
        if self.usage.reset_busy == Some(index) {
            self.usage.reset_busy = None;
            self.usage.reset_status = Some(
                "The connection dropped before the reset answered. Check your limits before trying again."
                    .into(),
            );
        }
        cx.notify();
    }

    /// Reads again for a host that has just (re)connected. Every selected host
    /// is asked together, so rolling hours keep one shared window.
    pub(super) fn usage_host_connected(&mut self, endpoint: ChatEndpoint) {
        if !self.usage_open {
            return;
        }
        self.usage_read_current(false);
        if let Some(dialog) = &self.usage.price
            && let Some(host) = self.endpoint_host(endpoint)
        {
            let _ = host.usage_prices(dialog.id);
        }
    }

    fn usage_reply(
        &mut self,
        index: usize,
        kind: &RequestKind,
        body: &Map<String, Value>,
        cx: &mut Context<Self>,
    ) -> bool {
        let ok = body.get("ok").and_then(Value::as_bool) == Some(true);
        let error = || {
            body.get("error")
                .and_then(Value::as_str)
                .unwrap_or("The xd host rejected the request.")
                .to_owned()
        };
        let environment = &mut self.usage.environments[index];
        match kind {
            RequestKind::UsageSummary { generation } => {
                if !ok && environment.summary_generation == *generation {
                    environment.summary_loading = false;
                    environment.summary_error = Some(error());
                }
            }
            RequestKind::UsageLimits { generation } => {
                if !ok && environment.limits_generation == *generation {
                    environment.limits_loading = false;
                    environment.limits_error = Some(error());
                }
            }
            RequestKind::UsagePrices { dialog } => {
                if self
                    .usage
                    .price
                    .as_ref()
                    .is_some_and(|held| held.id == *dialog)
                {
                    if ok {
                        environment.prices = Some(Value::Object(body.clone()));
                        self.usage_fill_price_dialog(cx);
                    } else if let Some(held) = &mut self.usage.price {
                        held.error = Some(error());
                    }
                }
            }
            RequestKind::SetUsagePrices { dialog } => {
                if self
                    .usage
                    .price
                    .as_ref()
                    .is_some_and(|held| held.id == *dialog)
                {
                    if ok {
                        environment.prices = Some(Value::Object(body.clone()));
                    }
                    if let Some(held) = &mut self.usage.price {
                        held.saving.retain(|host| *host != index);
                        if !ok {
                            held.error = Some(error());
                        } else if held.saving.is_empty() && held.error.is_none() {
                            self.usage.price = None;
                            self.usage_read_summary(false);
                        }
                    }
                }
            }
            RequestKind::UsageResetCredit => {
                if !ok && self.usage.reset_busy == Some(index) {
                    self.usage.reset_busy = None;
                    self.usage.reset_status = Some(error());
                }
            }
            _ => return false,
        }
        true
    }

    fn usage_event(&mut self, index: usize, name: &str, body: &Map<String, Value>) -> bool {
        let generation = body.get("generation").and_then(Value::as_u64).unwrap_or(0);
        let environment = &mut self.usage.environments[index];
        match name {
            "usage-summary" => {
                if generation != 0 && generation == environment.summary_generation {
                    environment.summary_loading = false;
                    let summary = body.get("summary").and_then(Summary::parse);
                    if summary.is_none() {
                        environment.summary_error = Some(
                            body.get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("The host sent an unreadable usage summary.")
                                .to_owned(),
                        );
                    } else {
                        environment.summary = summary;
                        environment.summary_window = environment.requested_window;
                        environment.summary_error = None;
                    }
                }
                true
            }
            "usage-limits" => {
                if generation != 0 && generation == environment.limits_generation {
                    environment.limits_loading = false;
                    environment.limits_error = None;
                    let mut accounts = parse_limit_accounts(
                        &Value::Object(body.clone()),
                        index,
                        environment_label(index),
                    );
                    // Place each report in this machine's clock by its age. Two hosts'
                    // clocks can disagree, and a probe can answer long after it read.
                    let received = now_ms();
                    let host_now = body.get("host_now_ms").and_then(Value::as_i64);
                    for account in &mut accounts {
                        let age = host_now.map_or(0, |now| (now - account.checked_at_ms).max(0));
                        account.checked_at_ms = received - age;
                    }
                    environment.accounts = Some(accounts);
                    self.usage.limits_now = now_ms();
                }
                true
            }
            "usage-reset-credit" => {
                if self.usage.reset_busy == Some(index) {
                    self.usage.reset_busy = None;
                }
                // An outcome settles its own attempt; an error may have lost the answer.
                if body.contains_key("outcome")
                    && let Some(key) = body.get("idempotency_key").and_then(Value::as_str)
                {
                    self.usage.reset_attempts.retain(|_, held| held != key);
                }
                self.usage.reset_status = Some(match body.get("error").and_then(Value::as_str) {
                    Some(error) => error.to_owned(),
                    None => match body.get("outcome").and_then(Value::as_str) {
                        Some("reset") => "Reset applied. Your windows have cleared.",
                        Some("nothingToReset") => "Nothing to reset right now.",
                        Some("noCredit") => "No reset credit left.",
                        Some("alreadyRedeemed") => "That credit was already redeemed.",
                        _ => "Could not use the reset credit.",
                    }
                    .to_owned(),
                });
                self.usage_read_limits();
                true
            }
            _ => false,
        }
    }

    fn select_usage_metric(&mut self, metric: &str, cx: &mut Context<Self>) {
        if self.settings.usage_metric == metric {
            return;
        }
        self.settings.usage_metric = metric.to_owned();
        self.usage.hover = None;
        self.usage.segment = None;
        self.usage_save_preferences();
        if metric == "limits" {
            // Limits only live in the flyout.
            self.usage.full_page = false;
            self.usage.model = None;
            self.usage.limits_now = now_ms();
            self.usage_read_limits();
        } else if self.usage_selected().iter().any(|index| {
            let environment = &self.usage.environments[*index];
            environment.summary.is_none() && !environment.summary_loading
        }) {
            self.usage_read_summary(false);
        }
        cx.notify();
    }

    fn select_usage_window(&mut self, days: u32, cx: &mut Context<Self>) {
        if self.settings.usage_window_days == days {
            return;
        }
        self.settings.usage_window_days = days;
        self.usage.hover = None;
        self.usage_save_preferences();
        self.usage_read_summary(false);
        cx.notify();
    }

    fn usage_save_preferences(&mut self) {
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
    }

    pub(super) fn usage_key_down(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        let page = &self.usage;
        if event.is_held || page.price.is_some() || page.confirm_reset.is_some() {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let plain = !modifiers.control && !modifiers.platform && !modifiers.alt && !modifiers.shift;
        if plain {
            match keystroke.key.as_str() {
                "c" => self.select_usage_metric("cost", cx),
                "t" => self.select_usage_metric("tokens", cx),
                "l" => self.select_usage_metric("limits", cx),
                _ => return,
            }
        } else if modifiers.secondary() && modifiers.shift && !self.usage_showing_limits() {
            match keystroke.key.as_str() {
                "1" | "!" => self.select_usage_window(1, cx),
                "2" | "@" => self.select_usage_window(7, cx),
                "3" | "#" => self.select_usage_window(30, cx),
                "4" | "$" => self.select_usage_window(90, cx),
                _ => return,
            }
        } else {
            return;
        }
        cx.stop_propagation();
    }

    #[allow(clippy::too_many_arguments)]
    fn usage_input(
        &mut self,
        placeholder: &'static str,
        text: &str,
        row: u64,
        field: Option<usize>,
        is_model: bool,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> (Entity<ComposerInput>, Subscription) {
        let text = text.to_owned();
        let input = cx.new(|cx| {
            let mut input = ComposerInput::new(cx, placeholder);
            input.set_colors(colors, cx);
            input.set_text(text, cx);
            input
        });
        let subscription = cx.subscribe(&input, move |this, _, event, cx| {
            let ComposerEvent::Changed(text) = event else {
                return;
            };
            let Some(dialog) = &mut this.usage.price else {
                return;
            };
            let Some(row) = dialog.rows.iter_mut().find(|held| held.id == row) else {
                return;
            };
            match field {
                Some(index) => row.values[index] = text.clone(),
                None if is_model => row.model = text.clone(),
                None => row.alias = text.clone(),
            }
            dialog.error = None;
            cx.notify();
        });
        (input, subscription)
    }

    fn usage_add_price_row(
        &mut self,
        model: &str,
        is_new: bool,
        alias: &str,
        values: [String; 4],
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) {
        self.usage.next_row += 1;
        let id = self.usage.next_row;
        let mut subscriptions = Vec::new();
        let model_input = is_new.then(|| {
            let (input, subscription) =
                self.usage_input("Model ID", model, id, None, true, colors, cx);
            subscriptions.push(subscription);
            input
        });
        let (alias_input, subscription) = self.usage_input(
            if is_new { "Optional" } else { "" },
            alias,
            id,
            None,
            false,
            colors,
            cx,
        );
        subscriptions.push(subscription);
        let cell_inputs = std::array::from_fn(|index| {
            let (_, optional) = PRICE_FIELDS[index];
            let placeholder = if optional { "Input rate" } else { "0.00" };
            let (input, subscription) = self.usage_input(
                placeholder,
                &values[index],
                id,
                Some(index),
                false,
                colors,
                cx,
            );
            subscriptions.push(subscription);
            input
        });
        if let Some(dialog) = &mut self.usage.price {
            dialog.rows.push(PriceRow {
                id,
                model: model.to_owned(),
                is_new,
                removed: false,
                alias: alias.to_owned(),
                initial_alias: if is_new {
                    String::new()
                } else {
                    alias.to_owned()
                },
                initial_values: if is_new {
                    Default::default()
                } else {
                    values.clone()
                },
                values,
                model_input,
                alias_input,
                cell_inputs,
                _subscriptions: subscriptions,
            });
        }
    }

    fn open_usage_prices(
        &mut self,
        model: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.usage.menu = None;
        self.usage.model = None;
        self.usage.generation += 1;
        let id = self.usage.generation;
        let mut error = None;
        for index in self.usage_selected() {
            self.usage.environments[index].prices = None;
            if let Some(host) = self.endpoint_host(endpoint_at(index))
                && let Err(failure) = host.usage_prices(id)
            {
                error = Some(failure);
            }
        }
        self.usage.price = Some(PriceDialog {
            rows: Vec::new(),
            pending_model: model,
            id,
            saving: Vec::new(),
            error,
        });
        window.focus(&self.usage.focus);
        cx.notify();
    }

    /// Fills the dialog once every selected host has answered. The first host
    /// holding a model supplies its cells; saving touches only edited rows.
    fn usage_fill_price_dialog(&mut self, cx: &mut Context<Self>) {
        let selected = self.usage_selected();
        self.usage_fill_price_rows(&selected, cx);
    }

    fn usage_fill_price_rows(&mut self, selected: &[usize], cx: &mut Context<Self>) {
        let Some(dialog) = &self.usage.price else {
            return;
        };
        if selected
            .iter()
            .any(|index| self.usage.environments[*index].prices.is_none())
        {
            return;
        }
        let held: Vec<String> = dialog.rows.iter().map(|row| row.model.clone()).collect();
        let colors = self.settings.theme.colors();
        let mut rows: BTreeMap<String, (String, [String; 4])> = BTreeMap::new();
        for index in selected {
            let Some(prices) = &self.usage.environments[*index].prices else {
                continue;
            };
            if let Some(entries) = prices["prices"].as_object() {
                for (model, price) in entries {
                    rows.entry(model.clone()).or_insert_with(|| {
                        let cell = |name: &str| {
                            price[name]
                                .as_f64()
                                .map_or_else(String::new, |rate| rate.to_string())
                        };
                        (
                            String::new(),
                            [
                                cell("input"),
                                cell("output"),
                                cell("cache_read"),
                                cell("cache_write"),
                            ],
                        )
                    });
                }
            }
            if let Some(entries) = prices["aliases"].as_object() {
                for (model, target) in entries {
                    rows.entry(model.clone()).or_default().0 =
                        target.as_str().unwrap_or_default().to_owned();
                }
            }
        }
        // Repeatable: a host that answers late only adds the models it alone holds.
        for (model, (alias, values)) in rows {
            if !held.contains(&model) {
                self.usage_add_price_row(&model, false, &alias, values, colors, cx);
            }
        }
        let pending = self
            .usage
            .price
            .as_mut()
            .and_then(|dialog| dialog.pending_model.take());
        if let Some(model) = pending {
            let exists = self
                .usage
                .price
                .as_ref()
                .is_some_and(|dialog| dialog.rows.iter().any(|row| row.model == model));
            if !exists {
                self.usage_add_price_row(&model, true, "", Default::default(), colors, cx);
            }
        }
    }

    /// A selected host whose saved prices have not been read. A save replaces a
    /// host's whole list, so sending one without that baseline would erase it.
    fn usage_unread_prices(&self, selected: &[usize]) -> Option<usize> {
        selected
            .iter()
            .copied()
            .find(|index| self.usage.environments[*index].prices.is_none())
    }

    fn save_usage_prices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let unread = self.usage_unread_prices(&self.usage_selected());
        let Some(dialog) = &mut self.usage.price else {
            return;
        };
        if let Some(index) = unread {
            dialog.error = Some(format!(
                "Still reading saved prices from {}.",
                environment_label(index)
            ));
            cx.notify();
            return;
        }
        if let Some(message) = dialog
            .rows
            .iter()
            .filter(|row| row.changed())
            .find_map(|row| row.validate().err())
        {
            dialog.error = Some(message.to_owned());
            cx.notify();
            return;
        }
        let edits: Vec<(String, Option<Value>, Option<String>)> = dialog
            .rows
            .iter()
            .filter(|row| row.changed())
            .map(|row| {
                let alias = row.alias.trim();
                (
                    row.model.trim().to_owned(),
                    row.validate().ok().flatten(),
                    (!row.removed && !alias.is_empty()).then(|| alias.to_owned()),
                )
            })
            .collect();
        let dialog_id = dialog.id;
        let mut sent = Vec::new();
        let mut failure = None;
        for index in self.usage_selected() {
            let saved = self.usage.environments[index].prices.clone();
            let mut prices = saved
                .as_ref()
                .and_then(|saved| saved["prices"].as_object().cloned())
                .unwrap_or_default();
            let mut aliases = saved
                .as_ref()
                .and_then(|saved| saved["aliases"].as_object().cloned())
                .unwrap_or_default();
            for (model, price, alias) in &edits {
                prices.remove(model);
                aliases.remove(model);
                if let Some(price) = price {
                    prices.insert(model.clone(), price.clone());
                }
                if let Some(alias) = alias {
                    aliases.insert(model.clone(), json!(alias));
                }
            }
            if let Some(host) = self.endpoint_host(endpoint_at(index)) {
                match host.set_usage_prices(
                    dialog_id,
                    &Value::Object(prices),
                    &Value::Object(aliases),
                ) {
                    Ok(()) => sent.push(index),
                    Err(error) => failure = Some(error),
                }
            }
        }
        let sending = !sent.is_empty();
        if let Some(dialog) = &mut self.usage.price {
            dialog.saving = sent;
            dialog.error = failure;
        }
        // The dialog is locked until the hosts answer and closes when they do, so
        // nothing typed in the meantime can be lost and focus has somewhere to be.
        if sending {
            window.focus(&self.usage.focus);
        }
        cx.notify();
    }

    fn usage_use_reset_credit(&mut self, confirm: ResetConfirm, cx: &mut Context<Self>) {
        self.usage.confirm_reset = None;
        self.usage.segment = None;
        self.usage.reset_status = None;
        self.usage.generation += 1;
        let fresh = format!(
            "xd-{}-{}-{}",
            now_ms(),
            std::process::id(),
            self.usage.generation
        );
        let key = self
            .usage
            .reset_attempts
            .entry(confirm.account_key.clone())
            .or_insert(fresh)
            .clone();
        let result = match self.endpoint_host(endpoint_at(confirm.environment)) {
            Some(host) => host.usage_consume_reset_credit(&key, confirm.identity.as_deref()),
            None => Err("That host is no longer connected.".to_owned()),
        };
        match result {
            Ok(()) => self.usage.reset_busy = Some(confirm.environment),
            Err(error) => self.usage.reset_status = Some(error),
        }
        cx.notify();
    }

    /// Hosts whose answers describe one window. Until the asked-for window has
    /// an answer the page keeps showing the previous one, never a mix of both;
    /// rolling hours also need the same instant, or their buckets do not line up.
    fn usage_shown_environments(&self) -> Vec<usize> {
        let held: Vec<(usize, (u32, i64), &str)> = self
            .usage_selected()
            .into_iter()
            .filter_map(|index| {
                let environment = &self.usage.environments[index];
                let start = environment.summary.as_ref()?.periods.first()?;
                Some((index, environment.summary_window?, start.key.as_str()))
            })
            .collect();
        shown_environments(&held, self.settings.usage_window_days)
    }

    fn usage_merged(&self, keep: impl Fn(&Bucket) -> bool) -> Merged {
        let environments: Vec<(&str, &Summary)> = self
            .usage_shown_environments()
            .iter()
            .filter_map(|index| {
                self.usage.environments[*index]
                    .summary
                    .as_ref()
                    .map(|summary| (environment_label(*index), summary))
            })
            .collect();
        merge(&environments, &self.settings.usage_hidden_providers, keep)
    }

    /// The window the newest answer describes, so the chart and totals agree.
    fn usage_shown_summary(&self) -> Option<&Summary> {
        self.usage_shown_environments()
            .iter()
            .find_map(|index| self.usage.environments[*index].summary.as_ref())
    }

    /// Limits, cost and tokens open as a flyout over the sidebar's chart icon.
    /// The full page holds what does not fit: filters, prices and breakdowns.
    fn usage_flyout_open(&self) -> bool {
        self.usage_showing_limits() || !self.usage.full_page
    }

    fn usage_busy(&self) -> bool {
        let showing_limits = self.usage_showing_limits();
        self.usage_selected().iter().any(|index| {
            let environment = &self.usage.environments[*index];
            if showing_limits {
                environment.limits_loading
            } else {
                environment.summary_loading
            }
        })
    }

    fn render_usage_refresh(
        &self,
        size: f32,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let busy = self.usage_busy();
        let refresh_icon = svg()
            .path(REFRESH_ICON)
            .size(px(size / 2.0 + 1.0))
            .flex_none()
            .text_color(rgb(colors.muted));
        let refresh_icon = if busy {
            refresh_icon
                .with_animation(
                    "usage-refreshing",
                    Animation::new(Duration::from_secs(1)).repeat(),
                    |icon, delta| {
                        icon.with_transformation(Transformation::rotate(percentage(delta)))
                    },
                )
                .into_any_element()
        } else {
            refresh_icon.into_any_element()
        };
        ui_button("usage-refresh", ButtonKind::Ghost, colors)
            .w(px(size))
            .h(px(size))
            .px_0()
            .when(busy, |button| button.cursor_default())
            .tooltip(usage_tooltip(
                vec![
                    if self.usage_showing_limits() {
                        "Refresh limits"
                    } else {
                        "Refresh usage"
                    }
                    .to_owned(),
                ],
                colors,
            ))
            .child(refresh_icon)
            .on_click(cx.listener(move |this, _, _, cx| {
                if !busy {
                    this.usage_read_current(true);
                    cx.notify();
                }
            }))
    }

    fn render_usage_no_environment(&self, colors: ThemeColors) -> gpui::Div {
        let noun = if self.usage_showing_limits() {
            "limits"
        } else {
            "usage"
        };
        let verb = if self.usage_connected().is_empty() {
            "Connect"
        } else {
            "Select"
        };
        div()
            .text_size(px(13.5))
            .text_color(rgb(colors.muted))
            .child(format!("{verb} an environment to see {noun}."))
    }

    /// Filters are set on the full page, so the flyout says when they hide anything.
    fn usage_filtered(&self) -> bool {
        !self.settings.usage_hidden_providers.is_empty()
            || self.usage_selected().len() != self.usage_connected().len()
    }

    fn usage_show_everything(&mut self, cx: &mut Context<Self>) {
        self.usage.hidden_environments.clear();
        self.settings.usage_hidden_providers.clear();
        self.usage_save_preferences();
        self.usage_read_current(false);
        cx.notify();
    }

    fn render_usage_flyout(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let showing_limits = self.usage_showing_limits();
        let metric = self.settings.usage_metric.clone();
        let days = self.settings.usage_window_days;
        let body = if self.usage_selected().is_empty() {
            self.render_usage_no_environment(colors)
                .px_2()
                .py_1()
                .into_any_element()
        } else if showing_limits {
            self.render_usage_limits(colors, cx)
        } else {
            self.render_usage_flyout_summary(colors, cx)
        };
        let windows = (!showing_limits).then(|| {
            div()
                .px_1()
                .pb_1()
                .child(
                    segmented_shell().children(WINDOW_OPTIONS.map(|(option, label)| {
                        segment_button(
                            ("usage-window", option as usize),
                            label,
                            option == days,
                            colors,
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_usage_window(option, cx)),
                        )
                    })),
                )
        });
        let details = (!showing_limits).then(|| {
            ui_button("usage-all-details", ButtonKind::Ghost, colors)
                .w(px(26.0))
                .h(px(26.0))
                .px_0()
                .tooltip(usage_tooltip(vec!["All details".to_owned()], colors))
                .child(icon(MAXIMIZE_ICON, 13.0, colors.muted))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.usage.full_page = true;
                    cx.notify();
                }))
        });
        let filtered = self.usage_filtered().then(|| {
            div()
                .h(px(32.0))
                .flex_none()
                .pl_3()
                .pr_1()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .border_t_1()
                .border_color(rgb(colors.border))
                .text_size(px(12.0))
                .text_color(rgb(colors.muted))
                .child("Filters are hiding some usage.")
                .child(
                    ui_button("usage-show-everything", ButtonKind::Ghost, colors)
                        .h(px(24.0))
                        .child("Show all")
                        .on_click(cx.listener(|this, _, _, cx| this.usage_show_everything(cx))),
                )
        });
        div()
            .relative()
            .w(px(FLYOUT_WIDTH))
            .max_h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.chrome))
            .shadow_lg()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    if this.usage.menu.take().is_some() || this.usage.segment.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .h(px(38.0))
                            .pl(px(6.0))
                            .pr_1()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(segmented_shell().children(FLYOUT_METRICS.map(
                                |(option, label)| {
                                    segment_button(
                                        gpui::ElementId::Name(
                                            format!("usage-metric-{option}").into(),
                                        ),
                                        label,
                                        option == metric,
                                        colors,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| this.select_usage_metric(option, cx),
                                    ))
                                },
                            )))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(2.0))
                                    .children(details)
                                    .child(self.render_usage_refresh(26.0, colors, cx)),
                            ),
                    )
                    .children(windows),
            )
            .child(
                div()
                    .id("usage-scroll")
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_1()
                    .child(body),
            )
            .children(filtered)
            .into_any_element()
    }

    fn render_usage_flyout_summary(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.usage_selected();
        let errors: Vec<String> = selected
            .iter()
            .filter_map(|index| {
                self.usage.environments[*index]
                    .summary_error
                    .as_ref()
                    .map(|error| format!("{}: {error}", environment_label(*index)))
            })
            .collect();
        let Some(summary) = self.usage_shown_summary().cloned() else {
            let lines = if errors.is_empty() {
                vec!["Reading usage…".to_owned()]
            } else {
                errors
            };
            return div()
                .px_2()
                .py_1()
                .flex()
                .flex_col()
                .gap_1()
                .text_size(px(13.0))
                .text_color(rgb(colors.muted))
                .children(lines)
                .into_any_element();
        };
        let metric = self.usage_chart_metric();
        let hidden = &self.settings.usage_hidden_providers;
        let messages = source_messages(
            self.usage_shown_environments()
                .iter()
                .filter_map(|index| self.usage.environments[*index].summary.as_ref())
                .flat_map(|summary| summary.sources.iter())
                .filter(|source| !hidden.contains(&source.provider)),
        );
        let merged = self.usage_merged(|_| true);
        let loading = selected
            .iter()
            .any(|index| self.usage.environments[*index].summary_loading);
        let providers = rank_providers(&merged, metric, &[]);
        let models = match metric {
            ChartMetric::Tokens => sort_models_by_tokens(&merged.models),
            ChartMetric::Cost => merged.models.clone(),
        };
        let peak = models
            .iter()
            .map(|model| metric_amount(metric, model.cost_usd, model.total_tokens))
            .fold(0.0_f64, f64::max);
        let (total, caption) = match metric {
            ChartMetric::Cost => (
                format_usd(merged.cost_usd),
                format!(
                    "{} tokens · API estimate",
                    format_tokens(merged.total_tokens as f64)
                ),
            ),
            ChartMetric::Tokens => (
                format_tokens(merged.total_tokens as f64),
                format!(
                    "{} · {} sessions",
                    format_usd(merged.cost_usd),
                    format_count(merged.sessions)
                ),
            ),
        };
        let unpriced = (metric == ChartMetric::Cost && merged.unpriced_share > 0.0).then(|| {
            div()
                .id("usage-unpriced-info")
                .tooltip(usage_tooltip(
                    vec![format!(
                        "API estimate excludes {} unpriced records.",
                        format_percent(merged.unpriced_share)
                    )],
                    colors,
                ))
                .child(icon(INFO_ICON, 12.0, colors.muted))
        });
        let rows: Vec<AnyElement> = models
            .iter()
            .enumerate()
            .map(|(index, model)| {
                let key = model.key();
                let amount = metric_amount(metric, model.cost_usd, model.total_tokens);
                let share = if peak > 0.0 { amount / peak } else { 0.0 };
                div()
                    .id(("usage-model-row", index))
                    .px_2()
                    .py(px(5.0))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(colors.surface_high)))
                    .tooltip(usage_tooltip(
                        vec![
                            model.model.clone(),
                            format!(
                                "{} · {} tokens",
                                if model.cost_unknown() {
                                    "Unpriced".to_owned()
                                } else {
                                    format_usd(model.cost_usd)
                                },
                                format_tokens(model.total_tokens as f64)
                            ),
                        ],
                        colors,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage.full_page = true;
                        this.usage.model = Some(key.clone());
                        this.usage.hover = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .text_size(px(13.0))
                            .text_color(rgb(colors.text))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(assistant_icon(
                                        provider_icon(&model.provider),
                                        14.0,
                                        colors.muted,
                                        colors,
                                    ))
                                    .child(div().min_w_0().truncate().child(model.model.clone())),
                            )
                            .child(div().flex_none().text_size(px(12.0)).child(match metric {
                                ChartMetric::Cost if model.cost_unknown() => "Unpriced".to_owned(),
                                ChartMetric::Cost => format_usd(model.cost_usd),
                                ChartMetric::Tokens => format_tokens(model.total_tokens as f64),
                            })),
                    )
                    .child(flyout_bar(
                        share,
                        flyout_provider_shade(&model.provider, colors),
                        4.0,
                        colors,
                    ))
                    .into_any_element()
            })
            .collect();
        figures(div().flex().flex_col().gap_1(), loading)
            .children(errors.into_iter().chain(messages).map(|message| {
                div()
                    .px_2()
                    .text_size(px(12.0))
                    .text_color(rgb(colors.muted))
                    .child(message)
            }))
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .text_size(px(22.0))
                            .line_height(px(28.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(colors.text))
                            .child(total),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(small_muted(caption, colors))
                            .children(unpriced),
                    ),
            )
            .child(div().px_2().pt_1().child(flyout_period_bars(
                &summary.periods,
                &merged.periods,
                metric,
                &providers,
                colors,
            )))
            .child(flyout_heading(None, "Models", colors))
            .when(rows.is_empty(), |page| {
                page.child(
                    div()
                        .px_2()
                        .pb_1()
                        .child(small_muted("No activity in this window.", colors)),
                )
            })
            .children(rows)
            .into_any_element()
    }

    fn render_usage_full_page(
        &mut self,
        colors: ThemeColors,
        viewport: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = if self.usage_selected().is_empty() {
            self.render_usage_no_environment(colors).into_any_element()
        } else if self.usage_shown_summary().is_none() {
            self.render_usage_placeholder(colors)
        } else {
            self.render_usage_overview(colors, viewport, cx)
        };
        div()
            .relative()
            .size_full()
            .max_w(px(1280.0))
            .flex()
            .flex_col()
            .min_h_0()
            .rounded(px(16.0))
            .bg(rgb(colors.background))
            .shadow_lg()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    if this.usage.menu.take().is_some() || this.usage.segment.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(self.render_usage_header(colors, viewport, cx))
            .child(
                div()
                    .id("usage-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(1120.0))
                            .mx_auto()
                            .px_6()
                            .pt_6()
                            .pb_8()
                            .child(body),
                    ),
            )
            .when_some(
                self.render_usage_model_dialog(colors, cx),
                |modal, dialog| modal.child(dialog),
            )
            .when_some(
                self.render_usage_price_dialog(colors, cx),
                |modal, dialog| modal.child(dialog),
            )
            .into_any_element()
    }

    pub(super) fn render_usage(
        &mut self,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = f32::from(window.viewport_size().width);
        let flyout = self.usage_flyout_open();
        let panel = if flyout {
            self.render_usage_flyout(colors, cx)
        } else {
            self.render_usage_full_page(colors, viewport, cx)
        };
        // Over the whole window: the flyout can be shorter than this dialog.
        let confirm_reset = self
            .usage
            .confirm_reset
            .clone()
            .map(|confirm| self.render_usage_reset_confirm(confirm, colors, cx));
        div()
            .id("usage-page")
            .occlude()
            .absolute()
            .inset_0()
            .flex()
            .when(flyout, |page| {
                page.pl(px(FLYOUT_LEFT))
                    .pr_6()
                    .pt_6()
                    .pb(px(FLYOUT_BOTTOM))
                    .items_end()
                    .justify_start()
            })
            .when(!flyout, |page| {
                page.p_6()
                    .items_center()
                    .justify_center()
                    .bg(rgba(colors.backdrop))
            })
            .font_family(UI_FONT)
            .track_focus(&self.usage.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.usage_key_down(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // Most of the window is outside the flyout, so a press there
                    // may only be meant for an open popover or menu.
                    if this.usage.menu.take().is_some() || this.usage.segment.take().is_some() {
                        cx.notify();
                    } else {
                        this.close_usage(window, cx);
                    }
                }),
            )
            .child(panel)
            .children(confirm_reset)
            .into_any_element()
    }

    fn render_usage_header(
        &mut self,
        colors: ThemeColors,
        viewport: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let days = self.settings.usage_window_days;
        let metric = self.settings.usage_metric.clone();
        let window_label = (viewport >= 1280.0)
            .then(|| {
                self.usage_shown_summary()
                    .map(|summary| summary.window_label.clone())
            })
            .flatten();
        div()
            .min_h(px(44.0))
            .flex_none()
            .pl_5()
            .pr_2()
            .py_1()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_3()
            .gap_y_2()
            .rounded_t(px(16.0))
            .bg(rgb(colors.chrome))
            .border_b_1()
            .border_color(rgb(colors.border))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.0))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(colors.text))
                            .child("Usage"),
                    )
                    .child(div().text_color(rgb(colors.faint)).child("/"))
                    .child(self.render_usage_environment_filter(colors, cx))
                    .child(div().text_color(rgb(colors.faint)).child("/"))
                    .child(self.render_usage_provider_filter(colors, cx)),
            )
            .when_some(window_label, |header, label| {
                header.child(small_muted(label, colors))
            })
            .child(div().flex_1())
            .child(
                segmented_shell().children(WINDOW_OPTIONS.map(|(option, label)| {
                    segment_button(
                        ("usage-window", option as usize),
                        label,
                        option == days,
                        colors,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_usage_window(option, cx)),
                    )
                })),
            )
            .child(
                segmented_shell().children(METRIC_OPTIONS.map(|(option, label)| {
                    segment_button(
                        gpui::ElementId::Name(format!("usage-metric-{option}").into()),
                        label,
                        option == metric,
                        colors,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_usage_metric(option, cx)),
                    )
                })),
            )
            .child(self.render_usage_refresh(28.0, colors, cx))
            .child(
                ui_button("usage-close", ButtonKind::Ghost, colors)
                    .w(px(28.0))
                    .h(px(28.0))
                    .px_0()
                    .tooltip(usage_tooltip(vec!["Close".to_owned()], colors))
                    .child(icon(CLOSE_ICON, 15.0, colors.muted))
                    .on_click(cx.listener(|this, _, window, cx| this.close_usage(window, cx))),
            )
            .into_any_element()
    }

    fn render_usage_environment_filter(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let connected = self.usage_connected();
        let selected = self.usage_selected();
        let label = if selected.len() == connected.len() {
            "All environments".to_owned()
        } else if selected.len() == 1 {
            environment_label(selected[0]).to_owned()
        } else {
            format!("{} environments", selected.len())
        };
        let open = self.usage.menu == Some(UsageMenu::Environment);
        let has_issue = selected
            .iter()
            .any(|index| self.usage.environments[*index].summary_error.is_some());
        let partial = selected
            .iter()
            .any(|index| self.usage.environments[*index].summary_loading);
        let duplicates = self.usage_merged(|_| true).duplicate_sources;
        let menu = open.then(|| {
            let rows = connected.iter().map(|index| {
                let index = *index;
                let environment = &self.usage.environments[index];
                let status = if environment.summary_error.is_some() {
                    "Unavailable"
                } else if environment.summary_loading {
                    if environment.summary.is_some() {
                        "Refreshing…"
                    } else {
                        "Scanning…"
                    }
                } else {
                    "Ready"
                };
                menu_row(
                    ("usage-environment", index),
                    selected.contains(&index),
                    colors,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let hidden = &mut this.usage.hidden_environments;
                    if let Some(position) = hidden.iter().position(|held| *held == index) {
                        hidden.remove(position);
                        this.usage_read_current(false);
                    } else {
                        hidden.push(index);
                    }
                    cx.notify();
                }))
                .child(div().flex_1().child(environment_label(index)))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(if environment.summary_error.is_some() {
                            colors.danger
                        } else {
                            colors.muted
                        }))
                        .child(status),
                )
            });
            let notices: Vec<String> = connected
                .iter()
                .filter(|index| selected.contains(index))
                .filter_map(|index| {
                    self.usage.environments[*index]
                        .summary_error
                        .as_ref()
                        .map(|error| format!("{}: {error}", environment_label(*index)))
                })
                .chain((!duplicates.is_empty()).then(|| {
                    format!(
                        "Counted once across environments sharing a transcript directory: {}",
                        duplicates.join(", ")
                    )
                }))
                .collect();
            menu_shell(colors)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    menu_row(
                        "usage-environment-all",
                        selected.len() == connected.len(),
                        colors,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.usage.hidden_environments.is_empty() {
                            this.usage.hidden_environments = vec![0, 1];
                        } else {
                            this.usage.hidden_environments.clear();
                            this.usage_read_current(false);
                        }
                        cx.notify();
                    }))
                    .child("All environments"),
                )
                .child(div().h(px(1.0)).my_1().bg(rgb(colors.rule)))
                .children(rows)
                .when(partial, |menu| {
                    menu.child(
                        small_muted(
                            "Totals are partial while selected environments scan.",
                            colors,
                        )
                        .px_2()
                        .py_2(),
                    )
                })
                .when(!notices.is_empty(), |menu| {
                    menu.child(
                        div()
                            .max_w(px(320.0))
                            .mt_1()
                            .px_2()
                            .py_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .border_t_1()
                            .border_color(rgb(colors.rule))
                            .children(
                                notices
                                    .into_iter()
                                    .map(|notice| small_muted(notice, colors)),
                            ),
                    )
                })
                .child(div().h(px(1.0)).my_1().bg(rgb(colors.rule)))
                .child(
                    menu_row("usage-model-prices", false, colors)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_usage_prices(None, window, cx)
                        }))
                        .child(icon(SLIDERS_ICON, 14.0, colors.muted))
                        .child("Model prices"),
                )
        });
        div()
            .relative()
            .min_w_0()
            .child(
                div()
                    .id("usage-environment-filter")
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_color(rgb(colors.text))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage.menu = if open {
                            None
                        } else {
                            Some(UsageMenu::Environment)
                        };
                        cx.notify();
                    }))
                    .child(label)
                    .child(if has_issue {
                        icon(ALERT_ICON, 14.0, colors.waiting)
                    } else {
                        icon(CHEVRON_DOWN_ICON, 14.0, colors.muted)
                    }),
            )
            .children(menu.map(|menu| deferred(menu).with_priority(1)))
            .into_any_element()
    }

    fn render_usage_provider_filter(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hidden = self.settings.usage_hidden_providers.clone();
        let visible: Vec<&str> = PROVIDER_ORDER
            .into_iter()
            .filter(|provider| !hidden.iter().any(|held| held == provider))
            .collect();
        let label = match visible.as_slice() {
            all if all.len() == PROVIDER_ORDER.len() => "All providers".to_owned(),
            [] => "No providers".to_owned(),
            [one] => provider_label(one).to_owned(),
            many => format!("{} providers", many.len()),
        };
        let open = self.usage.menu == Some(UsageMenu::Provider);
        let menu = open.then(|| {
            menu_shell(colors)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    menu_row("usage-provider-all", hidden.is_empty(), colors)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.usage_hidden_providers =
                                if this.settings.usage_hidden_providers.is_empty() {
                                    PROVIDER_ORDER.map(str::to_owned).to_vec()
                                } else {
                                    Vec::new()
                                };
                            this.usage_save_preferences();
                            cx.notify();
                        }))
                        .child("All providers"),
                )
                .child(div().h(px(1.0)).my_1().bg(rgb(colors.rule)))
                .children(PROVIDER_ORDER.map(|provider| {
                    let checked = !hidden.iter().any(|held| held == provider);
                    menu_row(
                        gpui::ElementId::Name(format!("usage-provider-{provider}").into()),
                        checked,
                        colors,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let hidden = &mut this.settings.usage_hidden_providers;
                        if let Some(position) = hidden.iter().position(|held| held == provider) {
                            hidden.remove(position);
                        } else {
                            hidden.push(provider.to_owned());
                        }
                        this.usage_save_preferences();
                        cx.notify();
                    }))
                    .child(assistant_icon(
                        provider_icon(provider),
                        14.0,
                        colors.muted,
                        colors,
                    ))
                    .child(provider_label(provider))
                }))
        });
        div()
            .relative()
            .min_w_0()
            .child(
                div()
                    .id("usage-provider-filter")
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_color(rgb(colors.text))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage.menu = if open {
                            None
                        } else {
                            Some(UsageMenu::Provider)
                        };
                        cx.notify();
                    }))
                    .child(label)
                    .child(icon(CHEVRON_DOWN_ICON, 14.0, colors.muted)),
            )
            .children(menu.map(|menu| deferred(menu).with_priority(1)))
            .into_any_element()
    }

    fn render_usage_placeholder(&self, colors: ThemeColors) -> AnyElement {
        let errors: Vec<String> = self
            .usage_selected()
            .iter()
            .filter_map(|index| {
                self.usage.environments[*index]
                    .summary_error
                    .as_ref()
                    .map(|error| format!("{}: {error}", environment_label(*index)))
            })
            .collect();
        if !errors.is_empty() {
            return div()
                .py_12()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .text_size(px(13.5))
                .text_color(rgb(colors.muted))
                .children(errors)
                .into_any_element();
        }
        let block = |width: f32, height: f32| {
            div()
                .w(px(width))
                .h(px(height))
                .rounded(px(4.0))
                .bg(rgb(colors.surface))
        };
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(
                div()
                    .flex()
                    .gap_6()
                    .child(
                        div()
                            .w(px(288.0))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap_5()
                            .child(block(144.0, 40.0))
                            .child(block(128.0, 16.0))
                            .child(block(240.0, 36.0))
                            .child(block(240.0, 36.0)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(PLOT_HEIGHT))
                            .rounded(px(4.0))
                            .bg(rgb(colors.surface)),
                    ),
            )
            .child(section_title("Totals", colors))
            .child(block(600.0, 40.0))
            .child(section_title("Breakdown", colors))
            .child(
                div()
                    .w_full()
                    .h(px(176.0))
                    .rounded(px(4.0))
                    .bg(rgb(colors.surface)),
            )
            .into_any_element()
    }

    fn render_usage_overview(
        &mut self,
        colors: ThemeColors,
        viewport: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let metric = self.usage_chart_metric();
        let selected = self.usage_selected();
        let hidden = self.settings.usage_hidden_providers.clone();
        let merged = self.usage_merged(|_| true);
        let loading = selected
            .iter()
            .any(|index| self.usage.environments[*index].summary_loading);
        let summary = self.usage_shown_summary().cloned();
        let periods = summary
            .as_ref()
            .map(|summary| summary.periods.clone())
            .unwrap_or_default();
        let hourly = summary.as_ref().is_some_and(|summary| summary.hourly);
        let messages = source_messages(
            self.usage_shown_environments()
                .iter()
                .filter_map(|index| self.usage.environments[*index].summary.as_ref())
                .flat_map(|summary| summary.sources.iter())
                .filter(|source| !hidden.contains(&source.provider)),
        );
        let errors: Vec<String> = selected
            .iter()
            .filter_map(|index| {
                self.usage.environments[*index]
                    .summary_error
                    .as_ref()
                    .map(|error| format!("{}: {error}", environment_label(*index)))
            })
            .collect();
        let awaiting: Vec<&str> = if loading && merged.providers.is_empty() {
            PROVIDER_ORDER
                .into_iter()
                .filter(|provider| !hidden.iter().any(|held| held == provider))
                .collect()
        } else {
            Vec::new()
        };
        let providers = rank_providers(&merged, metric, &awaiting);
        let stacked = viewport < 1024.0;

        let hero = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(figures(
                div()
                    .text_size(px(36.0))
                    .line_height(px(40.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(colors.text))
                    .child(match metric {
                        ChartMetric::Cost => format_usd(merged.cost_usd),
                        ChartMetric::Tokens => format_tokens(merged.total_tokens as f64),
                    }),
                loading,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.0))
                    .text_color(rgb(colors.muted))
                    .child(figures(
                        div().child(format!("{} sessions", format_count(merged.sessions))),
                        loading,
                    ))
                    .when(metric == ChartMetric::Cost, |row| {
                        row.child("· API estimate")
                    })
                    .when(
                        metric == ChartMetric::Cost && merged.unpriced_share > 0.0,
                        |row| {
                            row.child(
                                div()
                                    .id("usage-unpriced-info")
                                    .tooltip(usage_tooltip(
                                        vec![format!(
                                            "API estimate excludes {} unpriced records.",
                                            format_percent(merged.unpriced_share)
                                        )],
                                        colors,
                                    ))
                                    .child(icon(INFO_ICON, 12.0, colors.muted)),
                            )
                        },
                    ),
            );
        let provider_rows = providers.iter().map(|provider| {
            let totals = merged
                .providers
                .iter()
                .find(|totals| totals.provider == *provider);
            let sessions = totals.map_or(0, |totals| totals.sessions);
            let share = totals.map_or(0.0, |totals| match metric {
                ChartMetric::Cost => totals.cost_share,
                ChartMetric::Tokens => totals.token_share,
            });
            let tokens = totals.map_or(0, |totals| totals.total_tokens);
            let cost = totals.map_or(0.0, |totals| totals.cost_usd);
            let awaiting_data = totals.is_none();
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .text_size(px(13.5))
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_color(rgb(colors.text))
                                .child(
                                    div()
                                        .size(px(8.0))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(rgb(provider_color(provider, colors))),
                                )
                                .child(assistant_icon(
                                    provider_icon(provider),
                                    16.0,
                                    colors.muted,
                                    colors,
                                ))
                                .child(div().truncate().child(provider_label(provider).to_owned()))
                                .when(!awaiting_data, |row| {
                                    row.child(figures(
                                        div()
                                            .flex_none()
                                            .text_size(px(11.0))
                                            .text_color(rgb(colors.muted))
                                            .child(format!(
                                                "{} {}",
                                                format_count(sessions),
                                                if sessions == 1 { "session" } else { "sessions" }
                                            )),
                                        loading,
                                    ))
                                }),
                        )
                        .child(figures(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(colors.text))
                                .child(if awaiting_data {
                                    "-".to_owned()
                                } else {
                                    match metric {
                                        ChartMetric::Cost => format_usd(cost),
                                        ChartMetric::Tokens => format_tokens(tokens as f64),
                                    }
                                }),
                            loading,
                        )),
                )
                .when(!awaiting_data, |row| {
                    row.child(figures(
                        small_muted(
                            match metric {
                                ChartMetric::Cost => format!(
                                    "{} of cost · {} tokens",
                                    format_percent(share),
                                    format_tokens(tokens as f64)
                                ),
                                ChartMetric::Tokens => format!(
                                    "{} of tokens · {}",
                                    format_percent(share),
                                    format_usd(cost)
                                ),
                            },
                            colors,
                        ),
                        loading,
                    ))
                })
        });
        let chart = build_chart(&periods, &merged.periods, metric, &providers, loading);
        let chart_title = format!(
            "{} {}",
            if hourly { "Hourly" } else { "Daily" },
            match metric {
                ChartMetric::Tokens => "processed tokens",
                ChartMetric::Cost => "cost",
            }
        );
        let chart_element = self.render_usage_chart(
            "usage-overview-chart",
            chart,
            periods.clone(),
            metric,
            providers.clone(),
            self.usage.overview_bounds.clone(),
            colors,
            cx,
        );
        let overview = div()
            .flex()
            .gap_6()
            .when(stacked, |row| row.flex_col())
            .child(
                div()
                    .when(!stacked, |column| column.w(px(288.0)).flex_none())
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(hero)
                    .children(provider_rows),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_title(chart_title, colors))
                    .child(chart_element),
            );
        let totals = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_title("Totals", colors))
            .child(
                div()
                    .py_1()
                    .flex()
                    .flex_wrap()
                    .gap_x_6()
                    .gap_y_4()
                    .child(metric_cell(
                        "Processed tokens",
                        format_tokens(merged.total_tokens as f64),
                        loading,
                        colors,
                    ))
                    .child(metric_cell(
                        "Cached input",
                        format_tokens(merged.tokens.cached_input as f64),
                        loading,
                        colors,
                    ))
                    .child(metric_cell(
                        "Uncached input",
                        format_tokens(merged.tokens.uncached_input as f64),
                        loading,
                        colors,
                    ))
                    .child(metric_cell(
                        "Output",
                        format_tokens(merged.tokens.output as f64),
                        loading,
                        colors,
                    ))
                    .child(metric_cell(
                        "Cache savings",
                        format_usd(merged.cache_savings_usd),
                        loading,
                        colors,
                    )),
            );
        let mut bars = Vec::new();
        if merged.total_tokens > 0 {
            match metric {
                ChartMetric::Tokens => bars.extend(share_bar(
                    "usage-tokens-by-type",
                    "Tokens by type",
                    token_segments(&merged.tokens, colors),
                    format_tokens_value,
                    None,
                    colors,
                )),
                ChartMetric::Cost => {
                    bars.extend(share_bar(
                        "usage-cost-by-type",
                        "Cost by type",
                        cost_segments(&merged.category_cost, colors),
                        format_usd,
                        None,
                        colors,
                    ));
                    if merged.speed_cost.fast + merged.speed_cost.ultrafast > 0.0 {
                        bars.extend(share_bar(
                            "usage-cost-by-speed",
                            "Cost by speed",
                            speed_segments(&merged.speed_cost, colors),
                            format_usd,
                            Some(merged.speed_cost.premium),
                            colors,
                        ));
                    }
                }
            }
        }
        let by_time = self.usage.breakdown_by_time;
        let breakdown = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(section_title("Breakdown", colors))
                    .child(
                        segmented_shell()
                            .child(
                                segment_button("usage-breakdown-model", "Model", !by_time, colors)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.usage.breakdown_by_time = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                segment_button(
                                    "usage-breakdown-time",
                                    if hourly { "Hour" } else { "Day" },
                                    by_time,
                                    colors,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.usage.breakdown_by_time = true;
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            )
            .child(if by_time {
                self.render_usage_time_table(&merged, &periods, &providers, hourly, loading, colors)
            } else {
                self.render_usage_model_table(&merged, metric, viewport, loading, colors, cx)
            });
        div()
            .flex()
            .flex_col()
            .gap_6()
            .children(errors.into_iter().chain(messages).map(|message| {
                div()
                    .text_size(px(13.5))
                    .text_color(rgb(colors.muted))
                    .child(message)
            }))
            .child(overview)
            .child(totals)
            .when(!bars.is_empty(), |page| {
                page.child(
                    figures(div().flex().flex_wrap().gap_x_12().gap_y_8(), loading).children(bars),
                )
            })
            .child(breakdown)
            .into_any_element()
    }

    fn render_usage_model_table(
        &self,
        merged: &Merged,
        metric: ChartMetric,
        viewport: f32,
        loading: bool,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let models = match metric {
            ChartMetric::Tokens => sort_models_by_tokens(&merged.models),
            ChartMetric::Cost => merged.models.clone(),
        };
        let value = |model: &ModelTotals| match metric {
            ChartMetric::Tokens => model.total_tokens as f64,
            ChartMetric::Cost => model.cost_usd,
        };
        let peak = models.iter().map(value).fold(0.0_f64, f64::max);
        let show_share = viewport >= 640.0;
        let column = |width: f32, label: gpui::AnyElement| {
            div()
                .w(px(width))
                .flex_none()
                .flex()
                .justify_end()
                .child(label)
        };
        let text = |value: String| div().child(value).into_any_element();
        let header = div()
            .flex()
            .items_center()
            .py_2()
            .px_2()
            .border_b_1()
            .border_color(rgb(colors.border))
            .text_size(px(12.0))
            .text_color(rgb(colors.muted))
            .child(div().w(px(24.0)).flex_none().child("#"))
            .child(div().flex_1().child("Model"))
            .child(column(96.0, text("Cost".into())))
            .when(show_share, |row| {
                row.child(column(72.0, text("Share".into())))
            })
            .child(column(88.0, text("Tokens".into())));
        let rows: Vec<AnyElement> = models
            .iter()
            .enumerate()
            .map(|(index, model)| {
                let key = model.key();
                let amount = value(model);
                let bar = (amount > 0.0 && peak > 0.0).then(|| {
                    let width = (amount / peak * 192.0).max(8.0) as f32;
                    figures(
                        div()
                            .mt(px(6.0))
                            .w(px(width))
                            .h(px(2.0))
                            .rounded_full()
                            .bg(rgb(provider_color(&model.provider, colors))),
                        loading,
                    )
                });
                let share = model.share(metric);
                div()
                    .id(("usage-model-row", index))
                    .flex()
                    .items_center()
                    .px_2()
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(with_alpha(colors.border, 0.5))
                    .text_size(px(13.5))
                    .text_color(rgb(colors.muted))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(colors.surface)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage.model = Some(key.clone());
                        this.usage.hover = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(24.0))
                            .flex_none()
                            .text_size(px(12.0))
                            .child((index + 1).to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_color(rgb(colors.text))
                                    .child(assistant_icon(
                                        provider_icon(&model.provider),
                                        14.0,
                                        colors.muted,
                                        colors,
                                    ))
                                    .child(div().truncate().child(model.model.clone())),
                            )
                            .children(bar),
                    )
                    .child(column(
                        96.0,
                        figures(
                            div().text_color(rgb(if model.cost_unknown() {
                                colors.muted
                            } else {
                                colors.text
                            })),
                            loading,
                        )
                        .child(if model.cost_unknown() {
                            "Unpriced".to_owned()
                        } else {
                            format_usd(model.cost_usd)
                        })
                        .into_any_element(),
                    ))
                    .when(show_share, |row| {
                        row.child(column(
                            72.0,
                            figures(div(), loading)
                                .child(share.map_or_else(String::new, format_percent))
                                .into_any_element(),
                        ))
                    })
                    .child(column(
                        88.0,
                        figures(div(), loading)
                            .child(format_tokens(model.total_tokens as f64))
                            .into_any_element(),
                    ))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .flex()
            .flex_col()
            .child(header)
            .children(rows)
            .when(empty, |table| {
                table.child(
                    div()
                        .py_6()
                        .flex()
                        .justify_center()
                        .text_size(px(13.5))
                        .text_color(rgb(colors.muted))
                        .child("No activity in this window."),
                )
            })
            .into_any_element()
    }

    fn render_usage_time_table(
        &self,
        merged: &Merged,
        periods: &[xd_desktop::usage::Period],
        providers: &[String],
        hourly: bool,
        loading: bool,
        colors: ThemeColors,
    ) -> AnyElement {
        let cell = |value: String, color: u32| {
            figures(
                div()
                    .w(px(96.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .text_color(rgb(color)),
                loading,
            )
            .child(value)
        };
        let header = div()
            .flex()
            .items_center()
            .py_2()
            .px_2()
            .border_b_1()
            .border_color(rgb(colors.border))
            .text_size(px(12.0))
            .text_color(rgb(colors.muted))
            .child(div().flex_1().child(if hourly { "Hour" } else { "Day" }))
            .children(providers.iter().map(|provider| {
                div()
                    .w(px(96.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(provider_label(provider).to_owned())
            }))
            .child(
                div()
                    .w(px(96.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child("Total"),
            )
            .child(
                div()
                    .w(px(96.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child("Tokens"),
            );
        let rows: Vec<AnyElement> = merged
            .periods
            .iter()
            .rev()
            .map(|period| {
                let label = periods
                    .iter()
                    .find(|held| held.key == period.key)
                    .map_or_else(|| period.key.clone(), |held| held.label.clone());
                div()
                    .flex()
                    .items_center()
                    .py_2()
                    .px_2()
                    .border_b_1()
                    .border_color(with_alpha(colors.border, 0.5))
                    .text_size(px(13.5))
                    .child(div().flex_1().text_color(rgb(colors.text)).child(label))
                    .children(providers.iter().map(|provider| {
                        cell(
                            format_usd(
                                period
                                    .by_provider
                                    .get(provider)
                                    .map_or(0.0, |entry| entry.0),
                            ),
                            colors.muted,
                        )
                    }))
                    .child(cell(format_usd(period.cost_usd), colors.text))
                    .child(cell(
                        format_tokens(period.total_tokens as f64),
                        colors.muted,
                    ))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .flex()
            .flex_col()
            .child(header)
            .children(rows)
            .when(empty, |table| {
                table.child(
                    div()
                        .py_6()
                        .flex()
                        .justify_center()
                        .text_size(px(13.5))
                        .text_color(rgb(colors.muted))
                        .child("No activity in this window."),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_usage_chart(
        &self,
        id: &'static str,
        chart: Chart,
        periods: Vec<xd_desktop::usage::Period>,
        metric: ChartMetric,
        providers: Vec<String>,
        bounds_cell: Rc<Cell<Bounds<Pixels>>>,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = self.usage.hover.filter(|hover| hover.chart == id);
        let period_count = periods.len();
        let labels: Vec<(f32, String)> = if chart.labeled {
            chart
                .ticks
                .iter()
                .map(|(tick, y)| {
                    let label = if *tick == 0.0 {
                        "0".to_owned()
                    } else {
                        metric.format(*tick)
                    };
                    (*y / VIEW_HEIGHT * PLOT_HEIGHT, label)
                })
                .collect()
        } else {
            Vec::new()
        };
        let ticks: Vec<f32> = chart.ticks.iter().map(|(_, y)| *y).collect();
        let series: Vec<(
            u32,
            Vec<xd_desktop::usage::CurveSegment>,
            Option<ChartPoint>,
        )> = chart
            .series
            .iter()
            .map(|series| {
                (
                    provider_color(&series.provider, colors),
                    series.line.clone(),
                    series.point,
                )
            })
            .collect();
        let step_x = chart.step_x;
        let crosshair = hover.map(|hover| hover.index as f32 * step_x);
        let border = colors.border;
        let muted = colors.muted;
        let measure = bounds_cell.clone();
        let plot_canvas = canvas(
            move |bounds, _, _| {
                measure.set(bounds);
                bounds
            },
            move |_, bounds, window, _| {
                paint_usage_chart(bounds, &ticks, &series, crosshair, border, muted, window);
            },
        )
        .size_full();
        let tooltip = hover.and_then(|hover| {
            let period = periods.get(hover.index)?;
            let column = chart.columns.get(hover.index)?;
            let height = 44.0 + providers.len() as f32 * 18.0;
            let width = 184.0;
            let plot_width = f32::from(bounds_cell.get().size.width);
            let gap = 12.0;
            let left = if hover.x + gap + width <= plot_width {
                hover.x + gap
            } else {
                hover.x - gap - width
            }
            .clamp(0.0, (plot_width - width).max(0.0));
            let top = if hover.y + gap + height <= PLOT_HEIGHT {
                hover.y + gap
            } else {
                hover.y - gap - height
            }
            .clamp(0.0, (PLOT_HEIGHT - height).max(0.0));
            let row = |label: String, mark: Option<&str>, value: String| {
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_color(rgb(colors.muted))
                            .when_some(mark, |row, mark| {
                                row.child(assistant_icon(
                                    provider_icon(mark),
                                    12.0,
                                    colors.muted,
                                    colors,
                                ))
                            })
                            .child(label),
                    )
                    .child(div().text_color(rgb(colors.text)).child(value))
            };
            Some(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(width))
                    .px(px(10.0))
                    .py_2()
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(with_alpha(colors.border, 0.5))
                    .bg(rgb(colors.chrome))
                    .shadow_lg()
                    .text_size(px(12.0))
                    .child(
                        div()
                            .mb_1()
                            .text_color(rgb(colors.muted))
                            .child(period.tooltip.clone()),
                    )
                    .children(providers.iter().enumerate().map(|(slot, provider)| {
                        row(
                            provider_label(provider).to_owned(),
                            Some(provider),
                            metric.format(column.get(slot).copied().unwrap_or(0.0)),
                        )
                    }))
                    .child(
                        div()
                            .mt_1()
                            .pt_1()
                            .border_t_1()
                            .border_color(rgb(colors.border))
                            .child(row(
                                "Total".to_owned(),
                                None,
                                metric.format(column.iter().sum()),
                            )),
                    ),
            )
        });
        let move_bounds = bounds_cell.clone();
        let edge = |index: usize| {
            periods
                .get(index)
                .map_or_else(String::new, |period| period.label.to_uppercase())
        };
        let axis = [
            edge(0),
            edge(period_count / 2),
            edge(period_count.saturating_sub(1)),
        ];
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .relative()
                            .w(px(56.0))
                            .h(px(PLOT_HEIGHT))
                            .flex_none()
                            .children(labels.into_iter().map(|(y, label)| {
                                div()
                                    .absolute()
                                    .right_0()
                                    .top(px(y - 7.0))
                                    .text_size(px(10.0))
                                    .text_color(rgb(colors.muted))
                                    .child(label)
                            })),
                    )
                    .child(
                        div()
                            .id(id)
                            .relative()
                            .h(px(PLOT_HEIGHT))
                            .flex_1()
                            .min_w_0()
                            .on_mouse_move(cx.listener(
                                move |this, event: &MouseMoveEvent, _, cx| {
                                    let bounds = move_bounds.get();
                                    let width = f32::from(bounds.size.width);
                                    if width <= 0.0 || period_count == 0 {
                                        return;
                                    }
                                    let x = f32::from(event.position.x - bounds.left())
                                        .clamp(0.0, width);
                                    let y = f32::from(event.position.y - bounds.top())
                                        .clamp(0.0, f32::from(bounds.size.height));
                                    let index =
                                        ((x / width) * (period_count - 1) as f32).round() as usize;
                                    this.usage.hover = Some(ChartHover {
                                        chart: id,
                                        index: index.min(period_count - 1),
                                        x,
                                        y,
                                    });
                                    cx.notify();
                                },
                            ))
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if !hovered
                                    && this.usage.hover.is_some_and(|hover| hover.chart == id)
                                {
                                    this.usage.hover = None;
                                    cx.notify();
                                }
                            }))
                            .child(plot_canvas)
                            .children(tooltip),
                    ),
            )
            .child(
                div()
                    .pl(px(64.0))
                    .flex()
                    .justify_between()
                    .text_size(px(10.0))
                    .text_color(rgb(colors.muted))
                    .children(axis.map(|label| div().child(label))),
            )
            .into_any_element()
    }

    fn render_usage_model_dialog(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let key = self.usage.model.clone()?;
        let metric = self.usage_chart_metric();
        let merged = self.usage_merged(|_| true);
        let model = merged
            .models
            .iter()
            .find(|model| model.key() == key)?
            .clone();
        let usage = self.usage_merged(|bucket| {
            bucket.provider == model.provider && bucket.model == model.model
        });
        let periods = self.usage_shown_summary()?.periods.clone();
        let cost_unknown = model.cost_unknown();
        // Unpriced cost is unknown, not zero, so its trend shows tokens.
        let chart_metric = if cost_unknown {
            ChartMetric::Tokens
        } else {
            metric
        };
        let providers = vec![model.provider.clone()];
        let chart = build_chart(&periods, &usage.periods, chart_metric, &providers, false);
        let chart_element = self.render_usage_chart(
            "usage-model-chart",
            chart,
            periods,
            chart_metric,
            providers,
            self.usage.model_bounds.clone(),
            colors,
            cx,
        );
        let mut stats = vec![
            (
                "Cost",
                if cost_unknown {
                    "Unpriced".to_owned()
                } else {
                    format_usd(model.cost_usd)
                },
            ),
            ("Tokens", format_tokens(model.total_tokens as f64)),
        ];
        if let Some(rate) = model.cost_per_million() {
            stats.push(("Per 1M tokens", format_usd(rate)));
        }
        if let Some(rate) = model.cache_hit_rate() {
            stats.push(("Cache hit", format_percent(rate)));
        }
        let mut bars = Vec::new();
        if !cost_unknown {
            bars.extend(share_bar(
                "usage-model-cost",
                "Cost by type",
                cost_segments(&usage.category_cost, colors),
                format_usd,
                None,
                colors,
            ));
        }
        bars.extend(share_bar(
            "usage-model-tokens",
            "Tokens by type",
            token_segments(&model.tokens, colors),
            format_tokens_value,
            None,
            colors,
        ));
        if usage.speed_cost.fast + usage.speed_cost.ultrafast > 0.0 {
            bars.extend(share_bar(
                "usage-model-speed",
                "Cost by speed",
                speed_segments(&usage.speed_cost, colors),
                format_usd,
                Some(usage.speed_cost.premium),
                colors,
            ));
        }
        let provider = provider_label(&model.provider).to_owned();
        let description = if cost_unknown {
            provider
        } else {
            format!("{provider} · {} of cost", format_percent(model.cost_share))
        };
        let model_name = model.model.clone();
        let unpriced_tokens = model.unpriced_tokens;
        let panel = usage_dialog_panel(768.0, colors)
            .child(
                div()
                    .px_6()
                    .pt_5()
                    .pb_3()
                    .flex()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(colors.text))
                                    .child(assistant_icon(
                                        provider_icon(&model.provider),
                                        20.0,
                                        colors.muted,
                                        colors,
                                    ))
                                    .child(div().truncate().child(model.model.clone())),
                            )
                            .child(small_muted(description, colors)),
                    )
                    .child(
                        ui_button("usage-model-close", ButtonKind::Ghost, colors)
                            .w(px(28.0))
                            .h(px(28.0))
                            .px_0()
                            .child(icon(CLOSE_ICON, 15.0, colors.muted))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.usage.model = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("usage-model-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_6()
                    .pb_6()
                    .flex()
                    .flex_col()
                    .gap_8()
                    .child(div().flex().flex_wrap().gap_x_6().gap_y_4().children(
                        stats.into_iter().map(|(label, value)| {
                            div()
                                .min_w(px(120.0))
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(small_muted(label, colors))
                                .child(
                                    div()
                                        .text_size(px(24.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(colors.text))
                                        .child(value),
                                )
                        }),
                    ))
                    .child(chart_element)
                    .child(div().flex().flex_wrap().gap_x_10().gap_y_8().children(bars)),
            )
            .when(unpriced_tokens > 0.0, |dialog| {
                dialog.child(
                    div()
                        .px_6()
                        .py_3()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .border_t_1()
                        .border_color(rgb(colors.border))
                        .child(small_muted(
                            format!(
                                "{} tokens have no known price",
                                format_tokens(unpriced_tokens)
                            ),
                            colors,
                        ))
                        .child(
                            ui_button("usage-set-price", ButtonKind::Primary, colors)
                                .child("Set price")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_usage_prices(Some(model_name.clone()), window, cx)
                                })),
                        ),
                )
            });
        Some(usage_dialog_layer(
            panel,
            colors,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.usage.model = None;
                cx.notify();
            }),
        ))
    }

    fn render_usage_price_dialog(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // The set of hosts being waited for can shrink without any of them answering.
        self.usage_fill_price_dialog(cx);
        let dialog = self.usage.price.as_ref()?;
        let selected = self.usage_selected();
        let loaded = selected
            .iter()
            .all(|index| self.usage.environments[*index].prices.is_some());
        let destination = match selected.as_slice() {
            [one] => environment_label(*one).to_owned(),
            many => format!("{} environments", many.len()),
        };
        let has_changes = dialog.rows.iter().any(PriceRow::changed);
        let saving = !dialog.saving.is_empty();
        let error = dialog.error.clone();
        let field = |input: Entity<ComposerInput>| {
            div()
                .h(px(28.0))
                .w_full()
                .min_w_0()
                .px_2()
                .flex()
                .items_center()
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.background))
                .text_size(px(12.5))
                .child(input)
        };
        let head =
            |label: &'static str, width: f32| div().w(relative(width)).min_w_0().child(label);
        let rows: Vec<AnyElement> = dialog
            .rows
            .iter()
            .map(|row| {
                let id = row.id;
                let alias = row.alias.trim().to_owned();
                let invalid = row
                    .changed()
                    .then(|| row.validate().err())
                    .flatten()
                    .filter(|_| {
                        !row.model.trim().is_empty()
                            || row.values.iter().any(|value| !value.is_empty())
                    });
                let model_cell = match &row.model_input {
                    Some(input) => field(input.clone()).into_any_element(),
                    None => div()
                        .text_size(px(12.5))
                        .text_color(rgb(if row.removed {
                            colors.muted
                        } else {
                            colors.text
                        }))
                        .when(row.removed, |cell| cell.line_through())
                        .child(row.model.clone())
                        .into_any_element(),
                };
                let is_new = row.is_new;
                let removed = row.removed;
                let tooltip = if removed {
                    "Undo reset"
                } else if is_new {
                    "Remove row"
                } else {
                    "Reset to automatic"
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().w(relative(0.22)).min_w_0().child(model_cell))
                            .when(removed, |line| {
                                line.child(
                                    div()
                                        .flex_1()
                                        .text_size(px(12.5))
                                        .text_color(rgb(colors.muted))
                                        .child("Automatic pricing after saving"),
                                )
                            })
                            .when(!removed, |line| {
                                line.child(
                                    div()
                                        .w(relative(0.18))
                                        .min_w_0()
                                        .child(field(row.alias_input.clone())),
                                )
                                .when(!alias.is_empty(), |line| {
                                    line.child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(12.5))
                                            .text_color(rgb(colors.muted))
                                            .child(format!("Counted as {alias}")),
                                    )
                                })
                                .when(alias.is_empty(), |line| {
                                    line.children(row.cell_inputs.iter().map(|input| {
                                        div().flex_1().min_w_0().child(field(input.clone()))
                                    }))
                                })
                            })
                            .child(
                                ui_button(
                                    ("usage-price-reset", id as usize),
                                    ButtonKind::Ghost,
                                    colors,
                                )
                                .w(px(24.0))
                                .h(px(24.0))
                                .px_0()
                                .tooltip(usage_tooltip(vec![tooltip.to_owned()], colors))
                                .child(icon(
                                    if is_new { CLOSE_ICON } else { ROTATE_CCW_ICON },
                                    14.0,
                                    colors.muted,
                                ))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if let Some(dialog) = &mut this.usage.price {
                                            if is_new {
                                                dialog.rows.retain(|row| row.id != id);
                                            } else if let Some(row) =
                                                dialog.rows.iter_mut().find(|row| row.id == id)
                                            {
                                                row.removed = !row.removed;
                                            }
                                            dialog.error = None;
                                        }
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .when_some(invalid, |line, message| {
                        line.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(colors.danger))
                                .child(message),
                        )
                    })
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        let table = div()
            .min_w_0()
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(colors.border))
            .child(
                div()
                    .px_2()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(rgb(colors.muted))
                    .child(head("Model ID", 0.22))
                    .child(head("Map to", 0.18))
                    .children(PRICE_FIELDS.map(|(label, _)| div().flex_1().min_w_0().child(label)))
                    .child(
                        ui_button("usage-price-add", ButtonKind::Ghost, colors)
                            .w(px(24.0))
                            .h(px(24.0))
                            .px_0()
                            .tooltip(usage_tooltip(vec!["Add model price".to_owned()], colors))
                            .child(icon(PLUS_ICON, 14.0, colors.muted))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.usage_add_price_row(
                                    "",
                                    true,
                                    "",
                                    Default::default(),
                                    colors,
                                    cx,
                                );
                                let focus = this
                                    .usage
                                    .price
                                    .as_ref()
                                    .and_then(|dialog| dialog.rows.last())
                                    .and_then(|row| row.model_input.as_ref())
                                    .map(|input| input.read(cx).focus_handle(cx));
                                if let Some(focus) = focus {
                                    window.focus(&focus);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .children(rows)
            .when(empty, |table| {
                table.child(
                    div()
                        .py_6()
                        .flex()
                        .justify_center()
                        .border_t_1()
                        .border_color(rgb(colors.border))
                        .text_size(px(13.5))
                        .text_color(rgb(colors.muted))
                        .child("No custom prices or mappings. Add a row to set one."),
                )
            });
        let panel = usage_dialog_panel(880.0, colors)
                .child(
                    div()
                        .px_6()
                        .pt_5()
                        .pb_3()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(colors.text))
                                .child("Model prices"),
                        )
                        .child(small_muted(
                            "USD per million tokens. Custom prices replace automatic pricing.",
                            colors,
                        )),
                )
                .child(
                    div()
                        .id("usage-price-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_6()
                        .pb_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(if loaded {
                            table.into_any_element()
                        } else {
                            small_muted("Loading model prices…", colors).into_any_element()
                        })
                        .child(small_muted(
                            "Blank cache rates use the input price. Enter 0 for free tokens. A mapped model’s usage counts as the model it maps to.",
                            colors,
                        ))
                        .when_some(error, |body, error| {
                            body.child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(colors.danger))
                                    .child(error),
                            )
                        }),
                )
                .child(
                    div()
                        .px_6()
                        .py_3()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .border_t_1()
                        .border_color(rgb(colors.border))
                        .child(small_muted(
                            if has_changes {
                                format!("Changes apply to {destination}")
                            } else {
                                String::new()
                            },
                            colors,
                        ))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    ui_button("usage-price-cancel", ButtonKind::Ghost, colors)
                                        .child(if has_changes { "Discard changes" } else { "Close" })
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.usage.price = None;
                                            window.focus(&this.usage.focus);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    ui_button(
                                        "usage-price-save",
                                        if has_changes && !saving {
                                            ButtonKind::Primary
                                        } else {
                                            ButtonKind::Secondary
                                        },
                                        colors,
                                    )
                                    .when(!has_changes || saving, |button| {
                                        button.opacity(0.6).cursor_default()
                                    })
                                    .child(if saving { "Saving…" } else { "Save changes" })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if has_changes && !saving {
                                            this.save_usage_prices(window, cx);
                                        }
                                    })),
                                ),
                        ),
                );
        let panel = panel.relative().when(saving, |panel| {
            panel.child(
                div()
                    .id("usage-price-saving")
                    .occlude()
                    .absolute()
                    .inset_0(),
            )
        });
        // Typed prices are not dropped by a stray press; Discard and Escape are explicit.
        Some(usage_dialog_layer(panel, colors, |_, _, cx| {
            cx.stop_propagation()
        }))
    }

    fn render_usage_reset_confirm(
        &self,
        confirm: ResetConfirm,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .occlude()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(colors.backdrop))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .w(px(420.0))
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.surface))
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(colors.text))
                            .child("Use a reset credit?"),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .text_color(rgb(colors.muted))
                            .child("This redeems one credit on your account and clears the current rate-limit windows. It cannot be undone."),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                ui_button("usage-reset-cancel", ButtonKind::Secondary, colors)
                                    .child("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.usage.confirm_reset = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                ui_button("usage-reset-confirm", ButtonKind::Primary, colors)
                                    .child("Use credit")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.usage_use_reset_credit(confirm.clone(), cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_usage_limits(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.usage_selected();
        let now = self.usage.limits_now;
        let hidden = self.settings.usage_hidden_providers.clone();
        let many = selected.len() > 1;
        let accounts = collect_accounts(
            selected
                .iter()
                .filter_map(|index| self.usage.environments[*index].accounts.clone())
                .flatten()
                .filter(|account| !hidden.contains(&account.provider)),
        );
        let answered = selected
            .iter()
            .any(|index| self.usage.environments[*index].accounts.is_some());
        let loading = selected
            .iter()
            .any(|index| self.usage.environments[*index].limits_loading);
        let pools = collect_pools(&accounts, now);
        let mut notices = collect_notices(&accounts, many);
        notices.extend(selected.iter().filter_map(|index| {
            self.usage.environments[*index]
                .limits_error
                .as_ref()
                .map(|error| format!("{}: {error}", environment_label(*index)))
        }));
        let empty = pools.is_empty() && notices.is_empty();
        let status = self
            .usage
            .reset_busy
            .map(|_| "Using reset credit…".to_owned())
            .or_else(|| self.usage.reset_status.clone());
        let sections: Vec<AnyElement> = pools
            .iter()
            .map(|pool| self.render_usage_pool(pool, now, colors, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .when(empty, |page| {
                page.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(13.5))
                        .text_color(rgb(colors.muted))
                        .child(if loading {
                            "Reading limits…"
                        } else if !answered {
                            "Refresh to read limits."
                        } else {
                            "No provider on the selected environments reports subscription limits."
                        }),
                )
            })
            .children(sections)
            .when_some(status, |page, status| {
                page.child(div().px_2().child(small_muted(status, colors)))
            })
            .when(!notices.is_empty(), |page| {
                page.child(
                    div()
                        .px_2()
                        .py_1()
                        .flex()
                        .items_start()
                        .gap_2()
                        .text_size(px(12.0))
                        .text_color(rgb(colors.waiting))
                        .child(div().flex_none().mt(px(1.0)).child(icon(
                            ALERT_ICON,
                            14.0,
                            colors.waiting,
                        )))
                        .child(div().min_w_0().flex().flex_col().gap_1().children(notices)),
                )
            })
            .into_any_element()
    }

    fn render_usage_pool(
        &self,
        pool: &LimitPool,
        now: i64,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows: Vec<AnyElement> = pool
            .windows
            .iter()
            .flat_map(|window| self.render_usage_pool_rows(pool, window, now, colors, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .child(flyout_heading(
                Some(provider_icon(&pool.provider)),
                provider_label(&pool.provider).to_owned(),
                colors,
            ))
            .children(rows)
            .into_any_element()
    }

    /// One row for a window one account reports; a pooled total over one row per
    /// account when several share it.
    fn render_usage_pool_rows(
        &self,
        pool: &LimitPool,
        window: &PoolWindow,
        now: i64,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let members: Vec<(&LimitAccount, &xd_desktop::usage::PoolMember)> = pool
            .accounts
            .iter()
            .filter_map(|account| {
                let member = window
                    .members
                    .iter()
                    .find(|member| member.account == account.key)?;
                Some((account, member))
            })
            .collect();
        let shared = members.len() > 1;
        let mut rows = Vec::with_capacity(members.len() + 1);
        if shared {
            let next_refill = window
                .resets
                .iter()
                .find(|reset| reset.restores_percent > 0);
            rows.push(
                limit_row(
                    LimitRow {
                        label: window.label.clone().into(),
                        nested: false,
                        remaining: window.remaining_percent,
                        time_left: None,
                        pace: window.pace,
                        detail: next_refill
                            .map(|reset| format!("+{}% next reset", reset.restores_percent)),
                        credits: 0,
                    },
                    colors,
                )
                .id(gpui::ElementId::Name(
                    format!("usage-pool-{}-{}", pool.provider, window.id).into(),
                ))
                .tooltip(usage_tooltip(
                    std::iter::once(window.label.clone())
                        .chain(window.pace.map(|pace| pace.label().to_owned()))
                        .collect(),
                    colors,
                ))
                .into_any_element(),
            );
        }
        let many_accounts = pool.accounts.len() > 1;
        for (account, member) in members {
            let environments = account.environments.join(", ");
            let label = if shared {
                environments
            } else if many_accounts {
                format!("{} · {environments}", window.label)
            } else {
                window.label.clone()
            };
            let restores = window
                .resets
                .iter()
                .find(|reset| reset.account == account.key)
                .map(|reset| reset.restores_percent);
            rows.push(self.render_usage_account_row(
                pool,
                window,
                account,
                &member.window,
                label,
                shared,
                restores,
                now,
                colors,
                cx,
            ));
        }
        rows
    }

    #[allow(clippy::too_many_arguments)]
    fn render_usage_account_row(
        &self,
        pool: &LimitPool,
        pool_window: &PoolWindow,
        account: &LimitAccount,
        window: &xd_desktop::usage::LimitWindow,
        label: String,
        nested: bool,
        restores: Option<i64>,
        now: i64,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let remaining = window.remaining_percent();
        let time_left = window.pace_remaining_percent(now);
        let resets_in = window.resets_in(now);
        let credits = account_credits(account);
        let key = format!("{}:{}:{}", pool.provider, pool_window.id, account.key);
        let pace = window.pace(now);
        // The row may clip a long name, so the tooltip leads with all of it.
        let mut lines = vec![
            label.clone(),
            format!(
                "{remaining}% left{}",
                time_left.map_or_else(String::new, |left| format!(" · {left}% of the window left"))
            ),
        ];
        if let Some(pace) = pace {
            lines.push(pace.label().to_owned());
        }
        if let Some(label) = &window.resets_at_label {
            lines.push(format!(
                "Resets {label}{}",
                resets_in
                    .as_ref()
                    .map_or_else(String::new, |resets| format!(" · {resets}"))
            ));
        }
        if let Some(banked) = account.reset_credits.as_ref().filter(|_| credits > 0) {
            lines.push(banked.summary(now));
        }
        let open = self.usage.segment.as_deref() == Some(key.as_str());
        let popover = open.then(|| {
            self.render_usage_segment_popover(account, window, restores, now, credits, colors, cx)
        });
        let click_key = key.clone();
        let row = limit_row(
            LimitRow {
                label: label.into(),
                nested,
                remaining,
                time_left,
                pace,
                detail: resets_in.map(|resets| resets.replacen("resets", "Resets", 1)),
                credits,
            },
            colors,
        );
        div()
            .relative()
            .child(
                row.id(gpui::ElementId::Name(format!("usage-limit-{key}").into()))
                    .cursor_pointer()
                    .when(open, |row| row.bg(rgb(colors.selected_surface)))
                    // The open popover already says all of this.
                    .when(!open, |row| {
                        row.hover(|style| style.bg(rgb(colors.surface_high)))
                            .tooltip(usage_tooltip(lines, colors))
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage.menu = None;
                        this.usage.segment =
                            if this.usage.segment.as_deref() == Some(click_key.as_str()) {
                                None
                            } else {
                                Some(click_key.clone())
                            };
                        cx.notify();
                    })),
            )
            .children(popover)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_usage_segment_popover(
        &self,
        account: &LimitAccount,
        window: &xd_desktop::usage::LimitWindow,
        restores: Option<i64>,
        now: i64,
        credits: u64,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = |label: &'static str, value: String| {
            div()
                .flex()
                .gap_3()
                .child(
                    div()
                        .w(px(72.0))
                        .flex_none()
                        .text_color(rgb(colors.muted))
                        .child(label),
                )
                .child(div().min_w_0().text_color(rgb(colors.text)).child(value))
        };
        let remaining = window.remaining_percent();
        let time_left = window.pace_remaining_percent(now);
        let resets_in = window.resets_in(now);
        let confirm = ResetConfirm {
            environment: account.environment,
            account_key: account.key.clone(),
            identity: account.identity.clone(),
        };
        let busy = self.usage.reset_busy.is_some();
        // A login that could not be named cannot be checked again before spending.
        let reset_credits: Option<&ResetCredits> = account
            .reset_credits
            .as_ref()
            .filter(|_| credits > 0 && account.identity.is_some());
        deferred(
            // Beside the flyout, level with the row, so the row stays in view.
            anchored()
                .anchor(gpui::Corner::BottomLeft)
                .snap_to_window_with_margin(px(8.0))
                .child(
                    div()
                        .occlude()
                        .ml(px(FLYOUT_WIDTH - 1.0))
                        .w(px(288.0))
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.chrome))
                        .shadow_lg()
                        .text_size(px(12.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(colors.text))
                                .child(assistant_icon(
                                    provider_icon(&account.provider),
                                    16.0,
                                    colors.muted,
                                    colors,
                                ))
                                .child(provider_label(&account.provider).to_owned()),
                        )
                        .child(
                            div()
                                .pt(px(10.0))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .border_t_1()
                                .border_color(with_alpha(colors.border, 0.6))
                                .when_some(account.plan.clone(), |rows, plan| {
                                    rows.child(row("Plan", plan))
                                })
                                .child(row("Signed in", account.environments.join(", "))),
                        )
                        .child(
                            div()
                                .pt(px(10.0))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .border_t_1()
                                .border_color(with_alpha(colors.border, 0.6))
                                .child(row("Left", format!("{remaining}%")))
                                .when_some(window.resets_at_label.clone(), |rows, label| {
                                    let suffix =
                                        resets_in.as_ref().map_or_else(String::new, |resets| {
                                            format!(" · {}", resets.replace("resets in ", "in "))
                                        });
                                    rows.child(row("Resets", format!("{label}{suffix}")))
                                })
                                .when_some(time_left, |rows, left| {
                                    rows.child(row("Even pace", format!("{left}% left by now")))
                                })
                                .when_some(
                                    restores.filter(|percent| *percent > 0),
                                    |rows, percent| {
                                        rows.child(row("Restores", format!("+{percent}% of pool")))
                                    },
                                ),
                        )
                        .when_some(reset_credits, |popover, credits| {
                            popover.child(
                                div()
                                    .pt(px(10.0))
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .border_t_1()
                                    .border_color(with_alpha(colors.border, 0.6))
                                    .text_color(rgb(colors.muted))
                                    .child(div().flex_1().min_w_0().child(credits.summary(now)))
                                    .child(
                                        ui_button("usage-use-reset", ButtonKind::Secondary, colors)
                                            .h(px(24.0))
                                            .when(busy, |button| {
                                                button.opacity(0.6).cursor_default()
                                            })
                                            .child(if busy { "Using…" } else { "Use reset" })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if this.usage.reset_busy.is_none() {
                                                    this.usage.segment = None;
                                                    this.usage.confirm_reset =
                                                        Some(confirm.clone());
                                                    cx.notify();
                                                }
                                            })),
                                    ),
                            )
                        }),
                ),
        )
        .with_priority(2)
        .into_any_element()
    }
}

fn shown_environments(held: &[(usize, (u32, i64), &str)], asked_days: u32) -> Vec<usize> {
    let days = if held.iter().any(|(_, window, _)| window.0 == asked_days) {
        asked_days
    } else {
        held.iter()
            .map(|(_, window, _)| window.0)
            .max()
            .unwrap_or(asked_days)
    };
    // The newest answer names the window. A window of the same length that
    // starts elsewhere, after midnight or a minute later, covers other periods.
    let Some(start) = held
        .iter()
        .filter(|(_, window, _)| window.0 == days)
        .max_by_key(|(_, window, _)| window.1)
        .map(|(_, _, start)| *start)
    else {
        return Vec::new();
    };
    held.iter()
        .filter(|(_, window, held_start)| window.0 == days && *held_start == start)
        .map(|(index, _, _)| *index)
        .collect()
}

fn usage_dialog_panel(width: f32, colors: ThemeColors) -> gpui::Div {
    div()
        .w_full()
        .max_w(px(width))
        .max_h_full()
        .min_h_0()
        .flex()
        .flex_col()
        .rounded(px(12.0))
        .border_1()
        .border_color(rgb(colors.border))
        .bg(rgb(colors.surface))
}

/// A dialog over the page. Presses on the panel stay in it, and a press on the
/// backdrop is the caller's to interpret.
fn usage_dialog_layer(
    panel: gpui::Div,
    colors: ThemeColors,
    on_backdrop: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .occlude()
        .absolute()
        .inset_0()
        .p_6()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(12.0))
        .bg(rgba(colors.backdrop))
        .on_mouse_down(MouseButton::Left, on_backdrop)
        .child(panel.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()))
        .into_any_element()
}

fn paint_usage_chart(
    bounds: Bounds<Pixels>,
    ticks: &[f32],
    series: &[(
        u32,
        Vec<xd_desktop::usage::CurveSegment>,
        Option<ChartPoint>,
    )],
    crosshair: Option<f32>,
    border: u32,
    muted: u32,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let at = |x: f32, y: f32| {
        point(
            bounds.left() + px(x / VIEW_WIDTH * width),
            bounds.top() + px(y / VIEW_HEIGHT * height),
        )
    };
    let map = |chart: ChartPoint| at(chart.x, chart.y);
    for tick in ticks {
        let start = at(0.0, *tick);
        // The bottom tick sits on the plot's edge, so it draws just inside it.
        let y = start.y.min(bounds.bottom() - px(1.0));
        window.paint_quad(gpui::fill(
            Bounds::new(point(bounds.left(), y), size(bounds.size.width, px(1.0))),
            rgb(border),
        ));
    }
    for (color, line, _) in series {
        let Some(first) = line.first() else {
            continue;
        };
        let mut area = PathBuilder::fill();
        area.move_to(map(first.from));
        for segment in line {
            area.cubic_bezier_to(
                map(segment.to),
                map(segment.control_a),
                map(segment.control_b),
            );
        }
        area.line_to(at(VIEW_WIDTH, VIEW_HEIGHT));
        area.line_to(at(0.0, VIEW_HEIGHT));
        area.close();
        if let Ok(path) = area.build() {
            window.paint_path(path, with_alpha(*color, 0.12));
        }
    }
    for (color, line, lone) in series {
        if let Some(lone) = lone {
            let mark = map(*lone);
            window.paint_quad(gpui::fill(
                Bounds::new(point(mark.x, mark.y - px(1.0)), size(px(3.0), px(3.0))),
                rgb(*color),
            ));
        }
        let Some(first) = line.first() else {
            continue;
        };
        let mut stroke = PathBuilder::stroke(px(2.0));
        stroke.move_to(map(first.from));
        for segment in line {
            stroke.cubic_bezier_to(
                map(segment.to),
                map(segment.control_a),
                map(segment.control_b),
            );
        }
        if let Ok(path) = stroke.build() {
            window.paint_path(path, rgb(*color));
        }
    }
    if let Some(x) = crosshair {
        let start = at(x, 8.0);
        window.paint_quad(gpui::fill(
            Bounds::new(start, size(px(1.0), bounds.bottom() - start.y)),
            rgb(muted),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary_event(generation: u64) -> Map<String, Value> {
        json!({
            "generation": generation,
            "summary": {
                "resolution": "day",
                "window_label": "Oct 9 to Nov 7",
                "periods": [{"key":"2026-11-07","label":"Nov 7","tooltip":"Nov 7"}],
                "sessions": {"claude": 1},
                "sources": [],
                "buckets": [{
                    "day":"2026-11-07", "provider":"claude", "model":"claude-opus", "cost_usd":2.0,
                    "cache_savings_usd":0.0, "totals":{"uncached_input":100,"output":50},
                    "records":1, "unpriced_records":0,
                }],
            },
        })
        .as_object()
        .cloned()
        .unwrap()
    }

    #[gpui::test]
    fn the_page_opens_closes_in_layers_and_drops_stale_answers(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update_window_entity(&desktop, |desktop, window, cx| {
            desktop.settings.usage_metric = "cost".into();
            desktop.toggle_usage(window, cx);
            assert!(desktop.usage_open);
            assert!(
                desktop.minimal_popup_is_open(),
                "the browser pane must yield to the page"
            );
            assert!(desktop.usage.focus.is_focused(window));

            desktop.usage.environments[0].summary_generation = 5;
            desktop.usage.environments[0].requested_window = Some((30, 1));
            assert!(desktop.usage_event(0, "usage-summary", &summary_event(4)));
            assert!(
                desktop.usage.environments[0].summary.is_none(),
                "an older answer is dropped"
            );
            assert!(desktop.usage_event(0, "usage-summary", &summary_event(5)));
            assert!(desktop.usage.environments[0].summary.is_some());
            assert!(
                !desktop.usage_event(0, "tree", &Map::new()),
                "other traffic passes through"
            );

            desktop.usage.model = Some("claude:claude-opus".into());
            desktop.usage.menu = Some(UsageMenu::Provider);
            assert!(desktop.dismiss_usage_layer(window, cx));
            assert!(desktop.usage.menu.is_none());
            assert!(desktop.usage.model.is_some(), "one press closes one layer");
            assert!(desktop.dismiss_usage_layer(window, cx));
            assert!(desktop.usage.model.is_none());
            assert!(desktop.usage_open);
            assert!(desktop.dismiss_usage_layer(window, cx));
            assert!(!desktop.usage_open);
            assert!(
                !desktop.dismiss_usage_layer(window, cx),
                "a closed page leaves Escape alone"
            );
        });
    }

    #[test]
    fn only_answers_for_one_window_merge() {
        // Both hosts on 30 days.
        let both = [(0, (30, 1), "oct-9"), (1, (30, 2), "oct-9")];
        assert_eq!(shown_environments(&both, 30), [0, 1]);
        // Switched to 7 days: the host that answered shows, the other's 30 days do not mix in.
        let switched = [(0, (7, 3), "nov-1"), (1, (30, 2), "oct-9")];
        assert_eq!(shown_environments(&switched, 7), [0]);
        // Nothing for the asked window yet: the old answers stay on screen.
        assert_eq!(shown_environments(&both, 7), [0, 1]);
        // Refreshed after midnight: same length, but a different set of days.
        let midnight = [(0, (7, 9), "nov-2"), (1, (7, 3), "nov-1")];
        assert_eq!(shown_environments(&midnight, 7), [0]);
        // Rolling hours from different instants have different buckets.
        let hours = [(0, (1, 5), "01:00"), (1, (1, 9), "01:04")];
        assert_eq!(shown_environments(&hours, 1), [1]);
        let same = [(0, (1, 9), "01:04"), (1, (1, 9), "01:04")];
        assert_eq!(shown_environments(&same, 1), [0, 1]);
        assert!(shown_environments(&[], 30).is_empty());
    }

    #[gpui::test]
    fn late_price_answers_and_other_hosts_disconnects_leave_a_dialog_alone(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update_window_entity(&desktop, |desktop, _, cx| {
            desktop.usage.price = Some(PriceDialog {
                rows: Vec::new(),
                pending_model: None,
                id: 9,
                saving: vec![0, 1],
                error: None,
            });
            let saved = json!({"ok": true, "prices": {}, "aliases": {}})
                .as_object()
                .cloned()
                .unwrap();
            let earlier = RequestKind::SetUsagePrices { dialog: 8 };
            assert!(desktop.usage_reply(0, &earlier, &saved, cx));
            assert_eq!(desktop.usage.price.as_ref().unwrap().saving, [0, 1]);

            let gone = HostUpdate::Disconnected {
                message: "gone".into(),
            };
            desktop.usage_host_update(ChatEndpoint::Remote, &gone, cx);
            let dialog = desktop.usage.price.as_ref().unwrap();
            assert_eq!(dialog.saving, [0], "the local save is still owed");
            assert!(dialog.error.as_deref().unwrap().starts_with("Remote"));
        });
    }

    #[gpui::test]
    fn a_disconnect_withdraws_the_confirmation_and_keeps_the_attempt(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update_window_entity(&desktop, |desktop, _, cx| {
            let confirm = ResetConfirm {
                environment: 1,
                account_key: "account".into(),
                identity: Some("account".into()),
            };
            desktop.usage.confirm_reset = Some(confirm.clone());
            desktop.usage.environments[1].accounts = Some(Vec::new());
            let gone = HostUpdate::Disconnected {
                message: "gone".into(),
            };
            // Another host dropping changes nothing about this confirmation.
            desktop.usage_host_update(ChatEndpoint::Local, &gone, cx);
            assert!(desktop.usage.confirm_reset.is_some());
            desktop.usage_host_update(ChatEndpoint::Remote, &gone, cx);
            assert!(
                desktop.usage.confirm_reset.is_none(),
                "that slot may return as another login"
            );
            assert!(desktop.usage.environments[1].accounts.is_none());

            // No host: the attempt fails at once and its key survives for a retry.
            desktop.usage_use_reset_credit(confirm.clone(), cx);
            assert_eq!(desktop.usage.reset_busy, None);
            let key = desktop.usage.reset_attempts["account"].clone();
            // Another account's reset neither takes nor replaces this attempt's key.
            let other = ResetConfirm {
                account_key: "other".into(),
                ..confirm.clone()
            };
            std::thread::sleep(Duration::from_millis(2));
            desktop.usage_use_reset_credit(other, cx);
            let other_key = desktop.usage.reset_attempts["other"].clone();
            assert_ne!(other_key, key);
            desktop.usage_use_reset_credit(confirm, cx);
            assert_eq!(desktop.usage.reset_attempts["account"], key);

            desktop.usage.reset_busy = Some(1);
            let answer = json!({"outcome": "reset", "idempotency_key": key})
                .as_object()
                .cloned()
                .unwrap();
            assert!(desktop.usage_event(1, "usage-reset-credit", &answer));
            assert_eq!(desktop.usage.reset_busy, None);
            assert!(
                !desktop.usage.reset_attempts.contains_key("account"),
                "settled"
            );
            assert!(
                desktop.usage.reset_attempts.contains_key("other"),
                "still unanswered"
            );
        });
    }

    #[gpui::test]
    fn a_dropped_host_leaves_nothing_behind(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update_window_entity(&desktop, |desktop, _, cx| {
            let environment = &mut desktop.usage.environments[1];
            environment.summary_generation = 3;
            environment.requested_window = Some((30, 1));
            environment.prices = Some(json!({"prices": {}}));
            assert!(desktop.usage_event(1, "usage-summary", &summary_event(3)));
            assert!(desktop.usage.environments[1].summary.is_some());

            let gone = HostUpdate::Disconnected {
                message: "gone".into(),
            };
            desktop.usage_host_update(ChatEndpoint::Remote, &gone, cx);
            let environment = &desktop.usage.environments[1];
            assert!(
                environment.summary.is_none(),
                "it may return as another host"
            );
            assert!(environment.summary_window.is_none());
            assert!(environment.prices.is_none());

            // An answer without a generation matches nothing now that none is owed.
            let mut unnumbered = summary_event(0);
            unnumbered.remove("generation");
            assert!(desktop.usage_event(1, "usage-summary", &unnumbered));
            assert!(desktop.usage.environments[1].summary.is_none());
        });
    }

    #[gpui::test]
    fn price_rows_fill_repeatably_and_never_save_from_an_unread_host(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update_window_entity(&desktop, |desktop, _, cx| {
            desktop.usage.price = Some(PriceDialog {
                rows: Vec::new(),
                pending_model: None,
                id: 1,
                saving: Vec::new(),
                error: None,
            });
            let price = json!({"input": 1.0, "output": 2.0});
            desktop.usage.environments[0].prices = Some(json!({"prices": {"a": price}}));
            // Waiting on the other host: nothing to show yet, and nothing may be saved.
            desktop.usage_fill_price_rows(&[0, 1], cx);
            assert!(desktop.usage.price.as_ref().unwrap().rows.is_empty());
            assert_eq!(desktop.usage_unread_prices(&[0, 1]), Some(1));
            // That host dropped: the survivor's prices appear without a new answer.
            desktop.usage_fill_price_rows(&[0], cx);
            desktop.usage_fill_price_rows(&[0], cx);
            let rows = &desktop.usage.price.as_ref().unwrap().rows;
            assert_eq!(rows.len(), 1, "filling twice adds nothing");
            assert_eq!(desktop.usage_unread_prices(&[0]), None);
            // It came back: only what it alone holds is added.
            desktop.usage.environments[1].prices =
                Some(json!({"prices": {"a": price, "b": price}}));
            desktop.usage_fill_price_rows(&[0, 1], cx);
            let rows = &desktop.usage.price.as_ref().unwrap().rows;
            let names: Vec<&str> = rows.iter().map(|row| row.model.as_str()).collect();
            assert_eq!(names, ["a", "b"]);
        });
    }

    #[test]
    fn a_limit_is_neutral_until_it_needs_attention() {
        let colors = xd_desktop::theme::ThemePreset::Dark.colors();
        assert_eq!(limit_tone(100, Some(Pace::Under), colors), colors.muted);
        assert_eq!(limit_tone(8, Some(Pace::On), colors), colors.muted);
        assert_eq!(limit_tone(50, None, colors), colors.muted);
        assert_eq!(limit_tone(7, Some(Pace::Ahead), colors), colors.waiting);
        assert_eq!(limit_tone(0, Some(Pace::Ahead), colors), colors.danger);
        assert_eq!(limit_tone(0, None, colors), colors.danger);
    }

    #[gpui::test]
    fn the_flyout_and_full_page_close_one_layer_at_a_time(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.simulate_resize(size(px(1600.0), px(1000.0)));
        let open = |metric: &str, full_page: bool, cx: &mut gpui::VisualTestContext| {
            cx.update_window_entity(&desktop, |desktop, _, cx| {
                desktop.settings.usage_metric = metric.to_owned();
                desktop.usage.full_page = full_page;
                desktop.usage_open = true;
                cx.notify();
            });
            cx.run_until_parked();
        };
        let press = |at: Point<Pixels>, cx: &mut gpui::VisualTestContext| {
            cx.simulate_click(at, gpui::Modifiers::default());
            cx.run_until_parked();
        };
        let beside = point(px(800.0), px(400.0));

        open("limits", false, cx);
        cx.update_window_entity(&desktop, |desktop, _, cx| {
            desktop.usage.segment = Some("codex:weekly:account".into());
            cx.notify();
        });
        cx.run_until_parked();
        press(beside, cx);
        desktop.read_with(cx, |desktop, _| {
            assert!(desktop.usage_open, "the press only closes the popover");
            assert!(desktop.usage.segment.is_none());
        });
        press(beside, cx);
        desktop.read_with(cx, |desktop, _| assert!(!desktop.usage_open));

        // Cost and tokens open in the flyout too.
        open("cost", false, cx);
        press(beside, cx);
        desktop.read_with(cx, |desktop, _| assert!(!desktop.usage_open));

        // The full page fills the window, so that spot is inside it.
        open("cost", true, cx);
        press(beside, cx);
        desktop.read_with(cx, |desktop, _| assert!(desktop.usage_open));
        cx.update_window_entity(&desktop, |desktop, window, cx| {
            assert!(desktop.dismiss_usage_layer(window, cx));
            assert!(desktop.usage_open, "Escape returns to the flyout");
            assert!(desktop.usage_flyout_open());
            desktop.usage.full_page = true;
            desktop.select_usage_metric("limits", cx);
            assert!(
                desktop.usage_flyout_open(),
                "limits only live in the flyout"
            );
            assert!(!desktop.usage.full_page);
        });
    }

    #[gpui::test]
    fn the_flyout_says_when_filters_hide_usage(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update_window_entity(&desktop, |desktop, _, cx| {
            assert!(!desktop.usage_filtered());
            desktop.settings.usage_hidden_providers = vec!["codex".to_owned()];
            assert!(desktop.usage_filtered());
            desktop.usage_show_everything(cx);
            assert!(!desktop.usage_filtered());
            assert!(desktop.settings.usage_hidden_providers.is_empty());
        });
    }

    #[gpui::test]
    fn limits_rank_by_arrival_not_by_a_hosts_clock(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update_window_entity(&desktop, |desktop, _, _| {
            let answer = |generation: u64, checked: i64, used: f64| {
                json!({
                    "generation": generation,
                    "accounts": [{
                        "provider": "codex", "account_key": "same", "checked_at_ms": checked,
                        "windows": [{"id":"primary","kind":"weekly","label":"Weekly","used_percent":used}],
                    }],
                })
                .as_object()
                .cloned()
                .unwrap()
            };
            desktop.usage.environments[1].limits_generation = 1;
            // The remote clock runs a day ahead of this machine.
            let ahead = now_ms() + 86_400_000;
            assert!(desktop.usage_event(1, "usage-limits", &answer(1, ahead, 100.0)));
            std::thread::sleep(Duration::from_millis(2));
            desktop.usage.environments[0].limits_generation = 2;
            assert!(desktop.usage_event(0, "usage-limits", &answer(2, 5, 0.0)));
            let reports = [0, 1].into_iter().filter_map(|index| {
                desktop.usage.environments[index].accounts.clone()
            });
            let accounts = collect_accounts(reports.flatten());
            assert_eq!(accounts[0].windows[0].used_percent, 0.0, "the newer answer wins");

            // A slow probe answers last, but what it says was read a minute ago.
            std::thread::sleep(Duration::from_millis(2));
            let mut slow = answer(3, 1_000_000 - 60_000, 100.0);
            slow.insert("host_now_ms".into(), json!(1_000_000));
            desktop.usage.environments[1].limits_generation = 3;
            assert!(desktop.usage_event(1, "usage-limits", &slow));
            let reports = [0, 1].into_iter().filter_map(|index| {
                desktop.usage.environments[index].accounts.clone()
            });
            let accounts = collect_accounts(reports.flatten());
            assert_eq!(accounts[0].windows[0].used_percent, 0.0, "age counts, not arrival");

            // A scan that died reports why and ends the wait.
            desktop.usage.environments[0].summary_generation = 4;
            desktop.usage.environments[0].summary_loading = true;
            let died = json!({"generation": 4, "error": "The host could not read usage."});
            assert!(desktop.usage_event(0, "usage-summary", died.as_object().unwrap()));
            let environment = &desktop.usage.environments[0];
            assert!(!environment.summary_loading);
            assert_eq!(environment.summary_error.as_deref(), Some("The host could not read usage."));
        });
    }

    #[gpui::test]
    fn every_view_renders(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update_window_entity(&desktop, |desktop, window, cx| {
            desktop.toggle_usage(window, cx);
            let accounts = parse_limit_accounts(
                &json!({"accounts":[
                    {"provider":"codex", "plan":"Pro", "windows":[
                        {"id":"primary","kind":"weekly","label":"Weekly","used_percent":93.0,
                         "resets_at_ms":now_ms() + 3_600_000,"resets_at_label":"soon","window_duration_mins":10080},
                    ], "reset_credits":{"available_count":1}},
                    {"provider":"claude", "windows":[], "unavailable":{"reason":"probeFailed"}},
                ]}),
                0,
                "Local",
            );
            desktop.usage.environments[0].accounts = Some(accounts);
            desktop.usage.environments[0].summary =
                summary_event(1).get("summary").and_then(Summary::parse);
            desktop.usage.environments[0].summary_window =
                Some((desktop.settings.usage_window_days, 1));
        });
        for metric in ["limits", "cost", "tokens"] {
            cx.update_window_entity(&desktop, |desktop, _, _| {
                desktop.settings.usage_metric = metric.into();
                desktop.usage.breakdown_by_time = metric == "tokens";
            });
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear();
            });
        }
        cx.update_window_entity(&desktop, |desktop, _, _| {
            desktop.settings.usage_metric = "cost".into();
            desktop.usage.model = Some("claude:claude-opus".into());
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
    }

    #[gpui::test]
    fn shortcuts_switch_views_only_outside_dialogs(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        let press = |key: &str| gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse(key).unwrap(),
            is_held: false,
        };
        cx.update_window_entity(&desktop, |desktop, window, cx| {
            desktop.settings.usage_metric = "cost".into();
            desktop.settings.usage_window_days = 30;
            desktop.toggle_usage(window, cx);
            desktop.usage_key_down(&press("t"), cx);
            assert_eq!(desktop.settings.usage_metric, "tokens");
            desktop.usage_key_down(&press("secondary-shift-1"), cx);
            assert_eq!(desktop.settings.usage_window_days, 1);
            desktop.usage_key_down(&press("secondary-shift-4"), cx);
            assert_eq!(desktop.settings.usage_window_days, 90);

            desktop.usage_key_down(&press("l"), cx);
            assert_eq!(desktop.settings.usage_metric, "limits");
            desktop.usage_key_down(&press("secondary-shift-2"), cx);
            assert_eq!(
                desktop.settings.usage_window_days, 90,
                "limits have no period"
            );

            desktop.usage.price = Some(PriceDialog {
                rows: Vec::new(),
                pending_model: None,
                id: 1,
                saving: Vec::new(),
                error: None,
            });
            desktop.usage_key_down(&press("c"), cx);
            assert_eq!(
                desktop.settings.usage_metric, "limits",
                "typing belongs to the dialog"
            );
        });
    }

    #[gpui::test]
    fn price_rows_validate_and_saved_prices_reopen(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.update_window_entity(&desktop, |desktop, window, cx| {
            desktop.host = None;
            desktop.remote_host = None;
            desktop.open_usage_prices(Some("set-me".into()), window, cx);
            desktop.usage.environments[0].prices = Some(json!({
                "prices": {"held": {"input": 1.5, "output": 3.0, "cache_read": null, "cache_write": null}},
                "aliases": {"old": "held"},
            }));
            desktop.usage.hidden_environments.clear();
            desktop.usage_fill_price_rows(&[0], cx);
            let dialog = desktop.usage.price.as_mut().unwrap();
            let names: Vec<&str> = dialog.rows.iter().map(|row| row.model.as_str()).collect();
            assert_eq!(names, ["held", "old", "set-me"]);
            assert!(!dialog.rows[0].changed());
            assert_eq!(dialog.rows[0].values[0], "1.5");
            assert_eq!(dialog.rows[1].alias, "held");
            assert!(dialog.rows[2].changed(), "a requested model is a new row");
            assert_eq!(dialog.rows[2].validate(), Err("Enter input and output prices."));

            dialog.rows[2].values[0] = "2".into();
            dialog.rows[2].values[1] = "abc".into();
            assert_eq!(dialog.rows[2].validate(), Err("Enter valid prices."));
            dialog.rows[2].values[1] = "4".into();
            assert_eq!(dialog.rows[2].validate().unwrap().unwrap()["output"], json!(4.0));
            dialog.rows[2].alias = "set-me".into();
            assert_eq!(dialog.rows[2].validate(), Err("A model cannot map to itself."));
            dialog.rows[2].alias = "held".into();
            assert_eq!(dialog.rows[2].validate(), Ok(None));
            dialog.rows[0].removed = true;
            assert!(dialog.rows[0].changed());
        });
    }
}
