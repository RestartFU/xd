use std::{
    io::{BufRead, BufReader, Read},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use serde_json::{Value, json};

use crate::{
    EventBus, StateStore,
    agent::{AgentCommand, AgentEvent, AgentParser},
    storage::GitDraftSpec,
};

const MAX_ACTIVE_DRAFTS: usize = 2;
const MAX_DRAFT_OUTPUT_BYTES: usize = 1024 * 1024;

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
            .ok_or_else(|| "Rust daemon state storage is not configured.".to_string())?;
        reserve_slot(&self.active)?;
        let request = request.clone();
        let service = self.clone();
        if let Err(error) = thread::Builder::new()
            .name("xd-git-draft".into())
            .spawn(move || {
                let event = match store.prepare_git_draft(&request) {
                    Ok(spec) => service.run(spec),
                    Err(error) => failure_event(&request, error.to_string()),
                };
                service.events.publish(event);
                service.active.fetch_sub(1, Ordering::AcqRel);
            })
        {
            self.active.fetch_sub(1, Ordering::AcqRel);
            return Err(format!("Cannot start Git draft worker: {error}"));
        }
        Ok(())
    }

    fn run(&self, spec: GitDraftSpec) -> Value {
        let empty_environment = Vec::new();
        let mut command = AgentCommand {
            backend: &spec.backend,
            prompt: &spec.prompt,
            system_prompt: Some(&spec.system_prompt),
            workdir: &spec.workdir,
            model: &spec.model,
            effort: &spec.effort,
            access: "read-only",
            session_id: None,
            environment: &empty_environment,
        }
        .build();
        let result = (|| -> Result<(String, String), String> {
            let mut child = command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| format!("Cannot start {}: {error}", spec.backend))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| format!("Cannot read {} output.", spec.backend))?;
            let mut stderr = child
                .stderr
                .take()
                .ok_or_else(|| format!("Cannot read {} errors.", spec.backend))?;
            let stderr_reader = thread::spawn(move || {
                let mut output = String::new();
                let _ = stderr
                    .by_ref()
                    .take(MAX_DRAFT_OUTPUT_BYTES as u64)
                    .read_to_string(&mut output);
                output
            });
            let mut parser = AgentParser::new(&spec.backend)?;
            let mut output = String::new();
            let mut completed = false;
            let mut latest_error = None;
            for line in BufReader::new(stdout).lines() {
                let line = line.map_err(|error| format!("Cannot read agent output: {error}"))?;
                for event in parser.feed(&line) {
                    match event {
                        AgentEvent::Text(text) | AgentEvent::TextDelta(text) => {
                            if output.len().saturating_add(text.len()) > MAX_DRAFT_OUTPUT_BYTES {
                                return Err("The generated Git draft is too large.".into());
                            }
                            output.push_str(&text);
                        }
                        AgentEvent::Completed => completed = true,
                        AgentEvent::Error(error) => latest_error = Some(error),
                        _ => {}
                    }
                }
            }
            let status = child
                .wait()
                .map_err(|error| format!("Cannot wait for {}: {error}", spec.backend))?;
            let stderr = stderr_reader.join().unwrap_or_default();
            if !status.success() || !completed {
                return Err(latest_error
                    .or_else(|| stderr.lines().next_back().map(str::to_owned))
                    .unwrap_or_else(|| {
                        format!("{} did not complete the Git draft.", spec.backend)
                    }));
            }
            parse_draft(&output)
        })();
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
    let value: Value = serde_json::from_str(&output[start..=end])
        .map_err(|_| "The agent returned an invalid JSON draft.".to_string())?;
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
