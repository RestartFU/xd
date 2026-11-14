//! Context compaction, as T3 Code does it for Claude and Codex.
//!
//! Both assistants compact a conversation on their own as it nears the model's
//! window. xd shows each compaction in the transcript, and for Claude it can
//! also compact an idle chat while that is still cheap:
//!
//! - **When you return** (the default): a message sent to a Claude chat that
//!   has sat idle past its prompt cache, with at least
//!   [`SUGGESTION_MIN_TOKENS`] in context, can run `/compact` first. The
//!   composer offers it and the user can keep the full history instead.
//! - **Before the cache expires**: xd sends `/compact` itself shortly before
//!   the cache of the chat's last request expires, so the summary is made from
//!   a warm cache rather than a full uncached read.
//! - **Never**.

use serde_json::Value;

/// The transcript row of one compaction: `context_compaction\n<state>\n<label>`.
pub(crate) const RECORD_PREFIX: &str = "context_compaction\n";

/// The prompt that asks the assistant to compact.
pub(crate) const COMMAND: &str = "/compact";

/// Context below which xd neither suggests nor runs a Claude compaction.
/// Matches Claude Code's `idleCompaction` default of 200K; below that a cold
/// re-read is cheap, so compacting mostly loses context for little saving.
pub(crate) const SUGGESTION_MIN_TOKENS: u64 = 200_000;

/// How long before the cache expires an idle compaction starts: enough for the
/// session to resume and send the compaction while the cache is still warm.
pub(crate) const LEAD_MS: i64 = 2 * 60_000;

/// How long a chat must be idle before a send offers to compact it, when the
/// last request did not say how long its cache lives. Claude Code's own
/// resume threshold.
pub(crate) const RESUME_FALLBACK_MS: i64 = 70 * 60_000;

/// The `meta` key that holds the idle compaction mode.
pub(crate) const MODE_META_KEY: &str = "idle_compaction";

/// When xd compacts an idle Claude chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleMode {
    OnReturn,
    BeforeCacheExpires,
    Off,
}

impl IdleMode {
    pub(crate) const DEFAULT: Self = Self::OnReturn;

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "onReturn" => Some(Self::OnReturn),
            "beforeCacheExpires" => Some(Self::BeforeCacheExpires),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OnReturn => "onReturn",
            Self::BeforeCacheExpires => "beforeCacheExpires",
            Self::Off => "off",
        }
    }
}

/// Why xd sent a `/compact` on its own. A turn without one is a compaction
/// someone asked for, or none at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleReason {
    BeforeCacheExpires,
    OnReturn,
}

impl IdleReason {
    fn compacting_label(self) -> &'static str {
        match self {
            Self::BeforeCacheExpires => "Compacting before the cache expires…",
            Self::OnReturn => "Compacting…",
        }
    }

    /// Leads the finished compaction's token counts, in place of "Context compacted".
    fn title(self) -> &'static str {
        match self {
            Self::BeforeCacheExpires => "Context automatically compacted before cache expiry",
            Self::OnReturn => "Context compacted before sending",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionState {
    Running,
    Completed,
    Failed,
    Stopped,
}

impl CompactionState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

/// One compaction, as the assistant last reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    /// The assistant's own id for it, which keeps one row per compaction.
    pub id: String,
    pub state: CompactionState,
    /// Context before and after, where the assistant says.
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub error: Option<String>,
}

impl Compaction {
    pub(crate) fn new(id: impl Into<String>, state: CompactionState) -> Self {
        Self {
            id: id.into(),
            state,
            before: None,
            after: None,
            error: None,
        }
    }

    /// What the transcript row says.
    pub(crate) fn label(&self, reason: Option<IdleReason>) -> String {
        match self.state {
            CompactionState::Running => reason
                .map_or("Compacting context", IdleReason::compacting_label)
                .to_owned(),
            CompactionState::Completed => {
                let title = reason.map_or("Context compacted", IdleReason::title);
                match (self.before, self.after) {
                    (Some(before), Some(after)) => format!(
                        "{title} {} → {} tokens",
                        format_tokens(before),
                        format_tokens(after)
                    ),
                    _ => title.to_owned(),
                }
            }
            CompactionState::Failed => match self.error.as_deref().map(str::trim) {
                Some(error) if !error.is_empty() => {
                    format!("Context compaction failed · {error}")
                }
                _ => "Context compaction failed".to_owned(),
            },
            CompactionState::Stopped => "Context compaction stopped".to_owned(),
        }
    }

    /// The transcript row for this compaction.
    pub(crate) fn record(&self, reason: Option<IdleReason>) -> String {
        format!(
            "{RECORD_PREFIX}{}\n{}",
            self.state.as_str(),
            self.label(reason)
        )
    }
}

/// Whether a prompt only asks the assistant to compact, so its turn is the
/// compaction itself.
pub(crate) fn is_command(prompt: &str) -> bool {
    prompt.trim().eq_ignore_ascii_case(COMMAND)
}

