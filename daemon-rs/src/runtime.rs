use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read},
    process::{Child, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Instant,
};

use serde_json::{Value, json};

use crate::{
    EventBus, StateStore,
    agent::{AgentCommand, AgentEvent, AgentParser},
    ask::{self, Ask},
    claude_proxy::ClaudeProxy,
    secrets::SecretsStore,
    storage::TurnSpec,
};

#[derive(Clone)]
pub struct TurnRuntime {
    inner: Arc<RuntimeInner>,
}

struct RuntimeInner {
    store: Arc<StateStore>,
    events: Arc<EventBus>,
    active: Mutex<HashMap<String, ActiveProcess>>,
    next_turn: AtomicU64,
    secrets: Arc<SecretsStore>,
    commands: Mutex<HashMap<String, (String, Vec<String>)>>,
    claude_proxy: ClaudeProxy,
}

impl Drop for RuntimeInner {
    fn drop(&mut self) {
        if let Ok(active) = self.active.lock() {
            for process in active.values() {
                process.cancelled.store(true, Ordering::Release);
                if let Ok(mut child) = process.child.lock() {
                    let _ = child.kill();
                }
            }
        }
    }
}

struct ActiveProcess {
    turn_id: u64,
    child: Arc<Mutex<Child>>,
    cancelled: Arc<AtomicBool>,
}

impl TurnRuntime {
    pub(crate) fn new(
        store: Arc<StateStore>,
        events: Arc<EventBus>,
        secrets: Arc<SecretsStore>,
    ) -> Self {
        Self {
            inner: Arc::new(RuntimeInner {
                store,
                events,
                active: Mutex::new(HashMap::new()),
                next_turn: AtomicU64::new(0),
                secrets,
                commands: Mutex::new(HashMap::new()),
                claude_proxy: ClaudeProxy::new(),
            }),
        }
    }

