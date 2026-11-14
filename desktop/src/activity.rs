use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const SUBAGENT_PREFIX: &str = "subagent\n";
const SUBAGENT_RUN_PREFIX: &str = "subagent_run\n";
const WORKFLOW_PREFIX: &str = "workflow_run\n";
const CI_RUN_PREFIX: &str = "ci_run\n";
const FILE_CHANGE_PREFIX: &str = "file_change\n";
/// A chat moved to another account. A notice, never a tool card.
const ACCOUNT_SWITCH_PREFIX: &str = "account_switch\n";
/// A context compaction: `context_compaction\n<state>\n<label>`. A divider in
/// the conversation, never a tool card.
const COMPACTION_PREFIX: &str = "context_compaction\n";
const PATCH_MARKER: &str = "diff --git ";
const MAX_AUTO_EXPANDED_PATCH_LINES: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityCard {
    pub kind: ActivityKind,
    pub title: String,
    pub name: String,
    pub status: String,
    pub elapsed: Option<String>,
    pub detail: String,
    pub footer: Option<String>,
    pub url: Option<String>,
    pub items: Vec<ActivityItem>,
    /// The unified patch an edit carries, rendered inline instead of `detail`.
    pub patch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityItem {
    pub kind: ActivityKind,
    pub name: String,
    pub status: String,
    pub elapsed: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Running,
    Success,
    Failure,
    Finished,
}

/// Plain tool activity, the kind that arrives several times in a row. Runs of it
/// are collapsed into one summary card so a burst of commands reads as a block.
pub fn is_plain_activity(content: &str) -> bool {
    !content.starts_with(SUBAGENT_PREFIX)
        && !content.starts_with(SUBAGENT_RUN_PREFIX)
        && !content.starts_with(WORKFLOW_PREFIX)
        && !content.starts_with(CI_RUN_PREFIX)
        && !content.starts_with(FILE_CHANGE_PREFIX)
        && !content.starts_with(ACCOUNT_SWITCH_PREFIX)
        && !content.starts_with(COMPACTION_PREFIX)
}

/// How a context compaction the host reported stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionState {
    Running,
    Completed,
    Failed,
    Stopped,
}

/// A compaction row's state and sentence, if `content` is one.
pub fn compaction_notice(content: &str) -> Option<(CompactionState, &str)> {
    let (state, label) = content.strip_prefix(COMPACTION_PREFIX)?.split_once('\n')?;
    let state = match state {
        "running" => CompactionState::Running,
        "completed" => CompactionState::Completed,
        "failed" => CompactionState::Failed,
        "stopped" => CompactionState::Stopped,
        _ => return None,
    };
    let label = label.trim();
    (!label.is_empty()).then_some((state, label))
}

/// Tokens as T3 Code writes them, as the host does: 950, 1.25K, 12.5K, 251K, 1M.
pub fn format_tokens(value: u64) -> String {
    fn trim(value: f64) -> String {
        let digits = if value >= 100.0 {
            0
        } else if value >= 10.0 {
            1
        } else {
            2
        };
        let fixed = format!("{value:.digits$}");
        match fixed.split_once('.') {
            Some((whole, fraction)) if fraction.chars().all(|digit| digit == '0') => {
                whole.to_owned()
            }
            _ => fixed,
        }
    }
    if value < 1_000 {
        return value.to_string();
    }
    const SUFFIXES: [&str; 4] = ["K", "M", "B", "T"];
    let mut scaled = value as f64 / 1e3;
    let mut unit = 0;
    // Choose the unit after rounding, so 999,999 reads 1M rather than 1000K.
    while trim(scaled).parse::<f64>().unwrap_or(scaled) >= 1e3 && unit < SUFFIXES.len() - 1 {
        scaled /= 1e3;
        unit += 1;
    }
    format!("{}{}", trim(scaled), SUFFIXES[unit])
}

/// A delegated agent, as the host's live record describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentView {
    /// The host's id for the agent, which actions on it name.
    pub key: String,
    /// Claude's own id for the agent, which its lead addresses it by.
    pub task_id: String,
    pub provider: String,
    pub agent: String,
    /// The assistant whose shell runs this agent as a command, if one does.
    pub launcher: String,
    pub model: String,
    pub description: String,
    pub prompt: String,
    pub status: SubagentState,
    pub background: bool,
    pub activity: String,
    pub steps: Vec<SubagentStepView>,
    pub tool_uses: u64,
    pub tokens: Option<u64>,
    pub duration_ms: Option<u64>,
    /// When the host first heard of the agent, in Unix milliseconds.
    pub started_at: Option<u64>,
    pub latest: String,
    pub summary: String,
    /// The xd chat this agent is, for one an agent started with xd's
    /// start_agent tool. Empty for an agent inside its lead's own process.
    pub chat: String,
}

/// Steps the host keeps per agent; a longer run keeps only its newest.
pub const SUBAGENT_KEPT_STEPS: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentStepView {
    pub tool: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentState {
    Running,
    Completed,
    Failed,
    Stopped,
}

impl SubagentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Completed => "Done",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
        }
    }
}

/// How the turn an agent chat reported on ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentOutcome {
    Done,
    Failed,
    Waiting,
}

/// The message xd sends a chat when an agent it started ends a turn:
/// `[xd] Agent "Title" (chat-id) is done with its turn.` and the reply after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReport {
    pub chat: String,
    pub title: String,
    pub outcome: AgentOutcome,
}

