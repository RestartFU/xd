use std::{
    collections::{HashMap, HashSet},
    env,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};

use crate::EventBus;

const PREFIX: &str = "workflow_run\n";
const MAX_BODY_BYTES: u64 = 2 * 1024 * 1024;
const MAX_JOBS: usize = 100;
const AUTHENTICATED_TTL: Duration = Duration::from_secs(8);
const ANONYMOUS_TTL: Duration = Duration::from_secs(3 * 60);
const FAILURE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkflowRun {
    id: String,
    repository: String,
}

struct Resolution {
    status: Value,
    authenticated: bool,
}

struct CacheEntry {
    status: Value,
    checked_at: Instant,
    active_ttl: Duration,
}

type Resolver = dyn Fn(&WorkflowRun) -> Result<Resolution, String> + Send + Sync;

pub(crate) struct WorkflowStatuses {
    entries: Arc<Mutex<HashMap<String, CacheEntry>>>,
    failures: Arc<Mutex<HashMap<String, Instant>>>,
    inflight: Arc<Mutex<HashSet<String>>>,
    resolver: Arc<Resolver>,
    failure_ttl: Duration,
    events: Arc<EventBus>,
}

impl WorkflowStatuses {
    pub(crate) fn new(events: Arc<EventBus>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .user_agent("xd-dev/0.1")
            .build()
            .into();
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            failures: Arc::new(Mutex::new(HashMap::new())),
            inflight: Arc::new(Mutex::new(HashSet::new())),
            resolver: Arc::new(move |run| resolve(&agent, run)),
            failure_ttl: FAILURE_TTL,
            events,
        }
    }

    pub(crate) fn start(&self, owner: u64, marker: &str) -> Result<Value, String> {
        let run = parse_marker(marker).ok_or_else(|| "Invalid workflow run marker.".to_owned())?;
        let key = format!("{}/{}", run.repository, run.id);
        let now = Instant::now();
        let cached = {
            let entries = self
                .entries
                .lock()
                .map_err(|_| "Workflow status cache is unavailable.".to_owned())?;
            entries.get(&key).map(|entry| {
                (
                    entry.status.clone(),
                    terminal(&entry.status)
                        || now.saturating_duration_since(entry.checked_at) < entry.active_ttl,
                )
            })
        };
        if let Some((status, true)) = cached.as_ref() {
            return Ok(status.clone());
        }

        let cooling_down = self
            .failures
            .lock()
            .map_err(|_| "Workflow status cache is unavailable.".to_owned())?
            .get(&key)
            .is_some_and(|failed_at| now.saturating_duration_since(*failed_at) < self.failure_ttl);
        if cooling_down {
            return cached
                .map(|(status, _)| stale_status(status, false))
                .ok_or_else(|| "Workflow status is temporarily unavailable.".to_owned());
        }

        let inserted = self
            .inflight
            .lock()
            .map_err(|_| "Workflow status cache is unavailable.".to_owned())?
            .insert(key.clone());
        if inserted {
            let entries = self.entries.clone();
            let failures = self.failures.clone();
            let inflight = self.inflight.clone();
            let resolver = self.resolver.clone();
            let events = self.events.clone();
            let marker = marker.to_owned();
            let worker_key = key.clone();
            thread::Builder::new()
                .name("xd-workflow-status".into())
                .spawn(move || {
                    let checked_at = Instant::now();
                    let event_status = match resolver(&run) {
                        Ok(resolution) => {
                            let active_ttl = if resolution.authenticated {
                                AUTHENTICATED_TTL
                            } else {
                                ANONYMOUS_TTL
                            };
                            let status = resolution.status;
                            if let Ok(mut entries) = entries.lock() {
                                entries.insert(
                                    worker_key.clone(),
                                    CacheEntry {
                                        status: status.clone(),
                                        checked_at,
                                        active_ttl,
                                    },
                                );
                            }
                            if let Ok(mut failures) = failures.lock() {
                                failures.remove(&worker_key);
                            }
                            status
                        }
                        Err(error) => {
                            if let Ok(mut failures) = failures.lock() {
                                failures.insert(worker_key.clone(), checked_at);
                            }
                            entries
                                .lock()
                                .ok()
                                .and_then(|entries| {
                                    entries.get(&worker_key).map(|entry| entry.status.clone())
                                })
                                .map(|status| stale_status(status, false))
                                .unwrap_or_else(|| json!({"ok": false, "error": error}))
                        }
                    };
                    if let Ok(mut inflight) = inflight.lock() {
                        inflight.remove(&worker_key);
                    }
                    events.publish_to(owner, workflow_event(&marker, event_status));
                })
                .map_err(|error| {
                    if let Ok(mut inflight) = self.inflight.lock() {
                        inflight.remove(&key);
                    }
                    format!("Cannot start workflow status refresh: {error}")
                })?;
        }

        Ok(cached
            .map(|(status, _)| stale_status(status, true))
            .unwrap_or_else(|| json!({"ok": true, "pending": true})))
    }

    #[cfg(test)]
    fn fetch(&self, marker: &str) -> Result<Value, String> {
        let run = parse_marker(marker).ok_or_else(|| "Invalid workflow run marker.".to_owned())?;
        let key = format!("{}/{}", run.repository, run.id);
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "Workflow status cache is unavailable.".to_owned())?;
        if let Some(entry) = entries.get(&key)
            && (terminal(&entry.status)
                || now.saturating_duration_since(entry.checked_at) < entry.active_ttl)
        {
            return Ok(entry.status.clone());
        }

        let mut failures = self
            .failures
            .lock()
            .map_err(|_| "Workflow status cache is unavailable.".to_owned())?;
        if failures
            .get(&key)
            .is_some_and(|failed_at| now.saturating_duration_since(*failed_at) < self.failure_ttl)
        {
            return entries
                .get(&key)
                .map(|entry| entry.status.clone())
                .ok_or_else(|| "Workflow status is temporarily unavailable.".to_owned());
        }

        match (self.resolver)(&run) {
            Ok(resolution) => {
                let active_ttl = if resolution.authenticated {
                    AUTHENTICATED_TTL
                } else {
                    ANONYMOUS_TTL
                };
                let status = resolution.status;
                entries.insert(
                    key.clone(),
                    CacheEntry {
                        status: status.clone(),
                        checked_at: now,
                        active_ttl,
                    },
                );
                failures.remove(&key);
                Ok(status)
            }
            Err(error) => {
                failures.insert(key.clone(), now);
                entries
                    .get(&key)
                    .map(|entry| entry.status.clone())
                    .ok_or(error)
            }
        }
    }

    #[cfg(test)]
    fn with_resolver(
        resolver: impl Fn(&WorkflowRun) -> Result<Resolution, String> + Send + Sync + 'static,
        failure_ttl: Duration,
    ) -> Self {
        Self::with_resolver_and_events(resolver, failure_ttl, Arc::new(EventBus::default()))
    }

    #[cfg(test)]
    fn with_resolver_and_events(
        resolver: impl Fn(&WorkflowRun) -> Result<Resolution, String> + Send + Sync + 'static,
        failure_ttl: Duration,
        events: Arc<EventBus>,
    ) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            failures: Arc::new(Mutex::new(HashMap::new())),
            inflight: Arc::new(Mutex::new(HashSet::new())),
            resolver: Arc::new(resolver),
            failure_ttl,
            events,
        }
    }
}

