//! Token, cost, and subscription limit reporting for the Usage page.
//!
//! Reads what the installed assistants already record: Claude Code and Codex
//! transcripts for tokens and cost, and each CLI's own account probe for
//! subscription limits. Parsing, pricing, and limit rules are ported from
//! T3 Code (MIT, see `data/licenses/t3code-LICENSE`).

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Datelike, Days, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    EventBus,
    accounts::{Accounts, Headroom, Identity, PROVIDERS},
    model_catalog::Probe,
};

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;
const MINUTE_MS: i64 = 60_000;
const LITELLM_RATES_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const LITELLM_RATES_API_URL: &str = "https://api.github.com/repos/BerriAI/litellm/contents/model_prices_and_context_window.json?ref=main";
const RATES_TTL_MS: i64 = DAY_MS;
const RATES_REFRESH_FLOOR_MS: i64 = MINUTE_MS;
const RATES_DOCUMENT_LIMIT: u64 = 64 * 1024 * 1024;
const RATES_CACHE_FILE: &str = "usage-rates.json";
const PRICES_FILE: &str = "usage-prices.json";
/// Transcripts embed whole tool payloads. A line past this cannot be a usage
/// record worth the memory, so it is skipped without being buffered.
const LINE_LIMIT: usize = 64 * 1024 * 1024;
/// The longest window is 90 days. Anything older cannot be shown, so a
/// long-running daemon neither reads nor keeps it.
const RETENTION_MS: i64 = 92 * DAY_MS;
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const SESSION_MINS: i64 = 5 * 60;
const WEEK_MINS: i64 = 7 * 24 * 60;
const MONTH_MINS: i64 = 30 * 24 * 60;
const CLAUDE_SYNTHETIC_MODEL: &str = "<synthetic>";
/// Forks copy and restamp parent history in a burst; genuine child usage follows a model turn.
const FORK_COPY_MAX_GAP_MS: i64 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Totals {
    uncached_input: u64,
    cached_input: u64,
    cache_creation: u64,
    output: u64,
    /// A subset of `output`, surfaced separately and never added again.
    reasoning: u64,
}

impl Totals {
    fn add(&mut self, other: &Totals) {
        self.uncached_input = self.uncached_input.saturating_add(other.uncached_input);
        self.cached_input = self.cached_input.saturating_add(other.cached_input);
        self.cache_creation = self.cache_creation.saturating_add(other.cache_creation);
        self.output = self.output.saturating_add(other.output);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Speed {
    Standard,
    Fast,
    Ultrafast,
}

#[derive(Clone, Debug, PartialEq)]
struct UsageRecord {
    provider: &'static str,
    timestamp_ms: i64,
    model: Arc<str>,
    session_id: Arc<str>,
    totals: Totals,
    reported_cost_usd: Option<f64>,
    speed: Speed,
    dedupe_key: Option<Arc<str>>,
}

fn timestamp_ms(value: &Value) -> Option<i64> {
    let text = value.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.timestamp_millis())
}

fn token_count(value: &Value) -> u64 {
    match value.as_f64() {
        Some(number) if number.is_finite() && number > 0.0 => {
            number.min(MAX_TOKEN_COUNT as f64).trunc() as u64
        }
        _ => 0,
    }
}

/// Claude writes one record per assistant content block, and each repeats the
/// message's complete `usage`. The caller must keep the first per `dedupe_key`.
fn parse_claude_record(record: &Value) -> Vec<UsageRecord> {
    if record["type"] != "assistant" {
        return Vec::new();
    }
    let message = &record["message"];
    let usage = &message["usage"];
    if !usage.is_object() {
        return Vec::new();
    }
    let Some(timestamp) = timestamp_ms(&record["timestamp"]) else {
        return Vec::new();
    };
    let model = message["model"].as_str().unwrap_or_default();
    if model.is_empty() || model == CLAUDE_SYNTHETIC_MODEL {
        return Vec::new();
    }
    let message_id = message["id"].as_str();
    let request_id = record["requestId"].as_str();
    let dedupe_key = (message_id.is_some() || request_id.is_some()).then(|| {
        format!(
            "{}:{}",
            message_id.unwrap_or_default(),
            request_id.unwrap_or_default()
        )
    });
    let session_id: Arc<str> = record["sessionId"].as_str().unwrap_or_default().into();
    let speed = if usage["speed"] == "fast" {
        Speed::Fast
    } else {
        Speed::Standard
    };
    let claude_totals = |usage: &Value| Totals {
        uncached_input: token_count(&usage["input_tokens"]),
        cached_input: token_count(&usage["cache_read_input_tokens"]),
        cache_creation: token_count(&usage["cache_creation_input_tokens"]),
        output: token_count(&usage["output_tokens"]),
        reasoning: 0,
    };
    let mut records = vec![UsageRecord {
        provider: "claude",
        timestamp_ms: timestamp,
        model: model.into(),
        session_id: session_id.clone(),
        totals: claude_totals(usage),
        reported_cost_usd: record["costUSD"].as_f64().filter(|cost| cost.is_finite()),
        speed,
        dedupe_key: dedupe_key.as_deref().map(Arc::from),
    }];
    // Top-level `usage` is the executor's alone. Advisor calls run on another
    // model at its own rates and appear only in `iterations`.
    if let Some(iterations) = usage["iterations"].as_array() {
        for (index, advisor) in iterations.iter().enumerate() {
            if advisor["type"] != "advisor_message" {
                continue;
            }
            let advisor_model = advisor["model"].as_str().unwrap_or_default();
            if advisor_model.is_empty() || advisor_model == CLAUDE_SYNTHETIC_MODEL {
                continue;
            }
            records.push(UsageRecord {
                provider: "claude",
                timestamp_ms: timestamp,
                model: advisor_model.into(),
                session_id: session_id.clone(),
                totals: claude_totals(advisor),
                reported_cost_usd: None,
                speed,
                dedupe_key: dedupe_key
                    .as_ref()
                    .map(|key| Arc::from(format!("{key}:advisor:{index}"))),
            });
        }
    }
    records
}

/// `[input, cached_input, cache_write_input, output, reasoning_output, total]`.
type CodexUsage = [i64; 6];
/// The largest counter accepted, as upstream's JSON numbers allow. Far past any
/// real count, and small enough that sums of these cannot overflow.
const MAX_TOKEN_COUNT: i64 = (1 << 53) - 1;
const CODEX_INPUT: usize = 0;
const CODEX_CACHED: usize = 1;
const CODEX_CACHE_WRITE: usize = 2;
const CODEX_OUTPUT: usize = 3;
const CODEX_REASONING: usize = 4;
const CODEX_TOTAL: usize = 5;

fn codex_integer(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return (0..=MAX_TOKEN_COUNT).contains(&number).then_some(number);
    }
    value
        .as_f64()
        .filter(|number| {
            number.fract() == 0.0 && *number >= 0.0 && *number <= MAX_TOKEN_COUNT as f64
        })
        .map(|number| number as i64)
}

fn valid_codex_usage(usage: CodexUsage) -> Option<CodexUsage> {
    (usage
        .iter()
        .all(|field| (0..=MAX_TOKEN_COUNT).contains(field))
        && usage[CODEX_CACHED] + usage[CODEX_CACHE_WRITE] <= usage[CODEX_INPUT]
        && usage[CODEX_REASONING] <= usage[CODEX_OUTPUT]
        && usage[CODEX_TOTAL] >= usage[CODEX_INPUT] + usage[CODEX_OUTPUT])
        .then_some(usage)
}

fn read_codex_usage(value: &Value) -> Option<CodexUsage> {
    let object = value.as_object()?;
    let input = codex_integer(object.get("input_tokens")?)?;
    let output = codex_integer(object.get("output_tokens")?)?;
    // An absent field takes its default; a present field must be valid.
    let optional = |name: &str, default: i64| match object.get(name) {
        Some(value) => codex_integer(value),
        None => Some(default),
    };
    valid_codex_usage([
        input,
        optional("cached_input_tokens", 0)?,
        optional("cache_write_input_tokens", 0)?,
        output,
        optional("reasoning_output_tokens", 0)?,
        optional("total_tokens", input + output)?,
    ])
}

fn combine_codex_usage(
    left: CodexUsage,
    right: CodexUsage,
    combine: fn(i64, i64) -> i64,
) -> Option<CodexUsage> {
    let mut result = [0; 6];
    for index in 0..6 {
        result[index] = combine(left[index], right[index]);
    }
    valid_codex_usage(result)
}

#[derive(Clone, Debug)]
struct CodexAttribution {
    timestamp_ms: i64,
    model: Arc<str>,
    speed: Speed,
}

#[derive(Clone, Debug)]
struct CodexUnreported {
    attribution: CodexAttribution,
    usage: CodexUsage,
}

/// Codex `token_count` events carry no model or service tier, so both carry
/// forward from the latest `turn_context` and `thread_settings_applied`.
#[derive(Clone, Debug)]
struct CodexScanState {
    model: Arc<str>,
    speed: Speed,
    session_id: Arc<str>,
    last_signature: Option<CodexUsage>,
    last_cumulative: Option<CodexUsage>,
    sequence_start: Option<(i64, CodexUsage)>,
    unreported: Option<Vec<CodexUnreported>>,
    malformed: u64,
    saw_session_meta: bool,
    suppressing_fork_copies: bool,
    fork_copy_anchor_ms: i64,
}

impl Default for CodexScanState {
    fn default() -> Self {
        Self {
            model: "".into(),
            speed: Speed::Standard,
            session_id: "".into(),
            last_signature: None,
            last_cumulative: None,
            sequence_start: None,
            unreported: None,
            malformed: 0,
            saw_session_meta: false,
            suppressing_fork_copies: false,
            fork_copy_anchor_ms: 0,
        }
    }
}

impl CodexScanState {
    /// A zeroed baseline rebases the counters, so the sequence start measured
    /// against it is forgotten.
    fn rebase(&mut self, cumulative: CodexUsage) {
        self.last_cumulative = Some(cumulative);
        self.unreported = None;
        if cumulative[CODEX_INPUT] + cumulative[CODEX_OUTPUT] == 0 {
            self.sequence_start = None;
        }
    }

    /// The first usage line past the copied burst ends suppression for the
    /// rest of the rollout.
    fn is_fork_copy(&mut self, timestamp_ms: i64) -> bool {
        if !self.suppressing_fork_copies {
            return false;
        }
        if timestamp_ms - self.fork_copy_anchor_ms < FORK_COPY_MAX_GAP_MS {
            self.fork_copy_anchor_ms = timestamp_ms;
            return true;
        }
        self.suppressing_fork_copies = false;
        self.last_signature = None;
        false
    }

    fn attribution(&self, timestamp_ms: i64) -> CodexAttribution {
        CodexAttribution {
            timestamp_ms,
            model: self.model.clone(),
            speed: self.speed,
        }
    }

    fn record(&self, usage: CodexUsage, attribution: &CodexAttribution) -> UsageRecord {
        UsageRecord {
            provider: "codex",
            timestamp_ms: attribution.timestamp_ms,
            model: attribution.model.clone(),
            session_id: self.session_id.clone(),
            totals: Totals {
                // Codex reports `input_tokens` inclusive of the cached portion.
                uncached_input: (usage[CODEX_INPUT]
                    - usage[CODEX_CACHED]
                    - usage[CODEX_CACHE_WRITE]) as u64,
                cached_input: usage[CODEX_CACHED] as u64,
                cache_creation: usage[CODEX_CACHE_WRITE] as u64,
                output: usage[CODEX_OUTPUT] as u64,
                reasoning: usage[CODEX_REASONING] as u64,
            },
            reported_cost_usd: None,
            speed: attribution.speed,
            dedupe_key: None,
        }
    }
}

fn is_forked_session_meta(payload: &Value) -> bool {
    payload["forked_from_id"].is_string()
        || payload["source"]["subagent"]["thread_spawn"]["parent_thread_id"].is_string()
}

/// Missing, default, and standard tiers bill as standard; fast aliases priority.
fn codex_speed(tier: &Value) -> Speed {
    match tier.as_str() {
        Some("priority" | "fast") => Speed::Fast,
        Some("ultrafast") => Speed::Ultrafast,
        _ => Speed::Standard,
    }
}

/// An interrupted turn can end before a snapshot reports its last request.
/// Bill those requests only when the `token_usage_record`s since the previous
/// snapshot explain the whole gap; `last` is the final one.
fn codex_unreported_requests(
    previous: Option<CodexUsage>,
    cumulative: CodexUsage,
    last: CodexUsage,
    pending: Option<&[CodexUnreported]>,
) -> Option<Vec<CodexUnreported>> {
    let previous = previous?;
    let pending = pending?;
    for index in 0..6 {
        let explained: i128 = pending
            .iter()
            .map(|request| i128::from(request.usage[index]))
            .sum();
        if i128::from(cumulative[index] - previous[index]) != explained {
            return None;
        }
    }
    let (end, rest) = pending.split_last()?;
    let remainder = combine_codex_usage(end.usage, last, |left, right| left - right)?;
    let mut requests = rest.to_vec();
    requests.push(CodexUnreported {
        attribution: end.attribution.clone(),
        usage: remainder,
    });
    requests.retain(|request| request.usage[CODEX_INPUT] + request.usage[CODEX_OUTPUT] > 0);
    Some(requests)
}

/// Cumulative counters validate request deltas. Older events without them use
/// normalized consecutive-request deduplication.
fn parse_codex_record(record: &Value, state: &mut CodexScanState) -> Vec<UsageRecord> {
    let payload = &record["payload"];
    if !payload.is_object() {
        return Vec::new();
    }
    let kind = record["type"].as_str();
    if kind == Some("session_meta") {
        // Only the first meta identifies this session; copied ancestor metas
        // would misattribute later records.
        if state.saw_session_meta {
            return Vec::new();
        }
        state.saw_session_meta = true;
        if let Some(id) = payload["id"].as_str().or(payload["session_id"].as_str()) {
            state.session_id = id.into();
        }
        if let Some(timestamp) = timestamp_ms(&record["timestamp"])
            && is_forked_session_meta(payload)
        {
            state.suppressing_fork_copies = true;
            state.fork_copy_anchor_ms = timestamp;
        }
        return Vec::new();
    }
    if kind == Some("turn_context") {
        if let Some(model) = payload["model"].as_str() {
            state.model = model.into();
        }
        return Vec::new();
    }
    // Several may sit between two snapshots, so they only explain a counter gap.
    if kind == Some("token_usage_record") {
        let (Some(timestamp), Some(usage)) = (
            timestamp_ms(&record["timestamp"]),
            read_codex_usage(&payload["usage"]),
        ) else {
            return Vec::new();
        };
        if state.model.is_empty() || state.is_fork_copy(timestamp) {
            return Vec::new();
        }
        let attribution = state.attribution(timestamp);
        state
            .unreported
            .get_or_insert_default()
            .push(CodexUnreported { attribution, usage });
        return Vec::new();
    }
    let payload_kind = payload["type"].as_str();
    if payload_kind == Some("thread_settings_applied") {
        if payload["thread_settings"].is_object() {
            state.speed = codex_speed(&payload["thread_settings"]["service_tier"]);
        }
        return Vec::new();
    }
    if payload_kind != Some("token_count") || !payload["info"].is_object() {
        return Vec::new();
    }
    let info = &payload["info"];
    // Codex may re-emit these counters once the model or timestamp is known.
    let Some(timestamp) = timestamp_ms(&record["timestamp"]) else {
        return Vec::new();
    };
    if state.model.is_empty() {
        return Vec::new();
    }
    let last = read_codex_usage(&info["last_token_usage"]);
    let raw_cumulative = &info["total_token_usage"];
    let has_cumulative = !raw_cumulative.is_null();
    let cumulative = has_cumulative
        .then(|| read_codex_usage(raw_cumulative))
        .flatten();

    if state.is_fork_copy(timestamp) {
        // Copied history is already billed in the parent, but sets the child's baseline.
        if let Some(cumulative) = cumulative {
            state.rebase(cumulative);
        } else if !has_cumulative
            && let Some(last) = last
            && Some(last) != state.last_signature
        {
            state.last_cumulative = match state.last_cumulative {
                Some(previous) => combine_codex_usage(previous, last, |a, b| a + b),
                None => Some(last),
            };
        }
        state.last_signature = last;
        return Vec::new();
    }

    let previous = state.last_cumulative;
    let pending = state.unreported.take();
    // A valid snapshot remains the next baseline even when its request is malformed.
    if let Some(cumulative) = cumulative {
        state.rebase(cumulative);
    } else {
        state.unreported = pending.clone();
    }
    let Some(last) = last.filter(|_| !has_cumulative || cumulative.is_some()) else {
        state.malformed += 1;
        return Vec::new();
    };
    // Zero-component counters report context size, not a request.
    if last[CODEX_INPUT] + last[CODEX_OUTPUT] == 0 {
        state.last_signature = None;
        return Vec::new();
    }
    if let (Some(cumulative), Some(previous)) = (cumulative, previous)
        && cumulative == previous
    {
        return Vec::new();
    }
    if last[CODEX_TOTAL] != last[CODEX_INPUT] + last[CODEX_OUTPUT] {
        state.malformed += 1;
        return Vec::new();
    }

    let mut unreported = Vec::new();
    if let Some(cumulative) = cumulative {
        // A session resumed in a new Codex process restarts its counters here.
        let restarted = cumulative == last;
        let sequence_start = (timestamp, last);
        // Only the same event is a replay; a later request can restart identically.
        if restarted && state.sequence_start == Some(sequence_start) {
            state.last_cumulative = previous;
            return Vec::new();
        }
        // Forks and paginated continuations can inherit totals without copying a
        // snapshot. Bill only the latest request, and require a valid unseen prefix.
        let reconciled = match previous {
            None => combine_codex_usage(cumulative, last, |a, b| a - b).is_some(),
            Some(previous) => {
                (0..6).all(|index| cumulative[index] - previous[index] == last[index])
            }
        };
        if !restarted && !reconciled {
            let Some(bridged) =
                codex_unreported_requests(previous, cumulative, last, pending.as_deref())
            else {
                state.malformed += 1;
                return Vec::new();
            };
            unreported = bridged;
        }
        if restarted {
            state.sequence_start = Some(sequence_start);
        }
    } else {
        if Some(last) == state.last_signature {
            return Vec::new();
        }
        // Advance expected totals so a cumulative re-emission reads as stale.
        let advanced = match previous {
            Some(previous) => combine_codex_usage(previous, last, |a, b| a + b),
            None => Some(last),
        };
        let Some(advanced) = advanced else {
            state.malformed += 1;
            return Vec::new();
        };
        state.last_cumulative = Some(advanced);
    }
    state.last_signature = Some(last);

    let attribution = state.attribution(timestamp);
    let mut records: Vec<UsageRecord> = unreported
        .iter()
        .map(|request| state.record(request.usage, &request.attribution))
        .collect();
    records.push(state.record(last, &attribution));
    records
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TokenRates {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_creation: f64,
}

/// Omits LiteLLM's size, flex, and batch variants, which transcripts do not record.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ModelRate {
    standard: TokenRates,
    fast: Option<TokenRates>,
    ultrafast: Option<TokenRates>,
}

