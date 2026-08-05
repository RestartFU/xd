use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::FileTypeExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    thread,
};

use serde_json::{Value, json};
use thiserror::Error;

pub const FRAME_LIMIT: usize = 96 * 1024 * 1024;
const REQUEST_ID: &str = "_xd_request";

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("cannot prepare daemon socket directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("refusing to replace non-socket path {0}")]
    UnsafeSocketPath(PathBuf),
    #[error("an xd daemon is already listening on {0}")]
    AlreadyRunning(PathBuf),
    #[error("cannot remove stale daemon socket {path}: {source}")]
    RemoveSocket {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot bind daemon socket {path}: {source}")]
    Bind {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("daemon socket failed: {0}")]
    Accept(std::io::Error),
}

pub struct LocalServer {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl LocalServer {
    pub fn bind(socket_path: impl Into<PathBuf>) -> Result<Self, ServerError> {
        let socket_path = socket_path.into();
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent).map_err(|source| ServerError::CreateDirectory {
                path: parent.to_owned(),
                source,
            })?;
        }
        match fs::symlink_metadata(&socket_path) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                if UnixStream::connect(&socket_path).is_ok() {
                    return Err(ServerError::AlreadyRunning(socket_path));
                }
                fs::remove_file(&socket_path).map_err(|source| ServerError::RemoveSocket {
                    path: socket_path.clone(),
                    source,
                })?;
            }
            Ok(_) => return Err(ServerError::UnsafeSocketPath(socket_path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ServerError::Bind {
                    path: socket_path,
                    source,
                });
            }
        }
        let listener = UnixListener::bind(&socket_path).map_err(|source| ServerError::Bind {
            path: socket_path.clone(),
            source,
        })?;
        Ok(Self {
            listener,
            socket_path,
        })
    }

    pub fn run(self) -> Result<(), ServerError> {
        for connection in self.listener.incoming() {
            let stream = connection.map_err(ServerError::Accept)?;
            thread::Builder::new()
                .name("xd-rust-local-client".into())
                .spawn(move || {
                    let _ = serve_connection(stream);
                })
                .map_err(ServerError::Accept)?;
        }
        Ok(())
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

pub fn serve_connection(mut stream: UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        let mut line = Vec::new();
        let count = reader
            .by_ref()
            .take((FRAME_LIMIT + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            return Ok(());
        }
        if line.len() > FRAME_LIMIT {
            return Ok(());
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }

        let reply = match serde_json::from_slice::<Value>(&line) {
            Ok(request) => dispatch(request),
            Err(error) => json!({"ok": false, "error": format!("Invalid JSON request: {error}")}),
        };
        serde_json::to_writer(&mut stream, &reply)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }
}

pub fn dispatch(request: Value) -> Value {
    let request_id = request.get(REQUEST_ID).cloned();
    let mut reply = match request.get("op").and_then(Value::as_str) {
        Some("ping") => json!({"ok": true}),
        Some(operation) => json!({
            "ok": false,
            "error": format!("Operation {operation} is not implemented by the Rust daemon yet.")
        }),
        None => json!({"ok": false, "error": "Request must include a string op."}),
    };
    if let (Some(request_id), Some(reply)) = (request_id, reply.as_object_mut()) {
        reply.insert(REQUEST_ID.into(), request_id);
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        io::{BufRead, BufReader},
        os::unix::net::UnixStream,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn ping_echoes_the_private_request_id() {
        assert_eq!(
            dispatch(json!({"op": "ping", "_xd_request": 42})),
            json!({"ok": true, "_xd_request": 42})
        );
    }

    #[test]
    fn unknown_operations_are_explicit_and_correlated() {
        let reply = dispatch(json!({"op": "tree", "_xd_request": 7}));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["_xd_request"], 7);
        assert!(reply["error"].as_str().unwrap().contains("tree"));
    }

    #[test]
    fn one_connection_handles_multiple_json_lines() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || serve_connection(server).unwrap());
        client
            .write_all(
                b"\n{\"op\":\"ping\",\"_xd_request\":1}\n{\"op\":\"ping\",\"_xd_request\":2}\n",
            )
            .unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let replies = BufReader::new(client)
            .lines()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(replies.len(), 2);
        assert_eq!(
            serde_json::from_str::<Value>(&replies[0]).unwrap()[REQUEST_ID],
            1
        );
        assert_eq!(
            serde_json::from_str::<Value>(&replies[1]).unwrap()[REQUEST_ID],
            2
        );
        worker.join().unwrap();
    }

    #[test]
    fn refuses_to_replace_a_regular_file_with_a_socket() {
        let root = test_directory();
        fs::create_dir_all(&root).unwrap();
        let path = root.join("daemon.sock");
        fs::write(&path, "keep me").unwrap();
        assert!(matches!(
            LocalServer::bind(&path),
            Err(ServerError::UnsafeSocketPath(found)) if found == path
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep me");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_unlink_a_live_daemon_socket() {
        let root = test_directory();
        fs::create_dir_all(&root).unwrap();
        let path = root.join("daemon.sock");
        let listener = UnixListener::bind(&path).unwrap();
        assert!(matches!(
            LocalServer::bind(&path),
            Err(ServerError::AlreadyRunning(found)) if found == path
        ));
        assert!(path.exists());
        drop(listener);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replaces_a_stale_socket() {
        let root = test_directory();
        fs::create_dir_all(&root).unwrap();
        let path = root.join("daemon.sock");
        drop(UnixListener::bind(&path).unwrap());
        let server = LocalServer::bind(&path).unwrap();
        assert!(path.exists());
        drop(server);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn socket_is_removed_when_the_server_is_dropped() {
        let root = test_directory();
        let path = root.join("daemon.sock");
        let server = LocalServer::bind(&path).unwrap();
        assert_eq!(server.socket_path(), path);
        assert!(path.exists());
        drop(server);
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn test_directory() -> PathBuf {
        env::temp_dir().join(format!(
            "xd-rust-daemon-test-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
