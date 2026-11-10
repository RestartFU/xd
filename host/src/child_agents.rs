//! Agents a chat's agent starts through xd's `start_agent` tool.
//!
//! Each one is an ordinary xd chat that names the chat that started it. It runs
//! like any other chat: on the account xd picks for it, in a workspace xd made,
//! owned by the daemon rather than by the agent that asked for it. So it keeps
//! running when that agent's process ends, it shows in every client nested
//! under its parent, and pausing, resuming and pull request links work on it.
//! Each finished turn is reported back to the parent chat when asked to.

use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
};

use serde_json::{Value, json};

use crate::{
    Engine, error_reply, required_string,
    runtime::SettledTurn,
    storage::{AgentOrigin, ChildAgent},
};

/// Agents one chat may have at work at once.
pub(crate) const MAX_WORKING: usize = 10;
/// Held while an agent starts, so two starting at once cannot both find room
/// under [`MAX_WORKING`]: an agent counts once its first turn has started.
static STARTING: Mutex<()> = Mutex::new(());
/// How much of an agent's reply its parent is sent, or `list_agents` shows.
const REPORT_CHARS: usize = 6_000;
const SUMMARY_CHARS: usize = 600;

impl Engine {
    /// `start-agent`: starts a chat under `chat` and gives it `prompt`.
    pub(crate) fn start_agent(&self, request: &Value) -> Value {
        let parent = match required_string(request, "chat", "start-agent needs a chat id") {
            Ok(parent) => parent,
            Err(error) => return error_reply(error),
        };
        let prompt = request
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if prompt.is_empty() {
            return error_reply("Give the agent a prompt.");
        }
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let _starting = STARTING.lock().unwrap_or_else(|error| error.into_inner());
        let origin = match store.agent_origin(parent) {
            Ok(origin) => origin,
            Err(error) => return error_reply(error),
        };
        if origin.parent.is_some() {
            return error_reply(
                "An agent another agent started cannot start agents of its own. Do the work \
                 here, or ask the agent that started you to start another.",
            );
        }
        let working = match store.child_agents(parent) {
            Ok(children) => children.iter().filter(|child| child.working).count(),
            Err(error) => return error_reply(error),
        };
        if working >= MAX_WORKING {
            return error_reply(format!(
                "{working} agents are already at work for this chat, the most it may have at \
                 once. Wait for one to finish, or stop one."
            ));
        }
        let new_chat = match new_chat_request(&origin, request, prompt) {
            Ok(new_chat) => new_chat,
            Err(error) => return error_reply(error),
        };
        let reports = request.get("notify").and_then(Value::as_bool) != Some(false);
        let created = {
            let _resources = self
                .session_resources
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            self.tree_mutation(|store| store.new_child_chat(&new_chat, parent, reports))
        };
        let Some(child) = created
            .get("id")
            .and_then(Value::as_str)
            .filter(|_| created["ok"] == true)
            .map(str::to_owned)
        else {
            return created;
        };
        self.worktree_names.start(child.clone(), new_chat.clone());
        // Before its turn starts, so a turn that ends at once is not then
        // shown as running again.
        self.publish_agent_record(store.open_agent_record(&child, prompt));
        let sent = self.send_message(
            &json!({"op": "send", "chat": child, "text": prompt, "from_parent": true}),
        );
        if sent["ok"] != true {
            // Nothing ran in it, so nothing is lost: the agent can try again.
            self.delete_chat(&json!({"op": "delete-chat", "chat": child}));
            return sent;
        }
        self.events
            .publish(json!({"event": "changed", "chat": child}));
        let started = store.agent_origin(&child).ok();
        json!({
            "ok": true,
            "agent": child,
            "backend": created["backend"],
            "model": started.as_ref().and_then(|origin| origin.model.clone()),
            "reports": reports,
        })
    }

    /// `list-agents`: the agents `chat`'s agent started, newest state first.
    pub(crate) fn list_agents(&self, request: &Value) -> Value {
        let parent = match required_string(request, "chat", "list-agents needs a chat id") {
            Ok(parent) => parent,
            Err(error) => return error_reply(error),
        };
        let only = request.get("agent").and_then(Value::as_str);
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        if let Err(error) = store.agent_origin(parent) {
            return error_reply(error);
        }
        let children = match store.child_agents(parent) {
            Ok(children) => children,
            Err(error) => return error_reply(error),
        };
        if only.is_some_and(|only| !children.iter().any(|child| child.id == only)) {
            return error_reply("This chat started no agent with that id.");
        }
        let limit = if only.is_some() {
            REPORT_CHARS
        } else {
            SUMMARY_CHARS
        };
        let agents = children
            .iter()
            .filter(|child| only.is_none_or(|only| child.id == only))
            .map(|child| describe(child, limit))
            .collect::<Vec<_>>();
        json!({"ok": true, "agents": agents})
    }

