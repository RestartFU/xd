//! Agents a chat's agent starts through xd's `start_agent` tool.
//!
//! Each one is an ordinary xd chat that names the chat that started it. It runs
//! like any other chat: on the account xd picks for it, in a workspace xd made,
//! owned by the daemon rather than by the agent that asked for it. So it keeps
//! running when that agent's process ends, it shows in every client nested
//! under its parent, and pausing, resuming and pull request links work on it.
//! Each finished turn is reported back to the parent chat when asked to, out of
//! the parent's queue: see [`Engine::deliver_report`].

use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
};

use serde_json::{Value, json};

use crate::{
    Engine, error_reply, required_string,
    runtime::{SettledTurn, TurnNews},
    storage::{AgentOrigin, ChildAgent, ReportDelivery, StateStore},
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
            // Its card goes with it.
            if let Ok(Some(parent)) = store.remove_agent_record(&child) {
                self.events
                    .publish(json!({"event": "changed", "chat": parent}));
            }
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
        // Read before the agent's state: any report written since is newer.
        let waiting = only.map(|only| store.latest_agent_report(parent, only).unwrap_or(0));
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
        let background = |child: &ChildAgent| {
            self.runtime
                .as_ref()
                .is_some_and(|runtime| runtime.background_tasks(&child.id) > 0)
        };
        let agents = children
            .iter()
            .filter(|child| only.is_none_or(|only| child.id == only))
            .map(|child| describe(child, limit, background(child)))
            .collect::<Vec<_>>();
        // Asked about one agent, the chat's agent has read its whole reply,
        // as a report would give it, so a report still waiting with that
        // news is not handed over again.
        if let (Some(only), Some(through)) = (only, waiting.filter(|through| *through > 0))
            && children.iter().any(|child| {
                child.id == only && !child.working && !child.paused && !background(child)
            })
        {
            let _ = store.drop_agent_reports(parent, only, through);
        }
        json!({"ok": true, "agents": agents})
    }

    /// `message-agent`: sends one of `chat`'s agents a message, queued behind
    /// its turn when it is at work. With `steer` it redirects that turn instead,
    /// as the user steering a queued message does: the turn takes the message
    /// when its agent can, otherwise it stops and the message goes first.
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
        let steer = request.get("steer").and_then(Value::as_bool) == Some(true);
        let reply = self.send_message(&json!({"op": "send", "chat": child.id, "text": text,
            "from_parent": true, "steer": steer}));
        if reply["ok"] == true {
            self.events
                .publish(json!({"event": "changed", "chat": child.id}));
        }
        reply
    }

    /// `stop-agent`: throws one of `chat`'s agents away. An agent is only
    /// worth showing while it works or may be asked for more, so stopping it
    /// deletes its chat as the user deleting it would: any turn it is taking
    /// stops, it leaves every client's tree, and its card leaves the parent.
    /// Files it wrote in a worktree stay on disk.
    pub(crate) fn stop_agent(&self, request: &Value) -> Value {
        let child = match self.own_agent(request) {
            Ok(child) => child,
            Err(error) => return error_reply(error),
        };
        let reply = self.delete_chat(&json!({"op": "delete-chat", "chat": child.id}));
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
        let (sender, receiver) = mpsc::channel::<TurnNews>();
        // Weak, since the engine owns the runtime that holds the sender.
        let engine = Arc::downgrade(self);
        // Off the turns' threads: sending takes the parent's own locks.
        let spawned = thread::Builder::new()
            .name("xd-agent-reports".into())
            .spawn(move || {
                for news in receiver {
                    let Some(engine) = engine.upgrade() else {
                        return;
                    };
                    match news {
                        TurnNews::Settled(settled) => engine.report_to_parent(&settled),
                        TurnNews::Unheld(agent) => engine.release_held_report(&agent),
                    }
                }
            });
        if spawned.is_err() {
            return;
        }
        let sender = Mutex::new(sender);
        runtime.set_turn_settled_handler(Box::new(move |news| {
            if let Ok(sender) = sender.lock() {
                let _ = sender.send(news);
            }
        }));
    }

    fn report_to_parent(&self, settled: &SettledTurn) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let child = settled.chat_id.as_str();
        // Reports that came while this chat was busy go once it is free, for
        // the ones a turn ending did not hand over, such as when the turn
        // after it could not start. Reports a failed start gave back wait for
        // something else to happen, or a start that keeps failing would
        // retry them forever.
        if store.has_fresh_reports(child).unwrap_or(false) {
            let _answer = self
                .restart_answer
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Err(error) = self.start_waiting_reports(child, false) {
                eprintln!("xd-host: cannot hand chat {child} its agents' reports: {error}");
            }
        }
        let Ok(Some((parent, reports))) = store.chat_parent(child) else {
            return;
        };
        // The card follows every turn; the message only when it was asked
        // for and has something to say. An agent waiting on background work
        // of its own is still at it: its next turn says how that went.
        let (status, summary) = match &settled.error {
            _ if settled.stopped => ("stopped", None),
            Some(error) => ("failed", Some(clip(error, SUMMARY_CHARS))),
            None if settled.background => {
                ("running", Some(clip_start(&settled.reply, SUMMARY_CHARS)))
            }
            None => ("completed", Some(clip_start(&settled.reply, SUMMARY_CHARS))),
        };
        self.write_agent_record(child, status, summary.as_deref());
        if !reports || settled.quiet || settled.stopped {
            // Nothing to say, so a report it held back has nothing to replace it.
            if !settled.background {
                let _ = store.drop_held_report(child);
            }
            return;
        }
        let title = store
            .child_agents(&parent)
            .ok()
            .and_then(|children| children.into_iter().find(|agent| agent.id == child))
            .map(|agent| agent.title)
            .unwrap_or_default();
        let text = report(child, &title, settled);
        // Kept, so a restart that cuts the agent short, whose next turn then
        // never comes, does not lose it.
        if settled.background {
            if let Err(error) = store.hold_agent_report(&parent, child, &text) {
                eprintln!("xd-host: cannot keep agent {child}'s report: {error}");
            }
            return;
        }
        // Stored in place of any report the agent held back, in one go. One
        // told again after being held goes only if that held report is still
        // there: a restart's recovery may have handed it over already.
        if let Err(error) = self.deliver_report(&parent, child, &text, settled.released) {
            eprintln!("xd-host: cannot report agent {child} to chat {parent}: {error}");
        }
    }

    /// Hands chat `parent` the report `text` from its agent `child`: now when
    /// the chat is free, otherwise held out of its queue until its turn ends
    /// or it resumes. Decided under the chat's gate, as a send is, so it is
    /// ordered with any pause.
    fn deliver_report(
        &self,
        parent: &str,
        child: &str,
        text: &str,
        released: bool,
    ) -> Result<(), String> {
        self.hand_over_report(parent, |store| {
            store
                .deliver_agent_report(parent, child, text, released)
                .map(Some)
                .map_err(|error| error.to_string())
        })
        .map(|_| ())
    }

    /// Stores a report for chat `chat` through `store_it`, under the chat's
    /// gate and the session resources, as a send is, and starts the turn that
    /// hands it over when the chat is free. `store_it` answers `None` when it
    /// stored nothing; so does this.
    pub(crate) fn hand_over_report(
        &self,
        chat: &str,
        store_it: impl FnOnce(&StateStore) -> Result<Option<ReportDelivery>, String>,
    ) -> Result<Option<()>, String> {
        let (Some(store), Some(runtime)) = (self.store.as_ref(), self.runtime.as_ref()) else {
            return Err("Rust host state storage is not configured.".into());
        };
        let _answer = self
            .restart_answer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let gate = self.chat_execution_gate(chat)?;
        let _execution = gate
            .lock()
            .map_err(|_| "This session's agent state is unavailable.".to_owned())?;
        let _resources = self
            .session_resources
            .lock()
            .map_err(|_| "Session resources are unavailable.".to_owned())?;
        // As for a message: a chat past its quota but not yet stopped stops now.
        if store
            .chat_quota(chat)
            .is_ok_and(|quota| !quota.reached && quota.spent())
        {
            self.stop_for_quota_gated(chat);
        }
        let settled = store.chat_has_settlement_override(chat).unwrap_or(false);
        let Some(delivery) = store_it(store)? else {
            return Ok(None);
        };
        match delivery {
            ReportDelivery::Held => Ok(Some(())),
            ReportDelivery::SteeredFirst(front) => self.start_steered(chat, &front).map(Some),
            ReportDelivery::Start(turn) => {
                if settled {
                    self.events.publish(json!({"event": "tree"}));
                }
                let started = runtime.start(turn.clone());
                if let Err(error) = &started {
                    let _ = store.abort_turn_start(&turn.chat_id, error);
                    runtime.report_start_failure(chat, error, false);
                }
                self.events
                    .publish(json!({"event": "changed", "chat": chat}));
                started.map(Some)
            }
        }
    }

    /// Starts the turn that hands chat `chat_id` the agent reports waiting
    /// for it, when it is free, or first the message the user steered to the
    /// front of its queue. True when either started. `gated` when the caller
    /// holds the chat's execution gate.
    pub(crate) fn start_waiting_reports(&self, chat_id: &str, gated: bool) -> Result<bool, String> {
        let (Some(store), Some(runtime)) = (self.store.as_ref(), self.runtime.as_ref()) else {
            return Ok(false);
        };
        let gate = if gated {
            None
        } else {
            Some(self.chat_execution_gate(chat_id)?)
        };
        let _execution = match &gate {
            Some(gate) => Some(
                gate.lock()
                    .map_err(|_| "This session's agent state is unavailable.".to_owned())?,
            ),
            None => None,
        };
        let _resources = self
            .session_resources
            .lock()
            .map_err(|_| "Session resources are unavailable.".to_owned())?;
        // As for a message: a chat past its quota but not yet stopped stops now.
        if store
            .chat_quota(chat_id)
            .is_ok_and(|quota| !quota.reached && quota.spent())
        {
            self.stop_for_quota_gated(chat_id);
        }
        let turn = match store
            .start_agent_reports(chat_id)
            .map_err(|error| error.to_string())?
        {
            ReportDelivery::Held => return Ok(false),
            // The message the user steered goes first; the reports follow.
            ReportDelivery::SteeredFirst(front) => {
                return self.start_steered(chat_id, &front).map(|()| true);
            }
            ReportDelivery::Start(turn) => turn,
        };
        let started = runtime.start(turn.clone());
        if let Err(error) = &started {
            let _ = store.abort_turn_start(&turn.chat_id, error);
            runtime.report_start_failure(chat_id, error, false);
        }
        self.events
            .publish(json!({"event": "changed", "chat": chat_id}));
        started.map(|()| true)
    }

    /// Starts the message the user steered to the front of chat `chat_id`'s
    /// queue, which its waiting reports follow. A start that fails is told
    /// of like any other, which hands the reports over after it. The caller
    /// holds the chat's gate.
    fn start_steered(&self, chat_id: &str, front: &str) -> Result<(), String> {
        let started = self.start_queued_if_idle(chat_id, front, true, false);
        if let (Err(error), Some(runtime)) = (&started, self.runtime.as_ref()) {
            runtime.report_start_failure(chat_id, error, false);
        }
        started
    }

    /// Lets reports agents held back go to their chats when no turn of the
    /// agent's will replace them: the agent is free, the user is not being
    /// asked whether it resumes, and the runtime holds no turn of its back,
    /// as after a restart that ended its background work. The runtime says
    /// so in order with the news of the agent's turns, which the reporting
    /// thread handles one at a time: see [`Self::release_held_report`].
    pub(crate) fn release_stale_held_reports(&self) {
        let (Some(store), Some(runtime)) = (self.store.as_ref(), self.runtime.as_ref()) else {
            return;
        };
        for (agent, _) in store.idle_agents_holding_reports().unwrap_or_default() {
            // Asked again under the agent's gate, which every turn of its
            // starts under, so none is starting meanwhile.
            let Ok(gate) = self.chat_execution_gate(&agent) else {
                continue;
            };
            let Ok(_execution) = gate.lock() else {
                continue;
            };
            if store.holds_idle_report(&agent).unwrap_or(false) {
                runtime.tell_unheld(std::slice::from_ref(&agent));
            }
        }
    }

    /// Lets the report agent `agent` held back wait for its chat, and hands
    /// it over if the chat is free. On the reporting thread: any later turn
    /// of the agent's is told of after this, and replaces nothing then.
    fn release_held_report(&self, agent: &str) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let Ok(Some((parent, _))) = store.chat_parent(agent) else {
            return;
        };
        let _answer = self
            .restart_answer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _ = store.release_held_report(agent);
        if let Err(error) = self.start_waiting_reports(&parent, false) {
            eprintln!("xd-host: cannot hand chat {parent} its agents' reports: {error}");
        }
    }

    /// Hands every free chat the agent reports left waiting for it, such as
    /// those a restart left behind. Off the startup path, since a turn can
    /// take a while to start.
    pub fn deliver_waiting_agent_reports(self: &Arc<Self>) {
        let engine = Arc::downgrade(self);
        let spawned = thread::Builder::new()
            .name("xd-waiting-reports".into())
            .spawn(move || {
                let Some(engine) = engine.upgrade() else {
                    return;
                };
                let Some(store) = engine.store.as_ref() else {
                    return;
                };
                let _answer = engine
                    .restart_answer
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                engine.release_stale_held_reports();
                for chat_id in store.chats_with_waiting_reports().unwrap_or_default() {
                    if let Err(error) = engine.start_waiting_reports(&chat_id, false) {
                        eprintln!(
                            "xd-host: cannot hand chat {chat_id} its agents' reports: {error}"
                        );
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("xd-host: cannot hand chats their agents' reports: {error}");
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

/// `background` when it waits on background agents or shells of its own,
/// which it reports on in a turn of its own once they end.
fn status(agent: &ChildAgent, background: bool) -> &'static str {
    if agent.paused {
        "paused"
    } else if agent.working {
        "working"
    } else if agent.error.is_some() {
        "stopped with an error"
    } else if agent.reply.as_deref().and_then(crate::ask::parse).is_some() {
        "waiting for an answer"
    } else if background {
        "working"
    } else if agent.reply.is_some() {
        "finished"
    } else {
        "idle"
    }
}

fn describe(agent: &ChildAgent, limit: usize, background: bool) -> Value {
    let mut described = json!({
        "id": agent.id,
        "title": agent.title,
        "assistant": agent.backend,
        "status": status(agent, background),
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
            stopped: false,
            quiet: false,
            background: false,
            released: false,
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
        assert_eq!(status(&asking, false), "waiting for an answer");
        assert_eq!(status(&asking, true), "waiting for an answer");
        // Waiting on background work of its own, it is still at work.
        let waiting = ChildAgent {
            reply: Some("Tests are running; I'll wait for them.".into()),
            ..agent()
        };
        assert_eq!(status(&waiting, true), "working");
        assert_eq!(status(&waiting, false), "finished");
        let failed = ChildAgent {
            error: Some("Could not start.".into()),
            ..asking
        };
        assert_eq!(status(&failed, false), "stopped with an error");
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
            false,
        );
        assert_eq!(described["status"], "working");
        assert_eq!(
            described["reply"].as_str().unwrap().chars().count(),
            SUMMARY_CHARS + 1
        );
    }
}
