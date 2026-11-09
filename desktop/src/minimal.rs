use xd_desktop::model::{ChatSummary, Folder};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentCli {
    Codex,
    Claude,
    /// An assistant this build does not run, such as the retired JCode. Its
    /// chats still list and open with their history, but the host refuses to
    /// send to them, and nothing new starts on it.
    Other,
}

impl AgentCli {
    pub(crate) fn from_backend(backend: &str) -> Option<Self> {
        if backend.eq_ignore_ascii_case("codex") {
            Some(Self::Codex)
        } else if backend.eq_ignore_ascii_case("claude") {
            Some(Self::Claude)
        } else if backend.is_empty() {
            None
        } else {
            Some(Self::Other)
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Other => "Assistant",
        }
    }

    pub(crate) fn protocol_name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            // Never sent: no chat or terminal starts on it.
            Self::Other => "",
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
    Sessions {
        project_id: Option<String>,
    },
    Terminal,
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
    /// The account whose avatar stands for this project, when the host named
    /// one for its folder.
    pub(crate) owner: Option<String>,
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
    /// Background tasks it is waiting on while no turn runs.
    pub(crate) background_tasks: u32,
}

/// A session on the Settled shelf, which lists every project's together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettledCard {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) agent: AgentCli,
    pub(crate) project_id: String,
    pub(crate) project: String,
    pub(crate) branch: String,
    pub(crate) settled_at: i64,
}

/// The shelf's rows, most recently settled first.
pub(crate) fn settled_cards(folders: &[Folder], chats: &[ChatSummary]) -> Vec<SettledCard> {
    let mut cards = chats
        .iter()
        .filter_map(|chat| {
            let settled_at = chat.settled_at?;
            let folder = folders.iter().find(|folder| folder.id == chat.folder)?;
            let session = project_sessions(&chat.folder, std::slice::from_ref(chat)).pop()?;
            Some(SettledCard {
                id: session.id,
                title: session.title,
                agent: session.agent,
                project_id: folder.id.clone(),
                project: folder.name.clone(),
                branch: session.branch,
                settled_at,
            })
        })
        .collect::<Vec<_>>();
    cards.sort_by(|left, right| {
        right
            .settled_at
            .cmp(&left.settled_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    cards
}

/// Where to go after settling the open session: the next active row down the
/// sidebar, wrapping to the top. `chats` holds the active rows and `current`
/// the chats of the session being settled.
pub(crate) fn next_active_session(
    folders: &[Folder],
    chats: &[ChatSummary],
    current: &[String],
) -> Option<(String, String, AgentCli)> {
    let rows = folders
        .iter()
        .flat_map(|folder| {
            project_sessions(&folder.id, chats)
                .into_iter()
                .map(|session| (folder.id.clone(), session.id, session.agent))
        })
        .collect::<Vec<_>>();
    let index = rows
        .iter()
        .position(|(_, chat_id, _)| current.contains(chat_id))?;
    rows[index + 1..]
        .iter()
        .chain(&rows[..index])
        .find(|(_, chat_id, _)| !current.contains(chat_id))
        .cloned()
}

/// How long ago something happened, as short as a sidebar row needs.
pub(crate) fn age_label(at: i64, now: i64) -> String {
    let seconds = now.saturating_sub(at).max(0);
    match seconds {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        86_400..604_800 => format!("{}d", seconds / 86_400),
        _ => format!("{}w", seconds / 604_800),
    }
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
                owner: folder.owner.clone(),
                sessions: project_chats.clone().count(),
                working: project_chats
                    .filter(|chat| chat.working || chat.terminal_working)
                    .count(),
            }
        })
        .collect()
}

/// One row of the sidebar's project tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TreeRow {
    /// The folder's place in the host's list.
    pub(crate) index: usize,
    /// How many folders it sits inside.
    pub(crate) depth: usize,
}

