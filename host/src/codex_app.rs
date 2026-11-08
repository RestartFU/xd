//! Codex app-server adapter. The host owns RPC IDs and pending requests.
use crate::{
    agent::{AgentEvent, TodoItem, TodoStatus, TodoUpdate},
    storage::TurnSpec,
    subagent::{SubagentPatch, SubagentStatus, SubagentStep},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
};

/// RPC ids from here up answer a `turn/steer`; those below are the handshake.
const FIRST_STEER_RPC: u64 = 100;
/// A process outlives its turns and so may answer a steer from an earlier one.
/// Ids are never reused, so that answer can't be taken for a later steer's.
static NEXT_STEER_RPC: AtomicU64 = AtomicU64::new(FIRST_STEER_RPC);

pub(crate) struct CodexAppParser {
    thread: Option<String>,
    thread_params: Value,
    input: Value,
    streamed: HashSet<String>,
    /// Threads of agents this thread spawned. Their notifications feed the
    /// agent's card instead of being dropped.
    children: HashSet<String>,
    /// The running native turn, which a steer has to name.
    turn: Option<String>,
    /// Steers Codex has not answered yet, the turn's number for each by RPC id,
    /// so oldest first.
    pending_steers: BTreeMap<u64, u64>,
    /// How the turn ended, held back until those answers are in: Codex refuses
    /// a steer that arrives too late only after reporting the turn finished.
    held_outcome: Vec<AgentEvent>,
    /// The agent message streaming now, which a steer cuts short.
    streaming_item: Option<String>,
    /// Messages a steer cut short. Their last words are still on the way, and
    /// would otherwise land after the user's message as part of the answer.
    superseded: HashSet<String>,
}
impl CodexAppParser {
    pub(crate) fn new(turn: &TurnSpec) -> Self {
        let sandbox = match turn.access.as_str() {
            "full" => "danger-full-access",
            "edit" => "workspace-write",
            _ => "read-only",
        };
        let approval = if turn.access == "full" {
            "never"
        } else {
            "on-request"
        };
        let mut thread_params = json!({"cwd":turn.workdir, "model":turn.model, "approvalPolicy":approval, "sandbox":sandbox, "developerInstructions":turn.system_prompt});
        if let Some(thread) = &turn.session_id {
            thread_params["threadId"] = json!(thread);
        }
        let input = input_items(&turn.prompt);
        Self {
            thread: turn.session_id.clone(),
            thread_params,
            input: json!({"input":input, "effort":turn.effort}),
            streamed: HashSet::new(),
            children: HashSet::new(),
            turn: None,
            pending_steers: BTreeMap::new(),
            held_outcome: Vec::new(),
            streaming_item: None,
            superseded: HashSet::new(),
        }
    }
    pub(crate) fn initialize() -> Value {
        json!({"id":1, "method":"initialize", "params":{"clientInfo":{"name":"xd", "title":"xd", "version":env!("CARGO_PKG_VERSION")}, "capabilities":{"experimentalApi":true}}})
    }
    pub(crate) fn turn_request(&self) -> Value {
        let mut params = self.input.clone();
        params["threadId"] = json!(self.thread);
        json!({"id":3, "method":"turn/start", "params":params})
    }
    /// Sends `prompt` into the running turn, if there is one to name yet.
    pub(crate) fn steer_request(&mut self, steer: u64, prompt: &str) -> Option<Value> {
        let thread = self.thread.as_ref()?;
        let turn = self.turn.as_ref()?;
        let id = NEXT_STEER_RPC.fetch_add(1, Ordering::Relaxed);
        let frame = json!({"id":id, "method":"turn/steer", "params":{
            "threadId":thread, "expectedTurnId":turn, "input":input_items(prompt),
        }});
        self.pending_steers.insert(id, steer);
        if let Some(item) = self.streaming_item.take() {
            self.superseded.insert(item);
        }
        Some(frame)
    }
    pub(crate) fn settling(&self) -> bool {
        !self.held_outcome.is_empty()
    }
    pub(crate) fn take_settled(&mut self) -> Vec<AgentEvent> {
        std::mem::take(&mut self.held_outcome)
    }
    pub(crate) fn take_unread_steers(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.pending_steers)
            .into_values()
            .collect()
    }
    fn steer_reply(&mut self, id: u64, refused: bool) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        if let Some(steer) = self.pending_steers.remove(&id)
            && refused
        {
            self.superseded.clear();
            events.push(AgentEvent::SteerRejected(steer));
        }
        if self.pending_steers.is_empty() {
            events.append(&mut self.held_outcome);
        }
        events
    }
    pub(crate) fn feed(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        if root.get("id").is_some() && root.get("method").is_none() {
            if let Some(id) = root["id"].as_u64().filter(|id| *id >= FIRST_STEER_RPC) {
                return self.steer_reply(id, root.get("error").is_some());
            }
            if let Some(error) = root.get("error") {
                return vec![AgentEvent::Error(
                    error["message"]
                        .as_str()
                        .unwrap_or("Codex request failed")
                        .into(),
                )];
            }
            match root["id"].as_u64() {
                Some(1) => {
                    return vec![
                        AgentEvent::Rpc(json!({"method":"initialized"})),
                        AgentEvent::Rpc(
                            json!({"id":2, "method":if self.thread.is_some() {"thread/resume"} else {"thread/start"}, "params":self.thread_params}),
                        ),
                    ];
                }
                Some(2) => {
                    let Some(thread) = root["result"]["thread"]["id"].as_str() else {
                        return vec![AgentEvent::Error("Codex did not return a thread ID".into())];
                    };
                    self.thread = Some(thread.into());
                    return vec![
                        AgentEvent::Session(thread.into()),
                        AgentEvent::Rpc(self.turn_request()),
                    ];
                }
                Some(3) => {
                    if let Some(turn) = root["result"]["turn"]["id"].as_str() {
                        self.turn = Some(turn.into());
                    }
                    return vec![AgentEvent::Ready];
                }
                _ => return Vec::new(),
            }
        }
        let method = root["method"].as_str().unwrap_or_default();
        let params = &root["params"];
        if let Some(id) = root.get("id") {
            return vec![AgentEvent::Interaction {
                id: id.clone(),
                method: method.into(),
                params: params.clone(),
            }];
        }
        if let Some(thread) = params.get("threadId").and_then(Value::as_str)
            && Some(thread) != self.thread.as_deref()
        {
            return if self.children.contains(thread) {
                child_event(thread, method, params)
            } else {
                Vec::new()
            };
        }
        // A command that runs another assistant is that assistant's card.
        if matches!(method, "item/started" | "item/completed")
            && params["item"]["type"].as_str() == Some("commandExecution")
            && let Some(patch) =
                crate::agent::codex_shell_agent(&params["item"], method == "item/completed")
        {
            return vec![AgentEvent::Subagent(patch)];
        }
        match method {
            "turn/started" => {
                if let Some(turn) = params["turn"]["id"].as_str() {
                    self.turn = Some(turn.into());
                }
                Vec::new()
            }
            "item/started" | "item/completed"
                if params["item"]["type"].as_str() == Some("collabAgentToolCall") =>
            {
                self.collab_call(&params["item"])
            }
            "item/agentMessage/delta" => {
                if let Some(id) = params["itemId"].as_str() {
                    if self.superseded.contains(id) {
                        return Vec::new();
                    }
                    self.streamed.insert(id.into());
                    self.streaming_item = Some(id.into());
                }
                params["delta"]
                    .as_str()
                    .map(|text| vec![AgentEvent::TextDelta(text.into())])
                    .unwrap_or_default()
            }
            "item/completed" => {
                let item = &params["item"];
                match item["type"].as_str() {
                    Some("agentMessage") => {
                        if item["id"]
                            .as_str()
                            .is_some_and(|id| self.streamed.contains(id))
                        {
                            Vec::new()
                        } else {
                            item["text"]
                                .as_str()
                                .map(|text| vec![AgentEvent::TextDelta(text.into())])
                                .unwrap_or_default()
                        }
                    }
                    Some("commandExecution") => vec![AgentEvent::Tool(format!(
                        "Command: {}\n{}",
                        item["command"].as_str().unwrap_or_default(),
                        item["aggregatedOutput"].as_str().unwrap_or_default()
                    ))],
                    Some("mcpToolCall") => vec![AgentEvent::Tool(format!(
                        "Tool: {} · {}\n{}",
                        item["server"].as_str().unwrap_or_default(),
                        item["tool"].as_str().unwrap_or_default(),
                        item["result"]
                    ))],
                    _ => Vec::new(),
                }
            }
            // The diff is the whole turn's so far, so each update replaces the
            // last rather than adding another card.
            "turn/diff/updated" => params["diff"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|diff| {
                    vec![AgentEvent::Record {
                        key: format!("diff:{}", params["turnId"].as_str().unwrap_or("turn")),
                        text: format!("file_change\n{diff}"),
                    }]
                })
                .unwrap_or_default(),
            "turn/plan/updated" => {
                let todos = params["plan"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, step)| {
                        TodoItem::new(
                            index.to_string(),
                            step["step"].as_str().unwrap_or_default(),
                            match step["status"].as_str() {
                                Some("completed") => TodoStatus::Completed,
                                Some("inProgress" | "in_progress") => TodoStatus::InProgress,
                                _ => TodoStatus::Pending,
                            },
                        )
                    })
                    .collect();
                vec![AgentEvent::Todos(TodoUpdate::Replace(todos))]
            }
            "serverRequest/resolved" => params
                .get("requestId")
                .map(|id| vec![AgentEvent::InteractionResolved(id.clone())])
                .unwrap_or_default(),
            "turn/completed" => {
                self.turn = None;
                let outcome = match params["turn"]["status"].as_str() {
                    Some("completed") => vec![AgentEvent::Completed],
                    Some("interrupted") => {
                        vec![
                            AgentEvent::Resumable,
                            AgentEvent::Error("Turn interrupted.".into()),
                        ]
                    }
                    _ => codex_failure(&params["turn"]["error"], "Codex turn failed."),
                };
                if self.pending_steers.is_empty() {
                    outcome
                } else {
                    self.held_outcome = outcome;
                    Vec::new()
                }
            }
            "error" if params["willRetry"] != true => {
                codex_failure(&params["error"], "Codex error.")
            }
            _ => Vec::new(),
        }
    }
}