/// The report `text` is, when xd sent it for one of the chat's agents.
pub fn agent_report(text: &str) -> Option<AgentReport> {
    let first = text.lines().next()?.strip_prefix("[xd] ")?;
    let (head, ending) = first.rsplit_once(") is ")?;
    let outcome = match ending {
        "done with its turn." => AgentOutcome::Done,
        "stopped with an error." => AgentOutcome::Failed,
        "waiting for an answer." => AgentOutcome::Waiting,
        _ => return None,
    };
    let (name, chat) = head.rsplit_once(" (")?;
    let title = match name {
        "An agent" => String::new(),
        name => name.strip_prefix("Agent \"")?.strip_suffix('"')?.to_owned(),
    };
    (!chat.is_empty() && !chat.contains(' ')).then(|| AgentReport {
        chat: chat.to_owned(),
        title,
        outcome,
    })
}

/// How a CI watch xd ran for the chat's agent ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiOutcome {
    Passed,
    Failed,
    Stopped,
}

/// The message xd sends a chat when a CI watch its agent asked for with xd's
/// watch_ci tool ends: `[xd] CI passed on <url> at <head>.`, `[xd] CI failed
/// on …`, or `[xd] Stopped watching CI on <url>.`, and the details after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiReport {
    pub url: String,
    pub number: String,
    pub outcome: CiOutcome,
}

/// The CI report `text` is, when xd sent it.
pub fn ci_report(text: &str) -> Option<CiReport> {
    let first = text.lines().next()?.strip_prefix("[xd] ")?;
    let (outcome, rest) = if let Some(rest) = first.strip_prefix("CI passed on ") {
        (CiOutcome::Passed, rest)
    } else if let Some(rest) = first.strip_prefix("CI failed on ") {
        (CiOutcome::Failed, rest)
    } else {
        (
            CiOutcome::Stopped,
            first.strip_prefix("Stopped watching CI on ")?,
        )
    };
    let url = match outcome {
        CiOutcome::Stopped => rest.strip_suffix('.')?,
        _ => rest.split_once(" at ")?.0,
    };
    let number = url.strip_prefix("https://")?.rsplit_once("/pull/")?.1;
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) && !url.contains(' '))
        .then(|| CiReport {
            url: url.to_owned(),
            number: number.to_owned(),
            outcome,
        })
}

impl SubagentView {
    /// A backgrounded shell command, which Claude reports the same way as a
    /// delegated agent.
    pub fn is_shell(&self) -> bool {
        self.agent == "Shell"
    }

    pub fn parse(content: &str) -> Option<Self> {
        let record: serde_json::Value =
            serde_json::from_str(content.strip_prefix(SUBAGENT_RUN_PREFIX)?).ok()?;
        let text = |key: &str| {
            record
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Some(Self {
            key: text("key"),
            task_id: text("task_id"),
            provider: text("provider"),
            agent: text("agent"),
            launcher: text("launcher"),
            model: text("model"),
            description: text("description"),
            prompt: text("prompt"),
            status: match record.get("status").and_then(serde_json::Value::as_str) {
                Some("completed") => SubagentState::Completed,
                Some("failed") => SubagentState::Failed,
                Some("stopped") => SubagentState::Stopped,
                _ => SubagentState::Running,
            },
            background: record
                .get("background")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            activity: text("activity"),
            steps: record
                .get("steps")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .map(|step| SubagentStepView {
                    tool: step
                        .get("tool")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    text: step
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                })
                .collect(),
            tool_uses: record
                .get("tool_uses")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            tokens: record.get("tokens").and_then(serde_json::Value::as_u64),
            duration_ms: record
                .get("duration_ms")
                .and_then(serde_json::Value::as_u64),
            started_at: record.get("started_at").and_then(serde_json::Value::as_u64),
            latest: text("latest"),
            summary: text("summary"),
            chat: text("chat"),
        })
    }

    /// "2 tools · 13.9k tokens · 2.9s", whatever of it is known. A running
    /// agent counts up from when it started.
    pub fn stats_at(&self, now_ms: u64) -> String {
        let mut view = self.clone();
        if view.status == SubagentState::Running
            && let Some(started) = view.started_at
        {
            view.duration_ms = Some(now_ms.saturating_sub(started).max(1));
        }
        view.stats()
    }

    /// Claude can stop its agents, and send one the turn waits on to the
    /// background; other backends report theirs but take no orders. An agent
    /// Claude runs as a command is Claude's too, but Claude can only stop that
    /// command once it runs in the background.
    pub fn can_stop(&self) -> bool {
        self.claude_controls() && (self.launcher.is_empty() || self.background)
    }

    pub fn can_background(&self) -> bool {
        self.claude_controls() && !self.background
    }

    /// The assistant that started the agent is the one that can act on it.
    fn claude_controls(&self) -> bool {
        let controller = if self.launcher.is_empty() {
            &self.provider
        } else {
            &self.launcher
        };
        self.status == SubagentState::Running
            && !self.key.is_empty()
            && controller.eq_ignore_ascii_case("claude")
    }

    /// Steps the record no longer lists: only a run longer than the host
    /// keeps has any.
    pub fn dropped_steps(&self) -> u64 {
        if self.steps.len() < SUBAGENT_KEPT_STEPS {
            0
        } else {
            self.tool_uses.saturating_sub(self.steps.len() as u64)
        }
    }

    /// "2 tools · 13.9k tokens · 2.9s", whatever of it is known.
    pub fn stats(&self) -> String {
        let mut parts = Vec::new();
        if self.tool_uses > 0 {
            parts.push(format!(
                "{} tool{}",
                self.tool_uses,
                if self.tool_uses == 1 { "" } else { "s" }
            ));
        }
        if let Some(tokens) = self.tokens {
            parts.push(format!("{} tokens", compact_count(tokens)));
        }
        if let Some(duration) = self.duration_ms {
            parts.push(compact_duration(duration));
        }
        parts.join(" · ")
    }
}

/// 950, 13.9k, 1.2M.
pub fn compact_count(value: u64) -> String {
    match value {
        0..=999 => value.to_string(),
        1_000..=999_999 => trim_decimal(value as f64 / 1_000.0, "k"),
        _ => trim_decimal(value as f64 / 1_000_000.0, "M"),
    }
}

fn trim_decimal(value: f64, unit: &str) -> String {
    if value >= 100.0 {
        format!("{value:.0}{unit}")
    } else {
        let text = format!("{value:.1}");
        format!("{}{unit}", text.trim_end_matches(".0"))
    }
}

/// 2.9s, 47s, 3m 05s, 1h 02m.
pub fn compact_duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    if seconds < 10 {
        return format!("{:.1}s", milliseconds as f64 / 1_000.0);
    }
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {:02}s", seconds % 60);
    }
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

