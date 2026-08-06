use std::{
    collections::{HashMap, HashSet},
    env,
    io::Read,
    process::{Command, Stdio},
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
const TOKEN_TIMEOUT: Duration = Duration::from_secs(2);
const TOKEN_OUTPUT_LIMIT: usize = 8 * 1024;

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
type TokenResolver = dyn Fn() -> Option<String> + Send + Sync;

struct TokenCache {
    value: Mutex<Option<Option<String>>>,
    resolver: Arc<TokenResolver>,
}

impl TokenCache {
    fn new(resolver: impl Fn() -> Option<String> + Send + Sync + 'static) -> Self {
        Self {
            value: Mutex::new(None),
            resolver: Arc::new(resolver),
        }
    }

    fn fetch(&self) -> Option<String> {
        let mut value = self.value.lock().ok()?;
        if let Some(value) = value.as_ref() {
            return value.clone();
        }
        let resolved = normalize_token((self.resolver)());
        *value = Some(resolved.clone());
        resolved
    }
}

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
        let tokens = Arc::new(TokenCache::new(resolve_token));
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            failures: Arc::new(Mutex::new(HashMap::new())),
            inflight: Arc::new(Mutex::new(HashSet::new())),
            resolver: Arc::new(move |run| {
                let token = tokens.fetch();
                resolve(&agent, run, token.as_deref())
            }),
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

fn resolve(
    agent: &ureq::Agent,
    run: &WorkflowRun,
    token: Option<&str>,
) -> Result<Resolution, String> {
    let run_url = format!(
        "https://api.github.com/repos/{}/actions/runs/{}",
        run.repository, run.id
    );
    let run_status = request_json(agent, &run_url, token)?;
    let jobs_url = format!("{run_url}/jobs?per_page={MAX_JOBS}");
    let jobs = request_json(agent, &jobs_url, token).ok();
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

fn resolve_token() -> Option<String> {
    if let Some(token) = environment_token() {
        return Some(token);
    }
    let mut child = Command::new("gh")
        .args(["auth", "token"])
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child
        .stdout
        .take()
        .map(|stream| thread::spawn(move || read_token_output(stream)))?;
    let deadline = Instant::now() + TOKEN_TIMEOUT;
    let mut status = None;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(result)) => {
                status = Some(result);
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    let timed_out_or_failed = status.is_none();
    if timed_out_or_failed {
        let _ = child.kill();
        let _ = child.wait();
    }
    let output = stdout.join().ok()?;
    if timed_out_or_failed || !status.is_some_and(|status| status.success()) {
        return None;
    }
    normalize_token(String::from_utf8(output).ok())
}

fn read_token_output(mut stream: impl Read) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let Ok(count) = stream.read(&mut buffer) else {
            break;
        };
        if count == 0 {
            break;
        }
        let remaining = TOKEN_OUTPUT_LIMIT.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    output
}

fn normalize_token(token: Option<String>) -> Option<String> {
    let token = token?;
    let token = token.trim();
    (!token.is_empty() && token.len() <= 4096 && !token.chars().any(char::is_whitespace))
        .then(|| token.to_owned())
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
        ("state".to_owned(), Value::String(state.clone())),
        ("jobs".to_owned(), Value::Array(normalize_jobs(jobs))),
    ]);
    if let Some(conclusion) = bounded_string(run.get("conclusion"), 40) {
        status.insert("conclusion".into(), Value::String(conclusion));
    }
    if let Some(started_at) = timestamp(
        run.get("run_started_at")
            .or_else(|| run.get("started_at"))
            .or_else(|| run.get("startedAt"))
            .or_else(|| run.get("created_at"))
            .or_else(|| run.get("createdAt")),
    ) {
        status.insert("started_at".into(), Value::Number(started_at.into()));
    }
    if state == "completed"
        && let Some(completed_at) = timestamp(
            run.get("completed_at")
                .or_else(|| run.get("updated_at"))
                .or_else(|| run.get("updatedAt")),
        )
    {
        status.insert("completed_at".into(), Value::Number(completed_at.into()));
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
    if let Some(started_at) = timestamp(job.get("started_at").or_else(|| job.get("startedAt"))) {
        fields.insert("started_at".into(), Value::Number(started_at.into()));
    }
    if let Some(completed_at) =
        timestamp(job.get("completed_at").or_else(|| job.get("completedAt")))
    {
        fields.insert("completed_at".into(), Value::Number(completed_at.into()));
    }
    Some(Value::Object(fields))
}

