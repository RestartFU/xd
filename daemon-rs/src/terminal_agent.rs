use std::path::PathBuf;

use crate::agent::{resolve_claude, resolve_codex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalAgent {
    Codex,
    Claude,
}

impl TerminalAgent {
    pub(crate) fn from_wire_name(agent: &str) -> Option<Self> {
        match agent {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            _ => None,
        }
    }

    pub(crate) fn wire_name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }

    pub(crate) fn executable(self) -> PathBuf {
        match self {
            Self::Codex => resolve_codex(),
            Self::Claude => resolve_claude(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(TerminalAgent::from_wire_name("shell"), None);
        assert_eq!(TerminalAgent::from_wire_name("Codex"), None);
    }

    #[test]
    fn direct_terminal_agents_have_stable_wire_names_and_titles() {
        assert_eq!(TerminalAgent::Codex.wire_name(), "codex");
        assert_eq!(TerminalAgent::Codex.title(), "Codex");
        assert_eq!(TerminalAgent::Claude.wire_name(), "claude");
        assert_eq!(TerminalAgent::Claude.title(), "Claude");
    }
}
