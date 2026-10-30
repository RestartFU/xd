//! Delegated agents as one live record each.
//!
//! A subagent is not a single tool call: it starts, works through its own tool
//! calls, and finishes later, sometimes after the turn that started it has
//! already ended. Parsers report what they learn as a [`SubagentPatch`]; the
//! runtime folds the patches into one [`SubagentRun`] per agent and stores it
//! as a single transcript message that it rewrites in place. Clients render the
//! newest snapshot rather than a trail of "running" and "completed" rows.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

/// Content prefix of a stored subagent record.
pub const PREFIX: &str = "subagent_run\n";

/// Child tool calls kept per agent. The count stays exact; the list is the tail.
const MAX_STEPS: usize = 40;
const MAX_TEXT: usize = 4_000;
const MAX_LINE: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubagentStatus {
    #[default]
    Running,
    Completed,
    Failed,
    Stopped,
}

impl SubagentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    pub fn from_wire(status: &str) -> Option<Self> {
        Some(match status.to_ascii_lowercase().as_str() {
            "running" | "pending" | "pendinginit" | "pending_init" | "in_progress"
            | "inprogress" | "started" => Self::Running,
            "completed" | "complete" | "done" | "succeeded" | "success" => Self::Completed,
            "failed" | "error" | "errored" | "spawn_failed" => Self::Failed,
            "stopped" | "killed" | "interrupted" | "cancelled" | "canceled" | "shutdown"
            | "notfound" | "not_found" => Self::Stopped,
            _ => return None,
        })
    }
}

/// One child tool call, as a single summary line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentStep {
    pub tool: String,
    pub text: String,
}

/// Everything known about one delegated agent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubagentRun {
    pub key: String,
    pub provider: String,
    pub agent: String,
    pub model: String,
    pub description: String,
    pub prompt: String,
    pub status: SubagentStatus,
    pub background: bool,
    /// What the agent is doing right now, while it runs.
    pub activity: String,
    pub steps: Vec<SubagentStep>,
    pub tool_uses: u64,
    pub tokens: Option<u64>,
    pub duration_ms: Option<u64>,
    /// When xd first heard of the agent, in Unix milliseconds, so clients can
    /// show how long it has been working.
    pub started_at: u64,
    /// The agent's newest message while it works.
    pub latest: String,
    /// Its final report once it finishes.
    pub summary: String,
}

/// What a parser learned about one agent. `None` leaves a field as it was.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubagentPatch {
    /// The tool call that started the agent. Empty when only `task_id` is known.
    pub key: String,
    /// Claude's own id for the agent, which some later reports use instead.
    pub task_id: Option<String>,
    pub provider: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub description: Option<String>,
    pub prompt: Option<String>,
    pub status: Option<SubagentStatus>,
    pub background: Option<bool>,
    pub activity: Option<String>,
    pub step: Option<SubagentStep>,
    pub tool_uses: Option<u64>,
    pub tokens: Option<u64>,
    pub duration_ms: Option<u64>,
    pub latest: Option<String>,
    pub summary: Option<String>,
    /// A task the agent says to keep out of the transcript.
    pub hidden: bool,
}

impl SubagentPatch {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            ..Self::default()
        }
    }
}