impl ModelRate {
    fn at(&self, speed: Speed) -> &TokenRates {
        match speed {
            Speed::Standard => &self.standard,
            Speed::Fast => self.fast.as_ref().unwrap_or(&self.standard),
            Speed::Ultrafast => self.ultrafast.as_ref().unwrap_or(&self.standard),
        }
    }
}

type RateTable = HashMap<String, ModelRate>;

/// Missing cache rates fall back to input pricing; Anthropic normally
/// discounts reads and charges extra for writes.
fn read_token_rates(
    entry: &Value,
    suffix: &str,
    standard: Option<&TokenRates>,
) -> Option<TokenRates> {
    let finite = |name: String| entry[name].as_f64().filter(|number| number.is_finite());
    let input = finite(format!("input_cost_per_token{suffix}"))?;
    let output = finite(format!("output_cost_per_token{suffix}"))?;
    let cache = |name: &str, base: Option<f64>| {
        finite(format!("{name}{suffix}")).unwrap_or_else(|| match (standard, base) {
            (Some(standard), Some(base)) if standard.input > 0.0 => base / standard.input * input,
            _ => input,
        })
    };
    Some(TokenRates {
        input,
        output,
        cache_read: cache(
            "cache_read_input_token_cost",
            standard.map(|rates| rates.cache_read),
        ),
        cache_creation: cache(
            "cache_creation_input_token_cost",
            standard.map(|rates| rates.cache_creation),
        ),
    })
}

fn normalize_rate_key(model: &str) -> String {
    model.trim().to_lowercase()
}

fn bare_model_name(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

/// Resellers often omit speed tiers, so a missing tier fills from the other
/// entry instead of conflicting.
fn merge_alias_rate(left: ModelRate, right: ModelRate) -> Option<ModelRate> {
    let fast = left.fast.or(right.fast);
    let ultrafast = left.ultrafast.or(right.ultrafast);
    (left.standard == right.standard
        && right.fast.is_none_or(|rate| Some(rate) == fast)
        && right.ultrafast.is_none_or(|rate| Some(rate) == ultrafast))
    .then_some(ModelRate {
        standard: left.standard,
        fast,
        ultrafast,
    })
}

/// LiteLLM prefixes for the makers' own APIs; their price beats a reseller's.
const FIRST_PARTY_PROVIDERS: [&str; 7] = [
    "anthropic",
    "deepseek",
    "gemini",
    "meta",
    "mistral",
    "openai",
    "xai",
];

/// Drops models missing input or output rates to avoid understating cost.
fn parse_rate_table(document: &Value) -> RateTable {
    let mut table = RateTable::new();
    let Some(document) = document.as_object() else {
        return table;
    };
    for (name, entry) in document {
        if !entry.is_object() {
            continue;
        }
        let Some(standard) = read_token_rates(entry, "", None) else {
            continue;
        };
        let key = normalize_rate_key(name);
        if key.is_empty() {
            continue;
        }
        let multiple = entry["provider_specific_entry"]["fast"]
            .as_f64()
            .filter(|fast| fast.is_finite() && *fast > 0.0);
        let fast = match multiple {
            Some(multiple) => Some(TokenRates {
                input: standard.input * multiple,
                output: standard.output * multiple,
                cache_read: standard.cache_read * multiple,
                cache_creation: standard.cache_creation * multiple,
            }),
            None => read_token_rates(entry, "_priority", Some(&standard)),
        };
        let ultrafast = read_token_rates(entry, "_ultrafast", Some(&standard));
        table.insert(
            key,
            ModelRate {
                standard,
                fast,
                ultrafast,
            },
        );
    }

    let mut candidates: BTreeMap<String, (Option<ModelRate>, bool)> = BTreeMap::new();
    let mut names: Vec<&String> = table.keys().collect();
    names.sort();
    for key in names {
        let alias = bare_model_name(key);
        if alias.is_empty() || alias == key || table.contains_key(alias) {
            continue;
        }
        let rate = table[key];
        let first_party = FIRST_PARTY_PROVIDERS.contains(&key.split('/').next().unwrap_or(key));
        match candidates.get(alias).copied() {
            Some((_, held_first_party)) if first_party && !held_first_party => {
                candidates.insert(alias.to_owned(), (Some(rate), true));
            }
            None => {
                candidates.insert(alias.to_owned(), (Some(rate), first_party));
            }
            Some((Some(held), held_first_party)) if first_party == held_first_party => {
                candidates.insert(
                    alias.to_owned(),
                    (merge_alias_rate(held, rate), first_party),
                );
            }
            _ => {}
        }
    }
    for (alias, (rate, _)) in candidates {
        if let Some(rate) = rate {
            table.insert(alias, rate);
        }
    }
    table
}

/// Synthetic messages are unbilled; bare family names are generation-ambiguous.
const UNPRICEABLE_MODELS: [&str; 6] = [
    "<synthetic>",
    "synthetic",
    "opus",
    "sonnet",
    "haiku",
    "fable",
];

fn lookup_rate<'a>(table: &'a RateTable, model: &str) -> Option<&'a ModelRate> {
    let key = normalize_rate_key(model);
    // Claude's bracketed context variants price at the base tier.
    let key = key.split('[').next().unwrap_or(&key);
    let bare = bare_model_name(key);
    if bare.is_empty() || UNPRICEABLE_MODELS.contains(&bare) {
        return None;
    }
    table.get(key)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CategoryCost {
    input: f64,
    cache_read: f64,
    cache_write: f64,
    output: f64,
}

impl CategoryCost {
    fn of(totals: &Totals, rates: &TokenRates) -> Self {
        Self {
            input: totals.uncached_input as f64 * rates.input,
            cache_read: totals.cached_input as f64 * rates.cache_read,
            cache_write: totals.cache_creation as f64 * rates.cache_creation,
            output: totals.output as f64 * rates.output,
        }
    }