fn stale_status(mut status: Value, refreshing: bool) -> Value {
    if let Some(status) = status.as_object_mut() {
        status.insert("stale".into(), Value::Bool(true));
        if refreshing {
            status.insert("refreshing".into(), Value::Bool(true));
        }
    }
    status
}

fn workflow_event(marker: &str, mut status: Value) -> Value {
    let fields = status
        .as_object_mut()
        .map(std::mem::take)
        .unwrap_or_default();
    let mut event = Map::from_iter([
        ("event".to_owned(), Value::String("workflow-status".into())),
        ("text".to_owned(), Value::String(marker.to_owned())),
    ]);
    event.extend(fields);
    Value::Object(event)
}

fn resolve(agent: &ureq::Agent, run: &WorkflowRun) -> Result<Resolution, String> {
    let token = environment_token();
    let run_url = format!(
        "https://api.github.com/repos/{}/actions/runs/{}",
        run.repository, run.id
    );
    let run_status = request_json(agent, &run_url, token.as_deref())?;
    let jobs_url = format!("{run_url}/jobs?per_page={MAX_JOBS}");
    let jobs = request_json(agent, &jobs_url, token.as_deref()).ok();
    Ok(Resolution {
        status: normalize_status(&run_status, jobs.as_ref())?,
        authenticated: token.is_some(),
    })
}