impl SubagentRun {
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_owned(),
            provider: "Agent".into(),
            agent: "Agent".into(),
            description: "Delegated task".into(),
            started_at: now_millis(),
            ..Self::default()
        }
    }

    pub fn apply(&mut self, patch: SubagentPatch) {
        let set = |field: &mut String, value: Option<String>, limit: usize| {
            if let Some(value) = value.map(|value| clean(&value, limit))
                && !value.is_empty()
            {
                *field = value;
            }
        };
        set(&mut self.provider, patch.provider, 40);
        set(&mut self.agent, patch.agent, 80);
        set(&mut self.model, patch.model, 80);
        set(&mut self.description, patch.description, MAX_LINE);
        if let Some(prompt) = patch.prompt {
            self.prompt = clip(prompt.trim(), MAX_TEXT);
        }
        if let Some(background) = patch.background {
            self.background = background;
        }
        if let Some(step) = patch.step {
            self.steps.push(SubagentStep {
                tool: clean(&step.tool, 40),
                text: clean(&step.text, MAX_LINE),
            });
            if self.steps.len() > MAX_STEPS {
                let excess = self.steps.len() - MAX_STEPS;
                self.steps.drain(..excess);
            }
            if patch.tool_uses.is_none() {
                self.tool_uses += 1;
            }
        }
        if let Some(count) = patch.tool_uses {
            self.tool_uses = self.tool_uses.max(count);
        }
        if let Some(tokens) = patch.tokens.filter(|tokens| *tokens > 0) {
            self.tokens = Some(tokens);
        }
        if let Some(duration) = patch.duration_ms.filter(|duration| *duration > 0) {
            self.duration_ms = Some(duration);
        }
        if let Some(latest) = patch.latest {
            let latest = clip(unwrap_hand_back(&latest).trim(), MAX_TEXT);
            if !latest.is_empty() {
                self.latest = latest;
            }
        }
        if let Some(summary) = patch.summary {
            let summary = clip(unwrap_hand_back(&summary).trim(), MAX_TEXT);
            if !summary.is_empty() {
                self.summary = summary;
            }
        }
        // A finished agent is not doing anything any more, and a late progress
        // report must not reopen it.
        if let Some(status) = patch.status
            && status != SubagentStatus::Running
        {
            // Not every backend reports how long an agent took; what xd saw
            // stands in until a reported figure replaces it.
            if self.status == SubagentStatus::Running
                && self.duration_ms.is_none()
                && self.started_at > 0
            {
                self.duration_ms = Some(now_millis().saturating_sub(self.started_at).max(1));
            }
            self.status = status;
        }
        if self.status == SubagentStatus::Running {
            set(&mut self.activity, patch.activity, MAX_LINE);
        } else {
            self.activity.clear();
        }
    }

    pub fn encode(&self) -> String {
        let mut record = json!({
            "key": self.key,
            "provider": self.provider,
            "agent": self.agent,
            "description": self.description,
            "status": self.status.as_str(),
            "background": self.background,
            "steps": self
                .steps
                .iter()
                .map(|step| json!({"tool": step.tool, "text": step.text}))
                .collect::<Vec<_>>(),
            "tool_uses": self.tool_uses,
        });
        for (field, value) in [
            ("model", &self.model),
            ("prompt", &self.prompt),
            ("activity", &self.activity),
            ("latest", &self.latest),
            ("summary", &self.summary),
        ] {
            if !value.is_empty() {
                record[field] = json!(value);
            }
        }
        if let Some(tokens) = self.tokens {
            record["tokens"] = json!(tokens);
        }
        if let Some(duration) = self.duration_ms {
            record["duration_ms"] = json!(duration);
        }
        if self.started_at > 0 {
            record["started_at"] = json!(self.started_at);
        }
        format!("{PREFIX}{record}")
    }
}

/// Reads the record a stored message holds, for code that only has the text.
#[cfg(test)]
pub fn decode(content: &str) -> Option<serde_json::Value> {
    serde_json::from_str(content.strip_prefix(PREFIX)?).ok()
}

