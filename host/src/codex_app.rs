//! Codex app-server adapter. The host owns RPC IDs and pending requests.
use crate::{
    agent::{AgentEvent, TodoItem, TodoStatus, TodoUpdate},
    storage::TurnSpec,
    subagent::{SubagentPatch, SubagentStatus, SubagentStep},
};
use serde_json::{Value, json};
use std::collections::HashSet;

pub(crate) struct CodexAppParser {
    thread: Option<String>,
    thread_params: Value,
    input: Value,
    streamed: HashSet<String>,
    /// Threads of agents this thread spawned. Their notifications feed the
    /// agent's card instead of being dropped.
    children: HashSet<String>,
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
        let mut input = vec![json!({"type":"text", "text":turn.prompt})];
        // Attachments are host paths materialized by storage, so no image bytes
        // or desktop-local paths need to cross the backend protocol.
        for line in turn.prompt.lines() {
            if let Some(path) = line
                .strip_prefix("[image: ")
                .and_then(|s| s.strip_suffix(']'))
            {
                input.push(json!({"type":"localImage", "path":path}));
            }
        }
        Self {
            thread: turn.session_id.clone(),
            thread_params,
            input: json!({"input":input, "effort":turn.effort}),
            streamed: HashSet::new(),
            children: HashSet::new(),
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
    pub(crate) fn feed(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        if root.get("id").is_some() && root.get("method").is_none() {
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
                Some(3) => return vec![AgentEvent::Ready],
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
        match method {
            "item/started" | "item/completed"
                if params["item"]["type"].as_str() == Some("collabAgentToolCall") =>
            {
                self.collab_call(&params["item"])
            }
            "item/agentMessage/delta" => {
                if let Some(id) = params["itemId"].as_str() {
                    self.streamed.insert(id.into());
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
            "turn/completed" => match params["turn"]["status"].as_str() {
                Some("completed") => vec![AgentEvent::Completed],
                Some("interrupted") => vec![AgentEvent::Error("Turn interrupted.".into())],
                _ => vec![AgentEvent::Error(
                    params["turn"]["error"]["message"]
                        .as_str()
                        .unwrap_or("Codex turn failed.")
                        .into(),
                )],
            },
            "error" if params["willRetry"] != true => vec![AgentEvent::Error(
                params["error"]["message"]
                    .as_str()
                    .unwrap_or("Codex error.")
                    .into(),
            )],
            _ => Vec::new(),
        }
    }
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
    #[test]
    fn handshake_stream_and_approval_are_structured() {
        let mut parser = CodexAppParser {
            thread: None,
            thread_params: json!({"cwd":"/repo"}),
            input: json!({"input":[{"type":"text","text":"hello"}]}),
            streamed: HashSet::new(),
            children: HashSet::new(),
        };
        assert_eq!(parser.feed(r#"{"id":1,"result":{}}"#).len(), 2);
        let events = parser.feed(r#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#);
        assert_eq!(events[0], AgentEvent::Session("thread-1".into()));
        assert_eq!(parser.feed(r#"{"method":"item/agentMessage/delta","params":{"threadId":"thread-1","itemId":"message-1","delta":"hello"}}"#), vec![AgentEvent::TextDelta("hello".into())]);
        assert!(parser.feed(r#"{"method":"item/completed","params":{"threadId":"thread-1","item":{"type":"agentMessage","id":"message-1","text":"hello"}}}"#).is_empty());
        assert!(matches!(parser.feed(r#"{"id":44,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","command":"cargo test"}}"#)[0], AgentEvent::Interaction { .. }));
        assert_eq!(parser.feed(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"status":"completed"}}}"#), vec![AgentEvent::Completed]);
    }

    fn parser_on(thread: &str) -> CodexAppParser {
        CodexAppParser {
            thread: Some(thread.into()),
            thread_params: json!({}),
            input: json!({}),
            streamed: HashSet::new(),
            children: HashSet::new(),
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
