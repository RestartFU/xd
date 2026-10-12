//! Codex app-server adapter. The host owns RPC IDs and pending requests.
use crate::{
    agent::{AgentEvent, TodoItem, TodoStatus, TodoUpdate},
    storage::TurnSpec,
};
use serde_json::{Value, json};
use std::collections::HashSet;

pub(crate) struct CodexAppParser {
    thread: Option<String>,
    thread_params: Value,
    input: Value,
    streamed: HashSet<String>,
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
        if params
            .get("threadId")
            .and_then(Value::as_str)
            .is_some_and(|thread| Some(thread) != self.thread.as_deref())
        {
            return Vec::new();
        }
        match method {
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
            "turn/diff/updated" => params["diff"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|diff| vec![AgentEvent::Tool(format!("file_change\n{diff}"))])
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
        };
        assert_eq!(parser.feed(r#"{"id":1,"result":{}}"#).len(), 2);
        let events = parser.feed(r#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#);
        assert_eq!(events[0], AgentEvent::Session("thread-1".into()));
        assert_eq!(parser.feed(r#"{"method":"item/agentMessage/delta","params":{"threadId":"thread-1","itemId":"message-1","delta":"hello"}}"#), vec![AgentEvent::TextDelta("hello".into())]);
        assert!(parser.feed(r#"{"method":"item/completed","params":{"threadId":"thread-1","item":{"type":"agentMessage","id":"message-1","text":"hello"}}}"#).is_empty());
        assert!(matches!(parser.feed(r#"{"id":44,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","command":"cargo test"}}"#)[0], AgentEvent::Interaction { .. }));
        assert_eq!(parser.feed(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"status":"completed"}}}"#), vec![AgentEvent::Completed]);
    }
}