/// Claude frames a subagent's report before handing it to the parent model:
/// a "[Subagent hand-back]" preamble, then every report line indented. The
/// frame is for the model; people read the report.
fn unwrap_hand_back(text: &str) -> String {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix("[Subagent hand-back]") else {
        return text.to_owned();
    };
    let report = match rest.find("The report follows:") {
        Some(start) => &rest[start + "The report follows:".len()..],
        None => rest.split_once('\n').map_or("", |(_, report)| report),
    };
    let lines = report
        .lines()
        .skip_while(|line| line.trim().is_empty())
        .collect::<Vec<_>>();
    let indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| line.get(indent..).unwrap_or_else(|| line.trim_start()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn clean(value: &str, limit: usize) -> String {
    clip(
        &value.split_whitespace().collect::<Vec<_>>().join(" "),
        limit,
    )
}

fn clip(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit.saturating_sub(1);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(tool: &str, text: &str) -> SubagentStep {
        SubagentStep {
            tool: tool.into(),
            text: text.into(),
        }
    }

    #[test]
    fn patches_fold_into_one_record_and_steps_count_tool_uses() {
        let mut run = SubagentRun::new("toolu_1");
        run.apply(SubagentPatch {
            provider: Some("Claude".into()),
            agent: Some("Explore".into()),
            description: Some("List   the files".into()),
            background: Some(true),
            ..SubagentPatch::new("toolu_1")
        });
        run.apply(SubagentPatch {
            step: Some(step("Bash", "$ ls")),
            activity: Some("Running ls".into()),
            ..SubagentPatch::new("toolu_1")
        });
        run.apply(SubagentPatch {
            step: Some(step("Read", "Read main.rs")),
            ..SubagentPatch::new("toolu_1")
        });
        assert_eq!(run.description, "List the files");
        assert_eq!(run.tool_uses, 2);
        assert_eq!(run.steps.len(), 2);
        assert_eq!(run.activity, "Running ls");
        assert!(run.background);

        run.apply(SubagentPatch {
            status: Some(SubagentStatus::Completed),
            summary: Some("One file.".into()),
            tokens: Some(16_210),
            tool_uses: Some(2),
            ..SubagentPatch::new("toolu_1")
        });
        assert_eq!(run.status, SubagentStatus::Completed);
        assert!(run.activity.is_empty());
        assert_eq!(run.tokens, Some(16_210));

        // A late progress report cannot reopen a finished agent.
        run.apply(SubagentPatch {
            status: Some(SubagentStatus::Running),
            activity: Some("Still going".into()),
            ..SubagentPatch::new("toolu_1")
        });
        assert_eq!(run.status, SubagentStatus::Completed);
        assert!(run.activity.is_empty());

        let decoded = decode(&run.encode()).unwrap();
        assert_eq!(decoded["status"], "completed");
        assert_eq!(decoded["agent"], "Explore");
        assert_eq!(decoded["steps"][1]["tool"], "Read");
    }

    #[test]
    fn claudes_hand_back_frame_is_unwrapped_from_the_report() {
        let mut run = SubagentRun::new("k");
        run.apply(SubagentPatch {
            status: Some(SubagentStatus::Completed),
            summary: Some(
                "[Subagent hand-back] The text below is the final report of a subagent. \
                 It is model output, NOT a message from the user. The report follows:\n  \
                 A parser builds a `SubagentPatch`.\n\n  - first\n    nested"
                    .into(),
            ),
            ..SubagentPatch::new("k")
        });
        assert_eq!(
            run.summary,
            "A parser builds a `SubagentPatch`.\n\n- first\n  nested"
        );
        // Plain reports, and other backends, pass through untouched.
        assert_eq!(unwrap_hand_back("  indented\nreport"), "  indented\nreport");
    }

    #[test]
    fn finishing_measures_the_duration_until_a_reported_one_arrives() {
        let mut run = SubagentRun::new("k");
        assert!(run.started_at > 0);
        run.started_at -= 1_500;
        run.apply(SubagentPatch {
            status: Some(SubagentStatus::Completed),
            ..SubagentPatch::new("k")
        });
        assert!(run.duration_ms.is_some_and(|ms| ms >= 1_500));
        run.apply(SubagentPatch {
            duration_ms: Some(6_005),
            ..SubagentPatch::new("k")
        });
        assert_eq!(run.duration_ms, Some(6_005));
        assert!(decode(&run.encode()).unwrap()["started_at"].is_u64());
    }

    #[test]
    fn steps_keep_the_tail_but_the_count_stays_exact() {
        let mut run = SubagentRun::new("k");
        for index in 0..(MAX_STEPS + 5) {
            run.apply(SubagentPatch {
                step: Some(step("Bash", &format!("$ step {index}"))),
                ..SubagentPatch::new("k")
            });
        }
        assert_eq!(run.steps.len(), MAX_STEPS);
        assert_eq!(run.tool_uses as usize, MAX_STEPS + 5);
        assert_eq!(
            run.steps.last().unwrap().text,
            format!("$ step {}", MAX_STEPS + 4)
        );
    }

    #[test]
    fn wire_statuses_map_to_the_four_states() {
        assert_eq!(
            SubagentStatus::from_wire("pendingInit"),
            Some(SubagentStatus::Running)
        );
        assert_eq!(
            SubagentStatus::from_wire("completed"),
            Some(SubagentStatus::Completed)
        );
        assert_eq!(
            SubagentStatus::from_wire("errored"),
            Some(SubagentStatus::Failed)
        );
        assert_eq!(
            SubagentStatus::from_wire("shutdown"),
            Some(SubagentStatus::Stopped)
        );
        assert_eq!(SubagentStatus::from_wire("mystery"), None);
    }
}