    fn sum(&self) -> f64 {
        self.input + self.cache_read + self.cache_write + self.output
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CostSource {
    ProviderReported,
    ModelPriced,
    Unpriced,
}

#[derive(Debug, PartialEq)]
struct PricedUsage {
    cost_usd: f64,
    cost_source: CostSource,
    category: Option<CategoryCost>,
    speed_premium_usd: f64,
}

fn price_usage(table: &RateTable, overrides: &RateTable, record: &UsageRecord) -> PricedUsage {
    let override_rate = overrides.get(record.model.trim());
    let reported = record
        .reported_cost_usd
        .filter(|cost| override_rate.is_none() && cost.is_finite());
    let unsplit = |cost_usd, cost_source| PricedUsage {
        cost_usd,
        cost_source,
        category: None,
        speed_premium_usd: 0.0,
    };
    let Some(rate) = override_rate.or_else(|| lookup_rate(table, &record.model)) else {
        return match reported {
            Some(cost) => unsplit(cost, CostSource::ProviderReported),
            None => unsplit(0.0, CostSource::Unpriced),
        };
    };
    let list = CategoryCost::of(&record.totals, rate.at(record.speed));
    let list_usd = list.sum();
    if let Some(cost) = reported
        && list_usd <= 0.0
    {
        return unsplit(cost, CostSource::ProviderReported);
    }
    let premium = if record.speed == Speed::Standard {
        0.0
    } else {
        list_usd - CategoryCost::of(&record.totals, &rate.standard).sum()
    };
    let scale = reported.map_or(1.0, |cost| cost / list_usd);
    PricedUsage {
        cost_usd: reported.unwrap_or(list_usd),
        cost_source: if reported.is_some() {
            CostSource::ProviderReported
        } else {
            CostSource::ModelPriced
        },
        category: Some(CategoryCost {
            input: list.input * scale,
            cache_read: list.cache_read * scale,
            cache_write: list.cache_write * scale,
            output: list.output * scale,
        }),
        speed_premium_usd: premium * scale,
    }
}

fn cache_savings_usd(table: &RateTable, overrides: &RateTable, record: &UsageRecord) -> f64 {
    let Some(rate) = overrides
        .get(record.model.trim())
        .or_else(|| lookup_rate(table, &record.model))
    else {
        return 0.0;
    };
    let rates = rate.at(record.speed);
    record.totals.cached_input as f64 * (rates.input - rates.cache_read)
}

fn round_usd(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

/// User-set prices and "map to" aliases, keyed by model id.
#[derive(Clone, Debug, Default, PartialEq)]
struct PriceSettings {
    /// USD per million tokens: `[input, output, cache_read?, cache_write?]`.
    prices: BTreeMap<String, [Option<f64>; 4]>,
    aliases: BTreeMap<String, String>,
}

impl PriceSettings {
    fn from_value(value: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        if let Some(prices) = value.get("prices") {
            let prices = prices
                .as_object()
                .ok_or_else(|| "Model prices must be an object.".to_owned())?;
            for (model, price) in prices {
                let model = model.trim();
                if model.is_empty() {
                    return Err("A model price needs a model id.".into());
                }
                let rate = |name: &str| -> Result<Option<f64>, String> {
                    match price.get(name) {
                        None | Some(Value::Null) => Ok(None),
                        Some(rate) => rate
                            .as_f64()
                            .filter(|rate| rate.is_finite() && *rate >= 0.0)
                            .map(Some)
                            .ok_or_else(|| format!("{model} has an invalid {name} price.")),
                    }
                };
                let input = rate("input")?;
                let output = rate("output")?;
                if input.is_none() || output.is_none() {
                    return Err(format!("{model} needs input and output prices."));
                }
                settings.prices.insert(
                    model.to_owned(),
                    [input, output, rate("cache_read")?, rate("cache_write")?],
                );
            }
        }
        if let Some(aliases) = value.get("aliases") {
            let aliases = aliases
                .as_object()
                .ok_or_else(|| "Model mappings must be an object.".to_owned())?;
            for (model, target) in aliases {
                let target = target.as_str().map(str::trim).unwrap_or_default();
                if model.trim().is_empty() || target.is_empty() {
                    return Err("A model mapping needs both model ids.".into());
                }
                settings
                    .aliases
                    .insert(model.trim().to_owned(), target.to_owned());
            }
        }
        Ok(settings)
    }

    fn to_value(&self) -> Value {
        let prices: Map<String, Value> = self
            .prices
            .iter()
            .map(|(model, [input, output, cache_read, cache_write])| {
                (
                    model.clone(),
                    json!({
                        "input": input,
                        "output": output,
                        "cache_read": cache_read,
                        "cache_write": cache_write,
                    }),
                )
            })
            .collect();
        json!({"prices": prices, "aliases": self.aliases})
    }

    /// Blank cache prices use the input rate; zero means free.
    fn override_table(&self) -> RateTable {
        self.prices
            .iter()
            .map(|(model, [input, output, cache_read, cache_write])| {
                let input = input.unwrap_or(0.0);
                (
                    model.trim().to_owned(),
                    ModelRate {
                        standard: TokenRates {
                            input: input / 1e6,
                            output: output.unwrap_or(0.0) / 1e6,
                            cache_read: cache_read.unwrap_or(input) / 1e6,
                            cache_creation: cache_write.unwrap_or(input) / 1e6,
                        },
                        fast: None,
                        ultrafast: None,
                    },
                )
            })
            .collect()
    }

    /// Follows alias chains; a cycle maps nothing.
    fn resolved_aliases(&self) -> HashMap<&str, &str> {
        let mut resolved = HashMap::new();
        for model in self.aliases.keys() {
            let mut seen = HashSet::from([model.as_str()]);
            let mut target = self.aliases[model].as_str();
            while let Some(next) = self.aliases.get(target) {
                if !seen.insert(target) {
                    break;
                }
                target = next;
            }
            if !self.aliases.contains_key(target) {
                resolved.insert(model.as_str(), target);
            }
        }
        resolved
    }
}

/// The requested window, in the viewer's zone. Hours are rolling; days are calendar days.
struct UsageWindow {
    time_zone: Tz,
    since_day: NaiveDate,
    until_day: NaiveDate,
    /// `Some` for the hourly "Past 24h" window.
    hourly: Option<(i64, i64)>,
}

impl UsageWindow {
    fn new(days: u32, time_zone: Tz, now_ms: i64) -> Self {
        let now = instant(now_ms);
        let until_day = now.with_timezone(&time_zone).date_naive();
        if days == 1 {
            let until_ms = now_ms.div_euclid(MINUTE_MS) * MINUTE_MS;
            let since_ms = until_ms - DAY_MS;
            return Self {
                time_zone,
                since_day: instant(since_ms).with_timezone(&time_zone).date_naive(),
                until_day,
                hourly: Some((since_ms, until_ms)),
            };
        }
        Self {
            time_zone,
            since_day: until_day
                .checked_sub_days(Days::new(u64::from(days.max(1)) - 1))
                .unwrap_or(until_day),
            until_day,
            hourly: None,
        }
    }

    /// Files untouched since well before the window cannot hold records in it.
    fn earliest_ms(&self) -> i64 {
        let start = self
            .since_day
            .and_hms_opt(0, 0, 0)
            .map_or(0, |start| start.and_utc().timestamp_millis());
        start - 2 * DAY_MS
    }

    fn day_of(&self, ms: i64) -> NaiveDate {
        instant(ms).with_timezone(&self.time_zone).date_naive()
    }

    fn periods(&self) -> Vec<Value> {
        match self.hourly {
            Some((since_ms, until_ms)) => (0..)
                .map(|index| since_ms + index * HOUR_MS)
                .take_while(|start| *start < until_ms)
                .map(|start| {
                    json!({
                        "key": iso(start),
                        "label": hour_label(start, self.time_zone),
                        "tooltip": relative_hour_label(start, until_ms, self.time_zone),
                    })
                })
                .collect(),
            None => self
                .since_day
                .iter_days()
                .take_while(|day| *day <= self.until_day)
                .map(|day| {
                    let label = day_label(day);
                    json!({"key": day.to_string(), "label": label, "tooltip": label})
                })
                .collect(),
        }
    }

    fn label(&self) -> String {
        match self.hourly {
            Some((since_ms, until_ms)) => format!(
                "{} to {}",
                date_hour_label(since_ms, self.time_zone),
                date_hour_label(until_ms, self.time_zone)
            ),
            None => format!(
                "{} to {}",
                day_label(self.since_day),
                day_label(self.until_day)
            ),
        }
    }
}

fn instant(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or(DateTime::UNIX_EPOCH)
}

fn iso(ms: i64) -> String {
    instant(ms).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn day_label(day: NaiveDate) -> String {
    format!("{} {}", MONTHS[day.month0() as usize], day.day())
}

fn twelve_hour(hour: u32) -> (u32, &'static str) {
    let suffix = if hour < 12 { "AM" } else { "PM" };
    (
        if hour.is_multiple_of(12) {
            12
        } else {
            hour % 12
        },
        suffix,
    )
}

/// A repeated wall-clock hour during a fall-back transition names its zone so
/// the two distinct buckets remain distinguishable.
fn hour_label(ms: i64, zone: Tz) -> String {
    let local = instant(ms).with_timezone(&zone);
    let (hour, suffix) = twelve_hour(local.hour());
    let wall = |ms: i64| {
        let local = instant(ms).with_timezone(&zone);
        (local.date_naive(), local.hour())
    };
    let repeated = wall(ms - HOUR_MS) == wall(ms) || wall(ms + HOUR_MS) == wall(ms);
    if repeated {
        format!("{hour} {suffix} {}", local.format("%Z"))
    } else {
        format!("{hour} {suffix}")
    }
}

fn date_hour_label(ms: i64, zone: Tz) -> String {
    let local = instant(ms).with_timezone(&zone);
    let (hour, suffix) = twelve_hour(local.hour());
    format!("{}, {hour} {suffix}", day_label(local.date_naive()))
}

fn relative_hour_label(ms: i64, reference_ms: i64, zone: Tz) -> String {
    let day = instant(ms).with_timezone(&zone).date_naive();
    let reference = instant(reference_ms).with_timezone(&zone).date_naive();
    match (reference - day).num_days() {
        0 => format!("{} today", hour_label(ms, zone)),
        1 => format!("{} yesterday", hour_label(ms, zone)),
        _ => date_hour_label(ms, zone),
    }
}

/// When an upcoming reset lands: a time today, "tomorrow at", or a dated time.
fn upcoming_label(ms: i64, now_ms: i64, zone: Tz) -> String {
    let local = instant(ms).with_timezone(&zone);
    let today = instant(now_ms).with_timezone(&zone).date_naive();
    let (hour, suffix) = twelve_hour(local.hour());
    let time = format!("{hour}:{:02} {suffix}", local.minute());
    let day = local.date_naive();
    match (day - today).num_days() {
        0 => time,
        1 => format!("tomorrow at {time}"),
        _ if day.year() == today.year() => format!("{}/{} {time}", day.month(), day.day()),
        _ => format!("{}/{}/{} {time}", day.month(), day.day(), day.year()),
    }
}

fn provider_home(provider: &str, values: &HashMap<String, std::ffi::OsString>) -> Option<PathBuf> {
    let (variable, directory, nested) = match provider {
        "claude" => ("CLAUDE_CONFIG_DIR", ".claude", "projects"),
        _ => ("CODEX_HOME", ".codex", "sessions"),
    };
    let lookup = |name: &str| {
        values
            .get(name)
            .cloned()
            .or_else(|| std::env::var_os(name))
            .filter(|value| !value.is_empty())
    };
    let home = lookup(variable)
        .map(PathBuf::from)
        .or_else(|| lookup("HOME").map(|home| PathBuf::from(home).join(directory)))?;
    Some(home.join(nested))
}

/// Where each provider's transcripts live. Tests point these at fixtures.
#[derive(Clone, Debug)]
struct TranscriptRoots {
    claude: Option<PathBuf>,
    codex: Option<PathBuf>,
}

impl TranscriptRoots {
    fn discover() -> Self {
        let values = crate::user_environment::values();
        Self {
            claude: provider_home("claude", values),
            codex: provider_home("codex", values),
        }
    }
}

struct CachedFile {
    size: u64,
    modified_ms: i64,
    records: Arc<Vec<UsageRecord>>,
    malformed: u64,
    /// Where reading left off, so a transcript that only grew is read on
    /// from there.
    resume: Resume,
}

/// A transcript read up to the end of its last complete line.
#[derive(Clone)]
struct Resume {
    /// Device and inode: a replaced file is read again from the start.
    identity: (u64, u64),
    offset: u64,
    /// The bytes just before `offset`, checked again before reading on, so
    /// a file rewritten rather than appended to is read again in full.
    check: Vec<u8>,
    records: Vec<UsageRecord>,
    codex: CodexScanState,
}

fn modified_ms(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn list_transcripts(root: &Path, failed: &mut u64) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            *failed += 1;
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(_) => {
                    if path
                        .extension()
                        .is_some_and(|extension| extension == "jsonl")
                    {
                        files.push(path);
                    }
                }
                Err(_) => *failed += 1,
            }
        }
    }
    // First-seen deduplication needs a stable order between scans.
    files.sort();
    files
}

/// Calls `line` per newline-terminated line, and for an unterminated tail
/// (with `false`). Oversized lines are dropped while reading, not after
/// buffering them. Returns how many bytes the complete lines took.
fn for_each_line(
    mut reader: impl BufRead,
    mut line: impl FnMut(&[u8], bool),
) -> std::io::Result<u64> {
    let mut pending = Vec::new();
    let mut skipping = false;
    let mut read = 0_u64;
    let mut complete = 0_u64;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            if !skipping && !pending.is_empty() {
                line(&pending, false);
            }
            return Ok(complete);
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let end = newline.unwrap_or(chunk.len());
        if !skipping {
            if pending.len() + end > LINE_LIMIT {
                skipping = true;
                pending = Vec::new();
            } else if pending.is_empty() && newline.is_some() {
                line(&chunk[..end], true);
            } else {
                pending.extend_from_slice(&chunk[..end]);
            }
        }
        let consumed = newline.map_or(chunk.len(), |index| index + 1);
        reader.consume(consumed);
        read += consumed as u64;
        if newline.is_some() {
            if !skipping && !pending.is_empty() {
                line(&pending, true);
            }
            pending.clear();
            skipping = false;
            complete = read;
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let Some((&first, rest)) = needle.split_first() else {
        return true;
    };
    let mut start = 0;
    while let Some(found) = haystack[start..].iter().position(|byte| *byte == first) {
        let at = start + found;
        if haystack[at + 1..].starts_with(rest) {
            return true;
        }
        start = at + 1;
    }
    false
}

const CODEX_MARKERS: [&[u8]; 5] = [
    b"\"token_count\"",
    b"\"token_usage_record\"",
    b"\"turn_context\"",
    b"\"thread_settings_applied\"",
    b"\"session_meta\"",
];

/// `None` means the file could not be read; caching that as empty would hide
/// its usage until its size or time changed.
#[cfg(test)]
fn read_transcript(
    provider: &str,
    path: &Path,
    retain_from_ms: i64,
) -> Option<(Vec<UsageRecord>, u64)> {
    let file = read_transcript_from(provider, path, retain_from_ms, None)?;
    Some((Arc::unwrap_or_clone(file.records), file.malformed))
}

/// Reads a transcript, from where `resume` left off when the file has only
/// grown since. `None` means the file could not be read.
fn read_transcript_from(
    provider: &str,
    path: &Path,
    retain_from_ms: i64,
    resume: Option<&Resume>,
) -> Option<CachedFile> {
    use std::io::{Seek, SeekFrom};
    use std::os::unix::fs::{FileExt, MetadataExt};
    let mut file = fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let identity = (metadata.dev(), metadata.ino());
    let resume = resume.filter(|resume| {
        resume.identity == identity && resume.offset <= metadata.len() && {
            let mut check = vec![0; resume.check.len()];
            let at = resume.offset - resume.check.len() as u64;
            file.read_exact_at(&mut check, at).is_ok() && check == resume.check
        }
    });
    let (offset, mut records, mut codex) = match resume {
        Some(resume) => (resume.offset, resume.records.clone(), resume.codex.clone()),
        None => (0, Vec::new(), CodexScanState::default()),
    };
    file.seek(SeekFrom::Start(offset)).ok()?;
    let reader = BufReader::with_capacity(256 * 1024, &file);
    // An unterminated last line is still being written; it counts for now
    // but is read again next time, from a copy of the Codex state.
    let mut tail = Vec::new();
    let mut tail_codex = None::<CodexScanState>;
    let complete = for_each_line(reader, |line, terminated| {
        let codex = if terminated {
            &mut codex
        } else {
            tail_codex.insert(codex.clone())
        };
        let parsed = if provider == "claude" {
            if contains(line, b"\"usage\"")
                && let Ok(value) = serde_json::from_slice::<Value>(line)
            {
                parse_claude_record(&value)
            } else {
                Vec::new()
            }
        } else if CODEX_MARKERS.iter().any(|marker| contains(line, marker))
            && let Ok(value) = serde_json::from_slice::<Value>(line)
        {
            // Old Codex events still run through the parser to keep its
            // counters right.
            parse_codex_record(&value, codex)
        } else {
            Vec::new()
        };
        let kept = parsed
            .into_iter()
            .filter(|record| record.timestamp_ms >= retain_from_ms);
        if terminated {
            records.extend(kept);
        } else {
            tail.extend(kept);
        }
    })
    .ok()?;
    let end = offset + complete;
    let length = end.min(64);
    let mut check = vec![0; length as usize];
    file.read_exact_at(&mut check, end - length).ok()?;
    let malformed = tail_codex.as_ref().unwrap_or(&codex).malformed;
    records.shrink_to_fit();
    let mut all = records.clone();
    all.extend(tail);
    Some(CachedFile {
        size: metadata.len(),
        modified_ms: modified_ms(&metadata),
        records: Arc::new(all),
        malformed,
        resume: Resume {
            identity,
            offset: end,
            check,
            records,
            codex,
        },
    })
}

#[derive(Default)]
struct ScanCache {
    files: HashMap<PathBuf, CachedFile>,
}

struct SourceScan {
    provider: &'static str,
    /// Files in scan order; a null root means there is no transcript directory.
    records: Vec<Arc<Vec<UsageRecord>>>,
    fingerprint: Value,
    status: &'static str,
    scanned_files: u64,
    skipped_files: u64,
    malformed_records: u64,
    message: Option<String>,
}

fn host_id() -> String {
    let mut buffer = [0_u8; 256];
    let written = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
    if written == 0 && end > 0 {
        String::from_utf8_lossy(&buffer[..end]).into_owned()
    } else {
        "unknown".into()
    }
}

fn volume_id(path: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path)
        .map(|metadata| format!("{}:{}", metadata.dev(), metadata.ino()))
        .unwrap_or_default()
}

fn scan_source(
    provider: &'static str,
    root: Option<&Path>,
    earliest_ms: i64,
    cache: &mut ScanCache,
) -> SourceScan {
    let resolved = root
        .and_then(|root| fs::canonicalize(root).ok())
        .or_else(|| root.map(Path::to_path_buf));
    let fingerprint = json!({
        "host_id": host_id(),
        "provider": provider,
        "resolved_home_path": resolved.as_deref().map(|path| path.display().to_string()),
        // Device and inode distinguish machines sharing a hostname and home path.
        "volume_id": root.map(volume_id).unwrap_or_default(),
    });
    let mut scan = SourceScan {
        provider,
        records: Vec::new(),
        fingerprint,
        status: "missing",
        scanned_files: 0,
        skipped_files: 0,
        malformed_records: 0,
        message: Some("No transcript directory on this environment.".into()),
    };
    let Some(root) = root.filter(|root| root.is_dir()) else {
        return scan;
    };
    scan.status = "ok";
    scan.message = None;
    let retain_from_ms = now_ms() - RETENTION_MS;
    let mut unread = 0;
    let paths = list_transcripts(root, &mut unread);
    let live: HashSet<&PathBuf> = paths.iter().collect();
    // Only this provider's entries: both providers share one cache.
    cache
        .files
        .retain(|path, _| !path.starts_with(root) || live.contains(path));
    for path in &paths {
        let Ok(metadata) = fs::metadata(path) else {
            unread += 1;
            continue;
        };
        let modified = modified_ms(&metadata);
        if modified < retain_from_ms {
            cache.files.remove(path);
            continue;
        }
        if modified < earliest_ms {
            continue;
        }
        let cached = cache
            .files
            .get(path)
            .filter(|cached| cached.size == metadata.len() && cached.modified_ms == modified);
        if let Some(cached) = cached {
            scan.records.push(cached.records.clone());
            scan.malformed_records += cached.malformed;
            scan.scanned_files += 1;
            continue;
        }
        // Only a file that grew is read on; anything else starts over.
        let resume = cache
            .files
            .get(path)
            .filter(|cached| cached.size < metadata.len())
            .map(|cached| &cached.resume);
        let Some(file) = read_transcript_from(provider, path, retain_from_ms, resume) else {
            cache.files.remove(path);
            unread += 1;
            continue;
        };
        scan.records.push(file.records.clone());
        scan.malformed_records += file.malformed;
        scan.scanned_files += 1;
        cache.files.insert(path.clone(), file);
    }
    scan.skipped_files = unread;
    // Skipped records leave the totals short, which the page says out loud.
    if scan.malformed_records > 0 {
        scan.status = "partial";
    }
    if unread > 0 {
        scan.status = "partial";
        scan.message = Some(format!(
            "{unread} transcript path(s) could not be read; usage may be incomplete."
        ));
    }
    scan
}

#[derive(Default)]
struct Bucket {
    totals: Totals,
    cost_usd: f64,
    cache_savings_usd: f64,
    category: Option<CategoryCost>,
    fast_cost_usd: f64,
    ultrafast_cost_usd: f64,
    speed_premium_usd: f64,
    records: u64,
    unpriced_records: u64,
    unpriced_tokens: u64,
    provider_reported_records: u64,
}

type CopyKey = (Arc<str>, i64, Arc<str>, [u64; 5]);

fn copy_key(record: &UsageRecord) -> CopyKey {
    let totals = &record.totals;
    (
        record.session_id.clone(),
        record.timestamp_ms,
        record.model.clone(),
        [
            totals.uncached_input,
            totals.cached_input,
            totals.cache_creation,
            totals.output,
            totals.reasoning,
        ],
    )
}

/// Codex sessions found in more than one file. A rollout that moved after it
/// was read leaves a copy in both places. Only these need cross-file matching.
fn sessions_across_files(scans: &[SourceScan]) -> HashSet<&str> {
    let mut first_file = HashMap::new();
    let mut shared = HashSet::new();
    let mut index = 0;
    for scan in scans {
        for file in &scan.records {
            index += 1;
            for record in file.iter() {
                if record.provider != "codex" || record.session_id.is_empty() {
                    continue;
                }
                let first = *first_file.entry(&*record.session_id).or_insert(index);
                if first != index {
                    shared.insert(&*record.session_id);
                }
            }
        }
    }
    shared
}

/// Deduplicates across the whole scan; Claude copies records into resumed and forked transcripts.
fn aggregate(
    scans: &[SourceScan],
    window: &UsageWindow,
    rates: &RateTable,
    settings: &PriceSettings,
) -> (Vec<Value>, BTreeMap<&'static str, usize>) {
    let overrides = settings.override_table();
    let aliases = settings.resolved_aliases();
    let mut seen = HashSet::new();
    let mut sessions: BTreeMap<&'static str, HashSet<Arc<str>>> = BTreeMap::new();
    #[allow(clippy::type_complexity)]
    let mut buckets: BTreeMap<(NaiveDate, Option<i64>, &'static str, Arc<str>), Bucket> =
        BTreeMap::new();
    let shared = sessions_across_files(scans);
    let mut seen_copies = HashSet::new();
    for scan in scans {
        for file in &scan.records {
            let mut occurrences: HashMap<CopyKey, u32> = HashMap::new();
            for record in file.iter() {
                if let Some(key) = &record.dedupe_key
                    && !seen.insert(key.clone())
                {
                    continue;
                }
                // Moved copies repeat a session's requests. Equal requests within
                // one file are distinct, so each is told apart by its occurrence.
                if record.provider == "codex" && shared.contains(&*record.session_id) {
                    let key = copy_key(record);
                    let occurrence = occurrences.entry(key.clone()).or_default();
                    *occurrence += 1;
                    if !seen_copies.insert((key, *occurrence)) {
                        continue;
                    }
                }
                let (day, hour) = match window.hourly {
                    Some((since_ms, until_ms)) => {
                        if record.timestamp_ms < since_ms || record.timestamp_ms >= until_ms {
                            continue;
                        }
                        let index = (record.timestamp_ms - since_ms).div_euclid(HOUR_MS);
                        (
                            window.day_of(record.timestamp_ms),
                            Some(since_ms + index * HOUR_MS),
                        )
                    }
                    None => {
                        let day = window.day_of(record.timestamp_ms);
                        if day < window.since_day || day > window.until_day {
                            continue;
                        }
                        (day, None)
                    }
                };
                // An alias prices and groups as the model it maps to.
                let mapped;
                let record = match aliases.get(record.model.trim()) {
                    Some(target) => {
                        mapped = UsageRecord {
                            model: Arc::from(*target),
                            ..record.clone()
                        };
                        &mapped
                    }
                    None => record,
                };
                let priced = price_usage(rates, &overrides, record);
                let bucket = buckets
                    .entry((day, hour, record.provider, record.model.clone()))
                    .or_default();
                bucket.totals.add(&record.totals);
                bucket.cost_usd += priced.cost_usd;
                if let Some(add) = priced.category {
                    let sum = bucket.category.get_or_insert_default();
                    sum.input += add.input;
                    sum.cache_read += add.cache_read;
                    sum.cache_write += add.cache_write;
                    sum.output += add.output;
                }
                match record.speed {
                    Speed::Fast => bucket.fast_cost_usd += priced.cost_usd,
                    Speed::Ultrafast => bucket.ultrafast_cost_usd += priced.cost_usd,
                    Speed::Standard => {}
                }
                bucket.speed_premium_usd += priced.speed_premium_usd;
                bucket.cache_savings_usd += cache_savings_usd(rates, &overrides, record);
                bucket.records += 1;
                match priced.cost_source {
                    CostSource::Unpriced => {
                        bucket.unpriced_records += 1;
                        let totals = &record.totals;
                        bucket.unpriced_tokens = bucket
                            .unpriced_tokens
                            .saturating_add(totals.uncached_input)
                            .saturating_add(totals.cached_input)
                            .saturating_add(totals.cache_creation)
                            .saturating_add(totals.output);
                    }
                    CostSource::ProviderReported => bucket.provider_reported_records += 1,
                    CostSource::ModelPriced => {}
                }
                if !record.session_id.is_empty() {
                    sessions
                        .entry(record.provider)
                        .or_default()
                        .insert(record.session_id.clone());
                }
            }
        }
    }
    let values = buckets
        .into_iter()
        .map(|((day, hour, provider, model), bucket)| {
            let cost_source = if bucket.unpriced_records == bucket.records {
                "unpriced"
            } else if bucket.provider_reported_records == bucket.records {
                "providerReported"
            } else {
                "modelPriced"
            };
            json!({
                "day": day.to_string(),
                "hour_start": hour.map(iso),
                "provider": provider,
                "model": &*model,
                "totals": {
                    "uncached_input": bucket.totals.uncached_input,
                    "cached_input": bucket.totals.cached_input,
                    "cache_creation": bucket.totals.cache_creation,
                    "output": bucket.totals.output,
                    "reasoning": bucket.totals.reasoning,
                },
                "cost_usd": bucket.cost_usd,
                "cache_savings_usd": bucket.cache_savings_usd,
                "category_cost_usd": bucket.category.map(|cost| json!({
                    "input": round_usd(cost.input),
                    "cache_read": round_usd(cost.cache_read),
                    "cache_write": round_usd(cost.cache_write),
                    "output": round_usd(cost.output),
                })),
                "fast_cost_usd": round_usd(bucket.fast_cost_usd),
                "ultrafast_cost_usd": round_usd(bucket.ultrafast_cost_usd),
                "speed_premium_usd": round_usd(bucket.speed_premium_usd),
                "cost_source": cost_source,
                "records": bucket.records,
                "unpriced_records": bucket.unpriced_records,
                "unpriced_tokens": bucket.unpriced_tokens,
            })
        })
        .collect();
    let sessions = sessions
        .into_iter()
        .map(|(provider, sessions)| (provider, sessions.len()))
        .collect();
    (values, sessions)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn parse_zone(request: &Value) -> Tz {
    request["time_zone"]
        .as_str()
        .and_then(|zone| zone.parse().ok())
        .unwrap_or(Tz::UTC)
}

type RatesFetcher = dyn Fn(&str, &str) -> Option<Value> + Send + Sync;

struct Rates {
    table: Arc<RateTable>,
    fetched_at_ms: Option<i64>,
    last_failed_at_ms: Option<i64>,
    status: &'static str,
}

struct Shared {
    roots: TranscriptRoots,
    scan: Mutex<ScanCache>,
    rates: Mutex<Rates>,
    prices: Mutex<()>,
    fetch_rates: Arc<RatesFetcher>,
    data_directory: Option<PathBuf>,
}

impl Shared {
    fn ensure_rates(&self, force: bool) -> (Arc<RateTable>, Value) {
        let mut rates = self
            .rates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = now_ms();
        let max_age = if force {
            RATES_REFRESH_FLOOR_MS
        } else {
            RATES_TTL_MS
        };
        let fresh = |fetched: Option<i64>| fetched.is_some_and(|fetched| now - fetched < max_age);
        if !fresh(rates.fetched_at_ms) {
            if rates.fetched_at_ms.is_none()
                && let Some((table, fetched_at)) = self.read_rates_cache()
            {
                rates.table = Arc::new(table);
                rates.fetched_at_ms = Some(fetched_at);
                rates.status = "cached";
            }
            let cooling_down = rates
                .last_failed_at_ms
                .is_some_and(|failed| now - failed < RATES_REFRESH_FLOOR_MS);
            if !fresh(rates.fetched_at_ms) && !cooling_down {
                let fetched = (self.fetch_rates)(LITELLM_RATES_URL, "application/json")
                    .filter(|document| !parse_rate_table(document).is_empty())
                    .or_else(|| {
                        (self.fetch_rates)(LITELLM_RATES_API_URL, "application/vnd.github.raw+json")
                            .filter(|document| !parse_rate_table(document).is_empty())
                    });
                match fetched {
                    Some(document) => {
                        rates.table = Arc::new(parse_rate_table(&document));
                        rates.fetched_at_ms = Some(now);
                        rates.last_failed_at_ms = None;
                        rates.status = "fresh";
                        self.write_rates_cache(now, &document);
                    }
                    None => {
                        rates.last_failed_at_ms = Some(now);
                        if !rates.table.is_empty() {
                            rates.status = "cached";
                        }
                    }
                }
            }
        }
        let pricing = json!({
            "status": rates.status,
            "source": LITELLM_RATES_URL,
            "fetched_at_ms": rates.fetched_at_ms,
            "known_models": rates.table.len(),
        });
        (rates.table.clone(), pricing)
    }

    fn read_rates_cache(&self) -> Option<(RateTable, i64)> {
        let path = self.data_directory.as_ref()?.join(RATES_CACHE_FILE);
        let cache: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        let table = parse_rate_table(&cache["document"]);
        (!table.is_empty()).then_some((table, cache["fetched_at_ms"].as_i64()?))
    }

    fn write_rates_cache(&self, now: i64, document: &Value) {
        if let Some(directory) = &self.data_directory {
            let body = json!({"fetched_at_ms": now, "document": document});
            let _ = write_private_atomic(&directory.join(RATES_CACHE_FILE), &body);
        }
    }

    fn read_prices(&self) -> PriceSettings {
        self.data_directory
            .as_ref()
            .and_then(|directory| fs::read(directory.join(PRICES_FILE)).ok())
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
            .and_then(|value| PriceSettings::from_value(&value).ok())
            .unwrap_or_default()
    }

    /// `read_at` comes from the client when it can: hosts answering one request
    /// then share a window, so their hourly buckets line up.
    fn summary(&self, days: u32, zone: Tz, refresh: bool, read_at: i64) -> Value {
        let started = Instant::now();
        let window = UsageWindow::new(days, zone, read_at);
        let (rates, pricing) = self.ensure_rates(refresh);
        let settings = self.read_prices();
        let scans = {
            let mut cache = self
                .scan
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            vec![
                scan_source(
                    "codex",
                    self.roots.codex.as_deref(),
                    window.earliest_ms(),
                    &mut cache,
                ),
                scan_source(
                    "claude",
                    self.roots.claude.as_deref(),
                    window.earliest_ms(),
                    &mut cache,
                ),
            ]
        };
        let (buckets, sessions) = aggregate(&scans, &window, &rates, &settings);
        let sources: Vec<Value> = scans
            .iter()
            .map(|scan| {
                json!({
                    "fingerprint": scan.fingerprint,
                    "provider": scan.provider,
                    "status": scan.status,
                    "scanned_files": scan.scanned_files,
                    "skipped_files": scan.skipped_files,
                    "malformed_records": scan.malformed_records,
                    "message": scan.message,
                })
            })
            .collect();
        json!({
            "read_at_ms": read_at,
            "time_zone": zone.name(),
            "resolution": if window.hourly.is_some() { "hour" } else { "day" },
            "window_label": window.label(),
            "periods": window.periods(),
            "buckets": buckets,
            "sessions": sessions,
            "sources": sources,
            "pricing": pricing,
            "scan_duration_ms": started.elapsed().as_millis() as u64,
        })
    }
}

fn write_private_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Usage settings have no directory.".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("Cannot create {parent:?}: {error}"))?;
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = crate::private_fs::create_private_file(&temporary)?;
        file.write_all(value.to_string().as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("Cannot save {path:?}: {error}"));
    }
    Ok(())
}

fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

fn iso_ms(value: &Value) -> Option<i64> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|time| time.timestamp_millis())
}