impl ActivityCard {
    /// Compact file edits lead with their useful content on desktop. A larger
    /// patch starts closed so it does not take over the transcript; the user's
    /// explicit toggle still inverts this default in either direction.
    pub fn starts_expanded(&self) -> bool {
        self.patch
            .as_deref()
            .is_some_and(patch_fits_expanded_viewport)
    }

    pub fn can_expand(&self) -> bool {
        !self.detail.is_empty()
            || self.footer.is_some()
            || self.url.is_some()
            || !self.items.is_empty()
            || self.patch.is_some()
    }

    pub fn parse(content: &str) -> Self {
        if let Some(card) = parse_subagent(content) {
            return card;
        }
        if let Some(card) = parse_workflow(content) {
            return card;
        }
        if let Some(card) = parse_ci_run(content, unix_seconds()) {
            return card;
        }
        if let Some(card) = parse_file_change(content) {
            return card;
        }
        let summary = compact(content, 180, "Used a tool");
        let failure = generic_failure(&summary);
        let detail = compact(content, 4_000, "Used a tool");
        let detail = if detail != summary {
            detail
        } else {
            String::new()
        };
        Self {
            kind: if failure {
                ActivityKind::Failure
            } else {
                ActivityKind::Finished
            },
            title: "Activity".into(),
            name: if summary.eq_ignore_ascii_case("error") {
                "Error".into()
            } else {
                summary
            },
            status: if failure { "Failed" } else { "Done" }.into(),
            elapsed: None,
            detail,
            footer: None,
            url: None,
            items: Vec::new(),
            patch: None,
        }
    }