fn request_json(agent: &ureq::Agent, url: &str, token: Option<&str>) -> Result<Value, String> {
    let mut request = agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(|error| match error {
        ureq::Error::StatusCode(code) => format!("GitHub returned HTTP {code}."),
        _ => "Cannot reach GitHub for workflow status.".to_owned(),
    })?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string()
        .map_err(|_| "GitHub returned an unreadable workflow status.".to_owned())?;
    serde_json::from_str(&body)
        .map_err(|_| "GitHub returned an invalid workflow status.".to_owned())
}

fn environment_token() -> Option<String> {
    ["GH_TOKEN", "GITHUB_TOKEN"].into_iter().find_map(|name| {
        let token = env::var(name).ok()?;
        let token = token.trim();
        (!token.is_empty() && token.len() <= 4096 && !token.chars().any(char::is_whitespace))
            .then(|| token.to_owned())
    })
}

fn parse_marker(marker: &str) -> Option<WorkflowRun> {
    let body = marker.strip_prefix(PREFIX)?;
    let (id, url) = body.split_once('\n')?;
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let prefix = "https://github.com/";
    let suffix = format!("/actions/runs/{id}");
    let repository = url.strip_prefix(prefix)?.strip_suffix(&suffix)?;
    let mut parts = repository.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    if parts.next().is_some() || !safe_component(owner) || !safe_component(name) {
        return None;
    }
    Some(WorkflowRun {
        id: id.to_owned(),
        repository: repository.to_owned(),
    })
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn normalize_status(run: &Value, jobs: Option<&Value>) -> Result<Value, String> {
    let name = bounded_string(run.get("name"), 160).unwrap_or_default();
    let state = bounded_string(run.get("status").or_else(|| run.get("state")), 40)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "GitHub returned an invalid workflow status.".to_owned())?;
    let mut status = Map::from_iter([
        ("ok".to_owned(), Value::Bool(true)),
        ("name".to_owned(), Value::String(name)),
        ("state".to_owned(), Value::String(state)),
        ("jobs".to_owned(), Value::Array(normalize_jobs(jobs))),
    ]);
    if let Some(conclusion) = bounded_string(run.get("conclusion"), 40) {
        status.insert("conclusion".into(), Value::String(conclusion));
    }
    Ok(Value::Object(status))
}

fn normalize_jobs(body: Option<&Value>) -> Vec<Value> {
    body.and_then(|body| body.get("jobs"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(normalize_job)
        .take(MAX_JOBS)
        .collect()
}

fn normalize_job(job: &Value) -> Option<Value> {
    let id = match job.get("id").or_else(|| job.get("databaseId"))? {
        Value::String(id) if !id.is_empty() && id.len() <= 40 => id.clone(),
        Value::Number(id) => id.to_string(),
        _ => return None,
    };
    let name = bounded_string(job.get("name"), 160).filter(|name| !name.is_empty())?;
    let state = bounded_string(job.get("status").or_else(|| job.get("state")), 40)
        .filter(|state| !state.is_empty())?;
    let mut fields = Map::from_iter([
        ("id".to_owned(), Value::String(id)),
        ("name".to_owned(), Value::String(name)),
        ("state".to_owned(), Value::String(state.clone())),
    ]);
    if let Some(conclusion) = bounded_string(job.get("conclusion"), 40) {
        fields.insert("conclusion".into(), Value::String(conclusion));
    }
    if let Some(log) =
        bounded_string(job.get("log"), 160).or_else(|| latest_job_activity(job, &state))
    {
        fields.insert("log".into(), Value::String(log));
    }
    Some(Value::Object(fields))
}

fn latest_job_activity(job: &Value, state: &str) -> Option<String> {
    let steps = job.get("steps")?.as_array()?;
    let selected = if state == "in_progress" {
        steps
            .iter()
            .rev()
            .find(|step| step.get("status").and_then(Value::as_str) == Some("in_progress"))
    } else {
        None
    }
    .or_else(|| {
        steps.iter().rev().find(|step| {
            step.get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| !matches!(status, "queued" | "pending" | "requested"))
        })
    })?;
    bounded_string(selected.get("name"), 160).filter(|name| !name.is_empty())
}

fn bounded_string(value: Option<&Value>, max: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| value.len() <= max)
        .map(str::to_owned)
}

