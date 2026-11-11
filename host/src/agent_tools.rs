//! The tools xd gives a chat's agent: linking pull requests to the chat, and
//! starting and minding agents of its own as chats nested under it.
//!
//! `xd-host mcp` serves them over MCP's stdio transport, one server per agent
//! process, bound to that process's chat. It hands each call to the daemon,
//! which owns the chat database. The daemon offers the tools only once it
//! knows how to start that server; see [`offer_link_tool`].

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use serde_json::{Value, json};

/// The server's name, as agents see it: Claude calls the tool
/// `mcp__xd__link_pull_requests`.
pub const SERVER: &str = "xd";
pub const TOOL: &str = "link_pull_requests";
pub const START: &str = "start_agent";
pub const LIST: &str = "list_agents";
pub const MESSAGE: &str = "message_agent";
pub const STOP: &str = "stop_agent";
const TOOLS: [&str; 5] = [TOOL, START, LIST, MESSAGE, STOP];
/// Added to the agent's instructions when it has the tools. Claude loads MCP
/// tools only when asked, so without this it would not know to.
const HINT: &str = "When you open, take over, or push to a pull request \
other than by running gh pr create here, link it to this chat with xd's \
link_pull_requests tool. To hand work to another Claude or Codex agent that \
should outlive this turn or work in parallel, start it with xd's start_agent \
tool, not codex exec, claude -p or a detached process: xd runs it on the \
user's preferred account, shows it to them, and sends you its reply here.";
/// The same for an agent another agent started, which cannot start its own.
const STARTED_HINT: &str = "When you open, take over, or push to a pull request \
other than by running gh pr create here, link it to this chat with xd's \
link_pull_requests tool. Another agent started you through xd and reads \
your final reply, so end each turn with what it needs to know.";

/// What the agent's instructions say about the tools. `started_by_agent` is
/// whether another chat's agent started this one.
pub(crate) fn hint(started_by_agent: bool) -> &'static str {
    if started_by_agent { STARTED_HINT } else { HINT }
}
/// How long an agent waits for one of these tools to answer, and the server
/// for the daemon. Codex gives up on a tool after a minute unless told.
pub const AGENT_TOOL_TIMEOUT_SECONDS: u64 = 300;
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

struct Launch {
    executable: PathBuf,
    database: PathBuf,
    workspaces: PathBuf,
}

static LAUNCH: OnceLock<Launch> = OnceLock::new();

/// Lets agents started from here on link pull requests, through `executable
/// mcp` with the daemon's own paths. Without it, agents get no tool.
/// Paths that are not UTF-8 cannot be written into an agent's configuration,
/// so they offer nothing.
pub fn offer_link_tool(executable: PathBuf, database: PathBuf, workspaces: PathBuf) {
    if [&executable, &database, &workspaces]
        .iter()
        .any(|path| path.to_str().is_none())
    {
        return;
    }
    let _ = LAUNCH.set(Launch {
        executable,
        database,
        workspaces,
    });
}

/// Whether an agent on `backend` gets the link tool.
pub(crate) fn offered(backend: &str) -> bool {
    LAUNCH.get().is_some() && matches!(backend, "claude" | "codex")
}

/// Gives the agent `command` starts the link tool for `chat_id`. Returns
/// whether it did: not when no tool is offered, for another assistant, or when
/// a path cannot be written into the agent's configuration.
pub(crate) fn add_link_tool(command: &mut Command, backend: &str, chat_id: &str) -> bool {
    LAUNCH
        .get()
        .is_some_and(|launch| add_server(launch, command, backend, chat_id))
}

