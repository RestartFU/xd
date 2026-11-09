//! The one tool xd gives a chat's agent: linking pull requests to the chat.
//!
//! `xd-host mcp` serves it over MCP's stdio transport, one server per agent
//! process, bound to that process's chat. It hands each call to the daemon,
//! which owns the chat database. The daemon offers the tool only once it knows
//! how to start that server; see [`offer_link_tool`].

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
/// Added to the agent's instructions when it has the tool. Claude loads MCP
/// tools only when asked, so without this it would not know to.
pub(crate) const HINT: &str = "When you open, take over, or push to a pull request \
other than by running gh pr create here, link it to this chat with xd's \
link_pull_requests tool.";
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
                // The tool only records links, so it never asks first.
                .arg("--allowedTools")
                .arg(format!("mcp__{SERVER}__{TOOL}"));
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
            ] {
                command.arg("-c").arg(setting);
            }
            true
        }
        _ => false,
    }
}

fn tool() -> Value {
    json!({
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
    })
}

/// Answers one MCP message, or `None` for a notification. `link` asks the
/// daemon to link the addresses given and returns its reply.
pub fn answer_mcp(
    message: &Value,
    link: impl FnOnce(&[String]) -> Result<Value, String>,
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
        Some("tools/list") => reply(Ok(json!({"tools": [tool()]}))),
        Some("tools/call") => {
            if params.get("name").and_then(Value::as_str) != Some(TOOL) {
                return reply(Err((-32602, "No such tool.")));
            }
            let urls = params
                .get("arguments")
                .and_then(|arguments| arguments.get("urls"))
                .and_then(Value::as_array)
                .and_then(|urls| {
                    urls.iter()
                        .map(|url| url.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                })
                .filter(|urls| !urls.is_empty());
            let Some(urls) = urls else {
                return reply(Ok(tool_result(
                    "Pass urls, a list of pull request URLs.",
                    true,
                )));
            };
            reply(Ok(match link(&urls) {
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
            }))
        }
        _ => reply(Err((-32601, "Method not found."))),
    }
}

fn tool_result(text: &str, error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": error})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_introduces_itself_and_lists_its_one_tool() {
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
        assert_eq!(tools["result"]["tools"][0]["name"], TOOL);
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
                |urls| {
                    assert!(!urls.is_empty());
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
            ["--allowedTools", "mcp__xd__link_pull_requests"]
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
            ]
        );
        assert!(!arguments("other").0);
    }
}