/// The folders in sidebar order: each one followed by what it holds, siblings
/// in the host's order. A folder whose parent is not listed sits at the top.
pub(crate) fn folder_tree(folders: &[Folder]) -> Vec<TreeRow> {
    let listed = |id: &str| folders.iter().any(|folder| folder.id == id);
    let mut children = std::collections::HashMap::<&str, Vec<usize>>::new();
    let mut roots = Vec::new();
    for (index, folder) in folders.iter().enumerate() {
        match folder.parent.as_deref() {
            Some(parent) if parent != folder.id && listed(parent) => {
                children.entry(parent).or_default().push(index)
            }
            _ => roots.push(index),
        }
    }
    let mut rows = Vec::with_capacity(folders.len());
    let mut placed = vec![false; folders.len()];
    let visit = |start: usize, rows: &mut Vec<TreeRow>, placed: &mut [bool]| {
        let mut stack = vec![(start, 0)];
        while let Some((index, depth)) = stack.pop() {
            if std::mem::replace(&mut placed[index], true) {
                continue;
            }
            rows.push(TreeRow { index, depth });
            if let Some(held) = children.get(folders[index].id.as_str()) {
                stack.extend(held.iter().rev().map(|&child| (child, depth + 1)));
            }
        }
    };
    for root in roots {
        visit(root, &mut rows, &mut placed);
    }
    // A parent loop is not reachable from the top; list it rather than lose it.
    for index in 0..folders.len() {
        visit(index, &mut rows, &mut placed);
    }
    rows
}

/// The rows `tree[position]` holds at any depth, in sidebar order: the run of
/// deeper rows that follows it in [`folder_tree`]'s order.
pub(crate) fn tree_descendants(
    tree: &[TreeRow],
    position: usize,
) -> impl Iterator<Item = &TreeRow> {
    let depth = tree[position].depth;
    tree[position + 1..]
        .iter()
        .take_while(move |row| row.depth > depth)
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
                background_tasks: chat.waiting_on(),
            })
        })
        .collect()
}