    /// `message-agent`: sends one of `chat`'s agents a message, queued behind
    /// its turn when it is at work.
    pub(crate) fn message_agent(&self, request: &Value) -> Value {
        let child = match self.own_agent(request) {
            Ok(child) => child,
            Err(error) => return error_reply(error),
        };
        let text = request
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if text.is_empty() {
            return error_reply("Give the message some text.");
        }
        // Waking an agent that finished counts against the agents at work.
        let _starting = STARTING.lock().unwrap_or_else(|error| error.into_inner());
        if let Err(error) = self.room_for_another(request, &child) {
            return error_reply(error);
        }
        let reply = self.send_message(
            &json!({"op": "send", "chat": child.id, "text": text, "from_parent": true}),
        );
        if reply["ok"] == true {
            self.events
                .publish(json!({"event": "changed", "chat": child.id}));
        }
        reply
    }

    /// `stop-agent`: stops the turn one of `chat`'s agents is taking.
    pub(crate) fn stop_agent(&self, request: &Value) -> Value {
        let child = match self.own_agent(request) {
            Ok(child) => child,
            Err(error) => return error_reply(error),
        };
        let reply = self.cancel(&json!({"op": "cancel", "chat": child.id}));
        if reply["ok"] == true {
            return json!({"ok": true, "was_working": child.working});
        }
        reply
    }

    /// Whether `child` may start a turn without going past [`MAX_WORKING`]:
    /// always when it is already at work, since its message only queues.
    fn room_for_another(&self, request: &Value, child: &ChildAgent) -> Result<(), String> {
        let parent = required_string(request, "chat", "That needs a chat id.")?;
        let store = self
            .store
            .as_ref()
            .ok_or("Rust host state storage is not configured.")?;
        let children = store
            .child_agents(parent)
            .map_err(|error| error.to_string())?;
        if children
            .iter()
            .any(|other| other.id == child.id && other.working)
        {
            return Ok(());
        }
        let working = children.iter().filter(|other| other.working).count();
        if working >= MAX_WORKING {
            return Err(format!(
                "{working} agents are already at work for this chat, the most it may have at \
                 once. Wait for one to finish, or stop one."
            ));
        }
        Ok(())
    }

    /// The agent `request` names, when `request`'s chat started it.
    fn own_agent(&self, request: &Value) -> Result<ChildAgent, String> {
        let parent = required_string(request, "chat", "That needs a chat id.")?;
        let agent = required_string(request, "agent", "Name the agent by its id.")?;
        let store = self
            .store
            .as_ref()
            .ok_or("Rust host state storage is not configured.")?;
        store
            .child_agents(parent)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|child| child.id == agent)
            .ok_or_else(|| "This chat started no agent with that id.".into())
    }

    /// Reports each finished turn of an agent that asked for it back to the
    /// chat that started it, one at a time and in the order they ended. A
    /// paused parent gets the report in its queue, so it does not wake for it.
    pub fn start_reporting_agents(self: &Arc<Self>) {
        let Some(runtime) = self.runtime.as_ref() else {
            return;
        };
        let (sender, receiver) = mpsc::channel::<SettledTurn>();
        // Weak, since the engine owns the runtime that holds the sender.
        let engine = Arc::downgrade(self);
        // Off the turns' threads: sending takes the parent's own locks.
        let spawned = thread::Builder::new()
            .name("xd-agent-reports".into())
            .spawn(move || {
                for settled in receiver {
                    let Some(engine) = engine.upgrade() else {
                        return;
                    };
                    engine.report_to_parent(&settled);
                }
            });
        if spawned.is_err() {
            return;
        }
        let sender = Mutex::new(sender);
        runtime.set_turn_settled_handler(Box::new(move |settled| {
            if let Ok(sender) = sender.lock() {
                let _ = sender.send(settled);
            }
        }));
    }

    fn report_to_parent(&self, settled: &SettledTurn) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let child = settled.chat_id.as_str();
        let Ok(Some((parent, reports))) = store.chat_parent(child) else {
            return;
        };
        // The card follows every turn; the message only when it was asked for.
        let summary = match &settled.error {
            Some(error) => clip(error, SUMMARY_CHARS),
            None => clip_start(&settled.reply, SUMMARY_CHARS),
        };
        let status = if settled.error.is_some() {
            "failed"
        } else {
            "completed"
        };
        self.write_agent_record(child, status, Some(&summary));
        if !reports {
            return;
        }
        let title = store
            .child_agents(&parent)
            .ok()
            .and_then(|children| children.into_iter().find(|agent| agent.id == child))
            .map(|agent| agent.title)
            .unwrap_or_default();
        let text = report(child, &title, settled);
        // Decided under the parent's own gate, as one with any pause: a paused
        // parent keeps it queued, a busy one queues it, an idle one takes it.
        let reply = self.send_message(
            &json!({"op": "send", "chat": parent, "text": text, "keep_paused": true}),
        );
        if reply["ok"] == true {
            self.events
                .publish(json!({"event": "changed", "chat": parent}));
        } else {
            eprintln!("xd-host: cannot report agent {child} to chat {parent}: {reply}");
        }
    }
}

