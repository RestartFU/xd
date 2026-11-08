use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::{EventBus, StateStore, agent::AgentCommand, agent_text, storage::GitDraftSpec};

const MAX_ACTIVE_DRAFTS: usize = 2;
/// A stalled run would otherwise hold its slot until the host restarts.
const DRAFT_TIMEOUT: Duration = Duration::from_secs(120);

/// Holds a draft's slot, and gives it back even if the run panics.
struct ActiveDraft(Arc<AtomicUsize>);

impl Drop for ActiveDraft {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone)]
pub(crate) struct GitDraftService {
    store: Option<Arc<StateStore>>,
    events: Arc<EventBus>,
    active: Arc<AtomicUsize>,
}

impl GitDraftService {
    pub(crate) fn new(store: Option<Arc<StateStore>>, events: Arc<EventBus>) -> Self {
        Self {
            store,
            events,
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) fn start(&self, request: &Value) -> Result<(), String> {
        let store = self
            .store
            .clone()
            .ok_or_else(|| "Rust host state storage is not configured.".to_string())?;
        reserve_slot(&self.active)?;
        let slot = ActiveDraft(self.active.clone());
        let request = request.clone();
        let service = self.clone();
        thread::Builder::new()
            .name("xd-git-draft".into())
            .spawn(move || {
                let _slot = slot;
                let event = match store.prepare_git_draft(&request) {
                    Ok(spec) => service.run(spec),
                    Err(error) => failure_event(&request, error.to_string()),
                };
                service.events.publish(event);
            })
            .map(|_| ())
            .map_err(|error| format!("Cannot start Git draft worker: {error}"))
    }

    fn run(&self, spec: GitDraftSpec) -> Value {
        let empty_environment = Vec::new();
        let mut command = AgentCommand {
            backend: &spec.backend,
            prompt: &spec.prompt,
            system_prompt: Some(&spec.system_prompt),
            workdir: &spec.workdir,
            model: &spec.model,
            effort: agent_text::LOW_EFFORT,
            access: "read-only",
            fast: false,
            session_id: None,
            environment: &empty_environment,
        }
        .build();
        // A draft follows the repository's own conventions, so its instructions
        // stay; nothing else the run would load helps describe a diff.
        agent_text::keep_the_run_minimal(&mut command, &spec.backend, true);
        let input = agent_text::turn_input(&spec.backend, &spec.prompt);
        let result = agent_text::run(command, &spec.backend, input, DRAFT_TIMEOUT, "Git drafting")
            .and_then(|output| parse_draft(&output));
        match result {
            Ok((title, body)) => json!({
                "event": "git-draft-finished",
                "chat": spec.chat_id,
                "kind": spec.kind,
                "request": spec.request_id,
                "success": true,
                "title": title,
                "body": body,
            }),
            Err(error) => json!({
                "event": "git-draft-finished",
                "chat": spec.chat_id,
                "kind": spec.kind,
                "request": spec.request_id,
                "success": false,
                "error": error,
            }),
        }
    }
}

fn reserve_slot(active: &AtomicUsize) -> Result<(), String> {
    active
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_ACTIVE_DRAFTS).then_some(count + 1)
        })
        .map(|_| ())
        .map_err(|_| "Two Git drafts are already being generated.".into())
}

fn failure_event(request: &Value, error: String) -> Value {
    json!({
        "event": "git-draft-finished",
        "chat": request.get("chat").and_then(Value::as_str).unwrap_or_default(),
        "kind": request.get("kind").and_then(Value::as_str).unwrap_or_default(),
        "request": request.get("request").and_then(Value::as_str).unwrap_or_default(),
        "success": false,
        "error": error,
    })
}

fn parse_draft(output: &str) -> Result<(String, String), String> {
    let start = output
        .find('{')
        .ok_or("The agent returned no JSON draft.")?;
    let end = output
        .rfind('}')
        .ok_or("The agent returned incomplete JSON.")?;
    // The closing brace can come first in a malformed reply.
    let value: Value = output
        .get(start..=end)
        .and_then(|json| serde_json::from_str(json).ok())
        .ok_or("The agent returned an invalid JSON draft.")?;
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let body = value
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if title.is_empty() || title.len() > 200 || body.len() > 16 * 1024 {
        return Err("The agent returned an invalid Git draft.".into());
    }
    Ok((title, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_json_draft_inside_agent_noise() {
        let (title, body) =
            parse_draft("note\n{\"title\":\" feat:   ship it \",\"body\":\"Tested.\\n\"}\ndone")
                .unwrap();
        assert_eq!(title, "feat: ship it");
        assert_eq!(body, "Tested.");
    }

    #[test]
    fn rejects_missing_and_oversized_titles() {
        assert!(parse_draft("{}").is_err());
        let output = json!({"title": "x".repeat(201), "body": ""}).to_string();
        assert!(parse_draft(&output).is_err());
    }
}