fn add_server(launch: &Launch, command: &mut Command, backend: &str, chat_id: &str) -> bool {
    let text = |path: &Path| path.to_str().map(str::to_owned);
    let (Some(executable), Some(database), Some(workspaces)) = (
        text(&launch.executable),
        text(&launch.database),
        text(&launch.workspaces),
    ) else {
        return false;
    };
    let arguments = [
        "mcp".to_owned(),
        "--database".to_owned(),
        database,
        "--workspaces".to_owned(),
        workspaces,
        "--chat".to_owned(),
        chat_id.to_owned(),
    ];
    match backend {
        "claude" => {
            let config = json!({"mcpServers": {SERVER: {
                "type": "stdio",
                "command": executable,
                "args": arguments,
            }}});
            command
                .arg("--mcp-config")
                .arg(config.to_string())
                // The tools do only what the agent could already do from a
                // shell, so they never ask first.
                .arg("--allowedTools")
                .arg(
                    TOOLS
                        .iter()
                        .map(|tool| format!("mcp__{SERVER}__{tool}"))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            true
        }
        "codex" => {
            // JSON strings and arrays of strings are valid TOML values.
            let executable = Value::String(executable).to_string();
            let arguments = json!(arguments).to_string();
            for setting in [
                format!("mcp_servers.{SERVER}.command={executable}"),
                format!("mcp_servers.{SERVER}.args={arguments}"),
                format!("mcp_servers.{SERVER}.default_tools_approval_mode=\"approve\""),
                format!("mcp_servers.{SERVER}.tool_timeout_sec={AGENT_TOOL_TIMEOUT_SECONDS}"),
            ] {
                command.arg("-c").arg(setting);
            }
            true
        }
        _ => false,
    }
}

fn tools() -> Value {
    let agent = json!({"type": "string", "description": "The agent's id, as start_agent or list_agents gave it."});
    json!([
        {
            "name": TOOL,
            "title": "Link pull requests",
            "description": "Link pull requests to this xd chat. xd shows them beside the chat and \
                settles it once they merge. Pull requests opened here with gh pr create, and the \
                branch's own, are linked already.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "urls": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1,
                        "maxItems": crate::pull_requests::MAX_PER_REQUEST,
                        "description": "Full pull request URLs.",
                    },
                },
                "required": ["urls"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false,
            },
        },
        {
            "name": START,
            "title": "Start an agent",
            "description": "Start a Claude or Codex agent on a task, in its own xd chat nested \
                under this one. xd runs it on the account the user ranks first that has usage \
                left, keeps it running after this session ends, and shows it to the user. Each \
                time it finishes a turn, xd sends you its reply as a message in this chat. Use \
                this instead of running codex exec, claude -p or another assistant from a shell.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "The task. The agent sees nothing \
                        of this chat, so say everything it needs to know."},
                    "title": {"type": "string", "description": "A short name for the user to \
                        tell it by. Made from the prompt when left out."},
                    "assistant": {"type": "string", "enum": ["claude", "codex"],
                        "description": "Which assistant runs it. Defaults to this chat's."},
                    "model": {"type": "string", "description": "A model id. Defaults to this \
                        chat's on the same assistant, and the user's last choice on the other."},
                    "effort": {"type": "string", "description": "Reasoning effort, such as low, \
                        medium, high or xhigh. Defaults like model."},
                    "workspace": {"type": "string", "enum": ["new_worktree", "this_worktree"],
                        "description": "new_worktree (the default) gives it a fresh git worktree \
                        and branch of its own; this_worktree has it work in this chat's checkout, \
                        as another tab of this session."},
                    "notify": {"type": "boolean", "description": "Send this chat the agent's \
                        reply each time it finishes a turn. Defaults to true."},
                },
                "required": ["prompt"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": false,
            },
        },
        {
            "name": LIST,
            "title": "List agents",
            "description": "List the agents this chat started with start_agent: whether each is \
                working, finished, waiting for an answer or stopped, and the start of its latest \
                reply. Name one agent to read its whole latest reply.",
            "inputSchema": {
                "type": "object",
                "properties": {"agent": agent},
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false,
            },
        },
        {
            "name": MESSAGE,
            "title": "Message an agent",
            "description": "Send one of this chat's agents a message, as the user would. It is \
                queued behind the agent's turn when it is working.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": agent,
                    "text": {"type": "string", "description": "The message."},
                },
                "required": ["agent", "text"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": false,
            },
        },
        {
            "name": STOP,
            "title": "Stop an agent",
            "description": "Stop one of this chat's agents and throw it away, once you need \
                nothing more from it. Any turn it is taking stops and its chat is deleted, so it \
                no longer shows to the user and cannot be messaged again. Files it wrote in its \
                worktree stay on disk. To change its course instead, use message_agent.",
            "inputSchema": {
                "type": "object",
                "properties": {"agent": agent},
                "required": ["agent"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": true,
                "openWorldHint": false,
            },
        },
    ])
}

