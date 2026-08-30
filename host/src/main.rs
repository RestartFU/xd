use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

use xd_host::{Engine, StateStore, serve_stdio};

struct HostOptions {
    database: PathBuf,
    workspaces: PathBuf,
}

enum CliCommand {
    Stdio(HostOptions),
    RecordAgentSession {
        database: PathBuf,
        chat: String,
        backend: String,
        notification: String,
    },
    Version,
    Help,
}

fn main() -> ExitCode {
    match arguments(env::args().skip(1)) {
        Ok(CliCommand::Version) => {
            println!("xd-host {}", version_string());
            ExitCode::SUCCESS
        }
        Ok(CliCommand::Help) => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Ok(CliCommand::Stdio(options)) => finish(host_stdio(options)),
        Ok(CliCommand::RecordAgentSession {
            database,
            chat,
            backend,
            notification,
        }) => finish(
            StateStore::record_agent_notification(&database, &chat, &backend, &notification)
                .map_err(|error| format!("cannot record agent session: {error}")),
        ),
        Err(error) => {
            eprintln!("xd-host: {error}");
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn finish(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xd-host: {error}");
            ExitCode::FAILURE
        }
    }
}

fn host_stdio(options: HostOptions) -> Result<(), String> {
    let store = StateStore::open(&options.database, &options.workspaces)
        .map_err(|error| error.to_string())?;
    // Every desktop and mobile SSH connection owns a separate stdio host.
    // Another process may still own any `host_working` turn in this shared
    // store, so merely connecting must never declare those turns abandoned.
    // The explicit cancel path retains its single-chat stuck-turn repair.
    let data_directory = options
        .database
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "--database must have a parent directory".to_owned())?;
    let engine = Engine::with_store_and_data(store, Some(data_directory));
    serve_stdio(engine, std::io::stdin(), std::io::stdout()).map_err(|error| error.to_string())
}

fn arguments(arguments: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut arguments = arguments.into_iter();
    match arguments.next().as_deref() {
        Some("stdio") => host_arguments(arguments).map(CliCommand::Stdio),
        Some("record-agent-session") => record_agent_session_arguments(arguments),
        Some("--version" | "-v") if arguments.next().is_none() => Ok(CliCommand::Version),
        Some("--help" | "-h") if arguments.next().is_none() => Ok(CliCommand::Help),
        Some("serve" | "pair") => {
            Err("socket serving and pairing were removed; connect with SSH".into())
        }
        _ => Err("expected the stdio command".into()),
    }
}

fn host_arguments(mut arguments: impl Iterator<Item = String>) -> Result<HostOptions, String> {
    let mut data = None;
    let mut database = None;
    let mut workspaces = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--data" => {
                data = Some(PathBuf::from(
                    arguments.next().ok_or("--data needs a directory")?,
                ));
            }
            _ if argument.starts_with("--data=") => {
                data = Some(PathBuf::from(&argument["--data=".len()..]));
            }
            "--database" => {
                database = Some(PathBuf::from(
                    arguments.next().ok_or("--database needs a path")?,
                ));
            }
            _ if argument.starts_with("--database=") => {
                database = Some(PathBuf::from(&argument["--database=".len()..]));
            }
            "--workspaces" | "--root" => {
                workspaces = Some(PathBuf::from(
                    arguments.next().ok_or("--workspaces needs a path")?,
                ));
            }
            _ if argument.starts_with("--workspaces=") => {
                workspaces = Some(PathBuf::from(&argument["--workspaces=".len()..]));
            }
            _ if argument.starts_with("--root=") => {
                workspaces = Some(PathBuf::from(&argument["--root=".len()..]));
            }
            _ => return Err(format!("unknown stdio argument {argument}")),
        }
    }
    if data.is_some() && (database.is_some() || workspaces.is_some()) {
        return Err("--data cannot be combined with --database, --workspaces, or --root".into());
    }
    if let Some(data) = data {
        database = Some(data.join("chats.db"));
        workspaces = Some(data.join("Workspaces"));
    }
    Ok(HostOptions {
        database: database.ok_or("stdio needs --data or --database")?,
        workspaces: workspaces.ok_or("stdio needs --data or --workspaces")?,
    })
}

