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
        Self {
            kind: ActivityKind::Finished,
            title: "Activity".into(),
            name: summary.clone(),
            status: "Done".into(),
            detail: summary,
            footer: None,
        }
    }
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
    fn generic_tools_stay_compact_and_bounded() {
        let card = ActivityCard::parse(&"read  ".repeat(1_000));
        assert_eq!(card.title, "Activity");
        assert!(card.name.chars().count() <= 180);
        assert_eq!(card.kind, ActivityKind::Finished);
    }
}
