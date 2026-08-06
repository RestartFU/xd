use std::{
    env,
    io::{BufRead, BufReader, ErrorKind, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use serde_json::{Value, json};

mod remote_proxy;

use remote_proxy::RemoteProxy;
use xd_daemon::{Engine, LocalServer, StateStore, remote_socket_path};

struct Options {
    socket: PathBuf,
    database: PathBuf,
    workspaces: PathBuf,
    bind: String,
    port: u16,
    pair: bool,
}

enum CliCommand {
    Serve(Options),
    Version,
    Help,
}

fn main() -> ExitCode {
    match arguments(env::args().skip(1)) {
        Ok(CliCommand::Version) => {
            println!("xd-daemon {}", version_string());
            ExitCode::SUCCESS
        }
        Ok(CliCommand::Help) => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Ok(CliCommand::Serve(options)) => match serve(options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("xd-daemon: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("xd-daemon: {error}");
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn serve(options: Options) -> Result<(), String> {
    if options.pair && pair_with_running_daemon(&options)? {
        return Ok(());
    }
    refuse_live_daemon(&options.socket)?;
    let store = StateStore::open(&options.database, &options.workspaces)
        .map_err(|error| error.to_string())?;
    let saved_listener = store.remote_listener().map_err(|error| error.to_string())?;
    let data_directory = options
        .database
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "--database must have a parent directory".to_owned())?;
    let proxy = Arc::new(RemoteProxy::new(
        remote_socket_path(&options.socket),
        data_directory.clone(),
    ));
    let pairing_endpoint = if options.pair {
        let endpoint = proxy.listen(&options.bind, options.port)?;
        store
            .save_remote_listener(&options.bind, endpoint.port)
            .map_err(|error| error.to_string())?;
        Some(endpoint)
    } else {
        None
    };
    let engine = Engine::with_store_and_data(store, Some(data_directory));
    let listener = proxy.clone();
    engine.set_peer_listener(move |bind, port| listener.listen(bind, port))?;
    let pairing_code = options
        .pair
        .then(|| engine.arm_pairing(Duration::from_secs(5 * 60)));
    let server = LocalServer::bind_with_engine(&options.socket, engine)
        .map_err(|error| error.to_string())?;
    if let Some(endpoint) = pairing_endpoint {
        println!(
            "xd-daemon serve: {}, listening on {}, workspaces at {}",
            version_string(),
            endpoint.port,
            options.workspaces.display()
        );
        println!(
            "pairing code (5 minutes, one use): {}",
            pairing_code.as_deref().unwrap_or_default()
        );
        std::io::stdout()
            .flush()
            .map_err(|error| format!("cannot print the pairing code: {error}"))?;
    } else if let Some((bind, port)) = saved_listener
        && let Err(error) = proxy.listen(&bind, port)
    {
        eprintln!("xd-daemon: cannot restore remote listener: {error}");
    }
    server.run().map_err(|error| error.to_string())
}

fn refuse_live_daemon(socket: &Path) -> Result<(), String> {
    match UnixStream::connect(socket) {
        Ok(_) => Err(format!(
            "an xd daemon is already listening on {}",
            socket.display()
        )),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(format!(
            "cannot check whether an xd daemon is already using {}: {error}",
            socket.display()
        )),
    }
}

fn pair_with_running_daemon(options: &Options) -> Result<bool, String> {
    let mut stream = match UnixStream::connect(&options.socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(format!("cannot connect to the running daemon: {error}")),
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("cannot configure the daemon connection: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("cannot configure the daemon connection: {error}"))?;
    writeln!(
        stream,
        "{}",
        json!({"op": "peer-pairing", "bind": options.bind, "port": options.port})
    )
    .map_err(|error| format!("cannot request a pairing code: {error}"))?;
    let mut response = String::new();
    BufReader::new(stream)
        .take(64 * 1024 + 1)
        .read_line(&mut response)
        .map_err(|error| format!("cannot read the pairing response: {error}"))?;
    if response.len() > 64 * 1024 {
        return Err("the running daemon returned an oversized pairing response".into());
    }
    let response: Value = serde_json::from_str(&response)
        .map_err(|error| format!("the running daemon returned invalid JSON: {error}"))?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the running daemon refused pairing")
            .to_owned());
    }
    let host = response
        .get("host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .ok_or("the running daemon returned no pairing host")?;
    let port = response
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .ok_or("the running daemon returned no pairing port")?;
    let code = response
        .get("code")
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty())
        .ok_or("the running daemon returned no pairing code")?;
    println!("xd-daemon serve: attached to running daemon at {host}:{port}");
    println!("pairing code (5 minutes, one use): {code}");
    Ok(true)
}

fn arguments(arguments: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut arguments = arguments.into_iter();
    match arguments.next().as_deref() {
        Some("--version" | "-v") if arguments.next().is_none() => return Ok(CliCommand::Version),
        Some("--help" | "-h") if arguments.next().is_none() => return Ok(CliCommand::Help),
        Some("serve") => {}
        _ => return Err("expected the serve command".into()),
    }
    let mut socket = None;
    let mut database = None;
    let mut workspaces = None;
    let mut data = None;
    let mut bind = "::".to_owned();
    let mut port = 4001;
    let mut pair = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(CliCommand::Help),
            "--pair" => pair = true,
            "--bind" => bind = arguments.next().ok_or("--bind needs an address")?,
            _ if argument.starts_with("--bind=") => {
                bind = argument["--bind=".len()..].to_owned();
            }
            "--port" => {
                port = parse_port(&arguments.next().ok_or("--port needs a number")?)?;
            }
            _ if argument.starts_with("--port=") => {
                port = parse_port(&argument["--port=".len()..])?;
            }
            "--data" => {
                data = Some(PathBuf::from(
                    arguments.next().ok_or("--data needs a directory")?,
                ));
            }
            _ if argument.starts_with("--data=") => {
                data = Some(PathBuf::from(&argument["--data=".len()..]));
            }
            "--socket" => {
                socket = Some(PathBuf::from(
                    arguments.next().ok_or("--socket needs a path")?,
                ));
            }
            _ if argument.starts_with("--socket=") => {
                socket = Some(PathBuf::from(&argument["--socket=".len()..]));
            }
            "--database" => {
                database = Some(PathBuf::from(
                    arguments.next().ok_or("--database needs a path")?,
                ));
            }
            _ if argument.starts_with("--database=") => {
                database = Some(PathBuf::from(&argument["--database=".len()..]));
            }
            "--workspaces" => {
                workspaces = Some(PathBuf::from(
                    arguments.next().ok_or("--workspaces needs a path")?,
                ));
            }
            _ if argument.starts_with("--workspaces=") => {
                workspaces = Some(PathBuf::from(&argument["--workspaces=".len()..]));
            }
            "--root" => {
                workspaces = Some(PathBuf::from(
                    arguments.next().ok_or("--root needs a path")?,
                ));
            }
            _ if argument.starts_with("--root=") => {
                workspaces = Some(PathBuf::from(&argument["--root=".len()..]));
            }
            _ => return Err(format!("unknown argument {argument}")),
        }
    }
    if data.is_some() && (socket.is_some() || database.is_some() || workspaces.is_some()) {
        return Err(
            "--data cannot be combined with --socket, --database, --workspaces, or --root".into(),
        );
    }
    if let Some(data) = data {
        socket = Some(data.join("daemon.sock"));
        database = Some(data.join("chats.db"));
        workspaces = Some(data.join("Workspaces"));
    }
    let socket = socket.ok_or_else(|| "--socket or --data is required".to_string())?;
    let data_directory = socket
        .parent()
        .ok_or_else(|| "--socket must have a parent directory".to_string())?;
    if bind.is_empty() {
        return Err("--bind cannot be empty".into());
    }
    Ok(CliCommand::Serve(Options {
        database: database.unwrap_or_else(|| data_directory.join("chats.db")),
        workspaces: workspaces.unwrap_or_else(|| data_directory.join("Workspaces")),
        socket,
        bind,
        port,
        pair,
    }))
}

fn parse_port(value: &str) -> Result<u16, String> {
    value.parse().map_err(|_| format!("invalid port: {value}"))
}

fn version_string() -> String {
    option_env!("XD_DEV_COMMIT")
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
    "usage: xd-daemon serve (--socket PATH | --data DIR) [options]\n\
     \n\
     options:\n\
       --data DIR         adopt one xd data root atomically\n\
       --database FILE    chat database beside the socket by default\n\
       --workspaces DIR   workspace root beside the socket by default\n\
       --root DIR         alias for --workspaces\n\
       --pair             listen remotely and print a five-minute pairing code\n\
       --bind ADDRESS     remote TLS bind address (default ::)\n\
       --port PORT        remote TLS port (default 4001)\n\
       -h, --help         show this help\n\
       -v, --version      show the daemon version"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::net::UnixListener, thread};

    #[test]
    fn parses_the_compatible_serve_command() {
        let CliCommand::Serve(options) =
            arguments(["serve".into(), "--socket".into(), "/tmp/xd.sock".into()]).unwrap()
        else {
            panic!("expected serve options");
        };
        assert_eq!(options.socket, PathBuf::from("/tmp/xd.sock"));
        assert_eq!(options.database, PathBuf::from("/tmp/chats.db"));
        assert_eq!(options.workspaces, PathBuf::from("/tmp/Workspaces"));
        assert_eq!(options.bind, "::");
        assert_eq!(options.port, 4001);
        assert!(!options.pair);

        let CliCommand::Serve(options) = arguments([
            "serve".into(),
            "--socket=/tmp/xd.sock".into(),
            "--database=/state/xd.db".into(),
            "--root=/work".into(),
            "--bind=127.0.0.1".into(),
            "--port=0".into(),
            "--pair".into(),
        ])
        .unwrap() else {
            panic!("expected serve options");
        };
        assert_eq!(options.database, PathBuf::from("/state/xd.db"));
        assert_eq!(options.workspaces, PathBuf::from("/work"));
        assert_eq!(options.bind, "127.0.0.1");
        assert_eq!(options.port, 0);
        assert!(options.pair);
        assert!(matches!(
            arguments(["--version".into()]).unwrap(),
            CliCommand::Version
        ));
        assert!(matches!(
            arguments(["serve".into(), "--help".into()]).unwrap(),
            CliCommand::Help
        ));
        assert!(
            arguments([
                "serve".into(),
                "--socket=/tmp/xd.sock".into(),
                "--port=65536".into(),
            ])
            .is_err()
        );

        let CliCommand::Serve(options) =
            arguments(["serve".into(), "--data=/state/xd-nightly".into()]).unwrap()
        else {
            panic!("expected serve options");
        };
        assert_eq!(
            options.socket,
            PathBuf::from("/state/xd-nightly/daemon.sock")
        );
        assert_eq!(
            options.database,
            PathBuf::from("/state/xd-nightly/chats.db")
        );
        assert_eq!(
            options.workspaces,
            PathBuf::from("/state/xd-nightly/Workspaces")
        );
        assert!(
            arguments([
                "serve".into(),
                "--data=/state/xd-nightly".into(),
                "--socket=/tmp/other.sock".into(),
            ])
            .is_err()
        );
        assert!(arguments(["serve".into()]).is_err());
    }

    #[test]
    fn refuses_a_live_socket_before_opening_state() {
        let directory = env::temp_dir().join(format!(
            "xd-rust-cli-live-check-{}-{}",
            std::process::id(),
            thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let error = refuse_live_daemon(&socket).unwrap_err();
        assert!(error.contains("already listening"));
        assert!(!directory.join("chats.db").exists());

        drop(listener);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn pairing_attaches_to_an_already_running_daemon() {
        let directory = env::temp_dir().join(format!("xd-rust-cli-pair-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["op"], "peer-pairing");
            assert_eq!(request["bind"], "127.0.0.1");
            assert_eq!(request["port"], 4444);
            writeln!(
                stream,
                "{}",
                json!({
                    "ok": true,
                    "host": "127.0.0.1",
                    "port": 4444,
                    "code": "ABCD-EFGH"
                })
            )
            .unwrap();
        });
        let options = Options {
            socket,
            database: directory.join("chats.db"),
            workspaces: directory.join("Workspaces"),
            bind: "127.0.0.1".into(),
            port: 4444,
            pair: true,
        };
        assert!(pair_with_running_daemon(&options).unwrap());
        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }
}
