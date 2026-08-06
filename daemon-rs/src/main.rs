use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

mod remote_proxy;

use remote_proxy::RemoteProxy;
use xd_daemon::{Engine, LocalServer, StateStore, remote_socket_path};

struct Options {
    socket: PathBuf,
    database: PathBuf,
    workspaces: PathBuf,
}

fn main() -> ExitCode {
    match arguments(env::args().skip(1)) {
        Ok(options) => match serve(options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("xd-daemon-dev: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("xd-daemon-dev: {error}");
            eprintln!(
                "usage: xd-daemon-dev serve --socket PATH [--database PATH] [--workspaces PATH]"
            );
            ExitCode::FAILURE
        }
    }
}

fn serve(options: Options) -> Result<(), String> {
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
        data_directory.join("tls"),
    ));
    let engine = Engine::with_store_and_data(store, Some(data_directory));
    let listener = proxy.clone();
    engine.set_peer_listener(move |bind, port| listener.listen(bind, port))?;
    let server = LocalServer::bind_with_engine(&options.socket, engine)
        .map_err(|error| error.to_string())?;
    if let Some((bind, port)) = saved_listener
        && let Err(error) = proxy.listen(&bind, port)
    {
        eprintln!("xd-daemon-dev: cannot restore remote listener: {error}");
    }
    server.run().map_err(|error| error.to_string())
}

fn arguments(arguments: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut arguments = arguments.into_iter();
    if arguments.next().as_deref() != Some("serve") {
        return Err("expected the serve command".into());
    }
    let mut socket = None;
    let mut database = None;
    let mut workspaces = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
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
            _ => return Err(format!("unknown argument {argument}")),
        }
    }
    let socket = socket.ok_or_else(|| "--socket is required".to_string())?;
    let data_directory = socket
        .parent()
        .ok_or_else(|| "--socket must have a parent directory".to_string())?;
    Ok(Options {
        database: database.unwrap_or_else(|| data_directory.join("chats.db")),
        workspaces: workspaces.unwrap_or_else(|| data_directory.join("Workspaces")),
        socket,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_compatible_serve_command() {
        let options =
            arguments(["serve".into(), "--socket".into(), "/tmp/xd.sock".into()]).unwrap();
        assert_eq!(options.socket, PathBuf::from("/tmp/xd.sock"));
        assert_eq!(options.database, PathBuf::from("/tmp/chats.db"));
        assert_eq!(options.workspaces, PathBuf::from("/tmp/Workspaces"));

        let options = arguments([
            "serve".into(),
            "--socket=/tmp/xd.sock".into(),
            "--database=/state/xd.db".into(),
            "--workspaces=/work".into(),
        ])
        .unwrap();
        assert_eq!(options.database, PathBuf::from("/state/xd.db"));
        assert_eq!(options.workspaces, PathBuf::from("/work"));
    }
}
