use std::{env, path::PathBuf, process::ExitCode};

use xd_daemon::LocalServer;

fn main() -> ExitCode {
    match arguments(env::args().skip(1)) {
        Ok(socket) => match LocalServer::bind(&socket).and_then(LocalServer::run) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("xd-daemon-dev: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("xd-daemon-dev: {error}");
            eprintln!("usage: xd-daemon-dev serve --socket PATH");
            ExitCode::FAILURE
        }
    }
}

fn arguments(arguments: impl IntoIterator<Item = String>) -> Result<PathBuf, String> {
    let mut arguments = arguments.into_iter();
    if arguments.next().as_deref() != Some("serve") {
        return Err("expected the serve command".into());
    }
    let mut socket = None;
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
            _ => return Err(format!("unknown argument {argument}")),
        }
    }
    socket.ok_or_else(|| "--socket is required".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_compatible_serve_command() {
        assert_eq!(
            arguments(["serve".into(), "--socket".into(), "/tmp/xd.sock".into()]).unwrap(),
            PathBuf::from("/tmp/xd.sock")
        );
        assert_eq!(
            arguments(["serve".into(), "--socket=/tmp/xd.sock".into()]).unwrap(),
            PathBuf::from("/tmp/xd.sock")
        );
    }
}