pub(crate) fn resumable_session(
    last_chat: Option<&str>,
    chats: &[ChatSummary],
) -> Option<(String, String, AgentCli)> {
    // Only a chat the user can carry on with: one on an assistant this build
    // runs.
    let supported = |chat: &&ChatSummary| {
        chat.settled_at.is_none()
            && AgentCli::from_backend(&chat.backend).is_some_and(|agent| agent != AgentCli::Other)
    };
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
        MinimalRoute::Sessions { project_id } => MinimalRoute::Sessions {
            project_id: project_id
                .as_ref()
                .filter(|project_id| folders.iter().any(|folder| &folder.id == *project_id))
                .cloned(),
        },
        MinimalRoute::Terminal => MinimalRoute::Terminal,
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
                if let Some(chat) = chats.iter().find(|chat| {
                    chat.folder == *project_id && AgentCli::from_backend(&chat.backend).is_some()
                }) {
                    return MinimalRoute::Cli {
                        project_id: project_id.clone(),
                        chat_id: chat.id.clone(),
                        agent: AgentCli::from_backend(&chat.backend)
                            .expect("a supported session has a CLI"),
                    };
                }
                return MinimalRoute::Sessions {
                    project_id: Some(project_id.clone()),
                };
            };
            let Some(agent) = AgentCli::from_backend(&chat.backend) else {
                return MinimalRoute::Sessions {
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
            owner: None,
            group: false,
        }
    }

    fn named_folder(id: &str) -> Folder {
        Folder {
            id: id.into(),
            name: format!("{id}-name"),
            parent: None,
            owner: None,
            group: false,
        }
    }

    fn child(id: &str, parent: &str) -> Folder {
        Folder {
            parent: Some(parent.into()),
            ..folder(id, id)
        }
    }

    #[test]
    fn the_folder_tree_lists_what_each_folder_holds_beneath_it() {
        let folders = vec![
            folder("acme", "acme"),
            folder("solo", "solo"),
            child("web", "acme"),
            child("clients", "acme"),
            child("portal", "clients"),
            child("lost", "gone"),
        ];

        let rows = folder_tree(&folders)
            .into_iter()
            .map(|row| (folders[row.index].id.as_str(), row.depth))
            .collect::<Vec<_>>();

        assert_eq!(
            rows,
            vec![
                ("acme", 0),
                ("web", 1),
                ("clients", 1),
                ("portal", 2),
                ("solo", 0),
                ("lost", 0),
            ]
        );
    }

    #[test]
    fn the_folder_tree_keeps_folders_caught_in_a_parent_loop() {
        let folders = vec![child("a", "b"), child("b", "a"), child("self", "self")];

        let rows = folder_tree(&folders);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], TreeRow { index: 2, depth: 0 });
    }

    #[test]
    fn a_folder_holds_everything_nested_beneath_it_and_nothing_after() {
        let folders = vec![
            folder("acme", "acme"),
            folder("solo", "solo"),
            child("web", "acme"),
            child("clients", "acme"),
            child("portal", "clients"),
        ];
        let tree = folder_tree(&folders);
        let held = |id: &str| {
            let position = tree
                .iter()
                .position(|row| folders[row.index].id == id)
                .expect("listed folder");
            tree_descendants(&tree, position)
                .map(|row| folders[row.index].id.as_str())
                .collect::<Vec<_>>()
        };

        assert_eq!(held("acme"), ["web", "clients", "portal"]);
        assert_eq!(held("clients"), ["portal"]);
        assert!(held("web").is_empty());
        assert!(held("solo").is_empty());
    }

    #[test]
    fn the_settled_shelf_lists_the_most_recently_settled_first() {
        let mut older = session("older", "one", "Older", "codex", false);
        older.settled_at = Some(100);
        let mut newer = session("newer", "two", "Newer", "claude", false);
        newer.settled_at = Some(200);
        let mut tied = session("a-tied", "one", "Tied", "codex", false);
        tied.settled_at = Some(100);
        let active = session("active", "one", "Active", "codex", false);
        let orphan = {
            let mut orphan = session("orphan", "gone", "Orphan", "codex", false);
            orphan.settled_at = Some(300);
            orphan
        };
        let cards = settled_cards(
            &[named_folder("one"), named_folder("two")],
            &[older, active, newer, tied, orphan],
        );
        assert_eq!(
            cards
                .iter()
                .map(|card| card.id.as_str())
                .collect::<Vec<_>>(),
            ["newer", "a-tied", "older"]
        );
        assert_eq!(cards[0].project, "two-name");
        assert_eq!(cards[0].project_id, "two");
        assert_eq!(cards[0].settled_at, 200);
    }

    #[test]
    fn settling_the_open_session_moves_down_the_sidebar_and_wraps() {
        let folders = [named_folder("one"), named_folder("two")];
        let chats = [
            session("a", "one", "A", "codex", false),
            session("b", "one", "B", "codex", false),
            session("c", "two", "C", "claude", false),
        ];
        let next = |current: &str| {
            next_active_session(&folders, &chats, &[current.to_owned()])
                .map(|(project, chat, _)| (project, chat))
        };
        assert_eq!(next("a"), Some(("one".into(), "b".into())));
        assert_eq!(next("b"), Some(("two".into(), "c".into())));
        assert_eq!(next("c"), Some(("one".into(), "a".into())));
        assert_eq!(next("missing"), None);
        assert_eq!(
            next_active_session(&folders, &chats[..1], &["a".to_owned()]),
            None,
            "nothing else is active"
        );
    }

    #[test]
    fn a_settled_session_is_not_offered_to_continue() {
        let mut settled = session("settled", "one", "Done", "codex", false);
        settled.settled_at = Some(1);
        let active = session("active", "one", "Open", "codex", false);
        let resumed = resumable_session(Some("settled"), &[settled, active]);
        assert_eq!(resumed.map(|(_, chat, _)| chat), Some("active".to_owned()));
    }

    #[test]
    fn ages_read_short_enough_for_a_row() {
        let now = 10_000_000;
        for (ago, label) in [
            (0, "now"),
            (59, "now"),
            (60, "1m"),
            (3_599, "59m"),
            (3_600, "1h"),
            (86_399, "23h"),
            (86_400, "1d"),
            (604_799, "6d"),
            (604_800, "1w"),
            (-5, "now"),
        ] {
            assert_eq!(age_label(now - ago, now), label);
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
            background_tasks: 0,
            settled_at: None,
            auto_settle_disabled: false,
            paused: false,
            pull_requests: Vec::new(),
        }
    }

    #[test]
    fn project_cards_carry_the_owner_behind_the_workspace() {
        let mut owned = folder("one", "Compiler");
        owned.owner = Some("RestartFU".into());

        let cards = project_cards(&[owned, folder("two", "Website")], &[]);

        assert_eq!(cards[0].owner.as_deref(), Some("RestartFU"));
        assert_eq!(cards[1].owner, None);
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
                    owner: None,
                    sessions: 2,
                    working: 1,
                },
                ProjectCard {
                    id: "two".into(),
                    name: "Website".into(),
                    owner: None,
                    sessions: 1,
                    working: 0,
                },
            ]
        );
    }

    #[test]
    fn project_sessions_keep_the_real_cli_provider() {
        let chats = vec![
            ChatSummary {
                // A turn running hides the tasks: the session is working.
                background_tasks: 3,
                ..session("a", "one", "Fix parser", "codex", true)
            },
            ChatSummary {
                background_tasks: 2,
                ..session("b", "one", "Review lexer", "claude", false)
            },
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
                    background_tasks: 0,
                },
                SessionCard {
                    id: "b".into(),
                    title: "Review lexer".into(),
                    agent: AgentCli::Claude,
                    branch: "session/b".into(),
                    working: false,
                    background_tasks: 2,
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
    fn sessions_on_an_unknown_assistant_still_list_and_open() {
        let folders = vec![folder("one", "Compiler")];
        let chats = vec![
            session("a", "one", "Supported", "codex", false),
            session("b", "one", "Old task", "jcode", false),
        ];

        let sessions = project_sessions("one", &chats);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[1].id, "b");
        assert_eq!(sessions[1].agent, AgentCli::Other);
        assert_eq!(sessions[1].agent.label(), "Assistant");
        assert_eq!(project_cards(&folders, &chats)[0].sessions, 2);
        assert_eq!(
            reconcile_route(
                &MinimalRoute::Cli {
                    project_id: "one".into(),
                    chat_id: "b".into(),
                    agent: AgentCli::Other,
                },
                &folders,
                &chats,
            ),
            MinimalRoute::Cli {
                project_id: "one".into(),
                chat_id: "b".into(),
                agent: AgentCli::Other,
            }
        );
        assert_eq!(AgentCli::from_backend(""), None);
        assert_eq!(AgentCli::from_protocol_name("jcode"), None);
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

        chats.push(session("b", "one", "Other session", "codex", false));
        chats.remove(0);
        assert_eq!(
            reconcile_route(&route, &folders, &chats),
            MinimalRoute::Cli {
                project_id: "one".into(),
                chat_id: "b".into(),
                agent: AgentCli::Codex,
            }
        );

        chats.clear();
        assert_eq!(
            reconcile_route(&route, &folders, &chats),
            MinimalRoute::Sessions {
                project_id: Some("one".into()),
            }
        );

        assert_eq!(
            reconcile_route(&route, &[], &chats),
            MinimalRoute::default()
        );
    }

    #[test]
    fn terminal_route_survives_tree_reconciliation() {
        assert_eq!(
            reconcile_route(&MinimalRoute::Terminal, &[], &[]),
            MinimalRoute::Terminal
        );
    }
}