fn timestamp(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(value) => value.as_i64().filter(|value| *value >= 0),
        Value::String(value) => parse_rfc3339(value),
        _ => None,
    }
}

fn parse_rfc3339(value: &str) -> Option<i64> {
    let (date, clock) = value.split_once('T')?;
    let mut date = date.split('-');
    let year = date.next()?.parse::<i64>().ok()?;
    let month = date.next()?.parse::<u32>().ok()?;
    let day = date.next()?.parse::<u32>().ok()?;
    if date.next().is_some()
        || !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
    {
        return None;
    }

    let (clock, offset) = if let Some(clock) = clock.strip_suffix('Z') {
        (clock, 0_i64)
    } else {
        let offset_index = clock
            .char_indices()
            .skip(1)
            .find_map(|(index, character)| matches!(character, '+' | '-').then_some(index))?;
        let (clock, zone) = clock.split_at(offset_index);
        let sign = if zone.starts_with('+') { 1_i64 } else { -1_i64 };
        let (hours, minutes) = zone.get(1..)?.split_once(':')?;
        let hours = hours.parse::<i64>().ok()?;
        let minutes = minutes.parse::<i64>().ok()?;
        if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
            return None;
        }
        (clock, sign * (hours * 3_600 + minutes * 60))
    };
    let mut components = clock.split(':');
    let hour = components.next()?.parse::<i64>().ok()?;
    let minute = components.next()?.parse::<i64>().ok()?;
    let seconds = components.next()?;
    if components.next().is_some() {
        return None;
    }
    let seconds = if let Some((seconds, fraction)) = seconds.split_once('.') {
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        seconds
    } else {
        seconds
    };
    let second = seconds.parse::<i64>().ok()?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let timestamp = days
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?
        .checked_sub(offset)?;
    (timestamp >= 0).then_some(timestamp)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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
    fn github_token_lookup_is_normalized_and_cached_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver_calls = calls.clone();
        let tokens = TokenCache::new(move || {
            resolver_calls.fetch_add(1, Ordering::Relaxed);
            Some("  github-token\n".into())
        });

        assert_eq!(tokens.fetch().as_deref(), Some("github-token"));
        assert_eq!(tokens.fetch().as_deref(), Some("github-token"));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(normalize_token(Some("has whitespace".into())), None);
        assert_eq!(normalize_token(Some("x".repeat(4097))), None);
    }

    #[test]
    fn github_payload_is_bounded_and_reports_the_active_step() {
        let status = normalize_status(
            &json!({
                "name": "Nightly",
                "status": "in_progress",
                "conclusion": null,
                "run_started_at": "1970-01-01T00:00:05Z"
            }),
            Some(&json!({"jobs": [{
                "id": 7,
                "name": "linux",
                "status": "in_progress",
                "conclusion": null,
                "started_at": "1970-01-01T01:00:07+01:00",
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
        assert_eq!(status["started_at"], 5);
        assert_eq!(status["jobs"][0]["id"], "7");
        assert_eq!(status["jobs"][0]["log"], "Test");
        assert_eq!(status["jobs"][0]["started_at"], 7);
        assert!(status.get("conclusion").is_none());
        assert!(status.get("completed_at").is_none());
    }

    #[test]
    fn workflow_timestamps_are_validated_and_completed_runs_use_the_last_update() {
        let status = normalize_status(
            &json!({
                "name": "Nightly",
                "status": "completed",
                "conclusion": "success",
                "createdAt": "1970-01-01T00:00:02.500Z",
                "updatedAt": "1970-01-01T00:01:02Z"
            }),
            Some(&json!({"jobs": [{
                "id": "job",
                "name": "linux",
                "state": "completed",
                "startedAt": "1970-01-01T00:00:03Z",
                "completedAt": "1970-01-01T00:01:01Z"
            }]})),
        )
        .unwrap();

        assert_eq!(status["started_at"], 2);
        assert_eq!(status["completed_at"], 62);
        assert_eq!(status["jobs"][0]["started_at"], 3);
        assert_eq!(status["jobs"][0]["completed_at"], 61);
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(parse_rfc3339("2023-02-29T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00+24:00"), None);
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