impl Engine {
    /// Writes the card standing for agent chat `child` in its parent's
    /// transcript, and tells clients: a new card reloads the chat, a changed
    /// one is rewritten in place like any other agent card.
    fn write_agent_record(&self, child: &str, status: &str, summary: Option<&str>) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        self.publish_agent_record(store.write_agent_record(child, status, summary));
    }

    fn publish_agent_record(
        &self,
        written: Result<Option<crate::storage::AgentRecord>, crate::StorageError>,
    ) {
        match written {
            Ok(Some(record)) if record.created => self
                .events
                .publish(json!({"event": "changed", "chat": record.parent})),
            Ok(Some(record)) => self.events.publish(json!({
                "event": "record",
                "chat": record.parent,
                "record": record.key,
                "message_id": record.message_id,
                "text": record.text,
            })),
            Ok(None) => {}
            Err(error) => eprintln!("xd-host: cannot write an agent's card: {error}"),
        }
    }
}

/// The `new-chat` request for an agent `origin`'s agent asked for.
fn new_chat_request(origin: &AgentOrigin, request: &Value, prompt: &str) -> Result<Value, String> {
    let text = |key: &str| -> Result<Option<&str>, String> {
        match request.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) if text.trim().is_empty() => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.trim())),
            Some(_) => Err(format!("{key} must be text.")),
        }
    };
    let backend = text("assistant")?.unwrap_or(&origin.backend).to_owned();
    if !matches!(backend.as_str(), "claude" | "codex") {
        return Err("The assistant is claude or codex.".into());
    }
    let same = backend == origin.backend;
    let mut new_chat = json!({
        "op": "new-chat",
        "folder": origin.folder,
        "backend": backend,
        "prompt": prompt,
    });
    if let Some(title) = text("title")? {
        new_chat["title"] = json!(title.chars().take(120).collect::<String>());
        // Its worktree and branch are named for what the user sees it as.
        new_chat["prompt"] = new_chat["title"].clone();
    }
    // On the same assistant, it starts on the parent's model and effort unless
    // told otherwise; on the other, on what that assistant last used.
    let model = text("model")?
        .map(str::to_owned)
        .or_else(|| same.then(|| origin.model.clone()).flatten());
    if let Some(model) = model {
        new_chat["model"] = json!(model);
    }
    let effort = text("effort")?
        .map(str::to_owned)
        .or_else(|| same.then(|| origin.effort.clone()).flatten());
    if let Some(effort) = effort {
        new_chat["effort"] = json!(effort);
    }
    // Never more than the parent may do.
    if let Some(access) = &origin.access {
        new_chat["access"] = json!(access);
    }
    match text("workspace")?.unwrap_or("new_worktree") {
        "new_worktree" => new_chat["worktree"] = json!({"kind": "new"}),
        "this_worktree" => new_chat["session"] = json!(origin.session),
        _ => return Err("The workspace is new_worktree or this_worktree.".into()),
    }
    Ok(new_chat)
}

fn status(agent: &ChildAgent) -> &'static str {
    if agent.paused {
        "paused"
    } else if agent.working {
        "working"
    } else if agent.error.is_some() {
        "stopped with an error"
    } else if agent.reply.as_deref().and_then(crate::ask::parse).is_some() {
        "waiting for an answer"
    } else if agent.reply.is_some() {
        "finished"
    } else {
        "idle"
    }
}

fn describe(agent: &ChildAgent, limit: usize) -> Value {
    let mut described = json!({
        "id": agent.id,
        "title": agent.title,
        "assistant": agent.backend,
        "status": status(agent),
    });
    if let Some(model) = &agent.model {
        described["model"] = json!(model);
    }
    if let Some(reply) = &agent.reply {
        described["reply"] = json!(clip(reply, limit));
    }
    if let Some(error) = &agent.error {
        described["error"] = json!(clip(error, limit));
    }
    described
}

/// The message a parent chat gets when its agent `id`, called `title`, ends
/// a turn the way `settled` says.
fn report(id: &str, title: &str, settled: &SettledTurn) -> String {
    let name = if title.trim().is_empty() {
        "An agent".to_owned()
    } else {
        format!("Agent \"{}\"", title.trim())
    };
    let ending = if settled.error.is_some() {
        "stopped with an error"
    } else if settled.waiting {
        "waiting for an answer"
    } else {
        "done with its turn"
    };
    let mut text = format!("[xd] {name} ({id}) is {ending}.");
    if !settled.reply.is_empty() {
        text.push_str("\n\nIts reply:\n\n");
        text.push_str(&clip_start(&settled.reply, REPORT_CHARS));
    }
    if let Some(error) = &settled.error {
        text.push_str("\n\nThe error:\n\n");
        text.push_str(&clip(error, REPORT_CHARS));
    }
    text.push_str(
        "\n\nAnswer it with message_agent if it needs more, or check on every agent with \
         list_agents.",
    );
    text
}

