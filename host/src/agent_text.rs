//! Runs one short, unattended assistant turn and collects its reply.
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::mpsc::{self, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use crate::agent::{AgentEvent, AgentParser};

const OUTPUT_LIMIT: usize = 1024 * 1024;

struct AgentOutput {
    text: String,
    completed: bool,
    error: Option<String>,
}

/// Runs `command` to completion and returns the text it replied with.
///
/// `input` is written to stdin as one line, which a backend that takes its turns
/// on stdin needs; without it stdin is closed from the start. `task` names the
/// work in error messages.
pub(crate) fn run(
    mut command: Command,
    backend: &str,
    input: Option<String>,
    timeout: Duration,
    task: &str,
) -> Result<String, String> {
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot start {backend}: {error}"))?;
    // On its own thread so a full pipe cannot stall the deadline below. Closing
    // stdin when the write ends is what tells a kept process to finish.
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        thread::spawn(move || {
            let _ = writeln!(stdin, "{input}");
        });
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("Cannot read {backend} output."))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("Cannot read {backend} errors."))?;
    let (output_sender, output_receiver) = mpsc::sync_channel(1);
    let parser_backend = backend.to_owned();
    thread::spawn(move || {
        let result = read_output(stdout, &parser_backend);
        let _ = output_sender.send(result);
    });
    let (error_sender, error_receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = error_sender.send(read_bounded(stderr));
    });

    let deadline = Instant::now() + timeout;
    let mut output = None;
    let status = loop {
        match output_receiver.try_recv() {
            Ok(result) => {
                if let Err(error) = &result {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.clone());
                }
                output = Some(result);
            }
            Err(TryRecvError::Disconnected) if output.is_none() => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Cannot read {backend} output."));
            }
            Err(_) => {}
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("Cannot wait for {backend}: {error}"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{task} timed out."));
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = match output {
        Some(result) => result?,
        None => output_receiver
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| format!("Cannot finish reading {backend} output."))??,
    };
    let stderr = error_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    if !status.success() || !output.completed {
        return Err(output
            .error
            .or_else(|| stderr.lines().next_back().map(str::to_owned))
            .unwrap_or_else(|| format!("{backend} did not complete {task}.")));
    }
    Ok(output.text)
}

fn read_output(stdout: impl Read, backend: &str) -> Result<AgentOutput, String> {
    let mut parser = AgentParser::new(backend)?;
    let mut output = String::new();
    let mut completed = false;
    let mut latest_error = None;
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| format!("Cannot read agent output: {error}"))?;
        for event in parser.feed(&line) {
            match event {
                AgentEvent::Text(text) | AgentEvent::TextDelta(text) => {
                    if output.len().saturating_add(text.len()) > OUTPUT_LIMIT {
                        return Err("The assistant's reply is too large.".into());
                    }
                    output.push_str(&text);
                }
                AgentEvent::Completed => completed = true,
                AgentEvent::Error(error) => latest_error = Some(error),
                _ => {}
            }
        }
    }
    Ok(AgentOutput {
        text: output,
        completed,
        error: latest_error,
    })
}

fn read_bounded(mut reader: impl Read) -> String {
    let mut retained = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];
    while let Ok(count) = reader.read(&mut chunk) {
        if count == 0 {
            break;
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(retained.len());
        retained.extend_from_slice(&chunk[..count.min(remaining)]);
    }
    String::from_utf8_lossy(&retained).into_owned()
}