    pub fn start(&self, mut turn: TurnSpec) -> Result<(), String> {
        let lineage = self
            .inner
            .store
            .folder_lineage_for_chat(&turn.chat_id)
            .map_err(|error| error.to_string())?;
        let secrets = self.inner.secrets.effective(&lineage)?;
        turn.environment = secrets.environment;
        if let Some(secret_prompt) = secrets.prompt {
            turn.system_prompt = Some(match turn.system_prompt.take() {
                Some(prompt) if !prompt.is_empty() => format!("{prompt}\n\n{secret_prompt}"),
                _ => secret_prompt,
            });
        }
        let mut command_backend = turn.backend.clone();
        let mut command_model = turn.model.clone();
        let mut command_effort = turn.effort.clone();
        if turn.claude_mode {
            let endpoint = self.inner.claude_proxy.endpoint()?;
            command_backend = "claude".into();
            command_model = claude_mode_model(&turn.model, turn.fast);
            if command_effort == "ultra" {
                command_effort = "max".into();
            }
            for (name, value) in
                claude_mode_environment(&endpoint, &command_model, turn.context_window)
            {
                set_environment(&mut turn.environment, name, value);
            }
        }
        let mut command = AgentCommand {
            backend: &command_backend,
            prompt: &turn.prompt,
            system_prompt: turn.system_prompt.as_deref(),
            workdir: &turn.workdir,
            model: &command_model,
            effort: &command_effort,
            access: &turn.access,
            fast: turn.fast && !turn.claude_mode,
            session_id: turn.session_id.as_deref(),
            environment: &turn.environment,
        }
        .build();
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Cannot start {command_backend}: {error}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("Cannot read {command_backend} output."))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| format!("Cannot read {command_backend} errors."))?;
        let turn_id = self.inner.next_turn.fetch_add(1, Ordering::Relaxed) + 1;
        let child = Arc::new(Mutex::new(child));
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut active = self
                .inner
                .active
                .lock()
                .map_err(|_| "Agent state is unavailable.".to_string())?;
            if active.contains_key(&turn.chat_id) {
                if let Ok(mut process) = child.lock() {
                    let _ = process.kill();
                }
                return Err("That chat already has a running turn.".into());
            }
            active.insert(
                turn.chat_id.clone(),
                ActiveProcess {
                    turn_id,
                    child: child.clone(),
                    cancelled: cancelled.clone(),
                },
            );
        }
        let chat_id = turn.chat_id.clone();
        let label = turn.label.clone();
        let backend = turn.backend.clone();
        let runtime = self.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("xd-agent-turn-{turn_id}"))
            .spawn({
                let child = child.clone();
                let cancelled = cancelled.clone();
                move || runtime.run(turn, turn_id, child, cancelled, stdout, stderr)
            })
        {
            if let Ok(mut active) = self.inner.active.lock() {
                active.remove(&chat_id);
            }
            cancelled.store(true, Ordering::Release);
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
            return Err(format!("Cannot supervise {backend}: {error}"));
        }
        self.inner.events.publish(json!({
            "event": "turn-started",
            "chat": chat_id,
            "label": label,
            "turn_id": turn_id,
            "turn_sequence": 0,
        }));
        Ok(())
    }

    pub fn cancel(&self, chat_id: &str) -> bool {
        let Ok(active) = self.inner.active.lock() else {
            return false;
        };
        let Some(process) = active.get(chat_id) else {
            return false;
        };
        process.cancelled.store(true, Ordering::Release);
        process
            .child
            .lock()
            .is_ok_and(|mut child| child.kill().is_ok())
    }

    pub fn commands(&self, chat_id: &str, backend: &str) -> Vec<String> {
        self.inner
            .commands
            .lock()
            .ok()
            .and_then(|commands| commands.get(chat_id).cloned())
            .filter(|(stored_backend, _)| stored_backend == backend)
            .map(|(_, commands)| commands)
            .unwrap_or_default()
    }

    fn run(
        &self,
        turn: TurnSpec,
        turn_id: u64,
        child: Arc<Mutex<Child>>,
        cancelled: Arc<AtomicBool>,
        stdout: impl Read,
        mut stderr: impl Read + Send + 'static,
    ) {
        let started = Instant::now();
        let stderr_reader = thread::spawn(move || {
            let mut output = String::new();
            let _ = stderr
                .by_ref()
                .take(1024 * 1024)
                .read_to_string(&mut output);
            output
        });
        let parser_backend = if turn.claude_mode {
            "claude"
        } else {
            &turn.backend
        };
        let mut parser = match AgentParser::new(parser_backend) {
            Ok(parser) => parser,
            Err(error) => {
                self.finish(turn, turn_id, 0, false, Some(error), 0, true, None, None);
                return;
            }
        };
        let mut completed = false;
        let mut latest_error = None;
        let mut sequence = 0_u64;
        let mut had_activity = false;
        let mut streamed_text = String::new();
        let mut assistant_text = String::new();
        let mut visible_streamed_bytes = 0;
        let mut published_text = false;
        let mut context_usage = None;

        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                latest_error = Some(format!("Cannot read {} output.", turn.backend));
                break;
            };
            for event in parser.feed(&line) {
                match event {
                    AgentEvent::Session(session) => {
                        if let Err(error) = self.inner.store.set_session(
                            &turn.chat_id,
                            &turn.session_backend,
                            &session,
                        ) {
                            latest_error = Some(error.to_string());
                        }
                    }
                    AgentEvent::Commands(commands) => {
                        if let Ok(mut cached) = self.inner.commands.lock() {
                            cached.insert(
                                turn.chat_id.clone(),
                                (turn.backend.clone(), commands.clone()),
                            );
                        }
                        self.inner.events.publish(json!({
                            "event": "commands",
                            "chat": turn.chat_id,
                            "backend": turn.backend,
                            "commands": commands,
                        }));
                    }
                    AgentEvent::Text(text) => {
                        assistant_text.push_str(&text);
                        match self.inner.store.append_turn_message(
                            &turn.chat_id,
                            "assistant",
                            &text,
                            Some(&turn.label),
                        ) {
                            Ok(_) => {
                                had_activity = true;
                                let visible = ask::visible_bytes(&text);
                                if visible > 0 {
                                    sequence += 1;
                                    // Each block is stored as its own message, so
                                    // the live view needs the same break.
                                    let separator = if published_text { "\n\n" } else { "" };
                                    self.publish_sequenced(
                                        "text",
                                        &turn,
                                        turn_id,
                                        sequence,
                                        json!({"text": format!("{separator}{}", &text[..visible])}),
                                    );
                                    published_text = true;
                                }
                            }
                            Err(error) => latest_error = Some(error.to_string()),
                        }
                    }
                    AgentEvent::TextDelta(text) => {
                        streamed_text.push_str(&text);
                        assistant_text.push_str(&text);
                        let visible = ask::visible_bytes(&streamed_text);
                        if visible > visible_streamed_bytes {
                            sequence += 1;
                            self.publish_sequenced(
                                "text",
                                &turn,
                                turn_id,
                                sequence,
                                json!({"text": &streamed_text[visible_streamed_bytes..visible]}),
                            );
                            visible_streamed_bytes = visible;
                            published_text = true;
                        }
                    }
                    AgentEvent::Tool(text) => {
                        match self.inner.store.append_turn_message(
                            &turn.chat_id,
                            "tool",
                            &text,
                            None,
                        ) {
                            Ok(_) => {
                                had_activity = true;
                                sequence += 1;
                                self.publish_sequenced(
                                    "tool",
                                    &turn,
                                    turn_id,
                                    sequence,
                                    json!({
                                        "text": text,
                                        "workdir": turn.workdir,
                                        "context": turn.workdir,
                                    }),
                                );
                            }
                            Err(error) => latest_error = Some(error.to_string()),
                        }
                    }
                    AgentEvent::Completed => {
                        completed = true;
                        latest_error = None;
                    }
                    AgentEvent::Error(error) => latest_error = Some(error),
                    AgentEvent::Usage {
                        input,
                        output,
                        window,
                    } => {
                        if let Some(usage) =
                            usage_snapshot(input, output, window, turn.context_window)
                        {
                            context_usage = Some(usage);
                        }
                    }
                }
            }
        }

        if !streamed_text.is_empty() {
            match self.inner.store.append_turn_message(
                &turn.chat_id,
                "assistant",
                &streamed_text,
                Some(&turn.label),
            ) {
                Ok(_) => had_activity = true,
                Err(error) => latest_error = Some(error.to_string()),
            }
        }

        let status = child.lock().ok().and_then(|mut child| child.wait().ok());
        let stderr = stderr_reader.join().unwrap_or_default();
        let was_cancelled = cancelled.load(Ordering::Acquire);
        let success = was_cancelled || (completed && status.is_some_and(|status| status.success()));
        let error = if success {
            None
        } else {
            latest_error.or_else(|| {
                let stderr = stderr.trim();
                Some(if stderr.is_empty() {
                    format!("{} stopped unexpectedly.", turn.backend)
                } else {
                    stderr.into()
                })
            })
        };
        if let Ok(mut active) = self.inner.active.lock()
            && active
                .get(&turn.chat_id)
                .is_some_and(|active| active.turn_id == turn_id)
        {
            active.remove(&turn.chat_id);
        }
        let asked = ask::parse(&assistant_text);
        self.finish(
            turn,
            turn_id,
            sequence,
            success,
            error,
            started.elapsed().as_secs(),
            !had_activity && !was_cancelled,
            asked,
            context_usage,
        );
    }

    fn finish(
        &self,
        turn: TurnSpec,
        turn_id: u64,
        sequence: u64,
        success: bool,
        error: Option<String>,
        duration: u64,
        silent: bool,
        asked: Option<Ask>,
        context_usage: Option<(u64, u64)>,
    ) {
        if let Some((used, window)) = context_usage {
            let _ = self.inner.store.set_context_usage(
                &turn.chat_id,
                &turn.session_backend,
                Some(&turn.model),
                used,
                window,
            );
        }
        let finish = self.inner.store.finish_turn(
            &turn.chat_id,
            success,
            error.as_deref(),
            duration,
            silent,
        );
        let Ok(finish) = finish else {
            return;
        };
        let mut event = json!({
            "event": "turn-finished",
            "chat": turn.chat_id,
            "turn_id": turn_id,
            "turn_sequence": sequence,
            "ok": success,
            "waiting": asked.is_some(),
            "silent": success && silent,
            "duration": duration,
            "last_message_id": finish.last_message_id,
        });
        if let Some(error) = error {
            event["error"] = Value::String(error);
        }
        if let Some(asked) = asked {
            event["question"] = Value::String(asked.question);
            event["options"] = serde_json::to_value(asked.options).unwrap_or(Value::Array(vec![]));
            event["accepts_input"] = Value::Bool(asked.accepts_input);
        }
        self.inner.events.publish(event);
        if let Some(queue_event) = finish.queue_event {
            self.inner.events.publish(queue_event);
        }
        if let Some(next) = finish.next
            && let Err(error) = self.start(next.clone())
        {
            let _ = self.inner.store.abort_turn_start(&next.chat_id, &error);
            self.inner
                .events
                .publish(json!({"event": "changed", "chat": next.chat_id}));
        }
    }

    fn publish_sequenced(
        &self,
        name: &str,
        turn: &TurnSpec,
        turn_id: u64,
        sequence: u64,
        fields: Value,
    ) {
        let mut event = fields.as_object().cloned().unwrap_or_default();
        event.insert("event".into(), name.into());
        event.insert("chat".into(), turn.chat_id.clone().into());
        event.insert("turn_id".into(), turn_id.into());
        event.insert("turn_sequence".into(), sequence.into());
        self.inner.events.publish(Value::Object(event));
    }
}

