use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

struct HostClient {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl HostClient {
    fn connect(data: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_xd-host"))
            .args(["stdio", "--persistent", "--data"])
            .arg(data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
        }
    }

    fn request(&mut self, request: Value) -> Value {
        writeln!(self.input, "{request}").unwrap();
        self.input.flush().unwrap();
        loop {
            let ready = unsafe {
                let mut descriptor = libc::pollfd {
                    fd: std::os::fd::AsRawFd::as_raw_fd(self.output.get_ref()),
                    events: libc::POLLIN,
                    revents: 0,
                };
                libc::poll(&mut descriptor, 1, 2_000)
            };
            assert_ne!(ready, 0, "host did not reply to {request}");
            assert!(
                ready > 0,
                "cannot poll the host reply: {}",
                std::io::Error::last_os_error()
            );
            let mut line = String::new();
            assert_ne!(self.output.read_line(&mut line).unwrap(), 0);
            let frame: Value = serde_json::from_str(&line).unwrap();
            if frame.get("event").is_none() {
                return frame;
            }
        }
    }

    fn disconnect_abruptly(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

#[test]
fn terminals_and_codex_turns_survive_the_stdio_client_that_started_them() {
    let data = fixture("persistent-stdio");
    let socket = data.join("runtime/v1/host.sock");
    let fake_codex = data.join("fake-codex");
    fs::write(&fake_codex, "#!/bin/sh\nexec sleep 30\n").unwrap();
    fs::set_permissions(&fake_codex, fs::Permissions::from_mode(0o700)).unwrap();
    let mut broker = Command::new(env!("CARGO_BIN_EXE_xd-host"))
        .args(["broker", "--data"])
        .arg(&data)
        .env("XD_CODEX_EXECUTABLE", &fake_codex)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_path(&socket, &mut broker);

    let mut first = HostClient::connect(&data);
    let folder = first.request(json!({"op": "new-folder", "name": "Project"}))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let chat = first.request(json!({
        "op": "new-chat",
        "folder": folder,
        "backend": "codex",
    }))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        first.request(json!({
            "op": "send",
            "chat": chat,
            "text": "keep running",
            "worktree_name": "persistent-turn",
        }))["ok"],
        true,
    );
    let opened = first.request(json!({
        "op": "terminal-open",
        "chat": "global:test",
        "columns": 80,
        "rows": 24,
    }));
    assert_eq!(opened["ok"], true);
    let terminal = opened["id"].as_str().unwrap().to_owned();
    first.disconnect_abruptly();

    let mut second = HostClient::connect(&data);
    let resumed_chat = second.request(json!({"op": "chat", "chat": chat}));
    assert_eq!(
        resumed_chat["working"], true,
        "turn stopped with its client"
    );
    assert!(
        resumed_chat["turn_id"].is_number(),
        "live turn was not reattached"
    );
    let listed = second.request(json!({"op": "terminal-list", "chat": "global:test"}));
    assert!(
        listed["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["id"] == terminal),
        "terminal disappeared with its stdio client: {listed}",
    );
    assert_eq!(
        second.request(json!({"op": "cancel", "chat": chat}))["ok"],
        true
    );
    assert_eq!(
        second.request(json!({"op": "terminal-kill", "terminal": terminal}))["ok"],
        true,
    );
    second.disconnect_abruptly();

    broker.kill().unwrap();
    broker.wait().unwrap();
    let _ = fs::remove_dir_all(data);
}

fn fixture(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("xd-host-{name}-{}-{unique}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

fn wait_for_path(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("broker exited before creating {}: {status}", path.display());
        }
        assert!(
            Instant::now() < deadline,
            "broker did not create {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}
