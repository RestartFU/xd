use std::path::{Path, PathBuf};

use crate::agent::{resolve_claude, resolve_codex, resolve_copilot, resolve_jcode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalAgent {
    Codex,
    Claude,
    Jcode,
    Copilot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentSession<'a> {
    New(&'a str),
    Resume(&'a str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRecorder {
    executable: PathBuf,
    database: PathBuf,
    chat: String,
    backend: String,
}

impl SessionRecorder {
    pub(crate) fn new(executable: &Path, database: &Path, chat: &str, backend: &str) -> Self {
        Self {
            executable: executable.to_owned(),
            database: database.to_owned(),
            chat: chat.to_owned(),
            backend: backend.to_owned(),
        }
    }

    fn codex_notify_override(&self) -> Result<String, String> {
        let executable = self
            .executable
            .to_str()
            .ok_or("The host executable path cannot be passed to Codex.")?;
        let database = self
            .database
            .to_str()
            .ok_or("The chat database path cannot be passed to Codex.")?;
        let command = [
            executable,
            "record-agent-session",
            "--database",
            database,
            "--chat",
            &self.chat,
            "--backend",
            &self.backend,
        ];
        serde_json::to_string(&command)
            .map(|command| format!("notify={command}"))
            .map_err(|error| format!("Cannot configure Codex session recording: {error}"))
    }
}

impl TerminalAgent {
    pub(crate) fn from_wire_name(agent: &str) -> Option<Self> {
        match agent {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            "jcode" => Some(Self::Jcode),
            "copilot" => Some(Self::Copilot),
            _ => None,
        }
    }

    pub(crate) fn wire_name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Jcode => "jcode",
            Self::Copilot => "copilot",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Jcode => "JCode",
            Self::Copilot => "Copilot",
        }
    }

    pub(crate) fn executable(self) -> PathBuf {
        match self {
            Self::Codex => resolve_codex(),
            Self::Claude => resolve_claude(),
            Self::Jcode => resolve_jcode(),
            Self::Copilot => resolve_copilot(),
        }
    }

    pub(crate) fn arguments(
        self,
        allow_all_permissions: bool,
        session: Option<AgentSession<'_>>,
        recorder: Option<&SessionRecorder>,
    ) -> Result<Vec<String>, String> {
        let mut arguments = Vec::new();
        match self {
            Self::Codex => {
                if let Some(AgentSession::Resume(session_id)) = session {
                    arguments.extend(["resume".into(), session_id.into()]);
                }
                arguments.extend([
                    "--no-alt-screen".into(),
                    "-c".into(),
                    "tui.terminal_title=[\"run-state\"]".into(),
                    "-c".into(),
                    "tui.terminal_resize_reflow_max_rows=5000".into(),
                ]);
                if let Some(recorder) = recorder {
                    // Codex chooses interactive thread ids itself. Its
                    // completion notifier supplies that id to the host
                    // helper after the first completed turn.
                    arguments.extend(["-c".into(), recorder.codex_notify_override()?]);
                }
                if allow_all_permissions {
                    arguments.push("--dangerously-bypass-approvals-and-sandbox".into());
                }
            }
            Self::Claude => {
                if let Some(session) = session {
                    let (flag, session_id) = match session {
                        AgentSession::New(session_id) => ("--session-id", session_id),
                        AgentSession::Resume(session_id) => ("--resume", session_id),
                    };
                    arguments.extend([flag.into(), session_id.into()]);
                }
                if allow_all_permissions {
                    arguments.push("--dangerously-skip-permissions".into());
                }
            }
            Self::Jcode => {
                if let Some(AgentSession::Resume(session_id)) = session {
                    arguments.extend(["--resume".into(), session_id.into()]);
                }
                arguments.push("--no-update".into());
            }
            Self::Copilot => {
                arguments.extend([
                    "--no-auto-update".into(),
                    "--no-banner".into(),
                    "--no-mouse".into(),
                ]);
                if let Some(session) = session {
                    let session_id = match session {
                        AgentSession::New(session_id) | AgentSession::Resume(session_id) => {
                            session_id
                        }
                    };
                    arguments.extend(["--session-id".into(), session_id.into()]);
                }
                if allow_all_permissions {
                    arguments.push("--allow-all".into());
                }
            }
        }
        Ok(arguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn direct_terminal_agent_names_are_strict() {
        assert_eq!(
            TerminalAgent::from_wire_name("codex"),
            Some(TerminalAgent::Codex)
        );
        assert_eq!(
            TerminalAgent::from_wire_name("claude"),
            Some(TerminalAgent::Claude)
        );
        assert_eq!(
            TerminalAgent::from_wire_name("jcode"),
            Some(TerminalAgent::Jcode)
        );
        assert_eq!(
            TerminalAgent::from_wire_name("copilot"),
            Some(TerminalAgent::Copilot)
        );
        assert_eq!(TerminalAgent::from_wire_name("shell"), None);
        assert_eq!(TerminalAgent::from_wire_name("Codex"), None);
    }

    #[test]
    fn direct_terminal_agents_have_stable_wire_names_and_titles() {
        assert_eq!(TerminalAgent::Codex.wire_name(), "codex");
        assert_eq!(TerminalAgent::Codex.title(), "Codex");
        assert_eq!(TerminalAgent::Claude.wire_name(), "claude");
        assert_eq!(TerminalAgent::Claude.title(), "Claude");
        assert_eq!(TerminalAgent::Jcode.wire_name(), "jcode");
        assert_eq!(TerminalAgent::Jcode.title(), "JCode");
        assert_eq!(TerminalAgent::Copilot.wire_name(), "copilot");
        assert_eq!(TerminalAgent::Copilot.title(), "Copilot");
    }

    #[test]
    fn direct_agent_arguments_start_and_resume_backend_sessions() {
        let recorder = SessionRecorder::new(
            Path::new("/opt/xd/xd-host"),
            Path::new("/state/xd/chats.db"),
            "chat-1",
            "codex",
        );

        let codex_new = TerminalAgent::Codex
            .arguments(false, None, Some(&recorder))
            .unwrap();
        assert_eq!(
            &codex_new[..5],
            [
                "--no-alt-screen",
                "-c",
                "tui.terminal_title=[\"run-state\"]",
                "-c",
                "tui.terminal_resize_reflow_max_rows=5000",
            ]
        );
        assert!(codex_new.iter().any(|argument| {
            argument.starts_with("notify=[\"/opt/xd/xd-host\",\"record-agent-session\"")
                && argument.contains("\"/state/xd/chats.db\"")
                && argument.contains("\"chat-1\"")
                && argument.contains("\"codex\"")
        }));

        let codex_resumed = TerminalAgent::Codex
            .arguments(
                true,
                Some(AgentSession::Resume("thread-1")),
                Some(&recorder),
            )
            .unwrap();
        assert_eq!(&codex_resumed[..2], ["resume", "thread-1"]);
        assert!(
            codex_resumed
                .iter()
                .any(|argument| argument == "--dangerously-bypass-approvals-and-sandbox")
        );

        assert_eq!(
            TerminalAgent::Claude
                .arguments(false, Some(AgentSession::Resume("claude-1")), None)
                .unwrap(),
            ["--resume", "claude-1"]
        );
        assert_eq!(
            TerminalAgent::Claude
                .arguments(false, Some(AgentSession::New("chat-session-1")), None,)
                .unwrap(),
            ["--session-id", "chat-session-1"]
        );
        assert_eq!(
            TerminalAgent::Jcode
                .arguments(false, Some(AgentSession::Resume("jcode-1")), None)
                .unwrap(),
            ["--resume", "jcode-1", "--no-update"]
        );
        assert_eq!(
            TerminalAgent::Copilot.arguments(false, None, None).unwrap(),
            ["--no-auto-update", "--no-banner", "--no-mouse"]
        );
        assert_eq!(
            TerminalAgent::Copilot
                .arguments(true, Some(AgentSession::Resume("copilot-session-1")), None,)
                .unwrap(),
            [
                "--no-auto-update",
                "--no-banner",
                "--no-mouse",
                "--session-id",
                "copilot-session-1",
                "--allow-all"
            ]
        );
        assert_eq!(
            TerminalAgent::Copilot
                .arguments(false, Some(AgentSession::New("copilot-session-2")), None)
                .unwrap(),
            [
                "--no-auto-update",
                "--no-banner",
                "--no-mouse",
                "--session-id",
                "copilot-session-2"
            ]
        );
    }
}