/// A message as Codex takes it. Attachments are host paths materialized by
/// storage, so no image bytes or desktop-local paths need to cross the backend
/// protocol.
fn input_items(prompt: &str) -> Vec<Value> {
    let mut input = vec![json!({"type":"text", "text":prompt})];
    for line in prompt.lines() {
        if let Some(path) = line
            .strip_prefix("[image: ")
            .and_then(|s| s.strip_suffix(']'))
        {
            input.push(json!({"type":"localImage", "path":path}));
        }
    }
    input
}

impl CodexAppParser {
    /// spawnAgent, sendInput, wait, closeAgent: each reports the agents it
    /// touched and what state they are in.
    fn collab_call(&mut self, item: &Value) -> Vec<AgentEvent> {
        let tool = item["tool"].as_str().unwrap_or_default();
        let receivers = item["receiverThreadIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut events = Vec::new();
        for receiver in receivers {
            self.children.insert(receiver.clone());
            let state = item
                .get("agentsStates")
                .and_then(|states| states.get(&receiver));
            let mut patch = SubagentPatch::new(receiver.clone());
            patch.status = state
                .and_then(|state| state["status"].as_str())
                .and_then(SubagentStatus::from_wire)
                .or_else(|| {
                    (item["status"].as_str() == Some("failed")).then_some(SubagentStatus::Failed)
                });
            if let Some(message) = state
                .and_then(|state| state["message"].as_str())
                .filter(|message| !message.trim().is_empty())
            {
                if patch
                    .status
                    .is_some_and(|status| status != SubagentStatus::Running)
                {
                    patch.summary = Some(message.to_owned());
                } else {
                    patch.latest = Some(message.to_owned());
                }
            }
            if matches!(tool, "spawnAgent" | "spawn_agent") {
                let prompt = item["prompt"].as_str().unwrap_or_default();
                patch.provider = Some("Codex".into());
                patch.agent = Some("Codex agent".into());
                patch.model = item["model"].as_str().map(str::to_owned);
                patch.description = prompt.lines().next().map(str::to_owned);
                patch.prompt = Some(prompt.to_owned());
                patch.status.get_or_insert(SubagentStatus::Running);
            }
            events.push(AgentEvent::Subagent(patch));
        }
        events
    }
}

/// A notification from a spawned agent's own thread.
/// A failed turn's error, marked resumable when it failed on a usage limit.
fn codex_failure(error: &Value, fallback: &str) -> Vec<AgentEvent> {
    let failure = AgentEvent::Error(error["message"].as_str().unwrap_or(fallback).into());
    if codex_usage_limited(&error["codexErrorInfo"]) {
        vec![AgentEvent::Resumable, failure]
    } else {
        vec![failure]
    }
}

/// `codexErrorInfo` is either a bare code or an object keyed by its code.
fn codex_usage_limited(info: &Value) -> bool {
    ["usageLimitExceeded", "rateLimitExceeded"]
        .iter()
        .any(|code| info.as_str() == Some(code) || info.get(code).is_some())
        || info["responseTooManyFailedAttempts"]["httpStatusCode"] == 429
}

fn child_event(thread: &str, method: &str, params: &Value) -> Vec<AgentEvent> {
    let item = &params["item"];
    let step = |tool: &str, text: String| {
        vec![AgentEvent::Subagent(SubagentPatch {
            activity: Some(text.clone()),
            step: Some(SubagentStep {
                tool: tool.into(),
                text,
            }),
            ..SubagentPatch::new(thread)
        })]
    };
    match (method, item["type"].as_str()) {
        ("item/completed", Some("commandExecution")) => step(
            "Bash",
            format!("$ {}", item["command"].as_str().unwrap_or_default()),
        ),
        ("item/completed", Some("fileChange")) => {
            let paths = item["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|change| change["path"].as_str())
                .collect::<Vec<_>>()
                .join(", ");
            step("Edit", format!("Edit {paths}"))
        }
        ("item/completed", Some("mcpToolCall")) => step(
            "Tool",
            format!(
                "{} · {}",
                item["server"].as_str().unwrap_or_default(),
                item["tool"].as_str().unwrap_or_default()
            ),
        ),
        ("item/completed", Some("agentMessage")) => item["text"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                vec![AgentEvent::Subagent(SubagentPatch {
                    latest: Some(text.to_owned()),
                    ..SubagentPatch::new(thread)
                })]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running_parser() -> CodexAppParser {
        let mut parser = CodexAppParser {
            thread: Some("thread-1".into()),
            thread_params: json!({}),
            input: json!({"input":[]}),
            streamed: HashSet::new(),
            children: HashSet::new(),
            turn: None,
            pending_steers: BTreeMap::new(),
            held_outcome: Vec::new(),
            streaming_item: None,
            superseded: HashSet::new(),
        };
        parser.feed(r#"{"id":3,"result":{"turn":{"id":"turn-1"}}}"#);
        parser
    }

    #[test]
    fn a_steer_names_the_running_turn_and_carries_images() {
        let mut parser = running_parser();
        let frame = parser
            .steer_request(1, "Instead\n[image: /tmp/a.png]")
            .unwrap();
        assert_eq!(frame["method"], "turn/steer");
        assert_eq!(frame["params"]["threadId"], "thread-1");
        assert_eq!(frame["params"]["expectedTurnId"], "turn-1");
        assert_eq!(
            frame["params"]["input"][0]["text"],
            "Instead\n[image: /tmp/a.png]"
        );
        assert_eq!(frame["params"]["input"][1]["path"], "/tmp/a.png");
        // Each needs its own id for its answer to find it.
        assert_ne!(parser.steer_request(2, "Again").unwrap()["id"], frame["id"]);
    }

    #[test]
    fn a_steer_needs_a_turn_to_name() {
        let mut parser = running_parser();
        parser.turn = None;
        assert_eq!(parser.steer_request(1, "Instead"), None);
        parser.feed(
            r#"{"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-2"}}}"#,
        );
        assert_eq!(
            parser.steer_request(1, "Instead").unwrap()["params"]["expectedTurnId"],
            "turn-2"
        );
        parser.feed(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"status":"completed"}}}"#);
        assert_eq!(parser.steer_request(9, "Too late"), None);
    }

    #[test]
    fn a_refused_steer_comes_back_and_does_not_fail_the_turn() {
        let mut parser = running_parser();
        let id = parser.steer_request(1, "Instead").unwrap()["id"]
            .as_u64()
            .unwrap();
        assert_eq!(
            parser.feed(&format!(
                r#"{{"id":{id},"error":{{"message":"no active turn"}}}}"#
            )),
            [AgentEvent::SteerRejected(1)]
        );
    }

    #[test]
    fn an_accepted_steer_keeps_the_turn_running() {
        let mut parser = running_parser();
        let id = parser.steer_request(1, "Instead").unwrap()["id"]
            .as_u64()
            .unwrap();
        assert_eq!(
            parser.feed(&format!(r#"{{"id":{id},"result":{{"turnId":"turn-1"}}}}"#)),
            []
        );
        assert!(!parser.settling());
        assert_eq!(
            parser.feed(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"status":"completed"}}}"#),
            [AgentEvent::Completed]
        );
    }

    /// Codex says the turn finished, then refuses the steer that arrived too
    /// late. Ending at the first would strand the message with the next turn.
    #[test]
    fn the_end_of_a_turn_waits_for_the_answer_to_a_steer() {
        let mut parser = running_parser();
        let id = parser.steer_request(1, "Instead").unwrap()["id"]
            .as_u64()
            .unwrap();
        assert_eq!(
            parser.feed(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"status":"completed"}}}"#),
            []
        );
        assert!(parser.settling());
        assert_eq!(
            parser.feed(&format!(
                r#"{{"id":{id},"error":{{"message":"no active turn"}}}}"#
            )),
            [AgentEvent::SteerRejected(1), AgentEvent::Completed]
        );
        assert!(!parser.settling());

        // An answer that never comes still lets the turn end, and the message
        // goes back to the user, in the order it was sent.
        let mut parser = running_parser();
        parser.steer_request(1, "Instead").unwrap();
        parser.steer_request(2, "And this").unwrap();
        parser.feed(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"status":"failed","error":{"message":"Boom"}}}}"#);
        assert_eq!(parser.take_settled(), [AgentEvent::Error("Boom".into())]);
        assert_eq!(parser.take_unread_steers(), [1, 2]);
    }

    /// The reply a steer cut short is still arriving when the steer lands. It
    /// must not run on into the answer, which comes as a new message.
    #[test]
    fn the_last_words_of_a_reply_cut_short_are_dropped() {
        let delta = |item: &str, text: &str| {
            format!(
                r#"{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-1","itemId":"{item}","delta":"{text}"}}}}"#
            )
        };
        let mut parser = running_parser();
        assert_eq!(
            parser.feed(&delta("old", "Bridges")),
            [AgentEvent::TextDelta("Bridges".into())]
        );
        parser.steer_request(1, "Instead").unwrap();
        assert_eq!(parser.feed(&delta("old", " are old")), []);
        assert_eq!(
            parser.feed(&delta("new", "Understood")),
            [AgentEvent::TextDelta("Understood".into())]
        );

        // A steer Codex refused cut nothing short.
        let mut parser = running_parser();
        parser.feed(&delta("old", "Bridges"));
        let id = parser.steer_request(1, "Instead").unwrap()["id"]
            .as_u64()
            .unwrap();
        parser.feed(&format!(r#"{{"id":{id},"error":{{"message":"no"}}}}"#));
        assert_eq!(
            parser.feed(&delta("old", " are old")),
            [AgentEvent::TextDelta(" are old".into())]
        );
    }

    /// A process outlives its turns, so an answer to one turn's steer must not
    /// be taken for the answer to a later turn's.
    #[test]
    fn steer_ids_are_not_reused_across_turns() {
        let mut first = running_parser();
        let late = first.steer_request(1, "Old").unwrap()["id"]
            .as_u64()
            .unwrap();
        let mut second = running_parser();
        let id = second.steer_request(2, "New").unwrap()["id"]
            .as_u64()
            .unwrap();
        assert_ne!(late, id);
        assert_eq!(
            second.feed(&format!(r#"{{"id":{late},"error":{{"message":"no"}}}}"#)),
            []
        );
        assert_eq!(second.take_unread_steers(), [2]);
    }

    /// An answer that outlived its turn reaches the next turn's parser, which
    /// must not take it for a failure of its own.
    #[test]
    fn a_stale_steer_answer_does_not_fail_the_next_turn() {
        let mut parser = running_parser();
        assert_eq!(
            parser.feed(&format!(
                r#"{{"id":{FIRST_STEER_RPC},"error":{{"message":"no active turn"}}}}"#
            )),
            []
        );
    }

    #[test]
    fn handshake_stream_and_approval_are_structured() {
        let mut parser = CodexAppParser {
            thread: None,
            thread_params: json!({"cwd":"/repo"}),
            input: json!({"input":[{"type":"text","text":"hello"}]}),
            streamed: HashSet::new(),
            children: HashSet::new(),
            turn: None,
            pending_steers: BTreeMap::new(),
            held_outcome: Vec::new(),
            streaming_item: None,
            superseded: HashSet::new(),
        };
        assert_eq!(parser.feed(r#"{"id":1,"result":{}}"#).len(), 2);
        let events = parser.feed(r#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#);
        assert_eq!(events[0], AgentEvent::Session("thread-1".into()));
        assert_eq!(parser.feed(r#"{"method":"item/agentMessage/delta","params":{"threadId":"thread-1","itemId":"message-1","delta":"hello"}}"#), vec![AgentEvent::TextDelta("hello".into())]);
        assert!(parser.feed(r#"{"method":"item/completed","params":{"threadId":"thread-1","item":{"type":"agentMessage","id":"message-1","text":"hello"}}}"#).is_empty());
        assert!(matches!(parser.feed(r#"{"id":44,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","command":"cargo test"}}"#)[0], AgentEvent::Interaction { .. }));
        assert_eq!(parser.feed(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"status":"completed"}}}"#), vec![AgentEvent::Completed]);
    }

    #[test]
    fn a_codex_failure_is_resumable_only_when_interrupted_or_limited() {
        let resumable = |line: &str| {
            parser_on("thread")
                .feed(line)
                .iter()
                .any(|event| matches!(event, AgentEvent::Resumable))
        };
        let failed = |info: &str| {
            format!(
                r#"{{"method":"error","params":{{"threadId":"thread","willRetry":false,"error":{{"message":"No","codexErrorInfo":{info}}}}}}}"#
            )
        };
        assert!(resumable(&failed(r#""usageLimitExceeded""#)));
        assert!(resumable(&failed(r#""rateLimitExceeded""#)));
        assert!(resumable(&failed(
            r#"{"responseTooManyFailedAttempts":{"httpStatusCode":429}}"#
        )));
        assert!(!resumable(&failed(
            r#"{"responseTooManyFailedAttempts":{"httpStatusCode":500}}"#
        )));
        assert!(!resumable(&failed(r#""badRequest""#)));
        assert!(resumable(
            r#"{"method":"turn/completed","params":{"threadId":"thread","turn":{"status":"interrupted"}}}"#
        ));
        assert!(resumable(
            r#"{"method":"turn/completed","params":{"threadId":"thread","turn":{"status":"failed","error":{"message":"No","codexErrorInfo":"usageLimitExceeded"}}}}"#
        ));
        assert!(!resumable(
            r#"{"method":"turn/completed","params":{"threadId":"thread","turn":{"status":"failed","error":{"message":"No"}}}}"#
        ));
    }

    fn parser_on(thread: &str) -> CodexAppParser {
        CodexAppParser {
            thread: Some(thread.into()),
            thread_params: json!({}),
            input: json!({}),
            streamed: HashSet::new(),
            children: HashSet::new(),
            turn: None,
            pending_steers: BTreeMap::new(),
            held_outcome: Vec::new(),
            streaming_item: None,
            superseded: HashSet::new(),
        }
    }

    #[test]
    fn provider_retries_preserve_the_live_turn_and_final_text_is_not_duplicated() {
        let mut parser = parser_on("thread");
        assert!(parser.feed(r#"{"method":"error","params":{"threadId":"thread","willRetry":true,"error":{"message":"Reconnecting"}}}"#).is_empty());
        assert_eq!(parser.feed(r#"{"method":"item/agentMessage/delta","params":{"threadId":"thread","itemId":"m","delta":"Recovered"}}"#), vec![AgentEvent::TextDelta("Recovered".into())]);
        assert!(parser.feed(r#"{"method":"item/completed","params":{"threadId":"thread","item":{"type":"agentMessage","id":"m","text":"Recovered"}}}"#).is_empty());
        assert_eq!(parser.feed(r#"{"method":"turn/completed","params":{"threadId":"thread","turn":{"status":"completed"}}}"#), vec![AgentEvent::Completed]);
    }

    #[test]
    fn a_command_that_runs_claude_is_a_subagent_card() {
        let mut parser = parser_on("parent");
        let started = parser.feed(r#"{"method":"item/started","params":{"threadId":"parent","item":{"type":"commandExecution","id":"cmd-1","command":"/bin/bash -lc 'claude -p --model opus \"Review the diff\"'","status":"inProgress"}}}"#);
        let AgentEvent::Subagent(patch) = &started[0] else {
            panic!("{started:?}");
        };
        assert_eq!(patch.key, "cmd-1");
        assert_eq!(patch.provider.as_deref(), Some("Claude"));
        assert_eq!(patch.model.as_deref(), Some("opus"));
        assert_eq!(patch.launcher.as_deref(), Some("Codex"));
        let finished = parser.feed(r#"{"method":"item/completed","params":{"threadId":"parent","item":{"type":"commandExecution","id":"cmd-1","command":"/bin/bash -lc 'claude -p --model opus \"Review the diff\"'","status":"failed","exitCode":1,"aggregatedOutput":"Not logged in","durationMs":900}}}"#);
        let AgentEvent::Subagent(patch) = &finished[0] else {
            panic!("{finished:?}");
        };
        assert_eq!(patch.status, Some(SubagentStatus::Failed));
        assert_eq!(patch.summary.as_deref(), Some("Not logged in"));
        assert_eq!(patch.duration_ms, Some(900));
        // Other commands are still commands.
        let plain = parser.feed(r#"{"method":"item/completed","params":{"threadId":"parent","item":{"type":"commandExecution","id":"cmd-2","command":"/bin/bash -lc 'cargo test'","aggregatedOutput":"ok"}}}"#);
        assert!(matches!(&plain[0], AgentEvent::Tool(text) if text.starts_with("Command: ")));
    }

    #[test]
    fn spawned_agents_become_live_records_fed_by_their_own_threads() {
        let mut parser = parser_on("parent");
        let spawned = parser.feed(r#"{"method":"item/started","params":{"threadId":"parent","item":{"type":"collabAgentToolCall","tool":"spawnAgent","receiverThreadIds":["child-1"],"prompt":"Review the diff\nCarefully","model":"gpt-5.6-sol","agentsStates":{"child-1":{"status":"pendingInit"}}}}}"#);
        let AgentEvent::Subagent(patch) = &spawned[0] else {
            panic!("{spawned:?}");
        };
        assert_eq!(patch.key, "child-1");
        assert_eq!(patch.provider.as_deref(), Some("Codex"));
        assert_eq!(patch.description.as_deref(), Some("Review the diff"));
        assert_eq!(patch.status, Some(SubagentStatus::Running));

        // The child's own work used to be dropped as another thread's noise.
        let step = parser.feed(r#"{"method":"item/completed","params":{"threadId":"child-1","item":{"type":"commandExecution","command":"git diff"}}}"#);
        let AgentEvent::Subagent(patch) = &step[0] else {
            panic!("{step:?}");
        };
        assert_eq!(patch.step.as_ref().unwrap().text, "$ git diff");
        assert!(parser.feed(r#"{"method":"item/completed","params":{"threadId":"stranger","item":{"type":"commandExecution","command":"ls"}}}"#).is_empty());

        let waited = parser.feed(r#"{"method":"item/completed","params":{"threadId":"parent","item":{"type":"collabAgentToolCall","tool":"wait","receiverThreadIds":["child-1"],"agentsStates":{"child-1":{"status":"completed","message":"Looks good."}}}}}"#);
        let AgentEvent::Subagent(patch) = &waited[0] else {
            panic!("{waited:?}");
        };
        assert_eq!(patch.status, Some(SubagentStatus::Completed));
        assert_eq!(patch.summary.as_deref(), Some("Looks good."));
    }

    #[test]
    fn turn_diffs_replace_one_record_instead_of_stacking() {
        let mut parser = parser_on("parent");
        let first = parser.feed(r#"{"method":"turn/diff/updated","params":{"threadId":"parent","turnId":"t1","diff":"diff --git a/x b/x"}}"#);
        let second = parser.feed(r#"{"method":"turn/diff/updated","params":{"threadId":"parent","turnId":"t1","diff":"diff --git a/x b/x\n+more"}}"#);
        let keys = [first, second]
            .iter()
            .map(|events| match &events[0] {
                AgentEvent::Record { key, .. } => key.clone(),
                other => panic!("{other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["diff:t1".to_owned(), "diff:t1".to_owned()]);
    }
}