/// Answers one MCP message, or `None` for a notification. `ask` sends the
/// daemon a request for this server's chat and returns its reply.
pub fn answer_mcp(
    message: &Value,
    ask: impl FnOnce(Value) -> Result<Value, String>,
) -> Option<Value> {
    let id = message.get("id").filter(|id| !id.is_null())?.clone();
    let reply = |outcome: Result<Value, (i64, &str)>| {
        Some(match outcome {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, text)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": text}})
            }
        })
    };
    let params = message.get("params").unwrap_or(&Value::Null);
    match message.get("method").and_then(Value::as_str) {
        Some("initialize") => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked
                .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
                .unwrap_or(PROTOCOL_VERSIONS[1]);
            reply(Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": SERVER, "version": env!("CARGO_PKG_VERSION")},
            })))
        }
        Some("ping") => reply(Ok(json!({}))),
        Some("tools/list") => reply(Ok(json!({"tools": tools()}))),
        Some("tools/call") => {
            let arguments = params.get("arguments").unwrap_or(&Value::Null);
            match params.get("name").and_then(Value::as_str) {
                Some(TOOL) => reply(Ok(link(arguments, ask))),
                Some(START) => reply(Ok(start(arguments, ask))),
                Some(LIST) => reply(Ok(list(arguments, ask))),
                Some(MESSAGE) => reply(Ok(message_agent(arguments, ask))),
                Some(STOP) => reply(Ok(stop(arguments, ask))),
                _ => reply(Err((-32602, "No such tool."))),
            }
        }
        _ => reply(Err((-32601, "Method not found."))),
    }
}

/// Asks the daemon, and turns a refusal or a daemon it cannot reach into the
/// tool's error. `done` words a reply that went through.
fn ask_daemon(
    ask: impl FnOnce(Value) -> Result<Value, String>,
    request: Value,
    done: impl FnOnce(&Value) -> Value,
) -> Value {
    match ask(request) {
        Ok(answer) if answer.get("ok").and_then(Value::as_bool) == Some(true) => done(&answer),
        Ok(answer) => tool_result(
            answer
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("xd refused that."),
            true,
        ),
        Err(error) => tool_result(&format!("xd could not be reached: {error}"), true),
    }
}

/// The text `key` holds in `arguments`, when it is text that says something.
fn text<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
}

