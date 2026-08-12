use xd_desktop::model::{ChatSummary, Folder};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentCli {
    Codex,
    Claude,
}

impl AgentCli {
    pub(crate) fn from_backend(backend: &str) -> Option<Self> {
        if backend.eq_ignore_ascii_case("codex") {
            Some(Self::Codex)
        } else if backend.eq_ignore_ascii_case("claude") {
            Some(Self::Claude)
        } else {
            None
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }

    pub(crate) fn protocol_name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub(crate) fn from_protocol_name(agent: &str) -> Option<Self> {
        match agent {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MinimalRoute {
    Projects {
        project_id: Option<String>,
    },
    Cli {
        project_id: String,
        chat_id: String,
        agent: AgentCli,
    },
}

impl Default for MinimalRoute {
    fn default() -> Self {
        Self::Projects { project_id: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectCard {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) sessions: usize,
    pub(crate) working: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionCard {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) agent: AgentCli,
    pub(crate) branch: String,
    pub(crate) working: bool,
}

pub(crate) fn project_cards(folders: &[Folder], chats: &[ChatSummary]) -> Vec<ProjectCard> {
    folders
        .iter()
        .map(|folder| {
            let project_chats = chats.iter().filter(|chat| {
                chat.folder == folder.id && AgentCli::from_backend(&chat.backend).is_some()
            });
            ProjectCard {
                id: folder.id.clone(),
                name: folder.name.clone(),
                sessions: project_chats.clone().count(),
                working: project_chats
                    .filter(|chat| chat.working || chat.terminal_working)
                    .count(),
            }
        })
        .collect()
}

pub(crate) fn project_sessions(project_id: &str, chats: &[ChatSummary]) -> Vec<SessionCard> {
    chats
        .iter()
        .filter(|chat| chat.folder == project_id)
        .filter_map(|chat| {
            Some(SessionCard {
                id: chat.id.clone(),
                title: chat
                    .title
                    .as_deref()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or("New Session")
                    .to_owned(),
                agent: AgentCli::from_backend(&chat.backend)?,
                branch: chat
                    .branch
                    .clone()
                    .unwrap_or_else(|| "Project directory".into()),
                working: chat.working || chat.terminal_working,
            })
        })
        .collect()
}

pub(crate) fn resumable_session(
    last_chat: Option<&str>,
    chats: &[ChatSummary],
) -> Option<(String, String, AgentCli)> {
    let supported = |chat: &&ChatSummary| AgentCli::from_backend(&chat.backend).is_some();
    let chat = last_chat
        .and_then(|chat_id| {
            chats
                .iter()
                .find(|chat| chat.id == chat_id && supported(chat))
        })
        .or_else(|| chats.iter().find(supported))?;
    let agent = AgentCli::from_backend(&chat.backend)?;
    Some((chat.folder.clone(), chat.id.clone(), agent))
}

pub(crate) fn reconcile_route(
    route: &MinimalRoute,
    folders: &[Folder],
    chats: &[ChatSummary],
) -> MinimalRoute {
    match route {
        MinimalRoute::Projects { project_id } => MinimalRoute::Projects {
            project_id: project_id
                .as_ref()
                .filter(|project_id| folders.iter().any(|folder| &folder.id == *project_id))
                .cloned(),
        },
        MinimalRoute::Cli {
            project_id,
            chat_id,
            ..
        } => {
            if !folders.iter().any(|folder| folder.id == *project_id) {
                return MinimalRoute::default();
            }
            let Some(chat) = chats
                .iter()
                .find(|chat| chat.id == *chat_id && chat.folder == *project_id)
            else {
                return MinimalRoute::Projects {
                    project_id: Some(project_id.clone()),
                };
            };
            let Some(agent) = AgentCli::from_backend(&chat.backend) else {
                return MinimalRoute::Projects {
                    project_id: Some(project_id.clone()),
                };
            };
            MinimalRoute::Cli {
                project_id: project_id.clone(),
                chat_id: chat_id.clone(),
                agent,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use xd_desktop::model::{ChatSummary, Folder};

    use super::*;

    fn folder(id: &str, name: &str) -> Folder {
        Folder {
            id: id.into(),
            name: name.into(),
            parent: None,
        }
    }

    fn session(id: &str, folder: &str, title: &str, backend: &str, working: bool) -> ChatSummary {
        ChatSummary {
            id: id.into(),
            folder: folder.into(),
            title: Some(title.into()),
            backend: backend.into(),
            branch: Some(format!("session/{id}")),
            working,
            terminal_working: false,
        }
    }

    #[test]
    fn projects_summarize_their_cli_sessions_in_workspace_order() {
        let folders = vec![folder("one", "Compiler"), folder("two", "Website")];
        let chats = vec![
            session("a", "one", "Fix parser", "codex", true),
            session("b", "one", "Review lexer", "claude", false),
            session("c", "two", "Polish home", "codex", false),
        ];

        assert_eq!(
            project_cards(&folders, &chats),
            vec![
                ProjectCard {
                    id: "one".into(),
                    name: "Compiler".into(),
                    sessions: 2,
                    working: 1,
                },
                ProjectCard {
                    id: "two".into(),
                    name: "Website".into(),
                    sessions: 1,
                    working: 0,
                },
            ]
        );
    }

    #[test]
    fn project_sessions_keep_the_real_cli_provider() {
        let chats = vec![
            session("a", "one", "Fix parser", "codex", true),
            session("b", "one", "Review lexer", "claude", false),
            session("c", "two", "Polish home", "codex", false),
        ];

        assert_eq!(
            project_sessions("one", &chats),
            vec![
                SessionCard {
                    id: "a".into(),
                    title: "Fix parser".into(),
                    agent: AgentCli::Codex,
                    branch: "session/a".into(),
                    working: true,
                },
                SessionCard {
                    id: "b".into(),
                    title: "Review lexer".into(),
                    agent: AgentCli::Claude,
                    branch: "session/b".into(),
                    working: false,
                },
            ]
        );
    }

    #[test]
    fn direct_cli_activity_marks_sessions_and_projects_working() {
        let folders = vec![folder("one", "Compiler")];
        let chats: Vec<ChatSummary> = serde_json::from_value(serde_json::json!([
            {
                "id": "a",
                "folder": "one",
                "title": "Fix parser",
                "backend": "codex",
                "working": false,
                "terminal_working": true
            }
        ]))
        .unwrap();

        assert!(project_sessions("one", &chats)[0].working);
        assert_eq!(project_cards(&folders, &chats)[0].working, 1);
    }

    #[test]
    fn sessions_tab_resumes_the_last_supported_cli_session() {
        let chats = vec![
            session("a", "one", "First", "codex", false),
            session("b", "two", "Last", "claude", true),
            session("c", "two", "Unsupported", "shell", false),
        ];

        assert_eq!(
            resumable_session(Some("b"), &chats),
            Some(("two".into(), "b".into(), AgentCli::Claude))
        );
        assert_eq!(
            resumable_session(Some("c"), &chats),
            Some(("one".into(), "a".into(), AgentCli::Codex))
        );
        assert_eq!(resumable_session(None, &chats).unwrap().1, "a");
    }

    #[test]
    fn unsupported_backends_never_become_direct_cli_sessions() {
        let folders = vec![folder("one", "Compiler")];
        let chats = vec![
            session("a", "one", "Supported", "codex", false),
            session("b", "one", "Unsupported", "shell", true),
        ];

        assert_eq!(project_sessions("one", &chats).len(), 1);
        assert_eq!(project_sessions("one", &chats)[0].id, "a");
        assert_eq!(project_cards(&folders, &chats)[0].sessions, 1);
        assert_eq!(project_cards(&folders, &chats)[0].working, 0);
    }

    #[test]
    fn route_reconciliation_follows_the_authoritative_tree() {
        let folders = vec![folder("one", "Compiler")];
        let mut chats = vec![session("a", "one", "Fix parser", "codex", false)];
        let route = MinimalRoute::Cli {
            project_id: "one".into(),
            chat_id: "a".into(),
            agent: AgentCli::Codex,
        };

        chats[0].backend = "claude".into();
        assert_eq!(
            reconcile_route(&route, &folders, &chats),
            MinimalRoute::Cli {
                project_id: "one".into(),
                chat_id: "a".into(),
                agent: AgentCli::Claude,
            }
        );

        chats.clear();
        assert_eq!(
            reconcile_route(&route, &folders, &chats),
            MinimalRoute::Projects {
                project_id: Some("one".into()),
            }
        );

        assert_eq!(
            reconcile_route(&route, &[], &chats),
            MinimalRoute::default()
        );
    }
}