fn epoch_seconds_ms(value: &Value) -> Option<i64> {
    let seconds = value
        .as_f64()
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)?;
    Some((seconds * 1000.0) as i64)
}

#[derive(Clone, Debug, PartialEq)]
struct LimitWindow {
    id: String,
    kind: &'static str,
    label: String,
    used_percent: f64,
    resets_at_ms: Option<i64>,
    window_duration_mins: Option<i64>,
}

/// `get_usage` percentages are already 0-100.
fn claude_windows(rate_limits: &Value) -> Vec<LimitWindow> {
    let mut windows = Vec::new();
    for (id, kind, label, duration) in [
        ("five_hour", "session", "Session", SESSION_MINS),
        ("seven_day", "weekly", "Weekly", WEEK_MINS),
    ] {
        let window = &rate_limits[id];
        let Some(utilization) = window["utilization"].as_f64() else {
            continue;
        };
        windows.push(LimitWindow {
            id: id.into(),
            kind,
            label: label.into(),
            used_percent: clamp_percent(utilization),
            resets_at_ms: iso_ms(&window["resets_at"]),
            window_duration_mins: Some(duration),
        });
    }
    if let Some(scoped) = rate_limits["model_scoped"].as_array() {
        for entry in scoped {
            let (Some(name), Some(utilization)) = (
                entry["display_name"].as_str(),
                entry["utilization"].as_f64(),
            ) else {
                continue;
            };
            let slug: String = name
                .to_lowercase()
                .split(|character: char| !character.is_ascii_alphanumeric())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("_");
            windows.push(LimitWindow {
                id: format!("seven_day_{slug}"),
                kind: "weekly",
                label: format!("Weekly · {name}"),
                used_percent: clamp_percent(utilization),
                resets_at_ms: iso_ms(&entry["resets_at"]),
                window_duration_mins: Some(WEEK_MINS),
            });
        }
    }
    if let Some(percent) = claude_spend_percent(rate_limits) {
        windows.push(LimitWindow {
            id: "monthly_spend".into(),
            kind: "monthly",
            label: "Monthly spend".into(),
            used_percent: percent,
            resets_at_ms: None,
            window_duration_mins: None,
        });
    }
    windows
}

fn percent_of(used: &Value, limit: &Value) -> Option<f64> {
    let used = used
        .as_f64()
        .filter(|used| used.is_finite() && *used >= 0.0)?;
    let limit = limit
        .as_f64()
        .filter(|limit| limit.is_finite() && *limit > 0.0)?;
    Some(clamp_percent(used / limit * 100.0))
}

fn claude_spend_percent(rate_limits: &Value) -> Option<f64> {
    let spend = &rate_limits["spend"];
    // Minor-unit ratios need matching currency and scale; `percent` is rounded.
    if spend["enabled"] == true
        && spend["used"]["currency"]
            .as_str()
            .is_some_and(|currency| !currency.trim().is_empty())
        && spend["used"]["currency"] == spend["limit"]["currency"]
        && spend["used"]["exponent"] == spend["limit"]["exponent"]
        && let Some(percent) = percent_of(
            &spend["used"]["amount_minor"],
            &spend["limit"]["amount_minor"],
        )
    {
        return Some(percent);
    }
    let extra = &rate_limits["extra_usage"];
    if extra["is_enabled"] != true {
        return None;
    }
    percent_of(&extra["used_credits"], &extra["monthly_limit"]).or_else(|| {
        extra["utilization"]
            .as_f64()
            .filter(|percent| percent.is_finite() && *percent >= 0.0)
            .map(clamp_percent)
    })
}

fn codex_kind(mins: i64) -> (&'static str, &'static str) {
    if mins >= MONTH_MINS {
        ("monthly", "Monthly")
    } else if mins >= WEEK_MINS {
        ("weekly", "Weekly")
    } else {
        ("session", "Session")
    }
}

/// `primary` and `secondary` are positions. Without a duration, paid plans use
/// 5-hour and weekly windows and Free and Go use monthly.
fn codex_windows(snapshot: &Value) -> Vec<LimitWindow> {
    // Model-specific limits stay out of the main allowance rows.
    if snapshot["limitId"]
        .as_str()
        .is_some_and(|id| !id.is_empty() && id != "codex")
    {
        return Vec::new();
    }
    let monthly_plan = matches!(snapshot["planType"].as_str(), Some("free" | "go"));
    let mut windows = Vec::new();
    for (id, fallback) in [
        (
            "primary",
            if monthly_plan {
                MONTH_MINS
            } else {
                SESSION_MINS
            },
        ),
        ("secondary", WEEK_MINS),
    ] {
        let window = &snapshot[id];
        let Some(used) = window["usedPercent"]
            .as_f64()
            .filter(|used| used.is_finite())
        else {
            continue;
        };
        let duration = window["windowDurationMins"].as_i64().unwrap_or(fallback);
        let (kind, label) = codex_kind(duration);
        windows.push(LimitWindow {
            id: id.into(),
            kind,
            label: label.into(),
            used_percent: clamp_percent(used),
            resets_at_ms: epoch_seconds_ms(&window["resetsAt"]),
            window_duration_mins: Some(duration),
        });
    }
    let individual = &snapshot["individualLimit"];
    if let Some(remaining) = individual["remainingPercent"]
        .as_f64()
        .filter(|left| left.is_finite())
    {
        windows.push(LimitWindow {
            id: "individual".into(),
            kind: "monthly",
            label: "Monthly credit limit".into(),
            used_percent: clamp_percent(100.0 - remaining),
            resets_at_ms: epoch_seconds_ms(&individual["resetsAt"]),
            window_duration_mins: None,
        });
    }
    windows
}

fn codex_reset_credits(summary: &Value) -> Option<Value> {
    let available = summary["availableCount"].as_i64()?.max(0);
    let next = summary["credits"].as_array().and_then(|credits| {
        credits
            .iter()
            .filter(|credit| credit["status"] == "available")
            .filter_map(|credit| credit["expiresAt"].as_f64())
            .fold(None, |soonest: Option<f64>, at| {
                Some(soonest.map_or(at, |soonest| soonest.min(at)))
            })
    });
    Some(json!({
        "available_count": available,
        "next_expires_at_ms": next.and_then(|at| epoch_seconds_ms(&json!(at))),
    }))
}

/// The plan as Codex's own status line names it. Codex reports billing ids,
/// and several are sold under another name: `pro` is "Pro 200" beside
/// `prolite` ("Pro 100"), and `team` is "Business". Ids Codex adds later
/// fall back to their own words.
fn codex_plan(plan_type: &str) -> String {
    match plan_type.to_ascii_lowercase().as_str() {
        "free" => "Free",
        "go" => "Go",
        "plus" => "Plus",
        "pro" => "Pro 200",
        "prolite" => "Pro 100",
        "promax" => "Pro 500",
        "team" | "self_serve_business_usage_based" => "Business",
        "self_serve_business_prolite" => "Business Premium",
        "business" | "ent26" | "enterprise" | "hc" | "enterprise_cbp_usage_based" => "Enterprise",
        "enterprise_cbp_automation" => "Enterprise (Automation)",
        "edu" | "education" => "Edu",
        "edu_plus" => "Edu Plus",
        "edu_pro" => "Edu Pro",
        "sci" => "Academic Researchers",
        "sci_plus" => "Academic Researchers Plus",
        _ => return title_case(plan_type),
    }
    .to_owned()
}