/// The end of `text`, where an agent's final answer is, within `limit`
/// characters.
fn clip_start(text: &str, limit: usize) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= limit {
        return text.to_owned();
    }
    let start = text
        .char_indices()
        .nth(count - limit)
        .map_or(0, |(start, _)| start);
    format!("…{}", text[start..].trim_start())
}

fn clip(text: &str, limit: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> AgentOrigin {
        AgentOrigin {
            folder: "project".into(),
            backend: "claude".into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            access: Some("full".into()),
            session: "session".into(),
            parent: None,
        }
    }

    fn agent() -> ChildAgent {
        ChildAgent {
            id: "child".into(),
            title: "Fix the tests".into(),
            backend: "codex".into(),
            model: None,
            working: false,
            paused: false,
            reply: Some("All green.".into()),
            error: None,
        }
    }

    #[test]
    fn an_agent_on_the_same_assistant_starts_like_its_parent_in_a_new_worktree() {
        let request = new_chat_request(&origin(), &json!({"title": "  Fix  "}), "Fix it").unwrap();
        assert_eq!(
            request,
            json!({"op": "new-chat", "folder": "project", "backend": "claude", "prompt": "Fix",
                "title": "Fix", "model": "opus", "effort": "high", "access": "full",
                "worktree": {"kind": "new"}})
        );
    }

    #[test]
    fn an_agent_on_the_other_assistant_keeps_only_the_parents_access_and_place() {
        let request = new_chat_request(
            &origin(),
            &json!({"assistant": "codex", "workspace": "this_worktree", "effort": "low"}),
            "Fix it",
        )
        .unwrap();
        assert_eq!(
            request,
            json!({"op": "new-chat", "folder": "project", "backend": "codex", "prompt": "Fix it",
                "effort": "low", "access": "full", "session": "session"})
        );
        assert!(new_chat_request(&origin(), &json!({"assistant": "gemini"}), "x").is_err());
        assert!(new_chat_request(&origin(), &json!({"workspace": "elsewhere"}), "x").is_err());
        assert!(new_chat_request(&origin(), &json!({"model": 4}), "x").is_err());
    }

    #[test]
    fn a_report_says_how_the_turn_ended_and_what_the_agent_said() {
        let settled = SettledTurn {
            chat_id: "child".into(),
            reply: "All green.".into(),
            error: None,
            waiting: false,
        };
        assert_eq!(
            report("child", "Fix the tests", &settled),
            "[xd] Agent \"Fix the tests\" (child) is done with its turn.\n\nIts reply:\n\n\
             All green.\n\nAnswer it with message_agent if it needs more, or check on every \
             agent with list_agents."
        );
        let failed = SettledTurn {
            reply: String::new(),
            error: Some("Usage limit".into()),
            ..settled.clone()
        };
        assert!(
            report("child", " ", &failed)
                .starts_with("[xd] An agent (child) is stopped with an error.")
        );
        assert!(!report("child", "", &failed).contains("Its reply"));
        assert!(report("child", "", &failed).contains("The error:\n\nUsage limit"));
        let asking = SettledTurn {
            waiting: true,
            ..settled
        };
        assert!(report("child", "", &asking).contains("is waiting for an answer."));
    }

    #[test]
    fn a_failed_turn_is_not_read_as_the_question_before_it() {
        let question = "<ask>\nWhich?\n- A\n- B\n</ask>";
        let asking = ChildAgent {
            reply: Some(question.into()),
            ..agent()
        };
        assert_eq!(status(&asking), "waiting for an answer");
        let failed = ChildAgent {
            error: Some("Could not start.".into()),
            ..asking
        };
        assert_eq!(status(&failed), "stopped with an error");
    }

    #[test]
    fn long_replies_are_clipped_on_a_character() {
        assert_eq!(clip("ééé", 2), "éé…");
        assert_eq!(clip_start("abc déf", 3), "…déf");
        assert_eq!(clip_start(" short ", 10), "short");
        assert_eq!(clip("  short ", 10), "short");
        let described = describe(
            &ChildAgent {
                reply: Some("x".repeat(SUMMARY_CHARS + 5)),
                working: true,
                ..agent()
            },
            SUMMARY_CHARS,
        );
        assert_eq!(described["status"], "working");
        assert_eq!(
            described["reply"].as_str().unwrap().chars().count(),
            SUMMARY_CHARS + 1
        );
    }
}
