//! `xd-host mcp`: the MCP server of xd's agent tools, which an agent starts over stdio.
//! Each call goes to the running daemon; this process keeps no state.

use std::{
    io::{self, BufRead, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex, PoisonError, mpsc},
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

/// The request being answered, which the agent may cancel while it waits.
#[derive(Default)]
struct Call {
    id: Value,
    /// The daemon connection it waits on. Closing it stops what it started
    /// that asked to stop with it, such as a browser command.
    stream: Option<UnixStream>,
    cancelled: bool,
}

#[derive(Default)]
struct Calls {
    current: Option<Call>,
    /// Requests cancelled while still waiting their turn, so they never run.
    cancelled: Vec<Value>,
}

/// How many cancelled requests that have not run yet are remembered.
const MAX_CANCELLED: usize = 64;

type CurrentCall = Arc<Mutex<Calls>>;

fn lock(calls: &CurrentCall) -> std::sync::MutexGuard<'_, Calls> {
    calls.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn serve(options: HostOptions, chat: &str) -> Result<(), String> {
    let mut output = io::stdout().lock();
    let current = CurrentCall::default();
    // Messages are read on their own thread, so the agent leaving or
    // cancelling a call is noticed even while the call waits on the daemon.
    // Unbounded, so the reader never waits behind a call; an agent sends one
    // message per call it waits on.
    let (messages, received) = mpsc::channel::<Vec<u8>>();
    {
        let current = current.clone();
        thread::spawn(move || read_messages(&messages, &current));
    }
    while let Ok(line) = received.recv() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let reply = match serde_json::from_slice::<Value>(&line) {
            Ok(message) => {
                let id = message.get("id").cloned().unwrap_or_default();
                {
                    let mut calls = lock(&current);
                    if let Some(at) = calls.cancelled.iter().position(|early| *early == id) {
                        // Cancelled before its turn: not run, and not answered.
                        calls.cancelled.remove(at);
                        continue;
                    }
                    calls.current = Some(Call {
                        id,
                        ..Call::default()
                    });
                }
                let reply =
                    xd_host::answer_mcp(&message, |request| ask(&options, chat, request, &current));
                // A cancelled request gets no answer.
                let call = lock(&current).current.take();
                reply.filter(|_| !call.is_some_and(|call| call.cancelled))
            }
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

/// Passes the agent's messages on, except a cancellation, which stops the
/// call it names at once. Exits the process once the agent stops sending,
/// which closes the call's daemon connection too.
fn read_messages(messages: &mpsc::Sender<Vec<u8>>, current: &CurrentCall) {
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
                if let Some(id) = cancelled_request(&line) {
                    cancel(current, &id);
                } else if messages.send(line).is_err() {
                    return;
                }
            }
        }
    }
}

/// The request a `notifications/cancelled` message names, if it is one.
fn cancelled_request(line: &[u8]) -> Option<Value> {
    let message = serde_json::from_slice::<Value>(line).ok()?;
    (message.get("method").and_then(Value::as_str) == Some("notifications/cancelled"))
        .then(|| message["params"]["requestId"].clone())
        .filter(|id| !id.is_null())
}

fn cancel(current: &CurrentCall, id: &Value) {
    let mut calls = lock(current);
    match calls.current.as_mut().filter(|call| call.id == *id) {
        Some(call) => {
            call.cancelled = true;
            if let Some(stream) = &call.stream {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        None => {
            if calls.cancelled.len() == MAX_CANCELLED {
                calls.cancelled.remove(0);
            }
            calls.cancelled.push(id.clone());
        }
    }
}

/// Sends the daemon `request` for this server's chat, which the agent cannot
/// choose: every request is about the chat the server was started for.
fn ask(
    options: &HostOptions,
    chat: &str,
    mut request: Value,
    current: &CurrentCall,
) -> Result<Value, String> {
    request["chat"] = json!(chat);
    let mut stream = daemon::connect(options, false, false).map_err(|error| error.to_string())?;
    {
        let mut calls = lock(current);
        if calls.current.as_ref().is_some_and(|call| call.cancelled) {
            return Err("The call was cancelled.".into());
        }
        if let Some(call) = calls.current.as_mut() {
            call.stream = stream.try_clone().ok();
        }
    }
    daemon::request_within(&mut stream, request, CALL_TIMEOUT).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancellation_closes_the_named_calls_daemon_connection() {
        let (stream, mut daemon) = UnixStream::pair().unwrap();
        let current = CurrentCall::default();
        lock(&current).current = Some(Call {
            id: json!(7),
            stream: Some(stream),
            cancelled: false,
        });
        let line = |id: Value| {
            serde_json::to_vec(
                &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                "params": {"requestId": id, "reason": "The user stopped it."}}),
            )
            .unwrap()
        };
        // Another call's cancellation leaves this one be, and is kept for
        // when that call's turn comes.
        cancel(&current, &cancelled_request(&line(json!(8))).unwrap());
        assert!(!lock(&current).current.as_ref().unwrap().cancelled);
        assert_eq!(lock(&current).cancelled, [json!(8)]);
        cancel(&current, &cancelled_request(&line(json!(7))).unwrap());
        assert!(lock(&current).current.as_ref().unwrap().cancelled);
        // The daemon sees the connection end, which stops the command.
        let mut rest = Vec::new();
        assert_eq!(daemon.read_to_end(&mut rest).unwrap(), 0);
        assert_eq!(
            cancelled_request(br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#),
            None
        );
    }
}