fn title_case(value: &str) -> String {
    value
        .split(['_', '-', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A login is identified without its address crossing the wire. Two connected
/// hosts signed in to one account then count as one account, not two.
fn account_key(provider: &str, identity: Option<&str>) -> Option<String> {
    let identity = identity?.trim().to_lowercase();
    if identity.is_empty() {
        return None;
    }
    let digest = Sha256::digest(format!("{provider}:{identity}"));
    Some(
        digest
            .iter()
            .take(12)
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn windows_json(windows: &[LimitWindow], now: i64, zone: Tz) -> Vec<Value> {
    windows
        .iter()
        .map(|window| {
            json!({
                "id": window.id,
                "kind": window.kind,
                "label": window.label,
                "used_percent": window.used_percent,
                "resets_at_ms": window.resets_at_ms,
                "resets_at_label": window.resets_at_ms.map(|at| upcoming_label(at, now, zone)),
                "window_duration_mins": window.window_duration_mins,
            })
        })
        .collect()
}

fn unavailable_account(provider: &str, reason: &str, message: Option<String>, now: i64) -> Value {
    json!({
        "provider": provider,
        "checked_at_ms": now,
        "windows": [],
        "unavailable": {"reason": reason, "message": message},
    })
}

/// Reads the assistant's own usage answer, spawning it the way chats do.
/// `None` means the CLI cannot be started, so the provider is not installed.
struct ProbeSession {
    _probe: Probe,
    stdin: std::process::ChildStdin,
    receiver: mpsc::Receiver<Value>,
    deadline: Instant,
}

impl ProbeSession {
    fn start(mut command: std::process::Command) -> Option<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut probe = Probe(command.spawn().ok()?);
        let stdin = probe.0.stdin.take()?;
        let stdout = probe.0.stdout.take()?;
        let (sender, receiver) = mpsc::sync_channel(64);
        thread::spawn(move || {
            let reader = BufReader::new(stdout.take(32 * 1024 * 1024));
            for line in reader.split(b'\n') {
                let Ok(line) = line else { break };
                if let Ok(value) = serde_json::from_slice::<Value>(&line)
                    && sender.send(value).is_err()
                {
                    break;
                }
            }
        });
        Some(Self {
            _probe: probe,
            stdin,
            receiver,
            deadline: Instant::now() + PROBE_TIMEOUT,
        })
    }

    fn send(&mut self, value: &Value) -> Option<()> {
        writeln!(self.stdin, "{value}").ok()
    }

    fn next(&mut self) -> Option<Value> {
        self.receiver
            .recv_timeout(self.deadline.checked_duration_since(Instant::now())?)
            .ok()
    }
}

fn claude_limits(mut command: std::process::Command, now: i64, zone: Tz) -> Option<Value> {
    command.args([
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--strict-mcp-config",
        "--mcp-config",
        "{\"mcpServers\":{}}",
    ]);
    let mut session = ProbeSession::start(command)?;
    let failed = || unavailable_account("claude", "probeFailed", None, now);
    let control = |id: &str, subtype: &str| json!({"type":"control_request", "request_id":id, "request":{"subtype":subtype}});
    if session
        .send(&control("xd-usage-init", "initialize"))
        .is_none()
    {
        return Some(failed());
    }
    let mut identity = None;
    let mut plan_name = None;
    loop {
        let Some(value) = session.next() else {
            return Some(failed());
        };
        if value["type"] != "control_response" {
            continue;
        }
        let response = &value["response"];
        if response["request_id"] == "xd-usage-init" {
            let signed_in = &response["response"]["account"];
            identity = signed_in["email"].as_str().map(str::to_owned);
            plan_name = signed_in["subscriptionType"].as_str().map(str::to_owned);
            if session.send(&control("xd-usage", "get_usage")).is_none() {
                return Some(failed());
            }
        } else if response["request_id"] == "xd-usage" {
            if response["subtype"] == "error" {
                return Some(failed());
            }
            let mut account = claude_account(
                &response["response"],
                identity.as_deref(),
                plan_name.as_deref(),
                now,
                zone,
            );
            account["checked_at_ms"] = json!(now_ms());
            // Read by the host's account list and removed before it is sent.
            account["email"] = json!(identity);
            return Some(account);
        }
    }
}

/// The plan as Claude Code names it. Its sign-in reports the account's own
/// plan name, such as "Claude Ultra", which can be a tier the usage reply only
/// calls `max`. The usage reply's tier is used when the sign-in names none.
fn claude_plan(plan_name: Option<&str>, tier: Option<&str>) -> Option<String> {
    plan_name
        .map(str::trim)
        .map(|name| name.strip_prefix("Claude ").unwrap_or(name).trim())
        .filter(|name| !name.is_empty() && *name != "API")
        .map(str::to_owned)
        .or_else(|| tier.map(title_case))
}

fn claude_account(
    response: &Value,
    identity: Option<&str>,
    plan_name: Option<&str>,
    now: i64,
    zone: Tz,
) -> Value {
    if response["rate_limits_available"] != true || !response["rate_limits"].is_object() {
        let reason = if response["rate_limits_available"] == true {
            "probeFailed"
        } else {
            "unsupported"
        };
        return unavailable_account("claude", reason, None, now);
    }
    let windows = claude_windows(&response["rate_limits"]);
    json!({
        "provider": "claude",
        "account_key": account_key("claude", identity),
        "plan": claude_plan(plan_name, response["subscription_type"].as_str()),
        "checked_at_ms": now,
        "windows": windows_json(&windows, now, zone),
    })
}

fn codex_session(mut command: std::process::Command) -> Option<ProbeSession> {
    command.arg("app-server");
    let mut session = ProbeSession::start(command)?;
    session.send(&crate::codex_app::CodexAppParser::initialize())?;
    loop {
        let value = session.next()?;
        if value["id"] == 1 {
            if value.get("error").is_some() {
                return None;
            }
            session.send(&json!({"method":"initialized"}))?;
            return Some(session);
        }
    }
}

fn codex_limits(command: std::process::Command, now: i64, zone: Tz) -> Option<Value> {
    let program_missing = which_missing(&command);
    if program_missing {
        return None;
    }
    let failed =
        |message: &str| unavailable_account("codex", "probeFailed", Some(message.to_owned()), now);
    let Some(mut session) = codex_session(command) else {
        return Some(failed("Codex could not be started to read usage."));
    };
    let request = json!({"id":2, "method":"account/rateLimits/read", "params":null});
    let account = json!({"id":3, "method":"account/read", "params":{}});
    if session.send(&request).is_none() || session.send(&account).is_none() {
        return Some(failed("Codex exited before it could report usage."));
    }
    let mut limits = None;
    let mut profile = None;
    let mut read_at = now;
    while limits.is_none() || profile.is_none() {
        let Some(value) = session.next() else {
            break;
        };
        if value["id"] == 2 {
            if let Some(code) = value["error"]["code"].as_i64() {
                return Some(failed(&format!(
                    "Codex could not read usage (JSON-RPC {code})."
                )));
            }
            limits = Some(value["result"].clone());
            read_at = now_ms();
        } else if value["id"] == 3 {
            profile = Some(value["result"].clone());
        }
    }
    let Some(limits) = limits else {
        return Some(failed("Codex did not answer the usage request."));
    };
    let profile = profile.unwrap_or(Value::Null);
    let mut account = codex_account(&limits, &profile, now, zone);
    account["checked_at_ms"] = json!(read_at);
    // Read by the host's account list and removed before it is sent.
    account["email"] = json!(profile["account"]["email"].as_str());
    Some(account)
}

fn which_missing(command: &std::process::Command) -> bool {
    let program = Path::new(command.get_program());
    if program.components().count() > 1 {
        return !program.exists();
    }
    let path = crate::user_environment::values()
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    !std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
}

/// Who Codex is signed in as: the address and, when Codex names one, the
/// workspace. A Codex session belongs to the workspace that wrote it, and one
/// address can belong to several.
fn codex_login(account: &Value) -> Option<String> {
    let email = account["email"].as_str()?;
    Some(
        match account["workspaceName"]
            .as_str()
            .filter(|workspace| !workspace.trim().is_empty())
        {
            Some(workspace) => format!("{email}\u{1f}{workspace}"),
            None => email.to_owned(),
        },
    )
}

fn codex_identity(limits: &Value, profile: &Value) -> Option<String> {
    codex_login(&profile["account"]).or_else(|| limits["accountId"].as_str().map(str::to_owned))
}

fn codex_account(limits: &Value, profile: &Value, now: i64, zone: Tz) -> Value {
    let snapshot = limits["rateLimitsByLimitId"]
        .get("codex")
        .filter(|snapshot| snapshot.is_object())
        .unwrap_or(&limits["rateLimits"]);
    let windows = codex_windows(snapshot);
    let account = &profile["account"];
    let plan = account["planType"]
        .as_str()
        .or(snapshot["planType"].as_str());
    json!({
        "provider": "codex",
        "account_key": account_key("codex", codex_identity(limits, profile).as_deref()),
        "plan": plan.map(codex_plan),
        "checked_at_ms": now,
        "windows": windows_json(&windows, now, zone),
        "reset_credits": codex_reset_credits(&limits["rateLimitResetCredits"]),
    })
}

/// The login can change between showing a credit and spending it, so the
/// account the user confirmed is checked again immediately before spending.
fn verify_codex_account(session: &mut ProbeSession, expected: &str) -> Result<(), String> {
    let changed =
        || "The signed-in Codex account changed. Refresh limits and try again.".to_owned();
    let unread = || "Could not confirm the signed-in Codex account.".to_owned();
    session
        .send(&json!({"id":3, "method":"account/read", "params":{}}))
        .ok_or_else(unread)?;
    session
        .send(&json!({"id":4, "method":"account/rateLimits/read", "params":null}))
        .ok_or_else(unread)?;
    let (mut profile, mut limits) = (None, None);
    while profile.is_none() || limits.is_none() {
        let value = session.next().ok_or_else(unread)?;
        if value["id"] == 3 {
            profile = Some(value["result"].clone());
        } else if value["id"] == 4 {
            limits = Some(value["result"].clone());
        }
    }
    let (profile, limits) = (profile.unwrap_or_default(), limits.unwrap_or_default());
    let key = account_key("codex", codex_identity(&limits, &profile).as_deref());
    if key.as_deref() == Some(expected) {
        Ok(())
    } else {
        Err(changed())
    }
}

/// Spends a banked credit. The request id makes a retried attempt redeem once.
fn consume_codex_reset_credit(
    command: std::process::Command,
    idempotency_key: &str,
    expected_account: Option<&str>,
) -> Result<String, String> {
    let mut session = codex_session(command)
        .ok_or_else(|| "Codex could not be started to use the reset credit.".to_owned())?;
    session.deadline = Instant::now() + Duration::from_secs(20);
    if let Some(expected) = expected_account {
        verify_codex_account(&mut session, expected)?;
    }
    session
        .send(&json!({
            "id": 2,
            "method": "account/rateLimitResetCredit/consume",
            "params": {"idempotencyKey": idempotency_key},
        }))
        .ok_or_else(|| "Codex exited before it could use the reset credit.".to_owned())?;
    loop {
        let value = session
            .next()
            .ok_or_else(|| "Codex did not answer the reset request.".to_owned())?;
        if value["id"] != 2 {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(error["message"]
                .as_str()
                .unwrap_or("Could not use the reset credit.")
                .to_owned());
        }
        return value["result"]["outcome"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "Codex returned an unknown reset outcome.".to_owned());
    }
}

/// Reads one account's limits by running its CLI as that account. `None`
/// means the CLI is not installed.
pub(crate) fn probe_account(
    provider: &str,
    environment: &[(String, String)],
    now: i64,
    zone: Tz,
) -> Option<Value> {
    let program = if provider == "claude" {
        crate::agent::resolve_claude()
    } else {
        crate::agent::resolve_codex()
    };
    let mut command = crate::background_process::command(program);
    command.envs(environment.iter().map(|(name, value)| (name, value)));
    if provider == "claude" {
        if which_missing(&command) {
            None
        } else {
            claude_limits(command, now, zone)
        }
    } else {
        codex_limits(command, now, zone)
    }
}

/// Who a probe found signed in and how much of each limit is used, or `None`
/// when the probe could not read the account.
pub(crate) fn probe_reading(account: &Value) -> Option<(Identity, Vec<Headroom>)> {
    if account
        .get("unavailable")
        .is_some_and(|value| !value.is_null())
    {
        return None;
    }
    let text = |name: &str| account[name].as_str().map(str::to_owned);
    let windows = account["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|window| {
            Some(Headroom {
                kind: window["kind"].as_str()?.to_owned(),
                label: window["label"].as_str().unwrap_or_default().to_owned(),
                used_percent: window["used_percent"].as_f64()?,
                resets_at_ms: window["resets_at_ms"].as_i64(),
            })
        })
        .collect();
    Some((
        Identity {
            email: text("email"),
            plan: text("plan"),
            account_key: text("account_key"),
        },
        windows,
    ))
}

/// Reads which login Codex is signed in to, and nothing else. Local and quick,
/// unlike a usage probe, which also asks the service for limits.
pub(crate) fn read_codex_login(environment: &[(String, String)]) -> Option<Identity> {
    let mut command = crate::background_process::command(crate::agent::resolve_codex());
    command.envs(environment.iter().map(|(name, value)| (name, value)));
    if which_missing(&command) {
        return None;
    }
    let mut session = codex_session(command)?;
    session.deadline = Instant::now() + Duration::from_secs(5);
    session.send(&json!({"id":3, "method":"account/read", "params":{}}))?;
    loop {
        let value = session.next()?;
        if value["id"] != 3 {
            continue;
        }
        let account = &value["result"]["account"];
        let email = account["email"].as_str()?;
        return Some(Identity {
            email: Some(email.to_owned()),
            plan: account["planType"].as_str().map(codex_plan),
            account_key: account_key("codex", codex_login(account).as_deref()),
        });
    }
}

/// Probes an account and records what it found. Returns who is signed in.
pub(crate) fn check_account(accounts: &Accounts, provider: &str, id: &str) -> Option<Identity> {
    let now = now_ms();
    let _lease = accounts.lease(provider, id)?;
    let ticket = accounts.ticket();
    let environment = accounts.environment(provider, id);
    let account = probe_account(provider, &environment, now, Tz::UTC)?;
    let (identity, windows) = probe_reading(&account)?;
    // Filed by when the reading began, which orders it among other readings.
    accounts.record_probe_from(ticket, provider, id, &identity, windows, now);
    Some(identity)
}

/// Answers the Usage page's reads off the connection thread: a cold scan or a
/// provider probe can take seconds, and each answer arrives as an event.
pub(crate) struct UsageService {
    events: Arc<EventBus>,
    accounts: Accounts,
    shared: Arc<Shared>,
    summaries: Coalescer,
    limits: Coalescer,
}

type UsageJob = Box<dyn FnOnce() + Send>;

/// Runs one read at a time per connection and keeps only the newest waiting.
/// Clients ignore answers to anything but their latest request, so clicking
/// through windows cannot queue up scans or spawn a pile of CLI probes.
struct Coalescer {
    name: &'static str,
    slots: Arc<Mutex<HashMap<u64, Option<UsageJob>>>>,
}

impl Coalescer {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            slots: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn submit(&self, owner: u64, job: UsageJob) -> Result<(), String> {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(waiting) = slots.get_mut(&owner) {
            *waiting = Some(job);
            return Ok(());
        }
        slots.insert(owner, None);
        drop(slots);
        let worker_slots = self.slots.clone();
        let spawned = thread::Builder::new()
            .name(self.name.into())
            .spawn(move || {
                let mut job = job;
                loop {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    let mut slots = worker_slots
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    match slots.get_mut(&owner).and_then(Option::take) {
                        Some(next) => job = next,
                        None => {
                            slots.remove(&owner);
                            return;
                        }
                    }
                }
            });
        if let Err(error) = spawned {
            self.slots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&owner);
            return Err(format!("Cannot read usage: {error}"));
        }
        Ok(())
    }
}

fn fetch_rates_document(agent: &ureq::Agent, url: &str, accept: &str) -> Option<Value> {
    let mut response = agent.get(url).header("Accept", accept).call().ok()?;
    let body = response
        .body_mut()
        .with_config()
        .limit(RATES_DOCUMENT_LIMIT)
        .read_to_vec()
        .ok()?;
    serde_json::from_slice(&body).ok()
}

/// Always an answer: a scan that dies must still end the asker's loading state.
fn summary_event(generation: Value, read: impl FnOnce() -> Value) -> Value {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)) {
        Ok(summary) => {
            json!({"event":"usage-summary", "generation":generation, "summary":summary})
        }
        Err(_) => json!({
            "event": "usage-summary",
            "generation": generation,
            "error": "The host could not read usage.",
        }),
    }
}