fn claude_mode_model(model: &str, fast: bool) -> String {
    format!(
        "{}{suffix}[1m]",
        model,
        suffix = if fast { "-fast" } else { "" }
    )
}

fn claude_mode_environment(
    endpoint: &str,
    model: &str,
    context_window: i64,
) -> Vec<(String, String)> {
    [
        ("ANTHROPIC_BASE_URL", endpoint.to_owned()),
        ("ANTHROPIC_AUTH_TOKEN", "unused".into()),
        ("ANTHROPIC_MODEL", model.to_owned()),
        ("ANTHROPIC_SMALL_FAST_MODEL", "gpt-5.6-luna[1m]".into()),
        (
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
            context_window.max(0).to_string(),
        ),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".into()),
        ("CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK", "1".into()),
        ("DISABLE_AUTOUPDATER", "1".into()),
    ]
    .into_iter()
    .map(|(name, value)| (name.into(), value))
    .collect()
}

fn set_environment(environment: &mut Vec<(String, String)>, name: String, value: String) {
    if let Some((_, current)) = environment
        .iter_mut()
        .find(|(existing, _)| existing == &name)
    {
        *current = value;
    } else {
        environment.push((name, value));
    }
}

fn usage_snapshot(
    input: u64,
    output: u64,
    reported_window: u64,
    configured_window: i64,
) -> Option<(u64, u64)> {
    let used = input.saturating_add(output);
    let window = if reported_window > 0 {
        reported_window
    } else {
        u64::try_from(configured_window).unwrap_or(0)
    };
    (used > 0 && window > 0).then_some((used, window))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_codex_models_through_the_proxy_alias() {
        assert_eq!(claude_mode_model("gpt-5.6-sol", false), "gpt-5.6-sol[1m]");
        assert_eq!(
            claude_mode_model("gpt-5.6-sol", true),
            "gpt-5.6-sol-fast[1m]"
        );
    }

    #[test]
    fn proxy_environment_overrides_agent_secrets() {
        let mut environment = vec![("ANTHROPIC_BASE_URL".into(), "private".into())];
        for (name, value) in
            claude_mode_environment("http://127.0.0.1:4321", "gpt-5.6-sol[1m]", 272_000)
        {
            set_environment(&mut environment, name, value);
        }
        assert_eq!(
            environment
                .iter()
                .find(|(name, _)| name == "ANTHROPIC_BASE_URL")
                .map(|(_, value)| value.as_str()),
            Some("http://127.0.0.1:4321")
        );
        assert!(environment.contains(&("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), "272000".into())));
    }

    #[test]
    fn usage_prefers_the_reported_window_and_falls_back_to_the_catalog() {
        assert_eq!(
            usage_snapshot(16_941, 7, 0, 272_000),
            Some((16_948, 272_000))
        );
        assert_eq!(
            usage_snapshot(21_328, 7, 1_000_000, 0),
            Some((21_335, 1_000_000))
        );
        assert_eq!(usage_snapshot(0, 0, 1_000_000, 0), None);
        assert_eq!(usage_snapshot(1, 0, 0, 0), None);
    }
}
