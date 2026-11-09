//! `xd-host mcp`: the link tool's MCP server, which an agent starts over stdio.
//! Each call goes to the running daemon; this process keeps no state.

use std::io::{self, BufRead, Read, Write};

use serde_json::{Value, json};

use crate::{HostOptions, daemon};

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
            Ok(message) => xd_host::answer_mcp(&message, |urls| link(&options, chat, urls)),
            Err(_) => Some(json!({"jsonrpc": "2.0", "id": null,
                "error": {"code": -32700, "message": "Parse error."}})),
        };
        if let Some(reply) = reply {
            writeln!(output, "{reply}").map_err(|error| error.to_string())?;
            output.flush().map_err(|error| error.to_string())?;
        }
    }
}

fn link(options: &HostOptions, chat: &str, urls: &[String]) -> Result<Value, String> {
    let mut stream = daemon::connect(options, false).map_err(|error| error.to_string())?;
    daemon::request(
        &mut stream,
        json!({"op": "link-pull-requests", "chat": chat, "urls": urls, "source": "agent"}),
    )
    .map_err(|error| error.to_string())
}