impl UsageService {
    pub(crate) fn new(events: Arc<EventBus>, data_directory: Option<PathBuf>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .user_agent("xd/0.1")
            .build()
            .into();
        let fetch: Arc<RatesFetcher> =
            Arc::new(move |url, accept| fetch_rates_document(&agent, url, accept));
        Self::with_parts(events, data_directory, TranscriptRoots::discover(), fetch)
    }

    fn with_parts(
        events: Arc<EventBus>,
        data_directory: Option<PathBuf>,
        roots: TranscriptRoots,
        fetch_rates: Arc<RatesFetcher>,
    ) -> Self {
        Self {
            events,
            accounts: Accounts::new(None),
            summaries: Coalescer::new("xd-usage-scan"),
            limits: Coalescer::new("xd-usage-limits"),
            shared: Arc::new(Shared {
                roots,
                scan: Mutex::new(ScanCache::default()),
                rates: Mutex::new(Rates {
                    table: Arc::new(RateTable::new()),
                    fetched_at_ms: None,
                    last_failed_at_ms: None,
                    status: "unavailable",
                }),
                prices: Mutex::new(()),
                fetch_rates,
                data_directory,
            }),
        }
    }

    /// Reads limits for each of these accounts instead of the user's own only.
    pub(crate) fn with_accounts(mut self, accounts: Accounts) -> Self {
        self.accounts = accounts;
        self
    }

    fn spawn(&self, name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), String> {
        thread::Builder::new()
            .name(name.into())
            .spawn(work)
            .map(|_| ())
            .map_err(|error| format!("Cannot read usage: {error}"))
    }

    pub(crate) fn summary(&self, owner: u64, request: &Value) -> Result<Value, String> {
        let days = request["days"].as_u64().unwrap_or(30);
        if !matches!(days, 1 | 7 | 30 | 90) {
            return Err("Usage windows are 1, 7, 30, or 90 days.".into());
        }
        let zone = parse_zone(request);
        let refresh = request["refresh"] == true;
        let generation = request["generation"].clone();
        let read_at = request["now_ms"].as_i64().unwrap_or_else(now_ms);
        let shared = self.shared.clone();
        let events = self.events.clone();
        self.summaries.submit(
            owner,
            Box::new(move || {
                events.publish_to(
                    owner,
                    summary_event(generation, || {
                        shared.summary(days as u32, zone, refresh, read_at)
                    }),
                );
            }),
        )?;
        Ok(json!({"ok": true, "pending": true}))
    }

    pub(crate) fn limits(&self, owner: u64, request: &Value) -> Result<Value, String> {
        let zone = parse_zone(request);
        let generation = request["generation"].clone();
        let events = self.events.clone();
        let known = self.accounts.clone();
        self.limits.submit(owner, Box::new(move || {
            let now = now_ms();
            // Every account of every assistant, read at once.
            let probes: Vec<_> = PROVIDERS
                .iter()
                .flat_map(|provider| {
                    let ids = known.ids(provider);
                    let several = ids.len() > 1;
                    ids.into_iter()
                        .enumerate()
                        .map(move |(index, id)| (*provider, id, several.then_some(index + 1)))
                })
                .map(|(provider, id, rank)| {
                    let lease = known.lease(provider, &id);
                    let ticket = known.ticket();
                    let environment = known.environment(provider, &id);
                    let probe = thread::spawn(move || {
                        let _lease = lease?;
                        probe_account(provider, &environment, now, zone)
                    });
                    (provider, id, rank, ticket, probe)
                })
                .collect();
            let mut accounts = Vec::new();
            for (provider, id, rank, ticket, probe) in probes {
                let Some(mut account) = probe.join().ok().flatten() else {
                    continue;
                };
                // Its place in the host's account list, matching Settings.
                if let Some(rank) = rank {
                    account["account_rank"] = json!(rank);
                }
                if let Some((identity, windows)) = probe_reading(&account) {
                    known.record_probe_from(ticket, provider, &id, &identity, windows, now);
                }
                // `account_key` is a hash, so no address crosses the wire.
                if let Some(fields) = account.as_object_mut() {
                    fields.remove("email");
                }
                // Two accounts can read as one when a login was repeated.
                if account["account_key"].is_string()
                    && accounts.iter().any(|seen: &Value| {
                        seen["provider"] == account["provider"]
                            && seen["account_key"] == account["account_key"]
                    })
                {
                    continue;
                }
                accounts.push(account);
            }
            events.publish_to(
                owner,
                json!({"event":"usage-limits", "generation":generation, "accounts":accounts, "host_now_ms":now_ms()}),
            );
        }))?;
        Ok(json!({"ok": true, "pending": true}))
    }

    pub(crate) fn consume_reset_credit(
        &self,
        owner: u64,
        request: &Value,
    ) -> Result<Value, String> {
        if request["provider"] != "codex" {
            return Err("Only Codex has reset credits.".into());
        }
        let key = request["idempotency_key"]
            .as_str()
            .filter(|key| !key.trim().is_empty() && key.len() <= 128)
            .ok_or_else(|| "A reset needs an idempotency key.".to_owned())?
            .to_owned();
        let expected = request["account_key"]
            .as_str()
            .filter(|account| !account.is_empty())
            .ok_or_else(|| "A reset needs the account it was confirmed for.".to_owned())?
            .to_owned();
        let events = self.events.clone();
        let account = self.accounts.with_key("codex", &expected);
        let lease = match &account {
            Some(id) => Some(
                self.accounts
                    .lease("codex", id)
                    .ok_or_else(|| "That account was removed.".to_owned())?,
            ),
            None => None,
        };
        let environment = account
            .map(|id| self.accounts.environment("codex", &id))
            .unwrap_or_default();
        self.spawn("xd-usage-reset", move || {
            let _lease = lease;
            let mut command = crate::background_process::command(crate::agent::resolve_codex());
            command.envs(environment.iter().map(|(name, value)| (name, value)));
            let event = match consume_codex_reset_credit(command, &key, Some(&expected)) {
                Ok(outcome) => {
                    json!({"event":"usage-reset-credit", "provider":"codex", "outcome":outcome, "idempotency_key":key})
                }
                // The key names the attempt even on failure, so a client
                // waiting on a newer attempt cannot take this answer for its own.
                Err(error) => {
                    json!({"event":"usage-reset-credit", "provider":"codex", "error":error, "idempotency_key":key})
                }
            };
            events.publish_to(owner, event);
        })?;
        Ok(json!({"ok": true, "pending": true}))
    }

    pub(crate) fn prices(&self) -> Result<Value, String> {
        let _guard = self
            .shared
            .prices
            .lock()
            .map_err(|_| "Usage prices are unavailable.")?;
        let settings = self.shared.read_prices();
        let mut reply = settings.to_value();
        reply["ok"] = json!(true);
        // Tells clients that leaving `aliases` out of set-usage-prices keeps them.
        reply["aliases_optional"] = json!(true);
        Ok(reply)
    }

