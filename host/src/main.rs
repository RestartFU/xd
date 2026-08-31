use std::{
    env, fs, io,
    os::unix::{net::UnixStream, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};

use xd_host::{Engine, HostServer, StateStore, serve_stdio};

#[derive(Clone)]
struct HostOptions {
    database: PathBuf,
    workspaces: PathBuf,
    persistent: bool,
}

enum CliCommand {
    Stdio(HostOptions),
    Broker(HostOptions),
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
        Ok(CliCommand::Broker(options)) => finish(host_broker(options)),
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
    if !options.persistent {
        return host_local_stdio(options);
    }
    let stream = connect_or_start_broker(&options)?;
    proxy_stdio(stream).map_err(|error| format!("host broker connection failed: {error}"))
}

fn host_local_stdio(options: HostOptions) -> Result<(), String> {
    let store = StateStore::open(&options.database, &options.workspaces)
        .map_err(|error| error.to_string())?;
    let data_directory = options
        .database
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "--database must have a parent directory".to_owned())?;
    let engine = Engine::with_store_and_data(store, Some(data_directory));
    serve_stdio(engine, io::stdin(), io::stdout()).map_err(|error| error.to_string())
}

fn host_broker(options: HostOptions) -> Result<(), String> {
    let server = HostServer::bind(host_socket(&options))
        .map_err(|error| format!("cannot bind the host broker: {error}"))?;
    let store = StateStore::open(&options.database, &options.workspaces)
        .map_err(|error| error.to_string())?;
    if let Err(error) = store.recover_interrupted_turns() {
        eprintln!("xd-host: cannot recover turns left by an earlier broker: {error}");
    }
    let data_directory = options
        .database
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "--database must have a parent directory".to_owned())?;
    let engine = Engine::with_store_and_data(store, Some(data_directory));
    server
        .run(engine)
        .map_err(|error| format!("host broker failed: {error}"))
}

fn connect_or_start_broker(options: &HostOptions) -> Result<UnixStream, String> {
    let socket = host_socket(options);
    match UnixStream::connect(&socket) {
        Ok(stream) => return Ok(stream),
        Err(error)
            if !matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(format!(
                "cannot connect to host broker {}: {error}",
                socket.display()
            ));
        }
        Err(_) => {}
    }

    let parent = socket
        .parent()
        .ok_or_else(|| "host socket has no parent directory".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create host runtime {}: {error}", parent.display()))?;
    let executable = env::current_exe()
        .map_err(|error| format!("cannot locate the host executable: {error}"))?;
    let mut command = Command::new(executable);
    command
        .args(["broker", "--database"])
        .arg(&options.database)
        .arg("--workspaces")
        .arg(&options.workspaces)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The broker belongs to the state directory, not to this desktop or SSH
    // process group. In particular, sshd may hang up the exec session while a
    // phone sleeps; that must not signal the broker or its agents.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start the host broker: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match UnixStream::connect(&socket) {
            Ok(stream) => return Ok(stream),
            Err(error) if Instant::now() >= deadline => {
                return Err(format!(
                    "host broker did not create {}: {error}",
                    socket.display()
                ));
            }
            Err(_) => {}
        }
        // Another simultaneous client may have won the bind race. Its broker
        // will become connectable even if our child has already exited.
        let _ = child.try_wait();
        thread::sleep(Duration::from_millis(10));
    }
}

fn proxy_stdio(mut stream: UnixStream) -> io::Result<()> {
    let mut writer = stream.try_clone()?;
    thread::Builder::new()
        .name("xd-host-input".into())
        .spawn(move || {
            let _ = io::copy(&mut io::stdin().lock(), &mut writer);
            let _ = writer.shutdown(std::net::Shutdown::Both);
        })?;
    io::copy(&mut stream, &mut io::stdout().lock())?;
    Ok(())
}

fn host_socket(options: &HostOptions) -> PathBuf {
    options
        .database
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("runtime/v1/host.sock")
}

fn arguments(arguments: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut arguments = arguments.into_iter();
    match arguments.next().as_deref() {
        Some("stdio") => host_arguments(arguments).map(CliCommand::Stdio),
        Some("broker") => host_arguments(arguments).map(CliCommand::Broker),
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
    let mut persistent = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--persistent" => persistent = true,
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
        persistent,
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
    "usage: xd-host stdio [--persistent] --data DIR\n\
     \n\
     The host reads JSON frames from stdin and writes replies/events to stdout."
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
        assert!(!options.persistent);
        let CliCommand::Stdio(options) = arguments([
            "stdio".into(),
            "--persistent".into(),
            "--data=/state/xd-nightly".into(),
        ])
        .unwrap() else {
            panic!("expected stdio command");
        };
        assert!(options.persistent);
        assert!(arguments(["stdio".into(), "--bind=::".into()]).is_err());
        assert!(arguments(["stdio".into(), "--socket=/tmp/xd.sock".into()]).is_err());
    }

    #[test]
    fn rejects_removed_socket_server_commands() {
        assert!(arguments(["serve".into(), "--socket=/tmp/xd.sock".into()]).is_err());
        assert!(arguments(["pair".into(), "--port=4444".into()]).is_err());
    }

    #[test]
    fn connecting_a_stdio_client_does_not_recover_the_brokers_turns() {
        let source = include_str!("main.rs");
        let startup = source
            .split_once("fn host_stdio(")
            .expect("stdio host startup")
            .1
            .split_once("fn host_broker(")
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
