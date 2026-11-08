//! Runs one short, unattended assistant turn and collects its reply.
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use crate::agent::{AgentCommand, AgentEvent, AgentParser};

const OUTPUT_LIMIT: usize = 1024 * 1024;

/// The lowest effort every assistant accepts. Titles, names and Git messages
/// need no reasoning, and thinking tokens are most of what a higher one costs.
pub(crate) const LOW_EFFORT: &str = "low";

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
                    stop(&mut child);
                    return Err(error.clone());
                }
                output = Some(result);
            }
            Err(TryRecvError::Disconnected) if output.is_none() => {
                stop(&mut child);
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
            stop(&mut child);
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

/// Codex built-ins that add tools a short run must not use.
const CODEX_TOOL_FEATURES: &[&str] = &[
    "shell_tool",
    "unified_exec",
    "apps",
    "plugins",
    "multi_agent",
    "view_image",
    "image_generation",
    "browser_use",
    "computer_use",
    "skill_search",
    "tool_suggest",
    "goals",
    "sleep_tool",
];

/// The line to write on stdin, for a backend that takes its turns there. Claude
/// reads its prompt only from stdin, so without this it exits having done nothing.
pub(crate) fn turn_input(backend: &str, prompt: &str) -> Option<String> {
    AgentCommand::keeps_its_process(backend).then(|| AgentCommand::encode_turn(prompt))
}

/// Drops what a short, unattended run does not need, for cost and speed: tools,
/// MCP servers and their schemas, slash commands, and a session left behind in
/// the assistant's own history.
///
/// Without `project_instructions` the run also skips CLAUDE.md, AGENTS.md, the
/// user's hooks, and Claude's settings, which carry them. With it, Claude still loads
/// the repository's own settings and instructions, and Codex its AGENTS.md, so
/// conventions written there still apply.
pub(crate) fn keep_the_run_minimal(
    command: &mut Command,
    backend: &str,
    project_instructions: bool,
) {
    match backend {
        "claude" => {
            command.args([
                "--tools",
                "",
                "--no-session-persistence",
                "--strict-mcp-config",
                "--disable-slash-commands",
                "--setting-sources",
                if project_instructions {
                    "project,local"
                } else {
                    ""
                },
            ]);
        }
        "codex" => {
            command.args(["--ephemeral", "-c", "mcp_servers={}"]);
            // Codex has no switch for all tools, so each built-in goes off by
            // name; a CLI that lacks one ignores it. The schemas are most of
            // what a one-line reply would otherwise send.
            for feature in CODEX_TOOL_FEATURES {
                command.args(["-c", &format!("features.{feature}=false")]);
            }
            if !project_instructions {
                command.args([
                    "-c",
                    "project_doc_max_bytes=0",
                    "-c",
                    "features.hooks=false",
                ]);
            }
        }
        _ => {}
    }
}

/// Ends a run that is still going, with anything it started: a hook left
/// holding the output pipes would otherwise outlive it, and so would the
/// threads reading them.
fn stop(child: &mut Child) {
    // SAFETY: AgentCommand::build makes the child lead a process group of its
    // own, and it has not been reaped, so this signal can only reach that group.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(backend: &str, project_instructions: bool) -> Vec<String> {
        let mut command = Command::new("assistant");
        keep_the_run_minimal(&mut command, backend, project_instructions);
        command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn keeps_short_runs_out_of_history_and_free_of_tools() {
        let claude = args("claude", false);
        assert!(claude.contains(&"--no-session-persistence".to_owned()));
        assert!(claude.contains(&"--strict-mcp-config".to_owned()));
        assert!(claude.windows(2).any(|pair| pair == ["--tools", ""]));
        assert!(
            claude
                .windows(2)
                .any(|pair| pair == ["--setting-sources", ""])
        );
        let codex = args("codex", false);
        assert!(codex.contains(&"--ephemeral".to_owned()));
        assert!(codex.contains(&"mcp_servers={}".to_owned()));
        assert!(codex.contains(&"project_doc_max_bytes=0".to_owned()));
        assert!(codex.contains(&"features.shell_tool=false".to_owned()));
        assert!(codex.contains(&"features.unified_exec=false".to_owned()));
        assert!(codex.contains(&"features.hooks=false".to_owned()));
        assert!(args("jcode", false).is_empty());
    }

    #[test]
    fn keeps_the_repository_instructions_when_asked() {
        let claude = args("claude", true);
        assert!(
            claude
                .windows(2)
                .any(|pair| pair == ["--setting-sources", "project,local"])
        );
        assert!(claude.windows(2).any(|pair| pair == ["--tools", ""]));
        let codex = args("codex", true);
        assert!(codex.contains(&"--ephemeral".to_owned()));
        assert!(!codex.contains(&"project_doc_max_bytes=0".to_owned()));
        assert!(codex.contains(&"features.shell_tool=false".to_owned()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_timed_out_run_takes_what_it_started_with_it() {
        use std::os::unix::process::CommandExt;
        let directory = std::env::temp_dir().join(format!("xd-agent-text-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let pid_file = directory.join("helper.pid");
        // A helper that inherits stdout, as a hook started by the CLI would.
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!(
                "sleep 60 & echo $! > {}; sleep 60",
                pid_file.display()
            ))
            .process_group(0);
        let error = run(command, "codex", None, Duration::from_millis(500), "naming").unwrap_err();
        assert_eq!(error, "naming timed out.");
        let helper = std::fs::read_to_string(&pid_file).unwrap();
        let stat = format!("/proc/{}/stat", helper.trim());
        let deadline = Instant::now() + Duration::from_secs(5);
        // Gone, or a zombie waiting for whoever adopted it to reap it.
        while std::fs::read_to_string(&stat)
            .is_ok_and(|stat| !stat.rsplit(") ").next().unwrap_or("").starts_with('Z'))
        {
            assert!(Instant::now() < deadline, "the helper outlived its run");
            thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn writes_the_prompt_on_stdin_only_for_claude() {
        let line = turn_input("claude", "name it").unwrap();
        assert!(line.contains("name it"));
        assert!(turn_input("codex", "name it").is_none());
        assert!(turn_input("jcode", "name it").is_none());
    }
}