    pub fn with_workflow_status(mut self, status: Option<&Value>, pending: bool) -> Self {
        if self.title != "Workflow" {
            return self;
        }
        let Some(status) = status else {
            if pending {
                self.status = "Checking…".into();
            }
            return self;
        };
        if status.get("ok").and_then(Value::as_bool) != Some(true) {
            self.kind = ActivityKind::Failure;
            self.status = "Status unavailable".into();
            self.detail = status
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("GitHub did not return this workflow run.")
                .to_owned();
            return self;
        }
        if status.get("pending").and_then(Value::as_bool) == Some(true) {
            self.status = "Checking…".into();
            self.detail = "Waiting for GitHub to report this run.".into();
            return self;
        }

        if let Some(name) = status
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        {
            self.name = compact(name, 120, &self.name);
        }
        let state = status
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let conclusion = status
            .get("conclusion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let stale = status.get("stale").and_then(Value::as_bool) == Some(true);
        let (kind, label) = workflow_state(state, conclusion);
        self.kind = kind;
        let now = unix_seconds();
        self.elapsed = workflow_elapsed(status, state == "completed", now);
        self.status = if stale {
            format!("{label} · cached")
        } else {
            label.into()
        };
        self.items = workflow_jobs(status, now);
        self.detail = if self.items.is_empty() {
            "GitHub has not reported any jobs for this run.".into()
        } else {
            format!(
                "{} job{}",
                self.items.len(),
                if self.items.len() == 1 { "" } else { "s" }
            )
        };
        self
    }
}

fn workflow_state(state: &str, conclusion: &str) -> (ActivityKind, &'static str) {
    if state == "completed" {
        return match conclusion {
            "success" => (ActivityKind::Success, "Passed"),
            "failure" | "timed_out" | "startup_failure" => (ActivityKind::Failure, "Failed"),
            "cancelled" => (ActivityKind::Finished, "Cancelled"),
            "skipped" | "neutral" => (ActivityKind::Finished, "Finished"),
            _ => (ActivityKind::Finished, "Completed"),
        };
    }
    match state {
        "in_progress" => (ActivityKind::Running, "In progress"),
        "queued" | "pending" | "requested" | "waiting" => (ActivityKind::Running, "Queued"),
        _ => (ActivityKind::Running, "Running"),
    }
}

fn workflow_jobs(status: &Value, now: i64) -> Vec<ActivityItem> {
    let Some(jobs) = status.get("jobs").and_then(Value::as_array) else {
        return Vec::new();
    };
    jobs.iter()
        .take(100)
        .filter_map(|job| {
            let name = job.get("name").and_then(Value::as_str)?;
            let state = job
                .get("status")
                .or_else(|| job.get("state"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let conclusion = job
                .get("conclusion")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (kind, label) = workflow_state(state, conclusion);
            let detail = job
                .get("log")
                .and_then(Value::as_str)
                .filter(|activity| !activity.is_empty())
                .map(str::to_owned);
            Some(ActivityItem {
                kind,
                name: compact(name, 160, "Job"),
                status: label.into(),
                elapsed: workflow_elapsed(job, state == "completed", now),
                detail,
            })
        })
        .collect()
}

fn workflow_elapsed(status: &Value, terminal: bool, now: i64) -> Option<String> {
    let started_at = status.get("started_at")?.as_i64()?;
    let completed_at = status.get("completed_at").and_then(Value::as_i64);
    if started_at < 0 || completed_at.is_some_and(|completed_at| completed_at < 0) {
        return None;
    }
    let finished_at = if terminal { completed_at? } else { now };
    Some(duration_label(
        finished_at.saturating_sub(started_at).max(0) as u64,
    ))
}

fn duration_label(seconds: u64) -> String {
    if seconds >= 3_600 {
        format!("{}h {:02}m", seconds / 3_600, (seconds % 3_600) / 60)
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn generic_failure(summary: &str) -> bool {
    let summary = summary.trim().to_ascii_lowercase();
    matches!(summary.as_str(), "error" | "failed" | "failure" | "errored")
        || summary.starts_with("error:")
        || summary.starts_with("failed:")
        || summary.starts_with("failure:")
}

fn parse_subagent(content: &str) -> Option<ActivityCard> {
    let lines = content
        .strip_prefix(SUBAGENT_PREFIX)?
        .lines()
        .collect::<Vec<_>>();
    let (identity, task) = match lines.as_slice() {
        [identity, task] => (*identity, *task),
        [_key, identity, task] => (*identity, *task),
        _ => return None,
    };
    if identity.is_empty() || task.is_empty() {
        return None;
    }
    let first = task.split(" · ").next().unwrap_or_default();
    let (kind, status) = match first.to_ascii_lowercase().as_str() {
        "completed" | "done" | "succeeded" => (ActivityKind::Success, "Completed"),
        "failed" | "errored" | "spawn failed" => (ActivityKind::Failure, "Failed"),
        "interrupted" | "stopped" | "not found" => (ActivityKind::Finished, first),
        _ => (ActivityKind::Running, "Running"),
    };
    Some(ActivityCard {
        kind,
        title: "Subagent".into(),
        name: compact(identity, 120, "Agent"),
        status: status.into(),
        elapsed: None,
        detail: compact(task, 360, "Delegated task"),
        footer: None,
        url: None,
        items: Vec::new(),
        patch: None,
    })
}

/// Whether `content` is the card of a CI watch, drawn as a card of its own.
pub fn is_ci_run(content: &str) -> bool {
    content.starts_with(CI_RUN_PREFIX)
}

/// The card the host keeps for a CI watch an agent asked for with xd's
/// watch_ci tool: the pull request's checks, rewritten in place as they go.
fn parse_ci_run(content: &str, now: i64) -> Option<ActivityCard> {
    let record: Value = serde_json::from_str(content.strip_prefix(CI_RUN_PREFIX)?).ok()?;
    let url = record.get("url").and_then(Value::as_str)?;
    let text = |key: &str| record.get(key).and_then(Value::as_str).unwrap_or_default();
    let (kind, status) = match text("status") {
        "passed" => (ActivityKind::Success, "Passed"),
        "failed" => (ActivityKind::Failure, "Failed"),
        "stopped" => (ActivityKind::Finished, "Stopped"),
        _ => (ActivityKind::Running, "Running"),
    };
    let checks = record
        .get("checks")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let items = checks
        .iter()
        .filter_map(|check| {
            let name = check.get("name").and_then(Value::as_str)?;
            let name = match check.get("workflow").and_then(Value::as_str) {
                Some(workflow) if !workflow.is_empty() => format!("{workflow} / {name}"),
                _ => name.to_owned(),
            };
            let (kind, status) = match check.get("state").and_then(Value::as_str) {
                Some("passed") => (ActivityKind::Success, "Passed".to_owned()),
                Some("skipped") => (ActivityKind::Finished, "Skipped".to_owned()),
                Some("failed") => (
                    ActivityKind::Failure,
                    check
                        .get("conclusion")
                        .and_then(Value::as_str)
                        .map(sentence_case)
                        .unwrap_or_else(|| "Failed".into()),
                ),
                _ => (ActivityKind::Running, "Running".to_owned()),
            };
            Some(ActivityItem {
                kind,
                name: compact(&name, 120, "Check"),
                status,
                elapsed: None,
                detail: None,
            })
        })
        .collect::<Vec<_>>();
    // The host counts every check; the list may be cut short.
    let listed = |kind: ActivityKind| items.iter().filter(|item| item.kind == kind).count();
    let counted = |key: &str, kind: ActivityKind| {
        record
            .get("counts")
            .and_then(|counts| counts.get(key))
            .and_then(Value::as_u64)
            .map_or_else(|| listed(kind), |count| count as usize)
    };
    let (running, failed, passed, skipped) = (
        counted("pending", ActivityKind::Running),
        counted("failed", ActivityKind::Failure),
        counted("passed", ActivityKind::Success),
        counted("skipped", ActivityKind::Finished),
    );
    let total = running + failed + passed + skipped;
    let mut items = items;
    if total > items.len() {
        items.push(ActivityItem {
            kind: ActivityKind::Finished,
            name: format!("{} more", total - items.len()),
            status: String::new(),
            elapsed: None,
            detail: None,
        });
    }
    let detail = if let Some(reason) = record.get("reason").and_then(Value::as_str) {
        reason.to_owned()
    } else if let Some(commit) = record.get("waiting_for").and_then(Value::as_str) {
        format!("Waiting for GitHub to show {commit}.")
    } else if items.is_empty() {
        "Waiting for checks to start.".into()
    } else if running > 0 {
        format!("{running} of {total} checks running")
    } else if failed > 0 {
        format!("{failed} failed, {passed} passed")
    } else {
        format!("{passed} passed")
    };
    let number = record.get("number").and_then(Value::as_u64);
    let name = match (number, text("title")) {
        (Some(number), "") => format!("#{number}"),
        (Some(number), title) => format!("#{number} · {title}"),
        (None, _) => "Pull request".into(),
    };
    let elapsed = record
        .get("started_at")
        .and_then(Value::as_i64)
        .map(|started| {
            let ended = record
                .get("finished_at")
                .and_then(Value::as_i64)
                .unwrap_or(now);
            duration_label(ended.saturating_sub(started).max(0) as u64)
        });
    let head = text("head");
    Some(ActivityCard {
        kind,
        title: "CI".into(),
        name: compact(&name, 120, "Pull request"),
        status: status.into(),
        elapsed,
        detail,
        footer: (!head.is_empty()).then(|| format!("Head {head}")),
        url: Some(format!("{url}/checks")),
        items,
        patch: None,
    })
}

fn sentence_case(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn parse_workflow(content: &str) -> Option<ActivityCard> {
    let mut lines = content.strip_prefix(WORKFLOW_PREFIX)?.lines();
    let id = lines.next()?.trim();
    let url = lines.next()?.trim();
    if id.is_empty() || url.is_empty() || lines.next().is_some() {
        return None;
    }
    let repository = url
        .split("github.com/")
        .nth(1)
        .and_then(|path| path.split("/actions/runs/").next())
        .filter(|repository| repository.contains('/'))
        .unwrap_or("GitHub Actions");
    Some(ActivityCard {
        kind: ActivityKind::Running,
        title: "Workflow".into(),
        name: compact(repository, 120, "GitHub Actions"),
        status: format!("Run #{id}"),
        elapsed: None,
        detail: "Waiting for GitHub to report this run.".into(),
        footer: Some("GitHub Actions".into()),
        url: Some(url.into()),
        items: Vec::new(),
        patch: None,
    })
}

/// A file edit arrives as the marker `file_change`, a newline, and a unified
/// patch — the shape the host writes and every client reads.
fn parse_file_change(content: &str) -> Option<ActivityCard> {
    let patch = content.strip_prefix(FILE_CHANGE_PREFIX)?;
    if !patch.starts_with(PATCH_MARKER) {
        return None;
    }
    let files = changed_files(patch);
    let (added, removed) = change_counts(patch);
    Some(ActivityCard {
        kind: ActivityKind::Finished,
        title: "File change".into(),
        name: match files.len() {
            0 => "Edited files".into(),
            1 => files[0].clone(),
            count => format!("{count} files changed"),
        },
        status: format!("+{added} −{removed}"),
        elapsed: None,
        detail: String::new(),
        footer: None,
        url: None,
        items: Vec::new(),
        patch: Some(patch.to_owned()),
    })
}

/// The `b/` paths a patch touches, in order. A rename reports its destination,
/// which is what someone scanning the change is looking for.
fn changed_files(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|line| line.strip_prefix(PATCH_MARKER))
        .filter_map(|header| header.split_once(" b/"))
        .map(|(_, path)| path.trim().to_owned())
        .filter(|path| !path.is_empty())
        .collect()
}

fn change_counts(patch: &str) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

fn patch_fits_expanded_viewport(patch: &str) -> bool {
    let mut visible_lines = 0;
    for line in patch.lines() {
        // The inline renderer replaces these Git headers with its own compact
        // file heading. Count that heading, but not the metadata it replaces.
        if line.starts_with(PATCH_MARKER) {
            visible_lines += 1;
        } else if line.starts_with("index ") || line.starts_with("--- ") || line.starts_with("+++ ")
        {
            continue;
        } else {
            visible_lines += 1;
        }
        if visible_lines > MAX_AUTO_EXPANDED_PATCH_LINES {
            return false;
        }
    }
    true
}

fn compact(value: &str, limit: usize, fallback: &str) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        return fallback.into();
    }
    if value.chars().count() <= limit {
        return value;
    }
    let mut shortened = value
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    shortened.push('…');
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subagent_records_parse_with_their_steps_and_stats() {
        let view = SubagentView::parse(
            r#"subagent_run
{"key":"toolu_1","provider":"Claude","agent":"Explore","description":"List files","status":"running","background":true,"activity":"Reading main.rs","steps":[{"tool":"Bash","text":"$ ls"}],"tool_uses":2,"tokens":13901,"duration_ms":2940}"#,
        )
        .unwrap();
        assert_eq!(view.agent, "Explore");
        assert_eq!(view.status, SubagentState::Running);
        assert!(view.background);
        assert_eq!(view.steps[0].text, "$ ls");
        assert_eq!(view.stats(), "2 tools · 13.9k tokens · 2.9s");
        assert!(!is_plain_activity("subagent_run\n{}"));
        // Claude counts calls the record never saw as steps; only a list cut
        // at the host's limit has steps it no longer shows.
        assert_eq!(view.dropped_steps(), 0);
        let running = SubagentView::parse(
            r#"subagent_run
{"agent":"Explore","status":"running","tool_uses":1,"started_at":1000}"#,
        )
        .unwrap();
        assert_eq!(running.stats_at(48_000), "1 tool · 47s");
        assert!(SubagentView::parse("subagent\nExplore\nRunning · x").is_none());
    }

    #[test]
    fn claude_controls_agents_it_runs_as_commands() {
        let parse = |record: &str| SubagentView::parse(&format!("subagent_run\n{record}")).unwrap();
        let agent =
            parse(r#"{"key":"t","provider":"Claude","agent":"Explore","status":"running"}"#);
        assert!(agent.can_stop() && agent.can_background());
        let codex = parse(r#"{"key":"t","provider":"Codex","agent":"worker","status":"running"}"#);
        assert!(!codex.can_stop() && !codex.can_background());

        // Claude can background the command; only then can it stop it.
        let launched = parse(
            r#"{"key":"toolu_b","provider":"Codex","launcher":"Claude","agent":"audit","status":"running"}"#,
        );
        assert_eq!(launched.launcher, "Claude");
        assert!(!launched.is_shell());
        assert!(!launched.can_stop() && launched.can_background());
        let backgrounded = parse(
            r#"{"key":"toolu_b","provider":"Codex","launcher":"Claude","agent":"audit","status":"running","background":true}"#,
        );
        assert!(backgrounded.can_stop() && !backgrounded.can_background());
        // A Claude that Codex runs is Codex's, and Codex takes no orders.
        let from_codex = parse(
            r#"{"key":"c","provider":"Claude","launcher":"Codex","agent":"Claude agent","status":"running"}"#,
        );
        assert!(!from_codex.can_stop() && !from_codex.can_background());
    }

    #[test]
    fn counts_and_durations_read_compactly() {
        assert_eq!(compact_count(950), "950");
        assert_eq!(compact_count(16_210), "16.2k");
        assert_eq!(compact_count(200_000), "200k");
        assert_eq!(compact_count(1_200_000), "1.2M");
        assert_eq!(compact_duration(2_940), "2.9s");
        assert_eq!(compact_duration(47_000), "47s");
        assert_eq!(compact_duration(185_000), "3m 05s");
        assert_eq!(compact_duration(3_720_000), "1h 02m");
    }

    #[test]
    fn parses_keyed_and_legacy_subagents_into_the_shared_model() {
        let keyed = ActivityCard::parse(
            "subagent\nthread-1\nCodex · gpt-5.6-sol · high\nRunning · Review the diff",
        );
        assert_eq!(keyed.title, "Subagent");
        assert_eq!(keyed.name, "Codex · gpt-5.6-sol · high");
        assert_eq!(keyed.status, "Running");
        assert_eq!(keyed.kind, ActivityKind::Running);

        let legacy = ActivityCard::parse("subagent\nExplore\nCompleted · Parser traced");
        assert_eq!(legacy.status, "Completed");
        assert_eq!(legacy.kind, ActivityKind::Success);
    }

    #[test]
    fn parses_workflows_into_the_same_card_model() {
        let card = ActivityCard::parse(
            "workflow_run\n31028502744\nhttps://github.com/RestartFU/xd/actions/runs/31028502744",
        );
        assert_eq!(card.title, "Workflow");
        assert_eq!(card.name, "RestartFU/xd");
        assert_eq!(card.status, "Run #31028502744");
        assert_eq!(card.kind, ActivityKind::Running);
        assert_eq!(
            card.url.as_deref(),
            Some("https://github.com/RestartFU/xd/actions/runs/31028502744")
        );
    }

    #[test]
    fn workflow_status_hydrates_the_shared_card_without_losing_cached_state() {
        let marker =
            "workflow_run\n31028502744\nhttps://github.com/RestartFU/xd/actions/runs/31028502744";
        let running = ActivityCard::parse(marker).with_workflow_status(
            Some(&serde_json::json!({
                "ok": true,
                "name": "GPUI dev",
                "state": "in_progress",
                "stale": true,
                "jobs": [{
                    "name": "linux",
                    "state": "in_progress",
                    "log": "Build release"
                }]
            })),
            false,
        );
        assert_eq!(running.name, "GPUI dev");
        assert_eq!(running.kind, ActivityKind::Running);
        assert_eq!(running.status, "In progress · cached");
        assert_eq!(running.detail, "1 job");
        assert_eq!(
            running.items,
            vec![ActivityItem {
                kind: ActivityKind::Running,
                name: "linux".into(),
                status: "In progress".into(),
                elapsed: None,
                detail: Some("Build release".into()),
            }]
        );

        let failed = ActivityCard::parse(marker).with_workflow_status(
            Some(&serde_json::json!({
                "ok": true,
                "name": "GPUI dev",
                "state": "completed",
                "conclusion": "failure",
                "jobs": []
            })),
            false,
        );
        assert_eq!(failed.kind, ActivityKind::Failure);
        assert_eq!(failed.status, "Failed");
    }

    #[test]
    fn workflow_status_formats_run_and_job_clocks_with_units() {
        let marker =
            "workflow_run\n31028502744\nhttps://github.com/RestartFU/xd/actions/runs/31028502744";
        let card = ActivityCard::parse(marker).with_workflow_status(
            Some(&serde_json::json!({
                "ok": true,
                "state": "completed",
                "conclusion": "success",
                "started_at": 10,
                "completed_at": 3_671,
                "jobs": [{
                    "name": "linux",
                    "state": "completed",
                    "conclusion": "success",
                    "started_at": 10,
                    "completed_at": 75
                }, {
                    "name": "missing clock",
                    "state": "completed",
                    "started_at": 10
                }]
            })),
            false,
        );

        assert_eq!(card.elapsed.as_deref(), Some("1h 01m"));
        assert_eq!(card.items[0].elapsed.as_deref(), Some("1m 05s"));
        assert_eq!(card.items[1].elapsed, None);
        assert_eq!(duration_label(5), "5s");
    }

    #[test]
    fn generic_tools_stay_compact_and_bounded() {
        let card = ActivityCard::parse(&"read  ".repeat(1_000));
        assert_eq!(card.title, "Activity");
        assert!(card.name.chars().count() <= 180);
        assert!(card.detail.chars().count() <= 4_000);
        assert_eq!(card.kind, ActivityKind::Finished);
    }

    #[test]
    fn expanding_a_command_shows_more_than_its_truncated_header() {
        let command = format!(
            "$ docker run --rm debian:trixie-slim sh -c '{}'",
            "x".repeat(400)
        );
        let card = ActivityCard::parse(&command);

        assert!(card.name.ends_with('…'));
        assert_eq!(card.detail, command);
        assert_ne!(card.detail, card.name);
        assert!(card.can_expand());
    }

    #[test]
    fn a_short_single_command_has_no_duplicate_disclosure_content() {
        let command = "$ git status --short && git log -3 --oneline";
        let card = ActivityCard::parse(command);

        assert_eq!(card.name, command);
        assert!(card.detail.is_empty());
        assert!(!card.can_expand());
    }

    #[test]
    fn file_changes_carry_their_patch_and_a_counted_header() {
        let patch = "diff --git a/src/main.rs b/src/main.rs\n\
                     --- a/src/main.rs\n\
                     +++ b/src/main.rs\n\
                     @@ -1,2 +1,3 @@\n\
                     context\n\
                     -removed\n\
                     +added\n\
                     +also added\n";
        let card = ActivityCard::parse(&format!("file_change\n{patch}"));

        assert_eq!(card.title, "File change");
        assert_eq!(card.name, "src/main.rs");
        assert_eq!(card.status, "+2 −1");
        assert_eq!(card.patch.as_deref(), Some(patch));
        assert!(card.starts_expanded());
        assert!(card.detail.is_empty(), "the patch replaces the detail text");

        let two = ActivityCard::parse(&format!(
            "file_change\n{patch}diff --git a/b.rs b/b.rs\n+one\n"
        ));
        assert_eq!(two.name, "2 files changed");
        assert_eq!(two.status, "+3 −1");
    }

    #[test]
    fn large_file_changes_start_collapsed() {
        let patch = format!(
            "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n{}",
            (0..MAX_AUTO_EXPANDED_PATCH_LINES)
                .map(|index| format!("+line {index}\n"))
                .collect::<String>()
        );
        let card = ActivityCard::parse(&format!("file_change\n{patch}"));

        assert!(!card.starts_expanded());
        assert!(card.can_expand());
    }

    #[test]
    fn malformed_file_change_markers_fall_back_to_plain_activity() {
        let card = ActivityCard::parse("file_change\nnot a patch");
        assert_eq!(card.title, "Activity");
        assert_eq!(card.patch, None);
        assert!(!card.starts_expanded());
    }

    #[test]
    fn only_plain_activity_stacks_into_a_run() {
        assert!(is_plain_activity("$ make build"));
        assert!(!is_plain_activity(
            "context_compaction\nrunning\nCompacting context"
        ));
        assert!(!is_plain_activity("subagent\nExplore\nRunning · Tracing"));
        assert!(!is_plain_activity(
            "workflow_run\n123\nhttps://github.com/RestartFU/xd/actions/runs/123"
        ));
    }

    #[test]
    fn explicit_generic_errors_are_not_reported_as_done() {
        for content in ["error", "failed", "Error: agent process exited"] {
            let card = ActivityCard::parse(content);
            assert_eq!(card.kind, ActivityKind::Failure);
            assert_eq!(card.status, "Failed");
        }
        assert_eq!(ActivityCard::parse("read file").status, "Done");
    }

    #[test]
    fn an_agent_report_names_its_chat_title_and_outcome() {
        assert_eq!(
            agent_report(
                "[xd] Agent \"Fix (the) tests\" (c-1) is done with its turn.\n\nIts reply:\n\nOk."
            ),
            Some(AgentReport {
                chat: "c-1".into(),
                title: "Fix (the) tests".into(),
                outcome: AgentOutcome::Done,
            })
        );
        assert_eq!(
            agent_report("[xd] An agent (c-2) is stopped with an error.")
                .map(|report| (report.title, report.outcome)),
            Some((String::new(), AgentOutcome::Failed))
        );
        assert_eq!(
            agent_report("[xd] Agent \"Q\" (c-3) is waiting for an answer.").map(|r| r.outcome),
            Some(AgentOutcome::Waiting)
        );
        assert_eq!(
            agent_report("Agent \"Q\" (c-3) is done with its turn."),
            None
        );
        assert_eq!(agent_report("[xd] something else"), None);
    }

    #[test]
    fn compaction_rows_are_read_from_the_host_record() {
        assert_eq!(
            compaction_notice("context_compaction\ncompleted\nContext compacted 251K → 32K tokens"),
            Some((
                CompactionState::Completed,
                "Context compacted 251K → 32K tokens"
            ))
        );
        assert_eq!(
            compaction_notice("context_compaction\nfailed\nContext compaction failed · No"),
            Some((CompactionState::Failed, "Context compaction failed · No"))
        );
        assert_eq!(compaction_notice("context_compaction\nodd\nx"), None);
        assert_eq!(compaction_notice("context_compaction\nrunning\n "), None);
        assert_eq!(compaction_notice("$ context_compaction\nrunning\nx"), None);
    }

    #[test]
    fn tokens_read_as_t3_code_writes_them() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(1_250), "1.25K");
        assert_eq!(format_tokens(1_500), "1.50K");
        assert_eq!(format_tokens(12_500), "12.5K");
        assert_eq!(format_tokens(251_000), "251K");
        assert_eq!(format_tokens(999_999), "1M");
    }

    #[test]
    fn a_ci_report_names_its_pull_request_and_outcome() {
        let report = |text: &str| ci_report(text).map(|report| (report.outcome, report.number));
        assert_eq!(
            ci_report(
                "[xd] CI failed on https://github.com/o/r/pull/291 at 1e3d4a4.\n\n1 of 2 checks did not pass"
            ),
            Some(CiReport {
                url: "https://github.com/o/r/pull/291".into(),
                number: "291".into(),
                outcome: CiOutcome::Failed,
            })
        );
        assert_eq!(
            report("[xd] CI passed on https://github.com/o/r/pull/7 at abc."),
            Some((CiOutcome::Passed, "7".into()))
        );
        assert_eq!(
            report("[xd] Stopped watching CI on https://github.com/o/r/pull/7.\n\nMerged."),
            Some((CiOutcome::Stopped, "7".into()))
        );
        assert_eq!(
            report("CI passed on https://github.com/o/r/pull/7 at abc."),
            None
        );
        assert_eq!(
            report("[xd] CI passed on https://github.com/o/r/issues/7 at a."),
            None
        );
        assert_eq!(report("[xd] CI passed on the moon."), None);
    }

    #[test]
    fn a_ci_card_lists_its_checks_and_how_they_stand() {
        let record = json!({"url": "https://github.com/o/r/pull/300", "number": 300,
        "title": "Fix the settings test", "head": "9f2c7b1", "status": "running",
        "started_at": 1_000, "checks": [
            {"name": "linux", "workflow": "pr", "state": "pending"},
            {"name": "macos", "workflow": "pr", "state": "passed"},
            {"name": "lint", "state": "failed", "conclusion": "timed out"},
        ]});
        let card = parse_ci_run(&format!("ci_run\n{record}"), 1_125).unwrap();
        assert_eq!(card.kind, ActivityKind::Running);
        assert_eq!(card.title, "CI");
        assert_eq!(card.name, "#300 · Fix the settings test");
        assert_eq!(card.status, "Running");
        assert_eq!(card.elapsed.as_deref(), Some("2m 05s"));
        assert_eq!(card.detail, "1 of 3 checks running");
        assert_eq!(card.footer.as_deref(), Some("Head 9f2c7b1"));
        assert_eq!(
            card.url.as_deref(),
            Some("https://github.com/o/r/pull/300/checks")
        );
        let items = card
            .items
            .iter()
            .map(|item| (item.name.as_str(), item.kind, item.status.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            items,
            [
                ("pr / linux", ActivityKind::Running, "Running"),
                ("pr / macos", ActivityKind::Success, "Passed"),
                ("lint", ActivityKind::Failure, "Timed out"),
            ]
        );
        assert!(!is_plain_activity(&format!("ci_run\n{record}")));
        let failed = json!({"url": "https://github.com/o/r/pull/300", "number": 300,
            "status": "failed", "started_at": 1_000, "finished_at": 1_060, "checks": [
                {"name": "linux", "state": "failed", "conclusion": "failure"},
                {"name": "macos", "state": "passed"}]});
        let card = ActivityCard::parse(&format!("ci_run\n{failed}"));
        assert_eq!(
            (card.kind, card.status.as_str()),
            (ActivityKind::Failure, "Failed")
        );
        assert_eq!(card.detail, "1 failed, 1 passed");
        assert_eq!(card.elapsed.as_deref(), Some("1m 00s"));
        let stopped = json!({"url": "https://github.com/o/r/pull/7", "status": "stopped",
            "reason": "Watched again below.", "checks": []});
        let card = ActivityCard::parse(&format!("ci_run\n{stopped}"));
        assert_eq!(
            (card.kind, card.detail.as_str()),
            (ActivityKind::Finished, "Watched again below.")
        );
        assert_eq!(card.name, "Pull request");
        assert!(parse_ci_run("ci_run\nnot json", 0).is_none());
        // A list cut short says how many more there are; the counts cover all.
        let long = json!({"url": "https://github.com/o/r/pull/7", "status": "running",
            "counts": {"failed": 0, "pending": 1, "passed": 60, "skipped": 0},
            "checks": [{"name": "slow", "state": "pending"}, {"name": "a", "state": "passed"}]});
        let card = ActivityCard::parse(&format!("ci_run\n{long}"));
        assert_eq!(card.detail, "1 of 61 checks running");
        assert_eq!(
            card.items.last().map(|item| item.name.as_str()),
            Some("59 more")
        );
    }
}
