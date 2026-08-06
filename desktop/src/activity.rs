const SUBAGENT_PREFIX: &str = "subagent\n";
const WORKFLOW_PREFIX: &str = "workflow_run\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityCard {
    pub kind: ActivityKind,
    pub title: String,
    pub name: String,
    pub status: String,
    pub detail: String,
    pub footer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Running,
    Success,
    Failure,
    Finished,
}

impl ActivityCard {
    pub fn parse(content: &str) -> Self {
        if let Some(card) = parse_subagent(content) {
            return card;
        }
        if let Some(card) = parse_workflow(content) {
            return card;
        }
        let summary = compact(content, 180, "Used a tool");
        let failure = generic_failure(&summary);
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
                summary.clone()
            },
            status: if failure { "Failed" } else { "Done" }.into(),
            detail: summary,
            footer: None,
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
        self.status = if stale {
            format!("{label} · cached")
        } else {
            label.into()
        };
        if let Some(detail) = workflow_jobs(status) {
            self.detail = detail;
        }
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

fn workflow_jobs(status: &Value) -> Option<String> {
    let jobs = status.get("jobs")?.as_array()?;
    if jobs.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for job in jobs.iter().take(100) {
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
        let (_, label) = workflow_state(state, conclusion);
        let activity = job
            .get("log")
            .and_then(Value::as_str)
            .filter(|activity| !activity.is_empty())
            .map(|activity| format!(" · {activity}"))
            .unwrap_or_default();
        lines.push(format!("{name} · {label}{activity}"));
    }
    Some(lines.join("\n"))
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
        detail: compact(task, 360, "Delegated task"),
        footer: None,
    })
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
        detail: url.into(),
        footer: Some("GitHub Actions".into()),
    })
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
        assert_eq!(running.detail, "linux · In progress · Build release");

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
    fn generic_tools_stay_compact_and_bounded() {
        let card = ActivityCard::parse(&"read  ".repeat(1_000));
        assert_eq!(card.title, "Activity");
        assert!(card.name.chars().count() <= 180);
        assert_eq!(card.kind, ActivityKind::Finished);
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
}
use serde_json::Value;
