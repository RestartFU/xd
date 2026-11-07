//! The Usage page's data: host answers merged across connected hosts, the
//! formats they display in, and the chart and limit-pool math. Ported from
//! T3 Code (MIT, see `data/licenses/t3code-LICENSE`).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde_json::Value;

pub const PROVIDER_ORDER: [&str; 2] = ["codex", "claude"];

pub fn provider_label(provider: &str) -> &str {
    match provider {
        "codex" => "Codex",
        "claude" => "Claude Code",
        other => other,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TokenTotals {
    pub uncached_input: u64,
    pub cached_input: u64,
    pub cache_creation: u64,
    pub output: u64,
    pub reasoning: u64,
}

impl TokenTotals {
    /// Reasoning is a subset of output and must not be added again.
    pub fn total(&self) -> u64 {
        self.uncached_input + self.cached_input + self.cache_creation + self.output
    }

    fn add(&mut self, other: &Self) {
        self.uncached_input += other.uncached_input;
        self.cached_input += other.cached_input;
        self.cache_creation += other.cache_creation;
        self.output += other.output;
        self.reasoning += other.reasoning;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CategoryCost {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
    pub unsplit: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpeedCost {
    pub standard: f64,
    pub fast: f64,
    pub ultrafast: f64,
    pub premium: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    pub day: String,
    pub hour_start: Option<String>,
    pub provider: String,
    pub model: String,
    pub totals: TokenTotals,
    pub cost_usd: f64,
    pub cache_savings_usd: f64,
    pub category: Option<CategoryCost>,
    pub fast_cost_usd: f64,
    pub ultrafast_cost_usd: f64,
    pub speed_premium_usd: f64,
    pub records: u64,
    pub unpriced_records: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Period {
    pub key: String,
    pub label: String,
    pub tooltip: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub provider: String,
    pub fingerprint: String,
    pub status: String,
    pub malformed_records: u64,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub hourly: bool,
    pub window_label: String,
    pub periods: Vec<Period>,
    pub buckets: Vec<Bucket>,
    pub sessions: BTreeMap<String, u64>,
    pub sources: Vec<Source>,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

fn count(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

impl Summary {
    pub fn parse(value: &Value) -> Option<Self> {
        let buckets = value["buckets"]
            .as_array()?
            .iter()
            .map(|bucket| {
                let totals = &bucket["totals"];
                Bucket {
                    day: text(&bucket["day"]),
                    hour_start: bucket["hour_start"].as_str().map(str::to_owned),
                    provider: text(&bucket["provider"]),
                    model: text(&bucket["model"]),
                    totals: TokenTotals {
                        uncached_input: count(&totals["uncached_input"]),
                        cached_input: count(&totals["cached_input"]),
                        cache_creation: count(&totals["cache_creation"]),
                        output: count(&totals["output"]),
                        reasoning: count(&totals["reasoning"]),
                    },
                    cost_usd: number(&bucket["cost_usd"]),
                    cache_savings_usd: number(&bucket["cache_savings_usd"]),
                    category: bucket["category_cost_usd"]
                        .as_object()
                        .map(|cost| CategoryCost {
                            input: cost.get("input").map_or(0.0, number),
                            cache_read: cost.get("cache_read").map_or(0.0, number),
                            cache_write: cost.get("cache_write").map_or(0.0, number),
                            output: cost.get("output").map_or(0.0, number),
                            unsplit: 0.0,
                        }),
                    fast_cost_usd: number(&bucket["fast_cost_usd"]),
                    ultrafast_cost_usd: number(&bucket["ultrafast_cost_usd"]),
                    speed_premium_usd: number(&bucket["speed_premium_usd"]),
                    records: count(&bucket["records"]),
                    unpriced_records: count(&bucket["unpriced_records"]),
                }
            })
            .collect();
        Some(Self {
            hourly: value["resolution"] == "hour",
            window_label: text(&value["window_label"]),
            periods: value["periods"]
                .as_array()?
                .iter()
                .map(|period| Period {
                    key: text(&period["key"]),
                    label: text(&period["label"]),
                    tooltip: text(&period["tooltip"]),
                })
                .collect(),
            buckets,
            sessions: value["sessions"]
                .as_object()
                .map(|sessions| {
                    sessions
                        .iter()
                        .map(|(provider, sessions)| (provider.clone(), count(sessions)))
                        .collect()
                })
                .unwrap_or_default(),
            sources: value["sources"]
                .as_array()
                .map(|sources| {
                    sources
                        .iter()
                        .map(|source| Source {
                            provider: text(&source["provider"]),
                            fingerprint: source["fingerprint"].to_string(),
                            status: text(&source["status"]),
                            malformed_records: count(&source["malformed_records"]),
                            message: source["message"].as_str().map(str::to_owned),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderTotals {
    pub provider: String,
    pub cost_usd: f64,
    pub total_tokens: u64,
    pub records: u64,
    pub sessions: u64,
    pub cost_share: f64,
    pub token_share: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelTotals {
    pub provider: String,
    pub model: String,
    pub cost_usd: f64,
    pub total_tokens: u64,
    pub tokens: TokenTotals,
    pub records: u64,
    pub unpriced_records: u64,
    pub unpriced_tokens: f64,
    pub cost_share: f64,
    pub token_share: f64,
}

impl ModelTotals {
    pub fn key(&self) -> String {
        format!("{}:{}", self.provider, self.model)
    }

    /// When every record lacks rates, cost is unknown rather than zero.
    pub fn cost_unknown(&self) -> bool {
        self.records > 0 && self.unpriced_records >= self.records
    }

    /// Unknown cost has no cost share; unpriced models still have token shares.
    pub fn share(&self, metric: ChartMetric) -> Option<f64> {
        match metric {
            ChartMetric::Tokens => Some(self.token_share),
            ChartMetric::Cost => (!self.cost_unknown()).then_some(self.cost_share),
        }
    }

    /// `None` without input. Cache writes count as misses: that input was fully processed.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let input =
            self.tokens.uncached_input + self.tokens.cached_input + self.tokens.cache_creation;
        (input > 0).then(|| self.tokens.cached_input as f64 / input as f64)
    }

    /// Effective USD per million priced tokens, or `None` when none were priced.
    pub fn cost_per_million(&self) -> Option<f64> {
        let priced = self.total_tokens as f64 - self.unpriced_tokens;
        (priced > 0.0 && !self.cost_unknown()).then(|| self.cost_usd / priced * 1_000_000.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeriodTotals {
    pub key: String,
    pub cost_usd: f64,
    pub total_tokens: u64,
    pub by_provider: BTreeMap<String, (f64, u64)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merged {
    pub cost_usd: f64,
    pub tokens: TokenTotals,
    pub total_tokens: u64,
    pub records: u64,
    pub sessions: u64,
    pub unpriced_share: f64,
    pub cache_savings_usd: f64,
    pub category_cost: CategoryCost,
    pub speed_cost: SpeedCost,
    pub providers: Vec<ProviderTotals>,
    pub models: Vec<ModelTotals>,
    /// Newest last, keyed by day or by hour start.
    pub periods: Vec<PeriodTotals>,
    pub duplicate_sources: Vec<String>,
}

fn add_period(periods: &mut BTreeMap<String, PeriodTotals>, key: &str, bucket: &Bucket) {
    let period = periods
        .entry(key.to_owned())
        .or_insert_with(|| PeriodTotals {
            key: key.to_owned(),
            ..PeriodTotals::default()
        });
    let tokens = bucket.totals.total();
    period.cost_usd += bucket.cost_usd;
    period.total_tokens += tokens;
    let provider = period
        .by_provider
        .entry(bucket.provider.clone())
        .or_default();
    provider.0 += bucket.cost_usd;
    provider.1 += tokens;
}

/// Merges answers from every selected host. Hosts sharing a transcript
/// directory are counted once. `keep` narrows to a provider or a model.
pub fn merge(
    environments: &[(&str, &Summary)],
    hidden: &[String],
    keep: impl Fn(&Bucket) -> bool,
) -> Merged {
    let mut merged = Merged::default();
    let mut owners = HashSet::new();
    let mut providers: BTreeMap<String, ProviderTotals> = BTreeMap::new();
    let mut models: BTreeMap<(String, String), ModelTotals> = BTreeMap::new();
    let mut periods: BTreeMap<String, PeriodTotals> = BTreeMap::new();
    let mut unpriced = 0;
    let mut duplicates = BTreeSet::new();
    for (label, summary) in environments {
        let mut dropped = HashSet::new();
        for source in &summary.sources {
            // A missing directory has no volume, so two of them are not one source.
            if source.status == "missing" {
                continue;
            }
            if !owners.insert(source.fingerprint.clone()) {
                dropped.insert(source.provider.as_str());
                duplicates.insert(format!("{label} {}", provider_label(&source.provider)));
            }
        }
        for (provider, sessions) in &summary.sessions {
            if hidden.contains(provider) || dropped.contains(provider.as_str()) {
                continue;
            }
            merged.sessions += sessions;
            providers
                .entry(provider.clone())
                .or_insert_with(|| ProviderTotals {
                    provider: provider.clone(),
                    ..ProviderTotals::default()
                })
                .sessions += sessions;
        }
        for bucket in &summary.buckets {
            if hidden.contains(&bucket.provider)
                || dropped.contains(bucket.provider.as_str())
                || !keep(bucket)
            {
                continue;
            }
            let tokens = bucket.totals.total();
            merged.cost_usd += bucket.cost_usd;
            merged.cache_savings_usd += bucket.cache_savings_usd;
            merged.tokens.add(&bucket.totals);
            merged.records += bucket.records;
            unpriced += bucket.unpriced_records;
            if let Some(cost) = &bucket.category {
                merged.category_cost.input += cost.input;
                merged.category_cost.cache_read += cost.cache_read;
                merged.category_cost.cache_write += cost.cache_write;
                merged.category_cost.output += cost.output;
            }
            merged.speed_cost.fast += bucket.fast_cost_usd;
            merged.speed_cost.ultrafast += bucket.ultrafast_cost_usd;
            merged.speed_cost.premium += bucket.speed_premium_usd;

            let provider =
                providers
                    .entry(bucket.provider.clone())
                    .or_insert_with(|| ProviderTotals {
                        provider: bucket.provider.clone(),
                        ..ProviderTotals::default()
                    });
            provider.cost_usd += bucket.cost_usd;
            provider.total_tokens += tokens;
            provider.records += bucket.records;

            let model = models
                .entry((bucket.provider.clone(), bucket.model.clone()))
                .or_insert_with(|| ModelTotals {
                    provider: bucket.provider.clone(),
                    model: bucket.model.clone(),
                    ..ModelTotals::default()
                });
            model.cost_usd += bucket.cost_usd;
            model.total_tokens += tokens;
            model.tokens.add(&bucket.totals);
            model.records += bucket.records;
            model.unpriced_records += bucket.unpriced_records;
            if bucket.records > 0 {
                model.unpriced_tokens +=
                    tokens as f64 * bucket.unpriced_records as f64 / bucket.records as f64;
            }

            add_period(
                &mut periods,
                bucket.hour_start.as_deref().unwrap_or(&bucket.day),
                bucket,
            );
        }
    }
    merged.total_tokens = merged.tokens.total();
    let share = |part: f64, whole: f64| if whole == 0.0 { 0.0 } else { part / whole };
    merged.providers = providers
        .into_values()
        .map(|mut provider| {
            provider.cost_share = share(provider.cost_usd, merged.cost_usd);
            provider.token_share = share(provider.total_tokens as f64, merged.total_tokens as f64);
            provider
        })
        .collect();
    merged
        .providers
        .sort_by(|left, right| right.cost_usd.total_cmp(&left.cost_usd));
    merged.models = models
        .into_values()
        .map(|mut model| {
            model.cost_share = share(model.cost_usd, merged.cost_usd);
            model.token_share = share(model.total_tokens as f64, merged.total_tokens as f64);
            model
        })
        .collect();
    merged.models.sort_by(|left, right| {
        right
            .cost_usd
            .total_cmp(&left.cost_usd)
            .then(right.total_tokens.cmp(&left.total_tokens))
    });
    merged.periods = periods.into_values().collect();
    merged.unpriced_share = share(unpriced as f64, merged.records as f64);
    let category = &mut merged.category_cost;
    category.unsplit = (merged.cost_usd
        - category.input
        - category.cache_read
        - category.cache_write
        - category.output)
        .max(0.0);
    merged.speed_cost.standard =
        (merged.cost_usd - merged.speed_cost.fast - merged.speed_cost.ultrafast).max(0.0);
    merged.duplicate_sources = duplicates.into_iter().collect();
    merged
}

pub fn sort_models_by_tokens(models: &[ModelTotals]) -> Vec<ModelTotals> {
    let mut sorted = models.to_vec();
    sorted.sort_by(|left, right| {
        right
            .total_tokens
            .cmp(&left.total_tokens)
            .then(right.cost_usd.total_cmp(&left.cost_usd))
    });
    sorted
}

/// Providers with data, ordered by the shown metric; ties keep the fixed order.
pub fn rank_providers(merged: &Merged, metric: ChartMetric, loading: &[&str]) -> Vec<String> {
    let value = |provider: &str| {
        merged
            .providers
            .iter()
            .find(|totals| totals.provider == provider)
            .map_or(0.0, |totals| match metric {
                ChartMetric::Cost => totals.cost_usd,
                ChartMetric::Tokens => totals.total_tokens as f64,
            })
    };
    let has_data = |provider: &str| {
        merged.providers.iter().any(|totals| {
            totals.provider == provider && (totals.total_tokens > 0 || totals.cost_usd > 0.0)
        })
    };
    let mut ranked: Vec<&str> = PROVIDER_ORDER
        .into_iter()
        .filter(|provider| has_data(provider) || loading.contains(provider))
        .collect();
    ranked.sort_by(|left, right| value(right).total_cmp(&value(left)));
    ranked.into_iter().map(str::to_owned).collect()
}

const MALFORMED_SUFFIX: &str =
    " malformed or inconsistent usage record(s) were skipped; usage may be incomplete.";

/// Read problems worth a line above the totals; a missing directory is not one.
pub fn source_messages<'a>(sources: impl Iterator<Item = &'a Source>) -> Vec<String> {
    let mut messages = Vec::new();
    let mut malformed = BTreeMap::new();
    for source in sources {
        if source.status != "partial"
            && source.status != "failed"
            && !(source.status == "missing" && source.malformed_records > 0)
        {
            continue;
        }
        if source.malformed_records > 0 {
            let held = malformed.entry(source.fingerprint.clone()).or_insert(0);
            *held = source.malformed_records.max(*held);
        }
        if let Some(message) = &source.message
            && !messages.contains(message)
        {
            messages.push(message.clone());
        }
    }
    let total: u64 = malformed.values().sum();
    if total > 0 {
        messages.push(format!("{total}{MALFORMED_SUFFIX}"));
    }
    messages
}

fn group_thousands(digits: &str) -> String {
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

pub fn format_usd(value: f64) -> String {
    let cents = format!("{:.2}", value.abs());
    let (whole, fraction) = cents.split_once('.').unwrap_or((&cents, "00"));
    let sign = if value < 0.0 && cents != "0.00" {
        "-"
    } else {
        ""
    };
    format!("{sign}${}.{fraction}", group_thousands(whole))
}

pub fn format_count(value: u64) -> String {
    group_thousands(&value.to_string())
}

fn trim_figure(value: f64) -> String {
    let abs = value.abs();
    let digits = if abs >= 100.0 {
        0
    } else if abs >= 10.0 {
        1
    } else {
        2
    };
    let fixed = format!("{value:.digits$}");
    // Drops only an all-zero fraction: "1.50" stays, "1.00" becomes "1".
    match fixed.split_once('.') {
        Some((whole, fraction)) if fraction.bytes().all(|byte| byte == b'0') => whole.to_owned(),
        _ => fixed,
    }
}

pub fn format_tokens(value: f64) -> String {
    let abs = value.abs();
    if abs >= 1e12 {
        format!("{}T", trim_figure(value / 1e12))
    } else if abs >= 1e9 {
        format!("{}B", trim_figure(value / 1e9))
    } else if abs >= 1e6 {
        format!("{}M", trim_figure(value / 1e6))
    } else if abs >= 1e3 {
        format!("{}K", trim_figure(value / 1e3))
    } else {
        format_count(value.round().max(0.0) as u64)
    }
}

pub fn format_percent(share: f64) -> String {
    let percent = share * 100.0;
    if percent > 0.0 && percent < 0.1 {
        "<0.1%".into()
    } else {
        format!("{percent:.1}%")
    }
}

pub fn format_duration(ms: i64) -> String {
    let remaining = ms.max(0);
    let days = remaining / 86_400_000;
    let hours = remaining % 86_400_000 / 3_600_000;
    let minutes = remaining % 3_600_000 / 60_000;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChartMetric {
    #[default]
    Cost,
    Tokens,
}

impl ChartMetric {
    pub fn format(self, value: f64) -> String {
        match self {
            Self::Cost => format_usd(value),
            Self::Tokens => format_tokens(value),
        }
    }
}

pub const VIEW_WIDTH: f32 = 960.0;
pub const VIEW_HEIGHT: f32 = 260.0;
const TICK_COUNT: f64 = 4.0;
const PLOT_TOP: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveSegment {
    pub from: ChartPoint,
    pub control_a: ChartPoint,
    pub control_b: ChartPoint,
    pub to: ChartPoint,
}

fn monotone_tangents(points: &[ChartPoint]) -> Vec<f32> {
    let count = points.len();
    if count < 2 {
        return vec![0.0];
    }
    let slopes: Vec<f32> = points
        .windows(2)
        .map(|pair| {
            let dx = pair[1].x - pair[0].x;
            if dx == 0.0 {
                0.0
            } else {
                (pair[1].y - pair[0].y) / dx
            }
        })
        .collect();
    let mut tangents = vec![0.0; count];
    tangents[0] = slopes[0];
    tangents[count - 1] = slopes[count - 2];
    for index in 1..count - 1 {
        let (previous, next) = (slopes[index - 1], slopes[index]);
        tangents[index] = if previous * next <= 0.0 {
            0.0
        } else {
            (previous + next) / 2.0
        };
    }
    for index in 0..count - 1 {
        let slope = slopes[index];
        if slope == 0.0 {
            tangents[index] = 0.0;
            tangents[index + 1] = 0.0;
            continue;
        }
        let a = tangents[index] / slope;
        let b = tangents[index + 1] / slope;
        let magnitude = a * a + b * b;
        if magnitude > 9.0 {
            let scale = 3.0 / magnitude.sqrt();
            tangents[index] = scale * a * slope;
            tangents[index + 1] = scale * b * slope;
        }
    }
    tangents
}

/// A curve that never overshoots its points, so a quiet day stays on the axis.
pub fn smooth_curve(points: &[ChartPoint]) -> Vec<CurveSegment> {
    if points.len() < 2 {
        return Vec::new();
    }
    let tangents = monotone_tangents(points);
    points
        .windows(2)
        .enumerate()
        .map(|(index, pair)| {
            let (from, to) = (pair[0], pair[1]);
            let dx = to.x - from.x;
            CurveSegment {
                from,
                control_a: ChartPoint {
                    x: from.x + dx / 3.0,
                    y: from.y + tangents[index] * dx / 3.0,
                },
                control_b: ChartPoint {
                    x: to.x - dx / 3.0,
                    y: to.y - tangents[index + 1] * dx / 3.0,
                },
                to,
            }
        })
        .collect()
}

pub fn nice_scale(peak: f64) -> (f64, Vec<f64>) {
    if peak <= 0.0 {
        return (0.0, vec![0.0]);
    }
    let raw_step = peak / TICK_COUNT;
    let magnitude = 10_f64.powf(raw_step.log10().floor());
    let normalized = raw_step / magnitude;
    let factor = if normalized > 5.0 {
        10.0
    } else if normalized > 2.0 {
        5.0
    } else if normalized > 1.0 {
        2.0
    } else {
        1.0
    };
    let step = factor * magnitude;
    let max = (peak / step).ceil() * step;
    let mut ticks = Vec::new();
    let mut value = 0.0;
    while value <= max + step * 1e-6 {
        ticks.push(value);
        value += step;
    }
    (max, ticks)
}

pub fn value_to_y(value: f64, max: f64) -> f32 {
    if max == 0.0 {
        VIEW_HEIGHT
    } else {
        VIEW_HEIGHT - (value / max) as f32 * (VIEW_HEIGHT - PLOT_TOP)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartSeries {
    pub provider: String,
    pub total: f64,
    pub line: Vec<CurveSegment>,
    /// A lone period has no line to draw, so it is drawn as a mark.
    pub point: Option<ChartPoint>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chart {
    pub ticks: Vec<(f64, f32)>,
    pub step_x: f32,
    pub series: Vec<ChartSeries>,
    /// Per period, per provider, in `providers` order.
    pub columns: Vec<Vec<f64>>,
    pub labeled: bool,
}

/// Layered series each start at zero, so the scale fits the tallest single
/// series. Summing peaks would waste plot height.
pub fn build_chart(
    periods: &[Period],
    totals: &[PeriodTotals],
    metric: ChartMetric,
    providers: &[String],
    loading: bool,
) -> Chart {
    let value = |period: &Period, provider: &str| {
        totals
            .iter()
            .find(|totals| totals.key == period.key)
            .and_then(|totals| totals.by_provider.get(provider))
            .map_or(0.0, |(cost, tokens)| match metric {
                ChartMetric::Cost => *cost,
                ChartMetric::Tokens => *tokens as f64,
            })
    };
    let columns: Vec<Vec<f64>> = periods
        .iter()
        .map(|period| {
            providers
                .iter()
                .map(|provider| value(period, provider))
                .collect()
        })
        .collect();
    let peak = columns
        .iter()
        .flatten()
        .fold(0.0_f64, |peak, value| peak.max(*value));
    let (max, ticks, labeled) = if peak == 0.0 && loading {
        (TICK_COUNT, (0..=4).map(f64::from).collect(), false)
    } else {
        let (max, ticks) = nice_scale(peak);
        (max, ticks, true)
    };
    let step_x = if periods.len() < 2 {
        0.0
    } else {
        VIEW_WIDTH / (periods.len() - 1) as f32
    };
    let mut series: Vec<ChartSeries> = providers
        .iter()
        .enumerate()
        .map(|(slot, provider)| {
            let points: Vec<ChartPoint> = columns
                .iter()
                .enumerate()
                .map(|(index, column)| ChartPoint {
                    x: index as f32 * step_x,
                    y: value_to_y(column[slot], max),
                })
                .collect();
            ChartSeries {
                provider: provider.clone(),
                total: columns.iter().map(|column| column[slot]).sum(),
                line: smooth_curve(&points),
                point: (points.len() == 1).then(|| points[0]),
            }
        })
        .collect();
    series.sort_by(|left, right| right.total.total_cmp(&left.total));
    Chart {
        ticks: ticks
            .iter()
            .map(|tick| (*tick, value_to_y(*tick, max)))
            .collect(),
        step_x,
        series,
        columns,
        labeled,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LimitWindow {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub used_percent: f64,
    pub resets_at_ms: Option<i64>,
    pub resets_at_label: Option<String>,
    pub window_duration_mins: Option<i64>,
}

impl LimitWindow {
    pub fn remaining_percent(&self) -> i64 {
        (100.0 - self.used_percent.clamp(0.0, 100.0)).round() as i64
    }

    pub fn elapsed_share(&self, now: i64) -> Option<f64> {
        let resets_at = self.resets_at_ms?;
        let length = self.window_duration_mins? * 60_000;
        if length <= 0 {
            return None;
        }
        Some(((length - (resets_at - now)) as f64 / length as f64).clamp(0.0, 1.0))
    }

    /// Bars show quota remaining, so even pace tracks time remaining.
    pub fn pace_remaining_percent(&self, now: i64) -> Option<i64> {
        self.elapsed_share(now)
            .map(|elapsed| ((1.0 - elapsed) * 100.0).round() as i64)
    }

    pub fn resets_in(&self, now: i64) -> Option<String> {
        let resets_at = self.resets_at_ms?;
        Some(if resets_at <= now {
            "resets now".into()
        } else if resets_at - now < 60_000 {
            "Resets momentarily".into()
        } else {
            format!("resets in {}", format_duration(resets_at - now))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    Ahead,
    On,
    Under,
}

impl Pace {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ahead => "Ahead of pace: spending faster than the window elapses",
            Self::On => "On pace with the window",
            Self::Under => "Under pace: headroom left for the rest of the window",
        }
    }
}

fn pace_of_shares(used_percent: f64, elapsed: f64) -> Pace {
    let gap = used_percent - elapsed * 100.0;
    if gap > 5.0 {
        Pace::Ahead
    } else if gap < -5.0 {
        Pace::Under
    } else {
        Pace::On
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResetCredits {
    pub available_count: u64,
    pub next_expires_at_ms: Option<i64>,
}

impl ResetCredits {
    pub fn summary(&self, now: i64) -> String {
        let expires = self.next_expires_at_ms.map(|at| format_duration(at - now));
        if self.available_count == 0 {
            return "No reset credits banked".into();
        }
        let noun = if self.available_count == 1 {
            "reset credit"
        } else {
            "reset credits"
        };
        match expires {
            Some(expires) => {
                format!(
                    "{} {noun} banked · next expires in {expires}",
                    self.available_count
                )
            }
            None => format!("{} {noun} banked", self.available_count),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LimitAccount {
    pub key: String,
    /// The login's hash, when the CLI could name the login.
    pub identity: Option<String>,
    pub provider: String,
    pub plan: Option<String>,
    pub environments: Vec<String>,
    /// Index of the host that can redeem this account's reset credit.
    pub environment: usize,
    pub checked_at_ms: i64,
    pub windows: Vec<LimitWindow>,
    pub reset_credits: Option<ResetCredits>,
    pub unavailable_reason: Option<String>,
    pub unavailable_message: Option<String>,
}

/// Parses one host's `usage-limits` answer.
pub fn parse_limit_accounts(value: &Value, environment: usize, label: &str) -> Vec<LimitAccount> {
    value["accounts"]
        .as_array()
        .map(|accounts| {
            accounts
                .iter()
                .map(|account| {
                    let provider = text(&account["provider"]);
                    LimitAccount {
                        // Without an identity, each host's login stands alone.
                        key: account["account_key"]
                            .as_str()
                            .map_or_else(|| format!("{environment}:{provider}"), str::to_owned),
                        identity: account["account_key"].as_str().map(str::to_owned),
                        provider,
                        plan: account["plan"].as_str().map(str::to_owned),
                        environments: vec![label.to_owned()],
                        environment,
                        checked_at_ms: account["checked_at_ms"].as_i64().unwrap_or(0),
                        windows: account["windows"]
                            .as_array()
                            .map(|windows| {
                                windows
                                    .iter()
                                    .map(|window| LimitWindow {
                                        id: text(&window["id"]),
                                        kind: text(&window["kind"]),
                                        label: text(&window["label"]),
                                        used_percent: number(&window["used_percent"]),
                                        resets_at_ms: window["resets_at_ms"].as_i64(),
                                        resets_at_label: window["resets_at_label"]
                                            .as_str()
                                            .map(str::to_owned),
                                        window_duration_mins: window["window_duration_mins"]
                                            .as_i64(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        reset_credits: account["reset_credits"].as_object().map(|credits| {
                            ResetCredits {
                                available_count: credits.get("available_count").map_or(0, count),
                                next_expires_at_ms: credits
                                    .get("next_expires_at_ms")
                                    .and_then(Value::as_i64),
                            }
                        }),
                        unavailable_reason: account["unavailable"]["reason"]
                            .as_str()
                            .map(str::to_owned),
                        unavailable_message: account["unavailable"]["message"]
                            .as_str()
                            .map(str::to_owned),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One login reported by two hosts is one account, signed in on both.
/// The newest report wins, so a host that failed to refresh cannot bring back
/// spent windows or a credit that is gone.
pub fn collect_accounts(reports: impl Iterator<Item = LimitAccount>) -> Vec<LimitAccount> {
    let mut accounts: Vec<LimitAccount> = Vec::new();
    for report in reports {
        let Some(held) = accounts
            .iter_mut()
            .find(|held| held.key == report.key && held.provider == report.provider)
        else {
            accounts.push(report);
            continue;
        };
        let mut environments = std::mem::take(&mut held.environments);
        environments.extend(report.environments.iter().cloned());
        if report.checked_at_ms > held.checked_at_ms {
            *held = report;
        }
        held.environments = environments;
    }
    accounts
}

pub fn limits_notice(account: &LimitAccount) -> Option<String> {
    match account.unavailable_reason.as_deref() {
        Some("unsupported") => Some(
            account
                .unavailable_message
                .clone()
                .unwrap_or_else(|| "This account has no subscription limits.".into()),
        ),
        Some("probeFailed") => Some(
            account
                .unavailable_message
                .clone()
                .unwrap_or_else(|| "Could not read limits.".into()),
        ),
        _ => account
            .windows
            .is_empty()
            .then(|| "No limits reported.".into()),
    }
}

/// Failed reads, but not accounts that simply have no subscription limits.
pub fn collect_notices(accounts: &[LimitAccount], many_environments: bool) -> Vec<String> {
    accounts
        .iter()
        .filter(|account| account.unavailable_reason.as_deref() != Some("unsupported"))
        .filter_map(|account| {
            let notice = limits_notice(account)?;
            let name = provider_label(&account.provider);
            Some(match account.environments.first() {
                Some(environment) if many_environments => {
                    format!("{environment} · {name}: {notice}")
                }
                _ => format!("{name}: {notice}"),
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct PoolMember {
    pub account: String,
    pub window: LimitWindow,
    pub refill_percent: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PoolReset {
    pub account: String,
    pub at_ms: i64,
    pub restores_percent: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PoolWindow {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub members: Vec<PoolMember>,
    pub remaining_percent: i64,
    pub pace: Option<Pace>,
    pub resets: Vec<PoolReset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LimitPool {
    pub provider: String,
    pub accounts: Vec<LimitAccount>,
    pub windows: Vec<PoolWindow>,
}

fn kind_order(kind: &str) -> u8 {
    match kind {
        "session" => 0,
        "weekly" => 1,
        "monthly" => 2,
        _ => 3,
    }
}

/// Codex ids name slots, so pool by kind; Claude ids identify distinct allowances.
fn pool_key(provider: &str, window: &LimitWindow) -> String {
    if provider == "codex" {
        window.kind.clone()
    } else {
        format!("{}:{}", window.kind, window.id)
    }
}

/// A spent weekly limit blocks the session window too, so the session bar
/// reports empty and refills on the weekly clock rather than its own.
fn pool_member(account: &LimitAccount, reported: &LimitWindow, now: i64) -> PoolMember {
    let spent_weekly = (reported.kind == "session")
        .then(|| {
            account.windows.iter().find(|window| {
                window.used_percent >= 100.0
                    && if account.provider == "claude" {
                        window.id == "seven_day"
                    } else {
                        account.provider == "codex" && window.kind == "weekly"
                    }
            })
        })
        .flatten();
    let Some(weekly) = spent_weekly else {
        return PoolMember {
            account: account.key.clone(),
            window: reported.clone(),
            refill_percent: reported.used_percent,
        };
    };
    let cutoff = now.max(account.checked_at_ms);
    let mut resets = (None, None);
    let mut refill_percent = 0.0;
    // A stale or missing clock cannot say when blocked quota becomes usable.
    if let (Some(weekly_reset), Some(session_reset)) = (weekly.resets_at_ms, reported.resets_at_ms)
        && weekly_reset > cutoff
        && session_reset > cutoff
    {
        if session_reset <= weekly_reset {
            resets = (weekly.resets_at_ms, weekly.resets_at_label.clone());
            refill_percent = 100.0;
        } else if reported.used_percent >= 100.0 {
            resets = (reported.resets_at_ms, reported.resets_at_label.clone());
            refill_percent = 100.0;
        } else {
            resets = (weekly.resets_at_ms, weekly.resets_at_label.clone());
            refill_percent = 100.0 - reported.used_percent.clamp(0.0, 100.0);
        }
    }
    PoolMember {
        account: account.key.clone(),
        window: LimitWindow {
            used_percent: 100.0,
            resets_at_ms: resets.0,
            resets_at_label: resets.1,
            // Suppression is not session spending, so it has no session pace clock.
            window_duration_mins: None,
            ..reported.clone()
        },
        refill_percent,
    }
}

fn pool_windows(accounts: &[LimitAccount], now: i64) -> Vec<PoolWindow> {
    let mut keyed: Vec<(String, Vec<PoolMember>)> = Vec::new();
    for account in accounts {
        for reported in &account.windows {
            let member = pool_member(account, reported, now);
            let pooled = pool_key(&account.provider, &member.window);
            let taken = keyed.iter().any(|(key, members)| {
                *key == pooled && members.iter().any(|held| held.account == account.key)
            });
            let key = if taken {
                format!("{}:{}", member.window.kind, member.window.id)
            } else {
                pooled
            };
            match keyed.iter_mut().find(|(held, _)| *held == key) {
                Some((_, members)) => members.push(member),
                None => keyed.push((key, vec![member])),
            }
        }
    }
    let mut pools: Vec<PoolWindow> = keyed
        .into_iter()
        .map(|(key, members)| {
            let first = members[0].window.clone();
            let count = members.len() as f64;
            let used = members
                .iter()
                .map(|member| member.window.used_percent)
                .sum::<f64>()
                / count;
            // Judge pace only for windows with reset clocks.
            let timed: Vec<(f64, f64)> = members
                .iter()
                .filter_map(|member| {
                    member
                        .window
                        .elapsed_share(now)
                        .map(|share| (member.window.used_percent, share))
                })
                .collect();
            let pace = (!timed.is_empty()).then(|| {
                let spread = timed.len() as f64;
                pace_of_shares(
                    timed.iter().map(|(used, _)| used).sum::<f64>() / spread,
                    timed.iter().map(|(_, elapsed)| elapsed).sum::<f64>() / spread,
                )
            });
            let mut resets: Vec<PoolReset> = members
                .iter()
                .filter_map(|member| {
                    Some(PoolReset {
                        account: member.account.clone(),
                        at_ms: member.window.resets_at_ms?,
                        restores_percent: (member.refill_percent / count).round() as i64,
                    })
                })
                .collect();
            resets.sort_by_key(|reset| reset.at_ms);
            PoolWindow {
                id: key,
                kind: first.kind,
                label: first.label,
                members,
                remaining_percent: (100.0 - used).round() as i64,
                pace,
                resets,
            }
        })
        .collect();
    pools.sort_by_key(|pool| kind_order(&pool.kind));
    pools
}

/// Groups accounts by provider; each window shows one segment per account.
pub fn collect_pools(accounts: &[LimitAccount], now: i64) -> Vec<LimitPool> {
    let mut providers: Vec<&str> = Vec::new();
    for account in accounts {
        if !account.windows.is_empty() && !providers.contains(&account.provider.as_str()) {
            providers.push(&account.provider);
        }
    }
    providers
        .into_iter()
        .map(|provider| {
            let mut members: Vec<LimitAccount> = accounts
                .iter()
                .filter(|account| account.provider == provider && !account.windows.is_empty())
                .cloned()
                .collect();
            let first_kind = members
                .iter()
                .flat_map(|account| account.windows.iter())
                .min_by_key(|window| kind_order(&window.kind))
                .cloned();
            let order = |account: &LimitAccount| {
                first_kind
                    .as_ref()
                    .and_then(|first| {
                        account
                            .windows
                            .iter()
                            .find(|window| pool_key(provider, window) == pool_key(provider, first))
                    })
                    .and_then(|window| pool_member(account, window, now).window.resets_at_ms)
                    .unwrap_or(i64::MAX)
            };
            members.sort_by(|left, right| {
                order(left)
                    .cmp(&order(right))
                    .then_with(|| left.key.cmp(&right.key))
            });
            let windows = pool_windows(&members, now);
            LimitPool {
                provider: provider.to_owned(),
                accounts: members,
                windows,
            }
        })
        .collect()
}

/// A price cell: blank is `Some(None)`, a valid rate `Some(Some(_))`, anything else `None`.
pub fn parse_price_cell(raw: &str) -> Option<Option<f64>> {
    let raw = raw.trim().replacen(',', ".", 1);
    if raw.is_empty() {
        return Some(None);
    }
    let (mantissa, exponent) = match raw.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (raw.as_str(), None),
    };
    let digits = mantissa.chars().filter(char::is_ascii_digit).count();
    let dots = mantissa
        .chars()
        .filter(|character| *character == '.')
        .count();
    if digits == 0
        || dots > 1
        || !mantissa
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
    {
        return None;
    }
    if let Some(exponent) = exponent {
        let body = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if body.is_empty() || !body.chars().all(|character| character.is_ascii_digit()) {
            return None;
        }
    }
    raw.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summary() -> Summary {
        Summary::parse(&json!({
            "resolution": "day",
            "window_label": "Oct 9 to Nov 7",
            "periods": [
                {"key":"2026-11-06","label":"Nov 6","tooltip":"Nov 6"},
                {"key":"2026-11-07","label":"Nov 7","tooltip":"Nov 7"},
            ],
            "sessions": {"claude": 2, "codex": 1},
            "sources": [
                {"provider":"claude","status":"ok","fingerprint":{"home":"a"},"malformed_records":0},
                {"provider":"codex","status":"ok","fingerprint":{"home":"b"},"malformed_records":3,"message":null},
            ],
            "buckets": [
                {"day":"2026-11-06","provider":"claude","model":"opus","cost_usd":2.0,"cache_savings_usd":1.0,
                 "totals":{"uncached_input":100,"cached_input":300,"cache_creation":100,"output":500},
                 "category_cost_usd":{"input":0.5,"cache_read":0.25,"cache_write":0.25,"output":0.5},
                 "fast_cost_usd":1.0,"speed_premium_usd":0.5,"cost_source":"modelPriced","records":4,"unpriced_records":1},
                {"day":"2026-11-07","provider":"codex","model":"gpt","cost_usd":6.0,"cache_savings_usd":0.0,
                 "totals":{"uncached_input":1000,"output":1000,"reasoning":100},
                 "cost_source":"modelPriced","records":2,"unpriced_records":0},
                {"day":"2026-11-07","provider":"claude","model":"local","cost_usd":0.0,"cache_savings_usd":0.0,
                 "totals":{"uncached_input":10},"cost_source":"unpriced","records":2,"unpriced_records":2},
            ],
        }))
        .unwrap()
    }

    #[test]
    fn merge_totals_providers_models_and_shares() {
        let summary = summary();
        let merged = merge(&[("Local", &summary)], &[], |_| true);
        assert_eq!(merged.cost_usd, 8.0);
        assert_eq!(merged.total_tokens, 3010);
        assert_eq!(merged.tokens.reasoning, 100);
        assert_eq!(merged.sessions, 3);
        assert_eq!(merged.providers[0].provider, "codex");
        assert_eq!(merged.providers[0].cost_share, 0.75);
        assert_eq!(merged.models[0].model, "gpt");
        assert_eq!(merged.unpriced_share, 3.0 / 8.0);
        assert_eq!(merged.cache_savings_usd, 1.0);
        assert_eq!(merged.category_cost.output, 0.5);
        // Cost without a split is reported rather than dropped.
        assert_eq!(merged.category_cost.unsplit, 6.5);
        assert_eq!(merged.speed_cost.fast, 1.0);
        assert_eq!(merged.speed_cost.standard, 7.0);
        assert_eq!(merged.speed_cost.premium, 0.5);
        assert_eq!(merged.periods.len(), 2);
        assert_eq!(merged.periods[1].by_provider["codex"], (6.0, 2000));
        let opus = merged
            .models
            .iter()
            .find(|model| model.model == "opus")
            .unwrap();
        assert_eq!(opus.unpriced_tokens, 250.0);
        assert_eq!(opus.cache_hit_rate(), Some(0.6));
        assert!((opus.cost_per_million().unwrap() - 2.0 / 750.0 * 1e6).abs() < 1e-6);
        let local = merged
            .models
            .iter()
            .find(|model| model.model == "local")
            .unwrap();
        assert!(local.cost_unknown());
        assert_eq!(local.share(ChartMetric::Cost), None);
        assert_eq!(local.cost_per_million(), None);
        assert!(sort_models_by_tokens(&merged.models)[0].model == "gpt");
    }

    #[test]
    fn hidden_providers_and_filters_leave_the_totals() {
        let summary = summary();
        let merged = merge(&[("Local", &summary)], &["codex".to_owned()], |_| true);
        assert_eq!(merged.cost_usd, 2.0);
        assert_eq!(merged.sessions, 2);
        assert!(
            merged
                .providers
                .iter()
                .all(|totals| totals.provider != "codex")
        );
        let model = merge(&[("Local", &summary)], &[], |bucket| bucket.model == "gpt");
        assert_eq!(model.total_tokens, 2000);
        assert_eq!(model.models.len(), 1);
    }

    #[test]
    fn hosts_sharing_a_transcript_directory_count_once() {
        let first = summary();
        let merged = merge(&[("Local", &first), ("Remote", &first)], &[], |_| true);
        assert_eq!(merged.cost_usd, 8.0);
        assert_eq!(merged.sessions, 3);
        assert_eq!(
            merged.duplicate_sources,
            ["Remote Claude Code", "Remote Codex"]
        );
        let mut other = summary();
        other.sources[0].fingerprint = "elsewhere".into();
        let merged = merge(&[("Local", &first), ("Remote", &other)], &[], |_| true);
        assert_eq!(merged.cost_usd, 10.0);
        assert_eq!(
            merged
                .providers
                .iter()
                .find(|p| p.provider == "claude")
                .unwrap()
                .sessions,
            4
        );
    }

    #[test]
    fn providers_rank_by_the_shown_metric_and_loading_keeps_their_row() {
        let summary = summary();
        let merged = merge(&[("Local", &summary)], &[], |_| true);
        assert_eq!(
            rank_providers(&merged, ChartMetric::Cost, &[]),
            ["codex", "claude"]
        );
        let quiet = merge(&[("Local", &summary)], &[], |bucket| {
            bucket.provider == "claude"
        });
        assert_eq!(rank_providers(&quiet, ChartMetric::Tokens, &[]), ["claude"]);
        assert_eq!(
            rank_providers(&quiet, ChartMetric::Tokens, &["codex"]),
            ["claude", "codex"]
        );
    }

    #[test]
    fn messages_report_read_problems_only() {
        let sources = [
            Source {
                provider: "claude".into(),
                fingerprint: "a".into(),
                status: "partial".into(),
                malformed_records: 2,
                message: Some(
                    "1 transcript path(s) could not be read; usage may be incomplete.".into(),
                ),
            },
            Source {
                provider: "codex".into(),
                fingerprint: "b".into(),
                status: "missing".into(),
                malformed_records: 0,
                message: Some("No transcript directory on this environment.".into()),
            },
        ];
        assert_eq!(
            source_messages(sources.iter()),
            [
                "1 transcript path(s) could not be read; usage may be incomplete.",
                "2 malformed or inconsistent usage record(s) were skipped; usage may be incomplete.",
            ]
        );
    }

    #[test]
    fn figures_format_like_the_page_expects() {
        assert_eq!(format_usd(1234.5), "$1,234.50");
        assert_eq!(format_usd(0.0), "$0.00");
        assert_eq!(format_usd(-3.0), "-$3.00");
        assert_eq!(format_usd(1_000_000.0), "$1,000,000.00");
        assert_eq!(format_count(1234567), "1,234,567");
        assert_eq!(format_tokens(999.0), "999");
        assert_eq!(format_tokens(1000.0), "1K");
        assert_eq!(format_tokens(1500.0), "1.50K");
        assert_eq!(format_tokens(12_340_000.0), "12.3M");
        assert_eq!(format_tokens(250_000_000.0), "250M");
        assert_eq!(format_tokens(3e12), "3T");
        assert_eq!(format_percent(0.1234), "12.3%");
        assert_eq!(format_percent(0.0001), "<0.1%");
        assert_eq!(format_percent(0.0), "0.0%");
        assert_eq!(format_duration(90 * 60_000), "1h 30m");
        assert_eq!(format_duration(26 * 3_600_000), "1d 2h");
        assert_eq!(format_duration(-5), "0m");
    }

    #[test]
    fn chart_scale_is_round_and_curves_stay_within_their_points() {
        assert_eq!(nice_scale(0.0), (0.0, vec![0.0]));
        let (max, ticks) = nice_scale(87.0);
        assert_eq!(max, 100.0);
        assert_eq!(ticks, [0.0, 50.0, 100.0]);
        let (max, ticks) = nice_scale(3.2);
        assert_eq!(max, 4.0);
        assert_eq!(ticks.len(), 5);

        let points = [
            ChartPoint { x: 0.0, y: 260.0 },
            ChartPoint { x: 10.0, y: 8.0 },
            ChartPoint { x: 20.0, y: 260.0 },
            ChartPoint { x: 30.0, y: 260.0 },
        ];
        for segment in smooth_curve(&points) {
            for control in [segment.control_a, segment.control_b] {
                assert!((8.0..=260.0).contains(&control.y), "{control:?} overshoots");
            }
        }
        assert!(smooth_curve(&points[..1]).is_empty());
    }

    #[test]
    fn chart_columns_follow_periods_and_series_paint_largest_first() {
        let summary = summary();
        let merged = merge(&[("Local", &summary)], &[], |_| true);
        let providers = ["claude".to_owned(), "codex".to_owned()];
        let chart = build_chart(
            &summary.periods,
            &merged.periods,
            ChartMetric::Cost,
            &providers,
            false,
        );
        assert_eq!(chart.columns, [[2.0, 0.0], [0.0, 6.0]]);
        assert_eq!(chart.series[0].provider, "codex");
        assert_eq!(chart.step_x, VIEW_WIDTH);
        assert!(chart.labeled);
        let empty = build_chart(&summary.periods, &[], ChartMetric::Cost, &providers, true);
        assert!(!empty.labeled, "a loading chart has no scale to label");
        assert_eq!(empty.ticks.len(), 5);
        let lone = build_chart(
            &summary.periods[..1],
            &merged.periods,
            ChartMetric::Cost,
            &providers,
            false,
        );
        assert!(lone.series[0].point.is_some());
    }

    fn window(
        id: &str,
        kind: &str,
        used: f64,
        resets: Option<i64>,
        mins: Option<i64>,
    ) -> LimitWindow {
        LimitWindow {
            id: id.into(),
            kind: kind.into(),
            label: kind.into(),
            used_percent: used,
            resets_at_ms: resets,
            resets_at_label: resets.map(|_| "soon".to_owned()),
            window_duration_mins: mins,
        }
    }

    fn account(provider: &str, key: &str, windows: Vec<LimitWindow>) -> LimitAccount {
        LimitAccount {
            key: key.into(),
            identity: Some(key.into()),
            provider: provider.into(),
            plan: None,
            environments: vec!["Local".into()],
            environment: 0,
            checked_at_ms: 0,
            windows,
            reset_credits: None,
            unavailable_reason: None,
            unavailable_message: None,
        }
    }

    const HOUR: i64 = 3_600_000;

    #[test]
    fn window_pace_compares_spend_to_elapsed_time() {
        let now = 0;
        let half = window("a", "session", 50.0, Some(150 * 60_000), Some(300));
        assert_eq!(half.remaining_percent(), 50);
        assert_eq!(half.pace_remaining_percent(now), Some(50));
        assert_eq!(half.resets_in(now).as_deref(), Some("resets in 2h 30m"));
        let pools = collect_pools(&[account("codex", "k", vec![half])], now);
        assert_eq!(pools[0].windows[0].pace, Some(Pace::On));
        let fast = window("a", "session", 90.0, Some(150 * 60_000), Some(300));
        let pools = collect_pools(&[account("codex", "k", vec![fast])], now);
        assert_eq!(pools[0].windows[0].pace, Some(Pace::Ahead));
        let slow = window("a", "session", 10.0, Some(150 * 60_000), Some(300));
        let pools = collect_pools(&[account("codex", "k", vec![slow])], now);
        assert_eq!(pools[0].windows[0].pace, Some(Pace::Under));
        let untimed = window("a", "monthly", 10.0, None, None);
        assert_eq!(untimed.pace_remaining_percent(now), None);
        assert_eq!(untimed.resets_in(now), None);
        assert_eq!(
            window("a", "x", 1.0, Some(30_000), None)
                .resets_in(now)
                .as_deref(),
            Some("Resets momentarily")
        );
        assert_eq!(
            window("a", "x", 1.0, Some(0), None)
                .resets_in(now)
                .as_deref(),
            Some("resets now")
        );
    }

    #[test]
    fn a_spent_weekly_limit_empties_the_session_bar() {
        let now = 1000;
        let accounts = [account(
            "claude",
            "k",
            vec![
                window("five_hour", "session", 20.0, Some(5 * HOUR), Some(300)),
                window("seven_day", "weekly", 100.0, Some(48 * HOUR), Some(10080)),
            ],
        )];
        let pools = collect_pools(&accounts, now);
        let session = &pools[0].windows[0];
        assert_eq!(session.kind, "session");
        assert_eq!(session.remaining_percent, 0);
        assert_eq!(session.pace, None, "suppression has no session clock");
        // The session clock lands first, so only the weekly reset frees it, entirely.
        assert_eq!(session.resets[0].at_ms, 48 * HOUR);
        assert_eq!(session.resets[0].restores_percent, 100);
        let later_session = [account(
            "claude",
            "k",
            vec![
                window("five_hour", "session", 20.0, Some(5 * HOUR), Some(300)),
                window("seven_day", "weekly", 100.0, Some(HOUR), Some(10080)),
            ],
        )];
        let session = &collect_pools(&later_session, now)[0].windows[0];
        // The weekly reset comes first and leaves the session's own spend in place.
        assert_eq!(session.resets[0].at_ms, HOUR);
        assert_eq!(session.resets[0].restores_percent, 80);
    }

    #[test]
    fn pools_order_kinds_and_split_accounts_into_segments() {
        let accounts = [
            account(
                "codex",
                "b",
                vec![window("secondary", "weekly", 40.0, Some(9), Some(10080))],
            ),
            account(
                "codex",
                "a",
                vec![
                    window("primary", "weekly", 80.0, Some(5), Some(10080)),
                    window("primary", "session", 10.0, Some(5), Some(300)),
                ],
            ),
            account("claude", "c", vec![]),
        ];
        let pools = collect_pools(&accounts, 0);
        assert_eq!(pools.len(), 1, "an account with no windows draws no pool");
        assert_eq!(pools[0].windows[0].kind, "session");
        let weekly = &pools[0].windows[1];
        // Codex slots pool by kind, whichever position reported them.
        assert_eq!(weekly.members.len(), 2);
        assert_eq!(weekly.remaining_percent, 40);
        assert_eq!(
            pools[0].accounts[0].key, "a",
            "the soonest reset comes first"
        );
    }

    #[test]
    fn claude_allowances_stay_separate_and_notices_skip_unsupported() {
        let accounts = [account(
            "claude",
            "k",
            vec![
                window("seven_day", "weekly", 10.0, None, None),
                window("seven_day_fable", "weekly", 20.0, None, None),
            ],
        )];
        assert_eq!(collect_pools(&accounts, 0)[0].windows.len(), 2);
        let mut unsupported = account("claude", "k", vec![]);
        unsupported.unavailable_reason = Some("unsupported".into());
        let mut failed = account("codex", "j", vec![]);
        failed.unavailable_reason = Some("probeFailed".into());
        failed.unavailable_message = Some("Codex did not answer the usage request.".into());
        let empty = account("codex", "i", vec![]);
        assert_eq!(
            collect_notices(&[unsupported, failed, empty], false),
            [
                "Codex: Codex did not answer the usage request.",
                "Codex: No limits reported."
            ]
        );
    }

    #[test]
    fn one_login_on_two_hosts_is_one_account() {
        let answer = json!({"accounts":[{
            "provider":"codex", "account_key":"same", "plan":"Pro", "checked_at_ms":5,
            "windows":[{"id":"primary","kind":"weekly","label":"Weekly","used_percent":93.0,
                "resets_at_ms":10,"resets_at_label":"soon","window_duration_mins":10080}],
            "reset_credits":{"available_count":1,"next_expires_at_ms":3_600_005},
        }]});
        let local = parse_limit_accounts(&answer, 0, "Local");
        let remote = parse_limit_accounts(&answer, 1, "Remote");
        let accounts = collect_accounts(local.into_iter().chain(remote));
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].environments, ["Local", "Remote"]);
        let credits = accounts[0].reset_credits.as_ref().unwrap();
        assert_eq!(
            credits.summary(5),
            "1 reset credit banked · next expires in 1h 0m"
        );
        assert_eq!(
            ResetCredits {
                available_count: 0,
                next_expires_at_ms: None
            }
            .summary(0),
            "No reset credits banked"
        );
        let anonymous = json!({"accounts":[{"provider":"claude","windows":[]}]});
        assert_ne!(
            parse_limit_accounts(&anonymous, 0, "Local")[0].key,
            parse_limit_accounts(&anonymous, 1, "Remote")[0].key
        );
    }

    #[test]
    fn the_newest_report_of_a_login_supplies_its_figures() {
        let mut stale = account(
            "codex",
            "same",
            vec![window("a", "weekly", 90.0, None, None)],
        );
        stale.checked_at_ms = 10;
        let mut fresh = account(
            "codex",
            "same",
            vec![window("a", "weekly", 5.0, None, None)],
        );
        fresh.checked_at_ms = 20;
        fresh.environment = 1;
        fresh.environments = vec!["Remote".into()];
        let accounts = collect_accounts([stale.clone(), fresh.clone()].into_iter());
        assert_eq!(accounts[0].windows[0].used_percent, 5.0);
        assert_eq!(accounts[0].environment, 1, "the fresh host redeems");
        assert_eq!(accounts[0].environments, ["Local", "Remote"]);
        let accounts = collect_accounts([fresh, stale].into_iter());
        assert_eq!(accounts[0].windows[0].used_percent, 5.0);
        assert_eq!(accounts[0].environments, ["Remote", "Local"]);
    }

    #[test]
    fn price_cells_accept_decimals_and_reject_junk() {
        assert_eq!(parse_price_cell(""), Some(None));
        assert_eq!(parse_price_cell("  "), Some(None));
        assert_eq!(parse_price_cell("1.5"), Some(Some(1.5)));
        assert_eq!(parse_price_cell("1,5"), Some(Some(1.5)));
        assert_eq!(parse_price_cell(".5"), Some(Some(0.5)));
        assert_eq!(parse_price_cell("3."), Some(Some(3.0)));
        assert_eq!(parse_price_cell("0"), Some(Some(0.0)));
        assert_eq!(parse_price_cell("1e-3"), Some(Some(0.001)));
        for junk in ["-1", "abc", "1.2.3", ".", "1e", "e5", "NaN", "inf", "1e999"] {
            assert_eq!(parse_price_cell(junk), None, "{junk}");
        }
    }
}
