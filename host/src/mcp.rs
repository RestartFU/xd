//! `xd-host mcp`: the MCP server of xd's agent tools, which an agent starts over stdio.
//! Each call goes to the running daemon; this process keeps no state.

use std::{
    io::{self, BufRead, Read, Write},
    time::Duration,
};

use serde_json::{Value, json};

use crate::{HostOptions, daemon};

/// How long a tool call waits for the daemon. Starting an agent makes its
/// chat and worktree and starts its first turn, which a busy disk slows; a
/// reply given up on would leave the agent running with no id to mind it by.
/// The agent's own MCP client is told to wait as long; see `agent_tools`.
pub(crate) const CALL_TIMEOUT: Duration = Duration::from_secs(xd_host::AGENT_TOOL_TIMEOUT_SECONDS);

/// A line longer than this is not a tool call xd answers.
const MAX_LINE_BYTES: usize = 1024 * 1024;

pub(super) fn serve(options: HostOptions, chat: &str) -> Result<(), String> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = input
            .by_ref()
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(());
        }
        if line.len() > MAX_LINE_BYTES {
            return Err("an MCP message was too long".into());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let reply = match serde_json::from_slice::<Value>(&line) {
            Ok(message) => xd_host::answer_mcp(&message, |request| ask(&options, chat, request)),
            Err(_) => Some(json!({"jsonrpc": "2.0", "id": null,
                "error": {"code": -32700, "message": "Parse error."}})),
        };
        if let Some(reply) = reply {
            writeln!(output, "{reply}").map_err(|error| error.to_string())?;
            output.flush().map_err(|error| error.to_string())?;
        }
    }
}

/// Sends the daemon `request` for this server's chat, which the agent cannot
/// choose: every request is about the chat the server was started for.
fn ask(options: &HostOptions, chat: &str, mut request: Value) -> Result<Value, String> {
    request["chat"] = json!(chat);
    let mut stream = daemon::connect(options, false, false).map_err(|error| error.to_string())?;
    daemon::request_within(&mut stream, request, CALL_TIMEOUT).map_err(|error| error.to_string())
}
