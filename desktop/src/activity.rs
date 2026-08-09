const SUBAGENT_PREFIX: &str = "subagent\n";
const WORKFLOW_PREFIX: &str = "workflow_run\n";
const FILE_CHANGE_PREFIX: &str = "file_change\n";
const PATCH_MARKER: &str = "diff --git ";

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
        && !content.starts_with(WORKFLOW_PREFIX)
        && !content.starts_with(FILE_CHANGE_PREFIX)
}

impl ActivityCard {
    pub fn parse(content: &str) -> Self {
        if let Some(card) = parse_subagent(content) {
            return card;
        }
        if let Some(card) = parse_workflow(content) {
            return card;
        }
        if let Some(card) = parse_file_change(content) {
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
                summary
            },
            status: if failure { "Failed" } else { "Done" }.into(),
            elapsed: None,
            detail: compact(content, 4_000, "Used a tool"),
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
/// patch — the shape the daemon writes and every client reads.
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
        assert!(card.detail.is_empty(), "the patch replaces the detail text");

        let two = ActivityCard::parse(&format!(
            "file_change\n{patch}diff --git a/b.rs b/b.rs\n+one\n"
        ));
        assert_eq!(two.name, "2 files changed");
        assert_eq!(two.status, "+3 −1");
    }

    #[test]
    fn malformed_file_change_markers_fall_back_to_plain_activity() {
        let card = ActivityCard::parse("file_change\nnot a patch");
        assert_eq!(card.title, "Activity");
        assert_eq!(card.patch, None);
    }

    #[test]
    fn only_plain_activity_stacks_into_a_run() {
        assert!(is_plain_activity("$ make build"));
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
}
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