fn terminal(status: &Value) -> bool {
    status.get("state").and_then(Value::as_str) == Some("completed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{os::unix::net::UnixStream, sync::mpsc::sync_channel};

    #[test]
    fn marker_parser_accepts_only_the_captured_github_run() {
        assert_eq!(
            parse_marker("workflow_run\n123\nhttps://github.com/RestartFU/xd/actions/runs/123"),
            Some(WorkflowRun {
                id: "123".into(),
                repository: "RestartFU/xd".into(),
            })
        );
        assert!(
            parse_marker("workflow_run\nabc\nhttps://github.com/a/b/actions/runs/abc").is_none()
        );
        assert!(
            parse_marker("workflow_run\n123\nhttps://example.com/a/b/actions/runs/123").is_none()
        );
        assert!(
            parse_marker("workflow_run\n123\nhttps://github.com/a/b/c/actions/runs/123").is_none()
        );
    }

    #[test]
    fn github_payload_is_bounded_and_reports_the_active_step() {
        let status = normalize_status(
            &json!({"name": "Nightly", "status": "in_progress", "conclusion": null}),
            Some(&json!({"jobs": [{
                "id": 7,
                "name": "linux",
                "status": "in_progress",
                "conclusion": null,
                "steps": [
                    {"name": "Build", "status": "completed"},
                    {"name": "Test", "status": "in_progress"}
                ]
            }]})),
        )
        .unwrap();
        assert_eq!(status["ok"], true);
        assert_eq!(status["name"], "Nightly");
        assert_eq!(status["state"], "in_progress");
        assert_eq!(status["jobs"][0]["id"], "7");
        assert_eq!(status["jobs"][0]["log"], "Test");
        assert!(status.get("conclusion").is_none());
    }

    #[test]
    fn cache_keeps_last_good_status_when_refresh_fails() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver_calls = calls.clone();
        let statuses = WorkflowStatuses::with_resolver(
            move |_| {
                if resolver_calls.fetch_add(1, Ordering::Relaxed) == 0 {
                    Ok(Resolution {
                        status: json!({
                            "ok": true,
                            "name": "Nightly",
                            "state": "in_progress",
                            "jobs": []
                        }),
                        authenticated: true,
                    })
                } else {
                    Err("network unavailable".into())
                }
            },
            Duration::ZERO,
        );
        let marker = "workflow_run\n123\nhttps://github.com/RestartFU/xd/actions/runs/123";

        assert_eq!(statuses.fetch(marker).unwrap()["state"], "in_progress");
        {
            let mut entries = statuses.entries.lock().unwrap();
            entries.values_mut().for_each(|entry| {
                entry.active_ttl = Duration::ZERO;
            });
        }
        assert_eq!(statuses.fetch(marker).unwrap()["state"], "in_progress");
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn completed_statuses_never_poll_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver_calls = calls.clone();
        let statuses = WorkflowStatuses::with_resolver(
            move |_| {
                resolver_calls.fetch_add(1, Ordering::Relaxed);
                Ok(Resolution {
                    status: json!({
                        "ok": true,
                        "name": "Nightly",
                        "state": "completed",
                        "conclusion": "success",
                        "jobs": []
                    }),
                    authenticated: true,
                })
            },
            Duration::ZERO,
        );
        let marker = "workflow_run\n123\nhttps://github.com/RestartFU/xd/actions/runs/123";

        assert_eq!(statuses.fetch(marker).unwrap()["conclusion"], "success");
        assert_eq!(statuses.fetch(marker).unwrap()["conclusion"], "success");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn background_refresh_replies_immediately_and_targets_the_completed_status() {
        let events = Arc::new(EventBus::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver_calls = calls.clone();
        let statuses = WorkflowStatuses::with_resolver_and_events(
            move |_| {
                resolver_calls.fetch_add(1, Ordering::Relaxed);
                Ok(Resolution {
                    status: json!({
                        "ok": true,
                        "name": "GPUI dev",
                        "state": "completed",
                        "conclusion": "success",
                        "jobs": []
                    }),
                    authenticated: true,
                })
            },
            Duration::ZERO,
            events.clone(),
        );
        let (sender, frames) = sync_channel(2);
        let (connection, _peer) = UnixStream::pair().unwrap();
        let owner = events.subscribe(sender, connection).unwrap();
        let marker = "workflow_run\n123\nhttps://github.com/RestartFU/xd/actions/runs/123";

        let reply = statuses.start(owner, marker).unwrap();
        assert_eq!(reply, json!({"ok": true, "pending": true}));
        let event = frames.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(event["event"], "workflow-status");
        assert_eq!(event["text"], marker);
        assert_eq!(event["conclusion"], "success");

        assert_eq!(
            statuses.start(owner, marker).unwrap()["conclusion"],
            "success"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