    pub(crate) fn set_prices(&self, request: &Value) -> Result<Value, String> {
        let mut settings = PriceSettings::from_value(request)?;
        let directory = self
            .shared
            .data_directory
            .as_ref()
            .ok_or_else(|| "Usage prices need a data directory.".to_owned())?;
        let _guard = self
            .shared
            .prices
            .lock()
            .map_err(|_| "Usage prices are unavailable.")?;
        // A client editing only prices leaves the model mappings as they are
        // now, read under the same lock, so it cannot revert another client's.
        if request.get("aliases").is_none() {
            settings.aliases = self.shared.read_prices().aliases;
        }
        write_private_atomic(&directory.join(PRICES_FILE), &settings.to_value())?;
        let mut reply = settings.to_value();
        reply["ok"] = json!(true);
        reply["aliases_optional"] = json!(true);
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(value: Value) -> Vec<UsageRecord> {
        parse_claude_record(&value)
    }

    fn assistant(id: &str, model: &str) -> Value {
        json!({
            "type":"assistant", "timestamp":"2026-11-05T12:00:00Z", "requestId":"req",
            "sessionId":"s1",
            "message":{"id":id, "model":model, "usage":{
                "input_tokens":10, "cache_read_input_tokens":100,
                "cache_creation_input_tokens":5, "output_tokens":20,
            }},
        })
    }

    #[test]
    fn one_address_in_two_codex_workspaces_is_two_logins() {
        let personal = json!({"email":"a@example.com","planType":"pro"});
        let work = json!({"email":"a@example.com","planType":"team","workspaceName":"Acme"});
        let key = |account: &Value| account_key("codex", codex_login(account).as_deref());
        assert_ne!(key(&personal), key(&work));
        // A usage probe and a login read file the same login under one key.
        let probed = codex_account(&json!({}), &json!({"account": work}), 0, Tz::UTC);
        assert_eq!(probed["account_key"].as_str(), key(&work).as_deref());
    }

    #[test]
    fn claude_records_ignore_synthetic_and_keep_advisors() {
        assert!(line(assistant("m", CLAUDE_SYNTHETIC_MODEL)).is_empty());
        assert!(line(json!({"type":"user","message":{"usage":{}}})).is_empty());
        let mut value = assistant("m", "claude-opus-5-5");
        value["costUSD"] = json!(0.5);
        value["message"]["usage"]["speed"] = json!("fast");
        value["message"]["usage"]["iterations"] = json!([
            {"type":"message", "model":"ignored"},
            {"type":"advisor_message", "model":"claude-fable-5-1", "input_tokens":7, "output_tokens":3},
        ]);
        let records = line(value);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].totals.uncached_input, 10);
        assert_eq!(records[0].totals.cached_input, 100);
        assert_eq!(records[0].reported_cost_usd, Some(0.5));
        assert_eq!(records[0].speed, Speed::Fast);
        assert_eq!(records[0].dedupe_key.as_deref(), Some("m:req"));
        assert_eq!(&*records[1].model, "claude-fable-5-1");
        assert_eq!(records[1].reported_cost_usd, None);
        assert_eq!(records[1].dedupe_key.as_deref(), Some("m:req:advisor:1"));
    }

    fn usage(input: i64, cached: i64, output: i64, reasoning: i64) -> Value {
        json!({
            "input_tokens":input, "cached_input_tokens":cached, "output_tokens":output,
            "reasoning_output_tokens":reasoning, "total_tokens":input + output,
        })
    }

    fn token_count_line(at: &str, last: Value, total: Value) -> Value {
        json!({"timestamp":at, "type":"event_msg", "payload":{"type":"token_count", "info":{
            "last_token_usage":last, "total_token_usage":total,
        }}})
    }

    fn codex_state() -> CodexScanState {
        let mut state = CodexScanState::default();
        parse_codex_record(
            &json!({"timestamp":"2026-11-05T12:00:00Z","type":"session_meta","payload":{"id":"thread"}}),
            &mut state,
        );
        parse_codex_record(
            &json!({"timestamp":"2026-11-05T12:00:00Z","type":"turn_context","payload":{"model":"gpt-5.5"}}),
            &mut state,
        );
        state
    }

    #[test]
    fn a_grown_transcript_is_read_on_from_where_it_left_off() {
        let directory = temporary_directory("incremental");
        let path = directory.join("rollout.jsonl");
        let line = |value: Value| format!("{value}\n");
        let lines = [
            line(
                json!({"timestamp":"2026-11-05T12:00:00Z","type":"session_meta","payload":{"id":"thread"}}),
            ),
            line(
                json!({"timestamp":"2026-11-05T12:00:00Z","type":"turn_context","payload":{"model":"gpt-5.5"}}),
            ),
            line(token_count_line(
                "2026-11-05T12:00:01Z",
                usage(100, 40, 10, 2),
                usage(100, 40, 10, 2),
            )),
            line(token_count_line(
                "2026-11-05T12:00:02Z",
                usage(50, 0, 5, 0),
                usage(150, 40, 15, 2),
            )),
            // Inconsistent with the line before: one malformed record.
            line(token_count_line(
                "2026-11-05T12:00:03Z",
                usage(5, 0, 1, 0),
                usage(500, 40, 90, 2),
            )),
            line(token_count_line(
                "2026-11-05T12:00:04Z",
                usage(7, 0, 1, 0),
                usage(507, 40, 91, 2),
            )),
        ];
        let full = |text: &str| {
            fs::write(&path, text).unwrap();
            read_transcript_from("codex", &path, 0, None).unwrap()
        };
        let append = |text: &str| {
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(text.as_bytes()).unwrap();
        };
        let same = |left: &CachedFile, right: &CachedFile| {
            assert_eq!(left.records, right.records);
            assert_eq!(left.malformed, right.malformed);
            assert_eq!(left.resume.offset, right.resume.offset);
        };
        let expected = |count: usize, partial: &str| {
            let mut text = lines[..count].concat();
            text.push_str(partial);
            full(&text)
        };

        let first = full(&lines[..3].concat());
        assert_eq!(first.records.len(), 1);
        // The next line arrives with half of the one after it.
        let (half, rest) = lines[4].split_at(20);
        append(&format!("{}{half}", lines[3]));
        let grown = read_transcript_from("codex", &path, 0, Some(&first.resume)).unwrap();
        same(&grown, &expected(4, half));
        append(&format!("{rest}{}", lines[5]));
        let finished = read_transcript_from("codex", &path, 0, Some(&grown.resume)).unwrap();
        same(&finished, &expected(6, ""));
        assert_eq!(finished.malformed, 1);

        // Rewritten rather than appended to: read again from the start.
        let rewritten = format!("{}{}", lines[..2].concat(), lines[3]).repeat(4);
        assert!(rewritten.len() as u64 > finished.size);
        let read = {
            fs::write(&path, &rewritten).unwrap();
            read_transcript_from("codex", &path, 0, Some(&finished.resume)).unwrap()
        };
        same(&read, &full(&rewritten));
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn codex_bills_each_request_once_and_splits_cached_input() {
        let mut state = codex_state();
        let first = token_count_line(
            "2026-11-05T12:00:01Z",
            usage(100, 40, 10, 2),
            usage(100, 40, 10, 2),
        );
        let records = parse_codex_record(&first, &mut state);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].totals.uncached_input, 60);
        assert_eq!(records[0].totals.cached_input, 40);
        assert_eq!(records[0].totals.reasoning, 2);
        assert_eq!(&*records[0].session_id, "thread");
        // A replayed event repeats cumulative counters and bills nothing.
        assert!(parse_codex_record(&first, &mut state).is_empty());
        let second = token_count_line(
            "2026-11-05T12:00:02Z",
            usage(50, 0, 5, 0),
            usage(150, 40, 15, 2),
        );
        assert_eq!(parse_codex_record(&second, &mut state).len(), 1);
    }

    #[test]
    fn codex_needs_a_model_and_counts_inconsistent_counters_as_malformed() {
        let mut state = CodexScanState::default();
        let event = token_count_line("2026-11-05T12:00:01Z", usage(1, 0, 1, 0), usage(1, 0, 1, 0));
        assert!(parse_codex_record(&event, &mut state).is_empty());
        let mut state = codex_state();
        parse_codex_record(
            &token_count_line(
                "2026-11-05T12:00:01Z",
                usage(10, 0, 1, 0),
                usage(10, 0, 1, 0),
            ),
            &mut state,
        );
        // The delta does not match the cumulative jump and nothing explains it.
        let records = parse_codex_record(
            &token_count_line(
                "2026-11-05T12:00:02Z",
                usage(5, 0, 1, 0),
                usage(50, 0, 9, 0),
            ),
            &mut state,
        );
        assert!(records.is_empty());
        assert_eq!(state.malformed, 1);
    }

    #[test]
    fn codex_bridges_a_gap_only_when_usage_records_explain_it() {
        let mut state = codex_state();
        parse_codex_record(
            &token_count_line(
                "2026-11-05T12:00:01Z",
                usage(10, 0, 1, 0),
                usage(10, 0, 1, 0),
            ),
            &mut state,
        );
        for (at, input) in [("2026-11-05T12:00:02Z", 20), ("2026-11-05T12:00:03Z", 30)] {
            parse_codex_record(
                &json!({"timestamp":at, "type":"token_usage_record", "payload":{"usage":usage(input, 0, 2, 0)}}),
                &mut state,
            );
        }
        let records = parse_codex_record(
            &token_count_line(
                "2026-11-05T12:00:04Z",
                usage(30, 0, 2, 0),
                usage(60, 0, 5, 0),
            ),
            &mut state,
        );
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].totals.uncached_input, 20);
        assert_eq!(records[1].totals.uncached_input, 30);
    }

    #[test]
    fn codex_skips_copied_fork_history_then_bills_new_work() {
        let mut state = CodexScanState::default();
        parse_codex_record(
            &json!({"timestamp":"2026-11-05T12:00:00.000Z","type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}}),
            &mut state,
        );
        parse_codex_record(
            &json!({"timestamp":"2026-11-05T12:00:00.001Z","type":"turn_context","payload":{"model":"gpt-5.5"}}),
            &mut state,
        );
        let copied = token_count_line(
            "2026-11-05T12:00:00.010Z",
            usage(100, 0, 10, 0),
            usage(100, 0, 10, 0),
        );
        assert!(parse_codex_record(&copied, &mut state).is_empty());
        let fresh = token_count_line(
            "2026-11-05T12:00:30Z",
            usage(5, 0, 1, 0),
            usage(105, 0, 11, 0),
        );
        let records = parse_codex_record(&fresh, &mut state);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].totals.uncached_input, 5);
    }

    #[test]
    fn codex_service_tier_sets_speed_and_context_size_events_bill_nothing() {
        let mut state = codex_state();
        parse_codex_record(
            &json!({"timestamp":"2026-11-05T12:00:00Z","type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"service_tier":"priority"}}}),
            &mut state,
        );
        let zero = token_count_line(
            "2026-11-05T12:00:01Z",
            json!({"input_tokens":0, "output_tokens":0, "total_tokens":9000}),
            usage(0, 0, 0, 0),
        );
        assert!(parse_codex_record(&zero, &mut state).is_empty());
        let records = parse_codex_record(
            &token_count_line("2026-11-05T12:00:02Z", usage(1, 0, 1, 0), usage(1, 0, 1, 0)),
            &mut state,
        );
        assert_eq!(records[0].speed, Speed::Fast);
    }

    fn rates_document() -> Value {
        json!({
            "claude-opus-5-5": {
                "input_cost_per_token": 0.000005, "output_cost_per_token": 0.000025,
                "cache_read_input_token_cost": 0.0000005, "cache_creation_input_token_cost": 0.00000625,
                "provider_specific_entry": {"fast": 6.0},
            },
            "openai/gpt-5.5": {
                "input_cost_per_token": 0.000002, "output_cost_per_token": 0.000008,
                "cache_read_input_token_cost": 0.0000002,
                "input_cost_per_token_priority": 0.000004, "output_cost_per_token_priority": 0.000016,
            },
            "reseller/gpt-5.5": {"input_cost_per_token": 0.5, "output_cost_per_token": 0.5},
            "no-rates": {"mode": "chat"},
        })
    }

    fn record(model: &str, totals: Totals, speed: Speed, reported: Option<f64>) -> UsageRecord {
        UsageRecord {
            provider: "claude",
            timestamp_ms: 0,
            model: model.into(),
            session_id: "s".into(),
            totals,
            reported_cost_usd: reported,
            speed,
            dedupe_key: None,
        }
    }

    const MILLION: Totals = Totals {
        uncached_input: 1_000_000,
        cached_input: 1_000_000,
        cache_creation: 0,
        output: 1_000_000,
        reasoning: 0,
    };

    #[test]
    fn rate_table_prefers_first_party_aliases_and_drops_unpriced_entries() {
        let table = parse_rate_table(&rates_document());
        assert!(!table.contains_key("no-rates"));
        assert_eq!(table["gpt-5.5"].standard.input, 0.000002);
        let fast = table["gpt-5.5"].fast.unwrap();
        assert_eq!(fast.input, 0.000004);
        // A tier without a cache read rate keeps the standard tier's ratio.
        assert!((fast.cache_read - 0.0000004).abs() < 1e-12);
        assert_eq!(
            table["claude-opus-5-5"].fast.unwrap().output,
            0.000025 * 6.0
        );
    }

    #[test]
    fn pricing_splits_categories_and_charges_speed_premium() {
        let table = parse_rate_table(&rates_document());
        let overrides = RateTable::new();
        let standard = price_usage(
            &table,
            &overrides,
            &record("claude-opus-5-5[1m]", MILLION, Speed::Standard, None),
        );
        assert_eq!(standard.cost_source, CostSource::ModelPriced);
        assert!((standard.cost_usd - 30.5).abs() < 1e-9);
        let fast = price_usage(
            &table,
            &overrides,
            &record("claude-opus-5-5", MILLION, Speed::Fast, None),
        );
        assert!((fast.cost_usd - 183.0).abs() < 1e-9);
        assert!((fast.speed_premium_usd - 152.5).abs() < 1e-9);
        // Reported cost wins and scales the category split to match.
        let reported = price_usage(
            &table,
            &overrides,
            &record("claude-opus-5-5", MILLION, Speed::Standard, Some(61.0)),
        );
        assert_eq!(reported.cost_source, CostSource::ProviderReported);
        assert!((reported.category.unwrap().output - 50.0).abs() < 1e-9);
        // Bare family names are ambiguous and stay unpriced.
        let unknown = price_usage(
            &table,
            &overrides,
            &record("opus", MILLION, Speed::Standard, None),
        );
        assert_eq!(unknown.cost_source, CostSource::Unpriced);
        let saved = cache_savings_usd(
            &table,
            &overrides,
            &record("claude-opus-5-5", MILLION, Speed::Standard, None),
        );
        assert!((saved - 4.5).abs() < 1e-9);
    }

    #[test]
    fn price_overrides_win_over_reported_cost_and_blank_cache_uses_input() {
        let settings = PriceSettings::from_value(&json!({
            "prices": {"local-model": {"input": 2.0, "output": 4.0}},
            "aliases": {"a": "b", "b": "c", "x": "y", "y": "x"},
        }))
        .unwrap();
        let table = RateTable::new();
        let priced = price_usage(
            &table,
            &settings.override_table(),
            &record("local-model", MILLION, Speed::Standard, Some(999.0)),
        );
        assert_eq!(priced.cost_source, CostSource::ModelPriced);
        assert!((priced.cost_usd - 8.0).abs() < 1e-9);
        let aliases = settings.resolved_aliases();
        assert_eq!(aliases["a"], "c");
        assert_eq!(aliases["b"], "c");
        assert!(!aliases.contains_key("x"), "a cycle maps nothing");
        assert!(PriceSettings::from_value(&json!({"prices":{"m":{"input":1}}})).is_err());
        assert!(
            PriceSettings::from_value(&json!({"prices":{"m":{"input":-1,"output":1}}})).is_err()
        );
        assert_eq!(
            PriceSettings::from_value(&settings.to_value()).unwrap(),
            settings
        );
    }

    fn scan_of(records: Vec<UsageRecord>) -> SourceScan {
        SourceScan {
            provider: "claude",
            records: vec![Arc::new(records)],
            fingerprint: Value::Null,
            status: "ok",
            scanned_files: 1,
            skipped_files: 0,
            malformed_records: 0,
            message: None,
        }
    }

    #[test]
    fn aggregate_dedupes_applies_aliases_and_bounds_the_window() {
        let at = |text: &str| {
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .timestamp_millis()
        };
        let now = at("2026-11-07T15:30:00Z");
        let window = UsageWindow::new(7, Tz::UTC, now);
        let keep = |model: &str, when: &str, key: &str| UsageRecord {
            timestamp_ms: at(when),
            dedupe_key: Some(key.into()),
            ..record(model, MILLION, Speed::Standard, None)
        };
        let scans = [scan_of(vec![
            keep("claude-opus-5-5", "2026-11-07T01:00:00Z", "a"),
            keep("claude-opus-5-5", "2026-11-07T02:00:00Z", "a"),
            keep("old-name", "2026-11-06T02:00:00Z", "b"),
            keep("claude-opus-5-5", "2026-10-01T02:00:00Z", "c"),
        ])];
        let settings = PriceSettings {
            aliases: BTreeMap::from([("old-name".to_owned(), "claude-opus-5-5".to_owned())]),
            ..PriceSettings::default()
        };
        let (buckets, sessions) = aggregate(
            &scans,
            &window,
            &parse_rate_table(&rates_document()),
            &settings,
        );
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0]["day"], "2026-11-06");
        assert_eq!(buckets[0]["model"], "claude-opus-5-5");
        assert_eq!(buckets[1]["records"], 1, "the repeated key is counted once");
        assert_eq!(sessions["claude"], 1);
    }

    #[test]
    fn unpriced_tokens_are_counted_rather_than_estimated() {
        let at = |text: &str| {
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .timestamp_millis()
        };
        let window = UsageWindow::new(7, Tz::UTC, at("2026-11-07T15:30:00Z"));
        let when = at("2026-11-07T01:00:00Z");
        let tokens = |output| Totals {
            output,
            ..Totals::default()
        };
        let scans = [scan_of(vec![
            UsageRecord {
                timestamp_ms: when,
                ..record("unlisted", tokens(100), Speed::Standard, Some(1.0))
            },
            UsageRecord {
                timestamp_ms: when,
                ..record("unlisted", tokens(900), Speed::Standard, None)
            },
        ])];
        let (buckets, _) = aggregate(
            &scans,
            &window,
            &RateTable::new(),
            &PriceSettings::default(),
        );
        assert_eq!(buckets[0]["unpriced_records"], 1);
        assert_eq!(buckets[0]["unpriced_tokens"], 900);
    }

    #[test]
    fn hourly_windows_roll_and_days_follow_the_viewers_zone() {
        let at = |text: &str| {
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .timestamp_millis()
        };
        let zone: Tz = "America/Toronto".parse().unwrap();
        let now = at("2026-11-07T15:30:20Z");
        let hourly = UsageWindow::new(1, zone, now);
        let (since, until) = hourly.hourly.unwrap();
        assert_eq!(until - since, DAY_MS);
        assert_eq!(hourly.periods().len(), 24);
        assert_eq!(hourly.periods()[23]["tooltip"], "9 AM today");
        let scans = [scan_of(vec![UsageRecord {
            timestamp_ms: at("2026-11-07T02:00:00Z"),
            ..record("claude-opus-5-5", MILLION, Speed::Standard, None)
        }])];
        let (buckets, _) = aggregate(
            &scans,
            &hourly,
            &RateTable::new(),
            &PriceSettings::default(),
        );
        // 02:00 UTC is still the previous evening in Toronto.
        assert_eq!(buckets[0]["day"], "2026-11-06");
        assert_eq!(buckets[0]["cost_source"], "unpriced");
        let daily = UsageWindow::new(30, zone, now);
        assert_eq!(daily.periods().len(), 30);
        assert_eq!(daily.label(), "Oct 9 to Nov 7");
    }

    #[test]
    fn fall_back_hours_name_their_zone() {
        let zone: Tz = "America/Toronto".parse().unwrap();
        let first = DateTime::parse_from_rfc3339("2026-11-01T05:30:00Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(hour_label(first, zone), "1 AM EDT");
        assert_eq!(hour_label(first + HOUR_MS, zone), "1 AM EST");
        assert_eq!(hour_label(first + 2 * HOUR_MS, zone), "2 AM");
    }

    #[test]
    fn upcoming_labels_read_relative_to_today() {
        let at = |text: &str| {
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .timestamp_millis()
        };
        let now = at("2026-11-07T15:00:00Z");
        assert_eq!(
            upcoming_label(at("2026-11-07T17:05:00Z"), now, Tz::UTC),
            "5:05 PM"
        );
        assert_eq!(
            upcoming_label(at("2026-11-08T00:30:00Z"), now, Tz::UTC),
            "tomorrow at 12:30 AM"
        );
        assert_eq!(
            upcoming_label(at("2026-11-10T09:00:00Z"), now, Tz::UTC),
            "11/10 9:00 AM"
        );
        assert_eq!(
            upcoming_label(at("2027-01-02T09:00:00Z"), now, Tz::UTC),
            "1/2/2027 9:00 AM"
        );
    }

    #[test]
    fn line_reader_drops_only_oversized_lines() {
        let mut lines = Vec::new();
        let mut input = b"one\n".to_vec();
        input.extend(vec![b'x'; LINE_LIMIT + 10]);
        input.extend_from_slice(b"\ntwo\ntail");
        let complete = for_each_line(BufReader::with_capacity(1024, &input[..]), |line, _| {
            lines.push(String::from_utf8_lossy(line).into_owned());
        })
        .unwrap();
        assert_eq!(
            complete,
            input.len() as u64 - 4,
            "all but the unterminated tail"
        );
        assert_eq!(lines, ["one", "two", "tail"]);
    }

    #[test]
    fn claude_limits_cover_model_scoped_and_spend_windows() {
        let windows = claude_windows(&json!({
            "five_hour": null,
            "seven_day": {"utilization": 87, "resets_at": "2026-11-09T15:00:00+00:00"},
            "model_scoped": [{"display_name": "Fable 5", "utilization": 12.5, "resets_at": null}],
            "spend": {"enabled": true,
                "used": {"amount_minor": 500, "currency": "USD", "exponent": 2},
                "limit": {"amount_minor": 2000, "currency": "USD", "exponent": 2}},
        }));
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].id, "seven_day");
        assert_eq!(windows[0].window_duration_mins, Some(WEEK_MINS));
        assert_eq!(windows[1].id, "seven_day_fable_5");
        assert_eq!(windows[1].label, "Weekly · Fable 5");
        assert_eq!(windows[2].id, "monthly_spend");
        assert_eq!(windows[2].used_percent, 25.0);
        let extra = claude_spend_percent(&json!({"extra_usage": {
            "is_enabled": true, "used_credits": 1, "monthly_limit": 4,
        }}));
        assert_eq!(extra, Some(25.0));
        let account = claude_account(
            &json!({"rate_limits_available": false}),
            None,
            None,
            0,
            Tz::UTC,
        );
        assert_eq!(account["unavailable"]["reason"], "unsupported");
    }

    #[test]
    fn plans_use_the_names_each_cli_shows() {
        assert_eq!(
            claude_plan(Some("Claude Ultra"), Some("max")).as_deref(),
            Some("Ultra")
        );
        assert_eq!(
            claude_plan(Some("Claude Max"), Some("max")).as_deref(),
            Some("Max")
        );
        assert_eq!(claude_plan(None, Some("max")).as_deref(), Some("Max"));
        assert_eq!(
            claude_plan(Some("Claude API"), Some("pro")).as_deref(),
            Some("Pro")
        );
        assert_eq!(claude_plan(Some(" "), None), None);
        for (id, name) in [
            ("plus", "Plus"),
            ("pro", "Pro 200"),
            ("prolite", "Pro 100"),
            ("promax", "Pro 500"),
            ("team", "Business"),
            ("business", "Enterprise"),
            ("self_serve_business_prolite", "Business Premium"),
            ("enterprise_cbp_automation", "Enterprise (Automation)"),
            ("hc", "Enterprise"),
            ("education", "Edu"),
            ("sci_plus", "Academic Researchers Plus"),
            ("future_tier", "Future Tier"),
        ] {
            assert_eq!(codex_plan(id), name, "{id}");
        }
    }

    #[test]
    fn codex_limits_read_durations_plans_and_reset_credits() {
        let limits = json!({
            "rateLimits": {"limitId": "other"},
            "rateLimitsByLimitId": {"codex": {
                "limitId": "codex", "planType": "pro",
                "primary": {"usedPercent": 93, "windowDurationMins": 10080, "resetsAt": 1794334260},
                "secondary": null,
            }},
            "rateLimitResetCredits": {"availableCount": 2, "credits": [
                {"status": "available", "expiresAt": 1796165225},
                {"status": "available", "expiresAt": 1796000000},
                {"status": "redeemed", "expiresAt": 1},
            ]},
        });
        let account = codex_account(&limits, &json!({"account":{"email":"a@b.c"}}), 0, Tz::UTC);
        assert_eq!(account["plan"], "Pro 200");
        assert_eq!(account["windows"][0]["kind"], "weekly");
        assert_eq!(account["windows"][0]["label"], "Weekly");
        assert_eq!(account["reset_credits"]["available_count"], 2);
        assert_eq!(
            account["reset_credits"]["next_expires_at_ms"],
            1796000000000_i64
        );
        let key = account["account_key"].as_str().unwrap();
        assert!(!key.contains('@'), "the address stays on the host");
        assert_eq!(
            account_key("codex", Some(" A@B.C ")).as_deref(),
            Some(key),
            "identity ignores case and spacing"
        );
        let fallback = codex_windows(&json!({
            "planType": "free", "primary": {"usedPercent": 10},
            "individualLimit": {"remainingPercent": 75, "resetsAt": 5},
        }));
        assert_eq!(fallback[0].kind, "monthly");
        assert_eq!(fallback[1].id, "individual");
        assert_eq!(fallback[1].used_percent, 25.0);
        assert!(codex_windows(&json!({"limitId":"gpt-5","primary":{"usedPercent":1}})).is_empty());
    }

    fn script(directory: &Path, name: &str, body: &str) -> std::process::Command {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        std::process::Command::new(path)
    }

    fn temporary_directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("xd-usage-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn probes_read_answers_from_each_cli() {
        let directory = temporary_directory("probes");
        let codex = script(
            &directory,
            "codex",
            r#"read a; echo '{"id":1,"result":{}}'; read b; read c; read d
echo '{"id":3,"result":{"account":{"planType":"plus"}}}'
echo '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":40}}}}'
sleep 30"#,
        );
        let account = codex_limits(codex, 0, Tz::UTC).unwrap();
        assert_eq!(account["plan"], "Plus");
        assert_eq!(account["windows"][0]["used_percent"], 40.0);
        assert_eq!(account["windows"][0]["kind"], "session");

        let failing = script(
            &directory,
            "codex2",
            r#"read a; echo '{"id":1,"result":{}}'; read b; read c; read d
echo '{"id":2,"error":{"code":-32600,"message":"nope"}}'; sleep 30"#,
        );
        let account = codex_limits(failing, 0, Tz::UTC).unwrap();
        assert_eq!(account["unavailable"]["reason"], "probeFailed");
        assert_eq!(
            account["unavailable"]["message"],
            "Codex could not read usage (JSON-RPC -32600)."
        );

        let claude = script(
            &directory,
            "claude",
            r#"read a; echo '{"type":"control_response","response":{"request_id":"xd-usage-init","subtype":"success","response":{"account":{"email":"a@b.c","subscriptionType":"Claude Ultra"}}}}'
read b; echo '{"type":"control_response","response":{"request_id":"xd-usage","subtype":"success","response":{"subscription_type":"max","rate_limits_available":true,"rate_limits":{"seven_day":{"utilization":50,"resets_at":null}}}}}'
sleep 30"#,
        );
        let account = claude_limits(claude, 0, Tz::UTC).unwrap();
        assert_eq!(account["plan"], "Ultra", "the sign-in's plan name wins");
        assert_eq!(account["windows"][0]["id"], "seven_day");
        assert_eq!(
            account["account_key"],
            json!(account_key("claude", Some("a@b.c")))
        );

        assert!(
            codex_limits(
                std::process::Command::new(directory.join("absent")),
                0,
                Tz::UTC
            )
            .is_none()
        );
        assert!(
            claude_limits(
                std::process::Command::new(directory.join("absent")),
                0,
                Tz::UTC
            )
            .is_none()
        );

        let reset = script(
            &directory,
            "codex3",
            r#"read a; echo '{"id":1,"result":{}}'; read b; read c
case "$c" in *idem-1*) echo '{"id":2,"result":{"outcome":"reset"}}';; esac; sleep 30"#,
        );
        assert_eq!(
            consume_codex_reset_credit(reset, "idem-1", None).unwrap(),
            "reset"
        );
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn a_reset_refuses_an_account_the_user_did_not_confirm() {
        let directory = temporary_directory("reset-account");
        let body = r#"read a; echo '{"id":1,"result":{}}'; read b; read c; read d
echo '{"id":3,"result":{"account":{"email":"now@example.com"}}}'
echo '{"id":4,"result":{}}'
read e; case "$e" in *consume*) echo '{"id":2,"result":{"outcome":"reset"}}';; esac; sleep 30"#;
        let same = account_key("codex", Some("now@example.com")).unwrap();
        let other = account_key("codex", Some("then@example.com")).unwrap();
        let wrong =
            consume_codex_reset_credit(script(&directory, "codex-a", body), "k", Some(&other));
        assert!(wrong.unwrap_err().contains("account changed"));
        let right =
            consume_codex_reset_credit(script(&directory, "codex-b", body), "k", Some(&same));
        assert_eq!(right.unwrap(), "reset");
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn a_copied_rollout_counts_once_but_equal_requests_within_a_file_do_not() {
        let at = |text: &str| {
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .timestamp_millis()
        };
        let window = UsageWindow::new(7, Tz::UTC, at("2026-11-07T15:30:00Z"));
        let request = UsageRecord {
            provider: "codex",
            session_id: "thread".into(),
            timestamp_ms: at("2026-11-07T01:00:00Z"),
            ..record("gpt-5.5", MILLION, Speed::Standard, None)
        };
        let file = vec![request.clone(), request.clone()];
        let copied = SourceScan {
            records: vec![Arc::new(file.clone()), Arc::new(file.clone())],
            ..scan_of(Vec::new())
        };
        let (buckets, sessions) = aggregate(
            &[copied],
            &window,
            &RateTable::new(),
            &PriceSettings::default(),
        );
        assert_eq!(buckets[0]["records"], 2, "the copy adds nothing");
        assert_eq!(sessions["codex"], 1);
        let alone = SourceScan {
            records: vec![Arc::new(file)],
            ..scan_of(Vec::new())
        };
        let (buckets, _) = aggregate(
            &[alone],
            &window,
            &RateTable::new(),
            &PriceSettings::default(),
        );
        assert_eq!(buckets[0]["records"], 2);
    }

    #[test]
    fn absurd_counters_are_malformed_rather_than_overflowing() {
        let mut state = codex_state();
        let huge = json!({"input_tokens": i64::MAX, "output_tokens": 1});
        assert_eq!(read_codex_usage(&huge), None);
        let event = token_count_line("2026-11-05T12:00:01Z", huge.clone(), huge);
        assert!(parse_codex_record(&event, &mut state).is_empty());
        assert_eq!(state.malformed, 1);
        let at_limit = json!({"input_tokens": MAX_TOKEN_COUNT, "output_tokens": MAX_TOKEN_COUNT});
        assert_eq!(
            read_codex_usage(&at_limit),
            None,
            "their total is past the limit"
        );
        assert_eq!(token_count(&json!(1e300)), MAX_TOKEN_COUNT as u64);
        let mut totals = Totals {
            output: u64::MAX,
            ..Totals::default()
        };
        totals.add(&MILLION);
        assert_eq!(totals.output, u64::MAX);
    }

    #[test]
    fn a_scan_that_dies_still_answers() {
        let answer = summary_event(json!(4), || panic!("scan failed"));
        assert_eq!(answer["generation"], 4);
        assert!(answer["error"].is_string());
        assert!(answer.get("summary").is_none());
        let answer = summary_event(json!(5), || json!({"buckets": []}));
        assert_eq!(answer["summary"]["buckets"], json!([]));
    }

    #[test]
    fn a_reset_without_a_confirmed_account_is_refused() {
        let directory = temporary_directory("reset-unknown");
        let usage_service = service(&directory, Arc::new(Mutex::new(0)));
        let error = usage_service
            .consume_reset_credit(0, &json!({"provider":"codex","idempotency_key":"k"}))
            .unwrap_err();
        assert!(error.contains("confirmed for"));
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn coalesced_reads_run_the_running_one_and_only_the_newest_waiting() {
        let coalescer = Coalescer::new("xd-usage-test");
        let ran = Arc::new(Mutex::new(Vec::new()));
        let (release, wait) = mpsc::channel::<()>();
        let (started, running) = mpsc::channel::<()>();
        let (finished, done) = mpsc::channel::<()>();
        let first = ran.clone();
        coalescer
            .submit(
                7,
                Box::new(move || {
                    first.lock().unwrap().push(1);
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                }),
            )
            .unwrap();
        running.recv().unwrap();
        for number in [2, 3] {
            let ran = ran.clone();
            let finished = finished.clone();
            coalescer
                .submit(
                    7,
                    Box::new(move || {
                        ran.lock().unwrap().push(number);
                        finished.send(()).unwrap();
                    }),
                )
                .unwrap();
        }
        release.send(()).unwrap();
        done.recv().unwrap();
        assert_eq!(*ran.lock().unwrap(), [1, 3]);
    }

    #[test]
    fn records_past_the_longest_window_are_not_kept() {
        let directory = temporary_directory("retention");
        let path = directory.join("a.jsonl");
        let mut old = assistant("old", "claude-opus-5-5");
        old["timestamp"] = json!(iso(now_ms() - RETENTION_MS - DAY_MS));
        let mut new = assistant("new", "claude-opus-5-5");
        new["timestamp"] = json!(iso(now_ms()));
        write_transcript(&path, &[old, new]);
        let (records, _) = read_transcript("claude", &path, now_ms() - RETENTION_MS).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].dedupe_key.as_deref(), Some("new:req"));
        fs::remove_dir_all(directory).ok();
    }

    fn write_transcript(path: &Path, lines: &[Value]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body: String = lines.iter().map(|line| format!("{line}\n")).collect();
        fs::write(path, body).unwrap();
    }

    fn service(directory: &Path, fetches: Arc<Mutex<u32>>) -> UsageService {
        let fetch: Arc<RatesFetcher> = Arc::new(move |_, _| {
            *fetches.lock().unwrap() += 1;
            Some(rates_document())
        });
        UsageService::with_parts(
            Arc::new(EventBus::default()),
            Some(directory.join("data")),
            TranscriptRoots {
                claude: Some(directory.join("claude/projects")),
                codex: Some(directory.join("codex/sessions")),
            },
            fetch,
        )
    }

    #[test]
    fn summary_scans_transcripts_rereads_changes_and_caches_rates() {
        let directory = temporary_directory("summary");
        let now = instant(now_ms()).to_rfc3339();
        let mut message = assistant("m", "claude-opus-5-5");
        message["timestamp"] = json!(now);
        let transcript = directory.join("claude/projects/p/s.jsonl");
        // The second copy is a content-block repeat; the fork is a resumed copy.
        write_transcript(&transcript, &[message.clone(), message.clone()]);
        write_transcript(
            &directory.join("claude/projects/p/fork.jsonl"),
            &[message.clone()],
        );
        let rollout = directory.join("codex/sessions/2026/r.jsonl");
        write_transcript(
            &rollout,
            &[
                json!({"timestamp":now,"type":"session_meta","payload":{"id":"thread"}}),
                json!({"timestamp":now,"type":"turn_context","payload":{"model":"gpt-5.5"}}),
                token_count_line(&now, usage(10, 0, 2, 0), usage(10, 0, 2, 0)),
            ],
        );
        let fetches = Arc::new(Mutex::new(0));
        let usage_service = service(&directory, fetches.clone());
        let summary = usage_service.shared.summary(30, Tz::UTC, false, now_ms());
        let buckets = summary["buckets"].as_array().unwrap();
        assert_eq!(buckets.len(), 2);
        let claude = buckets
            .iter()
            .find(|bucket| bucket["provider"] == "claude")
            .unwrap();
        assert_eq!(claude["records"], 1);
        assert_eq!(claude["cost_source"], "modelPriced");
        assert_eq!(summary["sessions"]["claude"], 1);
        assert_eq!(summary["sessions"]["codex"], 1);
        assert_eq!(summary["pricing"]["status"], "fresh");
        assert_eq!(summary["sources"][0]["provider"], "codex");
        assert_eq!(summary["sources"][1]["scanned_files"], 2);
        assert_eq!(*fetches.lock().unwrap(), 1);

        // A new line is picked up; rates are not fetched again within a day.
        let mut second = assistant("m2", "claude-opus-5-5");
        second["timestamp"] = json!(now);
        write_transcript(&transcript, &[message, second]);
        let summary = usage_service.shared.summary(30, Tz::UTC, false, now_ms());
        let claude = summary["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|bucket| bucket["provider"] == "claude")
            .unwrap()
            .clone();
        assert_eq!(claude["records"], 2);
        assert_eq!(*fetches.lock().unwrap(), 1);

        // A fresh process reads the rate cache from disk rather than the network.
        let restarted = service(&directory, fetches.clone());
        let summary = restarted.shared.summary(30, Tz::UTC, false, now_ms());
        assert_eq!(summary["pricing"]["status"], "cached");
        assert_eq!(*fetches.lock().unwrap(), 1);

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn skipped_records_mark_their_source_partial() {
        let directory = temporary_directory("malformed");
        let now = instant(now_ms()).to_rfc3339();
        write_transcript(
            &directory.join("codex/sessions/r.jsonl"),
            &[
                json!({"timestamp":now,"type":"turn_context","payload":{"model":"gpt-5.5"}}),
                token_count_line(&now, usage(10, 0, 1, 0), usage(10, 0, 1, 0)),
                // Counters that jump without anything to explain the gap.
                token_count_line(&now, usage(5, 0, 1, 0), usage(50, 0, 9, 0)),
            ],
        );
        let usage_service = service(&directory, Arc::new(Mutex::new(0)));
        let summary = usage_service.shared.summary(7, Tz::UTC, false, now_ms());
        assert_eq!(summary["sources"][0]["status"], "partial");
        assert_eq!(summary["sources"][0]["malformed_records"], 1);
        assert_eq!(summary["sources"][1]["status"], "missing");
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn missing_directories_and_failed_rate_fetches_degrade_without_failing() {
        let directory = temporary_directory("missing");
        let fetch: Arc<RatesFetcher> = Arc::new(|_, _| None);
        let usage_service = UsageService::with_parts(
            Arc::new(EventBus::default()),
            None,
            TranscriptRoots {
                claude: None,
                codex: Some(directory.join("absent")),
            },
            fetch,
        );
        let summary = usage_service.shared.summary(7, Tz::UTC, false, now_ms());
        assert_eq!(summary["pricing"]["status"], "unavailable");
        assert_eq!(summary["sources"][0]["status"], "missing");
        assert_eq!(summary["sources"][1]["status"], "missing");
        assert_eq!(summary["buckets"], json!([]));
        assert!(usage_service.set_prices(&json!({"prices":{}})).is_err());
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn prices_round_trip_through_a_private_file() {
        use std::os::unix::fs::PermissionsExt;
        let directory = temporary_directory("prices");
        let fetches = Arc::new(Mutex::new(0));
        let usage_service = service(&directory, fetches);
        assert_eq!(usage_service.prices().unwrap()["prices"], json!({}));
        let saved = usage_service
            .set_prices(&json!({
                "prices": {"m": {"input": 1.5, "output": 2}},
                "aliases": {"n": "m"},
            }))
            .unwrap();
        assert_eq!(saved["prices"]["m"]["input"], 1.5);
        assert_eq!(usage_service.prices().unwrap()["aliases"]["n"], "m");
        let mode = fs::metadata(directory.join("data").join(PRICES_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(
            usage_service
                .set_prices(&json!({"prices":{"m":{"input":1}}}))
                .is_err()
        );
        assert_eq!(
            usage_service.prices().unwrap()["prices"]["m"]["output"],
            2.0
        );
        // Leaving out the mappings keeps them; sending an empty set clears them.
        let kept = usage_service
            .set_prices(&json!({"prices": {"m": {"input": 3, "output": 4}}}))
            .unwrap();
        assert_eq!(kept["prices"]["m"]["input"], 3.0);
        assert_eq!(kept["aliases"]["n"], "m");
        let cleared = usage_service
            .set_prices(&json!({"prices": {}, "aliases": {}}))
            .unwrap();
        assert_eq!(cleared["aliases"], json!({}));
        assert_eq!(usage_service.prices().unwrap()["aliases_optional"], true);
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn requests_validate_before_doing_work() {
        let directory = temporary_directory("requests");
        let usage_service = service(&directory, Arc::new(Mutex::new(0)));
        assert!(usage_service.summary(0, &json!({"days": 3})).is_err());
        assert!(
            usage_service
                .consume_reset_credit(0, &json!({"provider":"claude","idempotency_key":"k"}))
                .is_err()
        );
        assert!(
            usage_service
                .consume_reset_credit(0, &json!({"provider":"codex"}))
                .is_err()
        );
        fs::remove_dir_all(directory).ok();
    }
}