/// Whether a prompt is a typed `/compact`, with or without instructions: that
/// is already the compaction, so compacting before it would charge for two.
pub(crate) fn starts_with_command(prompt: &str) -> bool {
    let trimmed = prompt.trim_start();
    trimmed
        .get(..COMMAND.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(COMMAND))
        && trimmed[COMMAND.len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
}

/// How long the prompt cache Claude's request wrote lives, from the request's
/// `cache_creation`. None when it wrote none: a request served fully from
/// cache still refreshes the cache but reports no lifetime.
pub(crate) fn claude_prompt_cache_ttl_ms(usage: &Value) -> Option<i64> {
    let creation = usage.get("cache_creation")?;
    let tokens = |name: &str| creation.get(name).and_then(Value::as_u64).unwrap_or(0);
    if tokens("ephemeral_1h_input_tokens") > 0 {
        Some(60 * 60_000)
    } else if tokens("ephemeral_5m_input_tokens") > 0 {
        Some(5 * 60_000)
    } else {
        None
    }
}

/// Tokens as T3 Code writes them: 950, 1.25K, 12.5K, 250K, 1M.
pub(crate) fn format_tokens(value: u64) -> String {
    if value < 1_000 {
        return value.to_string();
    }
    const SUFFIXES: [&str; 4] = ["K", "M", "B", "T"];
    let mut scaled = value as f64 / 1e3;
    let mut unit = 0;
    // Choose the unit after rounding, so 999,999 reads 1M rather than 1000K.
    while trim(scaled).parse::<f64>().unwrap_or(scaled).abs() >= 1e3 && unit < SUFFIXES.len() - 1 {
        scaled /= 1e3;
        unit += 1;
    }
    format!("{}{}", trim(scaled), SUFFIXES[unit])
}

fn trim(value: f64) -> String {
    let abs = value.abs();
    let digits = if abs >= 100.0 {
        0
    } else if abs >= 10.0 {
        1
    } else {
        2
    };
    let fixed = format!("{value:.digits$}");
    // Only a fraction of zeros goes: 1.50 stays, 1.00 reads 1.
    match fixed.split_once('.') {
        Some((whole, fraction)) if fraction.chars().all(|digit| digit == '0') => whole.to_owned(),
        _ => fixed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tokens_read_as_t3_code_writes_them() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(1_000), "1K");
        assert_eq!(format_tokens(1_250), "1.25K");
        assert_eq!(format_tokens(1_500), "1.50K");
        assert_eq!(format_tokens(12_500), "12.5K");
        assert_eq!(format_tokens(250_000), "250K");
        assert_eq!(format_tokens(999_999), "1M");
        assert_eq!(format_tokens(1_000_000), "1M");
    }

    #[test]
    fn labels_follow_the_state_and_why_xd_compacted() {
        let mut compaction = Compaction::new("c", CompactionState::Running);
        assert_eq!(compaction.label(None), "Compacting context");
        assert_eq!(
            compaction.label(Some(IdleReason::BeforeCacheExpires)),
            "Compacting before the cache expires…"
        );
        assert_eq!(compaction.label(Some(IdleReason::OnReturn)), "Compacting…");

        compaction.state = CompactionState::Completed;
        assert_eq!(compaction.label(None), "Context compacted");
        compaction.before = Some(250_000);
        compaction.after = Some(32_000);
        assert_eq!(
            compaction.label(None),
            "Context compacted 250K → 32K tokens"
        );
        assert_eq!(
            compaction.label(Some(IdleReason::BeforeCacheExpires)),
            "Context automatically compacted before cache expiry 250K → 32K tokens"
        );
        assert_eq!(
            compaction.record(Some(IdleReason::OnReturn)),
            "context_compaction\ncompleted\nContext compacted before sending 250K → 32K tokens"
        );

        compaction.state = CompactionState::Failed;
        compaction.error = Some("Not enough messages".into());
        assert_eq!(
            compaction.label(None),
            "Context compaction failed · Not enough messages"
        );
        compaction.state = CompactionState::Stopped;
        assert_eq!(compaction.label(None), "Context compaction stopped");
    }

    #[test]
    fn only_a_bare_compact_is_the_command() {
        assert!(is_command(" /compact "));
        assert!(is_command("/COMPACT"));
        assert!(!is_command("/compact keep the plan"));
        assert!(starts_with_command("/compact keep the plan"));
        assert!(starts_with_command("/compact"));
        assert!(!starts_with_command("/compaction"));
        assert!(!starts_with_command("please /compact"));
    }

    #[test]
    fn cache_lifetime_comes_from_what_the_request_wrote() {
        let usage = |one_hour: u64, five_minutes: u64| {
            json!({"cache_creation": {
                "ephemeral_1h_input_tokens": one_hour,
                "ephemeral_5m_input_tokens": five_minutes,
            }})
        };
        assert_eq!(claude_prompt_cache_ttl_ms(&usage(10, 0)), Some(3_600_000));
        assert_eq!(claude_prompt_cache_ttl_ms(&usage(0, 10)), Some(300_000));
        assert_eq!(claude_prompt_cache_ttl_ms(&usage(0, 0)), None);
        assert_eq!(claude_prompt_cache_ttl_ms(&json!({})), None);
    }

    #[test]
    fn modes_round_trip() {
        for mode in [
            IdleMode::OnReturn,
            IdleMode::BeforeCacheExpires,
            IdleMode::Off,
        ] {
            assert_eq!(IdleMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(IdleMode::parse("sometimes"), None);
    }
}