fn record_agent_session_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<CliCommand, String> {
    let mut database = None;
    let mut chat = None;
    let mut backend = None;
    let mut notification = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--database" => {
                database = Some(PathBuf::from(
                    arguments.next().ok_or("--database needs a path")?,
                ));
            }
            _ if argument.starts_with("--database=") => {
                database = Some(PathBuf::from(&argument["--database=".len()..]));
            }
            "--chat" => chat = Some(arguments.next().ok_or("--chat needs an id")?),
            _ if argument.starts_with("--chat=") => {
                chat = Some(argument["--chat=".len()..].to_owned());
            }
            "--backend" => backend = Some(arguments.next().ok_or("--backend needs a name")?),
            _ if argument.starts_with("--backend=") => {
                backend = Some(argument["--backend=".len()..].to_owned());
            }
            _ if notification.is_none() => notification = Some(argument),
            _ => return Err("record-agent-session received extra arguments".into()),
        }
    }
    Ok(CliCommand::RecordAgentSession {
        database: database.ok_or("record-agent-session needs --database")?,
        chat: chat
            .filter(|chat| !chat.is_empty())
            .ok_or("record-agent-session needs --chat")?,
        backend: backend
            .filter(|backend| !backend.is_empty())
            .ok_or("record-agent-session needs --backend")?,
        notification: notification.ok_or("record-agent-session needs a notification")?,
    })
}

fn version_string() -> String {
    option_env!("XD_COMMIT")
        .filter(|commit| !commit.is_empty() && *commit != "development")
        .map(|commit| {
            format!(
                "{} ({})",
                env!("CARGO_PKG_VERSION"),
                &commit[..7.min(commit.len())]
            )
        })
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}

fn usage() -> &'static str {
    "usage: xd-host stdio --data DIR\n\
     \n\
     The host reads JSON frames from stdin and writes replies/events to stdout.\n\
     It opens no socket and exits when its desktop or SSH connection closes."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stdio_host_without_a_socket_or_listener_options() {
        let CliCommand::Stdio(options) =
            arguments(["stdio".into(), "--data=/state/xd-nightly".into()]).unwrap()
        else {
            panic!("expected stdio host options");
        };
        assert_eq!(
            options.database,
            PathBuf::from("/state/xd-nightly/chats.db")
        );
        assert_eq!(
            options.workspaces,
            PathBuf::from("/state/xd-nightly/Workspaces")
        );
        assert!(arguments(["stdio".into(), "--bind=::".into()]).is_err());
        assert!(arguments(["stdio".into(), "--socket=/tmp/xd.sock".into()]).is_err());
    }

    #[test]
    fn rejects_removed_socket_server_commands() {
        assert!(arguments(["serve".into(), "--socket=/tmp/xd.sock".into()]).is_err());
        assert!(arguments(["pair".into(), "--port=4444".into()]).is_err());
    }

    #[test]
    fn connecting_a_stdio_client_does_not_recover_other_hosts_turns() {
        let source = include_str!("main.rs");
        let startup = source
            .split_once("fn host_stdio(")
            .expect("stdio host startup")
            .1
            .split_once("fn arguments(")
            .expect("end of stdio host startup")
            .0;

        assert!(!startup.contains("recover_interrupted_turns"));
    }

    #[test]
    fn parses_the_internal_agent_session_recorder_command() {
        let CliCommand::RecordAgentSession { chat, backend, .. } = arguments([
            "record-agent-session".into(),
            "--database=/state/chats.db".into(),
            "--chat=chat-1".into(),
            "--backend=codex".into(),
            r#"{"type":"agent-turn-complete"}"#.into(),
        ])
        .unwrap() else {
            panic!("expected recorder command");
        };
        assert_eq!(chat, "chat-1");
        assert_eq!(backend, "codex");
    }
}