fn link(arguments: &Value, ask: impl FnOnce(Value) -> Result<Value, String>) -> Value {
    let urls = arguments
        .get("urls")
        .and_then(Value::as_array)
        .and_then(|urls| {
            urls.iter()
                .map(|url| url.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .filter(|urls| !urls.is_empty());
    let Some(urls) = urls else {
        return tool_result("Pass urls, a list of pull request URLs.", true);
    };
    match ask(json!({"op": "link-pull-requests", "urls": urls, "source": "agent"})) {
        Ok(answer) if answer.get("ok").and_then(Value::as_bool) == Some(true) => {
            let list = |key: &str| {
                answer
                    .get(key)
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default()
            };
            let (linked, invalid) = (list("linked"), list("invalid"));
            let mut text = Vec::new();
            if !linked.is_empty() {
                text.push(format!("Linked to this chat: {linked}."));
            }
            if !invalid.is_empty() {
                text.push(format!("Not pull request URLs: {invalid}."));
            }
            tool_result(&text.join(" "), linked.is_empty())
        }
        Ok(answer) => tool_result(
            answer
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("xd could not link them."),
            true,
        ),
        Err(error) => tool_result(&format!("xd could not link them: {error}"), true),
    }
}

fn start(arguments: &Value, ask: impl FnOnce(Value) -> Result<Value, String>) -> Value {
    let Some(prompt) = text(arguments, "prompt") else {
        return tool_result("Pass prompt, the agent's task.", true);
    };
    let mut request = json!({"op": "start-agent", "prompt": prompt});
    for key in ["title", "assistant", "model", "effort", "workspace"] {
        if let Some(value) = text(arguments, key) {
            request[key] = json!(value);
        }
    }
    if let Some(notify) = arguments.get("notify").and_then(Value::as_bool) {
        request["notify"] = json!(notify);
    }
    ask_daemon(ask, request, |answer| {
        let id = answer["agent"].as_str().unwrap_or_default();
        let assistant = match answer["backend"].as_str() {
            Some("codex") => "Codex",
            _ => "Claude",
        };
        let model = answer["model"]
            .as_str()
            .map(|model| format!(" ({model})"))
            .unwrap_or_default();
        let after = if answer["reports"] == false {
            "Check on it with list_agents."
        } else {
            "xd will send you its reply here each time it finishes a turn, so there is no need \
             to poll it."
        };
        tool_result(
            &format!("Started agent {id} on {assistant}{model}. {after}"),
            false,
        )
    })
}

fn list(arguments: &Value, ask: impl FnOnce(Value) -> Result<Value, String>) -> Value {
    let mut request = json!({"op": "list-agents"});
    if let Some(agent) = text(arguments, "agent") {
        request["agent"] = json!(agent);
    }
    ask_daemon(ask, request, |answer| {
        match answer["agents"]
            .as_array()
            .filter(|agents| !agents.is_empty())
        {
            Some(agents) => tool_result(
                &serde_json::to_string_pretty(agents).unwrap_or_default(),
                false,
            ),
            None => tool_result("This chat has started no agents.", false),
        }
    })
}

fn message_agent(arguments: &Value, ask: impl FnOnce(Value) -> Result<Value, String>) -> Value {
    let (Some(agent), Some(message)) = (text(arguments, "agent"), text(arguments, "text")) else {
        return tool_result("Pass agent, the agent's id, and text, the message.", true);
    };
    ask_daemon(
        ask,
        json!({"op": "message-agent", "agent": agent, "text": message}),
        |answer| {
            tool_result(
                if answer["queued"] == true {
                    "Queued behind the agent's current turn."
                } else {
                    "Sent. The agent is working on it."
                },
                false,
            )
        },
    )
}

fn stop(arguments: &Value, ask: impl FnOnce(Value) -> Result<Value, String>) -> Value {
    let Some(agent) = text(arguments, "agent") else {
        return tool_result("Pass agent, the agent's id.", true);
    };
    ask_daemon(ask, json!({"op": "stop-agent", "agent": agent}), |answer| {
        tool_result(
            if answer["was_working"] == false {
                "Threw the agent away."
            } else {
                "Stopped the agent's turn and threw it away."
            },
            false,
        )
    })
}

fn tool_result(text: &str, error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": error})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_introduces_itself_and_lists_its_tools() {
        let started = answer_mcp(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-03-26"}}),
            |_| unreachable!(),
        )
        .unwrap();
        assert_eq!(started["result"]["protocolVersion"], "2025-03-26");
        assert!(started["result"]["capabilities"]["tools"].is_object());
        let unknown_version = answer_mcp(
            &json!({"id": 2, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}}),
            |_| unreachable!(),
        )
        .unwrap();
        assert_eq!(unknown_version["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            answer_mcp(
                &json!({"method": "notifications/initialized"}),
                |_| unreachable!()
            ),
            None
        );
        let tools = answer_mcp(
            &json!({"id": "t", "method": "tools/list"}),
            |_| unreachable!(),
        )
        .unwrap();
        assert_eq!(tools["id"], "t");
        let names = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, TOOLS);
        assert_eq!(
            answer_mcp(&json!({"id": 3, "method": "resources/list"}), |_| {
                unreachable!()
            })
            .unwrap()["error"]["code"],
            -32601
        );
    }

    #[test]
    fn a_call_hands_the_addresses_to_the_daemon_and_reports_back() {
        let call = |arguments: Value, answer: Result<Value, String>| {
            answer_mcp(
                &json!({"id": 9, "method": "tools/call",
                    "params": {"name": TOOL, "arguments": arguments}}),
                |request| {
                    assert_eq!(request["op"], "link-pull-requests");
                    assert_eq!(request["source"], "agent");
                    assert!(!request["urls"].as_array().unwrap().is_empty());
                    answer
                },
            )
            .unwrap()["result"]
                .clone()
        };
        let linked = call(
            json!({"urls": ["https://github.com/o/r/pull/1/files", "nope"]}),
            Ok(
                json!({"ok": true, "linked": ["https://github.com/o/r/pull/1"], "invalid": ["nope"]}),
            ),
        );
        assert_eq!(linked["isError"], false);
        assert_eq!(
            linked["content"][0]["text"],
            "Linked to this chat: https://github.com/o/r/pull/1. Not pull request URLs: nope."
        );
        let refused = call(
            json!({"urls": ["https://github.com/o/r/pull/1"]}),
            Ok(json!({"ok": false, "error": "No such chat."})),
        );
        assert_eq!(refused["isError"], true);
        assert_eq!(refused["content"][0]["text"], "No such chat.");
        let down = call(
            json!({"urls": ["https://github.com/o/r/pull/1"]}),
            Err("The xd daemon is not running.".into()),
        );
        assert_eq!(down["isError"], true);
        assert_eq!(
            down["content"][0]["text"],
            "xd could not link them: The xd daemon is not running."
        );
        let empty = answer_mcp(
            &json!({"id": 1, "method": "tools/call", "params": {"name": TOOL, "arguments": {"urls": []}}}),
            |_| unreachable!(),
        )
        .unwrap();
        assert_eq!(empty["result"]["isError"], true);
    }

    #[test]
    fn each_assistant_gets_the_server_in_its_own_configuration() {
        let launch = Launch {
            executable: "/opt/xd/xd-host".into(),
            database: "/data/chats.db".into(),
            workspaces: "/data/Workspaces".into(),
        };
        let arguments = |backend: &str| {
            let mut command = Command::new("agent");
            let added = add_server(&launch, &mut command, backend, "chat-1");
            (
                added,
                command
                    .get_args()
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
            )
        };
        let (added, claude) = arguments("claude");
        assert!(added);
        let config: Value = serde_json::from_str(&claude[1]).unwrap();
        assert_eq!(config["mcpServers"]["xd"]["command"], "/opt/xd/xd-host");
        assert_eq!(
            config["mcpServers"]["xd"]["args"],
            json!([
                "mcp",
                "--database",
                "/data/chats.db",
                "--workspaces",
                "/data/Workspaces",
                "--chat",
                "chat-1"
            ])
        );
        assert_eq!(
            claude[2..],
            [
                "--allowedTools",
                "mcp__xd__link_pull_requests,mcp__xd__start_agent,mcp__xd__list_agents,\
                 mcp__xd__message_agent,mcp__xd__stop_agent"
            ]
        );
        let (added, codex) = arguments("codex");
        assert!(added);
        assert_eq!(
            codex,
            [
                "-c",
                "mcp_servers.xd.command=\"/opt/xd/xd-host\"",
                "-c",
                "mcp_servers.xd.args=[\"mcp\",\"--database\",\"/data/chats.db\",\"--workspaces\",\"/data/Workspaces\",\"--chat\",\"chat-1\"]",
                "-c",
                "mcp_servers.xd.default_tools_approval_mode=\"approve\"",
                "-c",
                "mcp_servers.xd.tool_timeout_sec=300",
            ]
        );
        assert!(!arguments("other").0);
    }

    fn call(tool: &str, arguments: Value, answer: Value) -> (Option<Value>, Value) {
        let mut asked = None;
        let reply = answer_mcp(
            &json!({"id": 1, "method": "tools/call",
                "params": {"name": tool, "arguments": arguments}}),
            |request| {
                asked = Some(request);
                Ok(answer)
            },
        )
        .unwrap();
        (asked, reply["result"].clone())
    }

    #[test]
    fn starting_an_agent_passes_on_only_what_the_agent_chose() {
        let (asked, result) = call(
            START,
            json!({"prompt": "Fix CI", "assistant": "codex", "model": " ", "notify": false,
                "chat": "someone-else"}),
            json!({"ok": true, "agent": "child", "backend": "codex", "model": "gpt-5.5",
                "reports": false}),
        );
        // The chat is the server's, added by the process that serves it.
        assert_eq!(
            asked.unwrap(),
            json!({"op": "start-agent", "prompt": "Fix CI", "assistant": "codex", "notify": false})
        );
        assert_eq!(result["isError"], false);
        assert_eq!(
            result["content"][0]["text"],
            "Started agent child on Codex (gpt-5.5). Check on it with list_agents."
        );
        let (asked, result) = call(START, json!({"prompt": "  "}), json!({}));
        assert!(asked.is_none());
        assert_eq!(result["isError"], true);
        let (_, refused) = call(
            START,
            json!({"prompt": "Fix CI"}),
            json!({"ok": false, "error": "Too many agents."}),
        );
        assert_eq!(refused["isError"], true);
        assert_eq!(refused["content"][0]["text"], "Too many agents.");
    }

    #[test]
    fn agents_are_listed_messaged_and_stopped_by_id() {
        let (asked, listed) = call(
            LIST,
            json!({}),
            json!({"ok": true, "agents": [{"id": "child", "status": "working"}]}),
        );
        assert_eq!(asked.unwrap(), json!({"op": "list-agents"}));
        let agents: Value =
            serde_json::from_str(listed["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(agents, json!([{"id": "child", "status": "working"}]));
        let (_, none) = call(LIST, json!({}), json!({"ok": true, "agents": []}));
        assert_eq!(
            none["content"][0]["text"],
            "This chat has started no agents."
        );

        let (asked, queued) = call(
            MESSAGE,
            json!({"agent": "child", "text": "Also lint"}),
            json!({"ok": true, "queued": true}),
        );
        assert_eq!(
            asked.unwrap(),
            json!({"op": "message-agent", "agent": "child", "text": "Also lint"})
        );
        assert_eq!(
            queued["content"][0]["text"],
            "Queued behind the agent's current turn."
        );
        let (asked, _) = call(MESSAGE, json!({"agent": "child"}), json!({}));
        assert!(asked.is_none());

        let (asked, stopped) = call(
            STOP,
            json!({"agent": "child"}),
            json!({"ok": true, "was_working": false}),
        );
        assert_eq!(
            asked.unwrap(),
            json!({"op": "stop-agent", "agent": "child"})
        );
        assert_eq!(stopped["content"][0]["text"], "Threw the agent away.");
    }

    #[test]
    fn an_agent_another_started_is_not_told_to_start_its_own() {
        assert!(hint(false).contains("start_agent"));
        assert!(!hint(true).contains("start_agent"));
    }
}
