//! `xd-host mcp`: the MCP server of xd's agent tools, which an agent starts over stdio.
//! Each call goes to the running daemon; this process keeps no state.

use std::{
    io::{self, BufRead, Read, Write},
    sync::mpsc,
    thread,
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
    let mut output = io::stdout().lock();
    // Messages are read on their own thread, so the agent leaving is noticed
    // even while a call waits on the daemon: the server then exits, closing
    // its daemon connection, which stops what the call started.
    // Unbounded, so the reader never waits behind a call and always sees
    // the end; an agent sends one message per call it waits on.
    let (messages, received) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            let mut line = Vec::new();
            let read = input
                .by_ref()
                .take(MAX_LINE_BYTES as u64 + 1)
                .read_until(b'\n', &mut line);
            match read {
                Ok(0) | Err(_) => std::process::exit(0),
                Ok(_) if line.len() > MAX_LINE_BYTES => {
                    eprintln!("xd-host: an MCP message was too long");
                    std::process::exit(1);
                }
                Ok(_) => {
                    if messages.send(line).is_err() {
                        return;
                    }
                }
            }
        }
    });
    while let Ok(line) = received.recv() {
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
    Ok(())
}

/// Sends the daemon `request` for this server's chat, which the agent cannot
/// choose: every request is about the chat the server was started for.
fn ask(options: &HostOptions, chat: &str, mut request: Value) -> Result<Value, String> {
    request["chat"] = json!(chat);
    let mut stream = daemon::connect(options, false, false).map_err(|error| error.to_string())?;
    daemon::request_within(&mut stream, request, CALL_TIMEOUT).map_err(|error| error.to_string())
}
