use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::Shutdown,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use xd_transport::{TlsReader, TlsSession, TlsWriter};

struct Daemon {
    root: PathBuf,
    socket: PathBuf,
    child: Child,
}
impl Daemon {
    fn start() -> Self {
        Self::start_with_discovery(false)
    }
    fn start_with_discovery(discover: bool) -> Self {
        let root = std::env::temp_dir().join(format!("xd-daemon-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let fake = root.join("codex");
        fs::write(&fake, r#"#!/usr/bin/env python3
import sys, json, time, pathlib, os
if sys.argv[1:] == ['login', 'status']:
    signed_in = pathlib.Path(os.environ.get('CODEX_HOME', '/nonexistent'), 'signed-in').exists()
    print('Logged in using existing CLI login' if signed_in else 'Not logged in')
    sys.exit(0 if signed_in else 1)
for line in sys.stdin:
    frame = json.loads(line)
    with pathlib.Path(__file__).with_suffix('.requests').open('a') as log:
        log.write(json.dumps({'pid': os.getpid(), 'argv': sys.argv, 'home': os.environ.get('CODEX_HOME'), 'frame': frame}) + '\n')
    method = frame.get('method')
    if method == 'initialize': print(json.dumps({'id':1,'result':{}}), flush=True)
    elif method in ('thread/start','thread/resume'): print(json.dumps({'id':2,'result':{'thread':{'id':'thread-test'}}}), flush=True)
    elif method == 'turn/start':
        print(json.dumps({'id':3,'result':{'turn':{'id':'turn-test'}}}), flush=True)
        prompt = frame['params']['input'][0]['text']
        if prompt == 'approval':
            print(json.dumps({'id':41,'method':'item/commandExecution/requestApproval','params':{'threadId':'thread-test','turnId':'turn-test','itemId':'cmd','command':'echo approved'}}), flush=True)
        elif prompt == 'mixed':
            print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'thread-test','itemId':'msg-before','delta':'before tool'}}), flush=True)
            print(json.dumps({'method':'item/completed','params':{'threadId':'thread-test','item':{'type':'commandExecution','id':'cmd','command':'echo done','aggregatedOutput':'done'}}}), flush=True)
            print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'thread-test','itemId':'msg-after','delta':'after tool'}}), flush=True)
            print(json.dumps({'id':42,'method':'item/commandExecution/requestApproval','params':{'threadId':'thread-test','turnId':'turn-test','itemId':'finish','command':'finish mixed turn'}}), flush=True)
        elif prompt == 'forever':
            time.sleep(30)
        elif prompt == 'wait':
            time.sleep(0.6)
            print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'thread-test','itemId':'msg','delta':'survived disconnect'}}), flush=True)
            print(json.dumps({'method':'turn/completed','params':{'threadId':'thread-test','turn':{'status':'completed'}}}), flush=True)
        else:
            print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'thread-test','itemId':'msg','delta':'hello'}}), flush=True)
            print(json.dumps({'method':'turn/completed','params':{'threadId':'thread-test','turn':{'status':'completed'}}}), flush=True)
    elif frame.get('id') == 42 and 'result' in frame:
        print(json.dumps({'method':'turn/completed','params':{'threadId':'thread-test','turn':{'status':'completed'}}}), flush=True)
    elif frame.get('id') == 41 and 'result' in frame:
        print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'thread-test','itemId':'msg','delta':'approved'}}), flush=True)
        print(json.dumps({'method':'turn/completed','params':{'threadId':'thread-test','turn':{'status':'completed'}}}), flush=True)
"#).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_xd-host"));
        command
            .args(["daemon", "--data"])
            .arg(&root)
            .env("HOME", &root)
            .env("SHELL", "/bin/sh");
        if discover {
            let shell = root.join("login-shell");
            fs::create_dir(root.join("cli-config")).unwrap();
            fs::write(root.join("cli-config/signed-in"), "fixture").unwrap();
            fs::write(&shell, format!("#!/bin/sh\nexport PATH='{}':\"$PATH\"\nexport CODEX_HOME='{}/cli-config'\nexec /bin/sh -c \"$2\"\n", root.display(), root.display())).unwrap();
            fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
            command
                .env_remove("XD_CODEX_EXECUTABLE")
                .env_remove("CODEX_HOME")
                .env("SHELL", shell);
        } else {
            command.env("XD_CODEX_EXECUTABLE", &fake);
        }
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let database = fs::canonicalize(&root).unwrap().join("chats.db");
        let hash = format!(
            "{:x}",
            Sha256::digest(database.as_os_str().as_encoded_bytes())
        );
        let socket = PathBuf::from(format!(
            "/tmp/xd-{}/daemon-{}.sock",
            unsafe { libc::geteuid() },
            &hash[..24]
        ));
        let daemon = Self {
            root,
            socket,
            child,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !daemon.socket.exists() {
            assert!(Instant::now() < deadline, "daemon startup timed out");
            thread::sleep(Duration::from_millis(10));
        }
        daemon
    }
    fn client(&self) -> Client<UnixStream, UnixStream> {
        let socket = UnixStream::connect(&self.socket).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Client {
            reader: BufReader::new(socket.try_clone().unwrap()),
            writer: socket,
            next: 1,
        }
    }
    fn remote(
        &self,
        listener: &Value,
        pin: Option<&str>,
    ) -> (Client<TlsReader, TlsWriter>, TlsSession) {
        let (mut reader, writer, session) = xd_transport::connect(
            "127.0.0.1",
            listener["port"].as_u64().unwrap() as u16,
            pin.unwrap_or(listener["fingerprint"].as_str().unwrap()),
        )
        .unwrap();
        reader.set_timeout(Some(Duration::from_secs(5)));
        (
            Client {
                reader: BufReader::new(reader),
                writer,
                next: 1,
            },
            session,
        )
    }
    fn chat(&self, client: &mut Client<UnixStream, UnixStream>) -> String {
        let folder = client.call(json!({"op":"new-folder", "name":"Project"}));
        assert_eq!(folder["ok"], true, "{folder}");
        let chat = client.call(json!({"op":"new-chat", "folder":folder["id"], "backend":"codex"}));
        assert_eq!(chat["ok"], true, "{chat}");
        chat["id"].as_str().unwrap().to_owned()
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(mut stream) = UnixStream::connect(&self.socket) {
            let _ = stream.write_all(b"{\"op\":\"daemon-stop\"}\n");
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.socket.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Client<R, W> {
    reader: BufReader<R>,
    writer: W,
    next: u64,
}
impl<R: Read, W: Write> Client<R, W> {
    fn call(&mut self, mut request: Value) -> Value {
        let id = self.next;
        self.next += 1;
        request["_xd_request"] = json!(id);
        writeln!(self.writer, "{request}").unwrap();
        self.writer.flush().unwrap();
        loop {
            let frame = self.frame();
            if frame["_xd_request"] == id {
                return frame;
            }
        }
    }
    /// Wait for observable host state instead of assuming a process scheduling delay.
    fn wait_for(&mut self, request: Value, ready: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let reply = self.call(request.clone());
            if ready(&reply) {
                return reply;
            }
            assert!(
                Instant::now() < deadline,
                "state did not become ready: {reply}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn frame(&mut self) -> Value {
        let mut line = String::new();
        assert!(
            self.reader.read_line(&mut line).unwrap() > 0,
            "daemon disconnected"
        );
        serde_json::from_str(&line).unwrap()
    }
    fn event(&mut self, name: &str) -> Value {
        loop {
            let frame = self.frame();
            if frame["event"] == name {
                return frame;
            }
        }
    }
}

#[test]
fn one_daemon_shares_local_and_paired_sessions_and_revokes_immediately() {
    let daemon = Daemon::start();
    let mut local = daemon.client();
    let info = local.call(json!({"op":"daemon-info"}));
    assert_eq!(info["pid"], daemon.child.id());
    let chat = daemon.chat(&mut local);
    assert_eq!(
        local.call(json!({"op":"chat", "chat":chat}))["session_mode"],
        "chat"
    );
    let pairing = local.call(json!({"op":"peer-pairing", "bind":"127.0.0.1", "port":0}));
    assert_eq!(pairing["ok"], true, "{pairing}");
    assert!(
        xd_transport::connect(
            "127.0.0.1",
            pairing["port"].as_u64().unwrap() as u16,
            &"00".repeat(32)
        )
        .is_err()
    );
    let (mut remote, session) = daemon.remote(&pairing, None);
    assert_eq!(remote.call(json!({"op":"tree"}))["ok"], false);
    let paired = remote.call(json!({"op":"pair", "code":pairing["code"], "name":"Test device"}));
    assert_eq!(paired["ok"], true, "{paired}");
    assert_eq!(remote.call(json!({"op":"tree"}))["chats"][0]["id"], chat);
    assert_eq!(remote.call(json!({"op":"devices"}))["ok"], false);
    assert_eq!(remote.call(json!({"op":"daemon-stop"}))["ok"], false);
    drop(remote);
    drop(session);
    let (mut resumed, _session) = daemon.remote(&pairing, None);
    assert_eq!(
        resumed.call(json!({"op":"hello", "token":paired["token"]}))["ok"],
        true
    );
    let (mut replay, _replay_session) = daemon.remote(&pairing, None);
    assert_eq!(
        replay.call(json!({"op":"pair", "code":pairing["code"], "name":"Replay"}))["ok"],
        false
    );
    let devices = local.call(json!({"op":"devices"}));
    assert_eq!(devices["devices"][0]["connected"], true);
    assert_eq!(
        local.call(json!({"op":"revoke-device", "device":devices["devices"][0]["id"]}))["ok"],
        true
    );
    let mut line = String::new();
    assert_eq!(resumed.reader.read_line(&mut line).unwrap(), 0);
}

#[test]
fn update_restart_protects_open_shells() {
    let daemon = Daemon::start();
    let mut client = daemon.client();
    let chat = daemon.chat(&mut client);
    let opened = client
        .call(json!({"op":"terminal-open", "chat":chat, "columns":80, "rows":24, "reuse":false}));
    assert_eq!(opened["ok"], true, "{opened}");
    let reply = client.call(json!({"op":"host-update", "action":"restart"}));
    assert_eq!(reply["ok"], false);
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("Close terminal tabs")
    );
    assert_eq!(
        client.call(json!({"op":"terminal-kill", "terminal":opened["id"]}))["ok"],
        true
    );
}

#[cfg(target_os = "linux")]
#[test]
fn installed_cli_reuses_sessions_and_resumes_after_process_loss() {
    let daemon = Daemon::start_with_discovery(true);
    let mut client = daemon.client();
    let chat = daemon.chat(&mut client);
    client.wait_for(json!({"op":"chat", "chat":chat}), |reply| {
        reply["auth_state"] == "signed-in"
    });
    let send = |client: &mut Client<UnixStream, UnixStream>| {
        assert_eq!(
            client.call(json!({"op":"send", "chat":chat, "text":"hello"}))["ok"],
            true
        );
        client.wait_for(json!({"op":"chat", "chat":chat}), |reply| {
            reply["working"] == false
        });
    };
    let requests = || -> Vec<Value> {
        fs::read_to_string(daemon.root.join("codex.requests"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };
    send(&mut client);
    send(&mut client);
    let frames = requests();
    assert_eq!(
        frames[0]["home"],
        daemon.root.join("cli-config").to_str().unwrap()
    );
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame["frame"]["method"] == "initialize")
            .count(),
        1
    );
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame["frame"]["method"] == "turn/start")
            .count(),
        2
    );
    let pid = frames[0]["pid"].as_i64().unwrap() as i32;
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| !stat.contains(") Z")) {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    send(&mut client);
    let frames = requests();
    let resumed = frames
        .iter()
        .find(|frame| frame["frame"]["method"] == "thread/resume")
        .unwrap();
    assert_eq!(resumed["frame"]["params"]["threadId"], "thread-test");
    assert_ne!(resumed["pid"], pid);
    assert_eq!(
        client.call(json!({"op":"set-option", "chat":chat, "option":"fast", "value":"true"}))["ok"],
        true
    );
    send(&mut client);
    let frames = requests();
    let launches = frames
        .iter()
        .filter(|frame| frame["frame"]["method"] == "initialize")
        .collect::<Vec<_>>();
    assert_eq!(launches.len(), 3);
    assert!(
        launches.last().unwrap()["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "service_tier=\"priority\"")
    );
    let opened = client.call(json!({"op":"terminal-open-agent", "chat":chat,
        "agent":"codex", "columns":80, "rows":24, "reuse":false}));
    assert_eq!(opened["ok"], true, "{opened}");
    assert_eq!(
        client.call(json!({"op":"terminal-kill", "terminal":opened["id"]}))["ok"],
        true
    );
}

#[test]
fn agent_turn_and_pending_approval_survive_client_disconnects() {
    let daemon = Daemon::start();
    let mut client = daemon.client();
    let chat = daemon.chat(&mut client);
    assert_eq!(
        client.call(json!({"op":"send", "chat":chat, "text":"wait"}))["ok"],
        true
    );
    client.writer.shutdown(Shutdown::Both).unwrap();
    drop(client);
    let mut client = daemon.client();
    let history = client.wait_for(json!({"op":"messages", "chat":chat}), |reply| {
        reply["messages"].as_array().is_some_and(|messages| {
            messages
                .iter()
                .any(|message| message["content"] == "survived disconnect")
        })
    });
    assert!(
        history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"] == "survived disconnect"),
        "{history}"
    );
    assert_eq!(
        client.call(json!({"op":"send", "chat":chat, "text":"approval"}))["ok"],
        true
    );
    let interaction = client.event("interaction");
    client.writer.shutdown(Shutdown::Both).unwrap();
    drop(client);
    let mut client = daemon.client();
    let snapshot = client.call(json!({"op":"chat", "chat":chat}));
    assert_eq!(
        snapshot["interactions"][0]["request_id"],
        interaction["request_id"]
    );
    assert_eq!(client.call(json!({"op":"respond", "chat":chat,"request_id":interaction["request_id"], "response":{"decision":"accept"}}))["ok"], true);
    assert_eq!(client.call(json!({"op":"respond", "chat":chat,"request_id":interaction["request_id"], "response":{"decision":"accept"}}))["ok"], false);
    let history = client.wait_for(json!({"op":"messages", "chat":chat}), |reply| {
        reply["messages"].as_array().is_some_and(|messages| {
            messages
                .iter()
                .any(|message| message["content"] == "approved")
        })
    });
    assert!(
        history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"] == "approved"),
        "{history}"
    );
}

#[test]
fn bridges_autostart_one_daemon_and_preserve_pairing_after_restart() {
    let mut daemon = Daemon::start();
    let mut local = daemon.client();
    let chat = daemon.chat(&mut local);
    let pairing = local.call(json!({"op":"peer-pairing","bind":"127.0.0.1","port":0}));
    let (mut remote, session) = daemon.remote(&pairing, None);
    let paired =
        remote.call(json!({"op":"pair","code":pairing["code"],"name":"Remembered device"}));
    assert_eq!(paired["ok"], true);
    drop(remote);
    drop(session);
    drop(local);
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    let root = daemon.root.clone();
    let bridges = (0..2)
        .map(|_| {
            let root = root.clone();
            thread::spawn(move || {
                let mut child = Command::new(env!("CARGO_BIN_EXE_xd-host"))
                    .args(["stdio", "--data"])
                    .arg(&root)
                    .env("XD_CODEX_EXECUTABLE", root.join("codex"))
                    .env("HOME", &root)
                    .env("SHELL", "/bin/sh")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                let mut client = Client {
                    reader: BufReader::new(child.stdout.take().unwrap()),
                    writer: child.stdin.take().unwrap(),
                    next: 1,
                };
                let reply = client.call(json!({"op":"daemon-info"}));
                assert_eq!(reply["ok"], true, "{reply}");
                child.kill().unwrap();
                child.wait().unwrap();
                reply["pid"].as_u64().unwrap()
            })
        })
        .collect::<Vec<_>>();
    let pids = bridges
        .into_iter()
        .map(|b| b.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(pids[0], pids[1]);
    let mut local = daemon.client();
    assert_eq!(local.call(json!({"op":"tree"}))["chats"][0]["id"], chat);
    let (mut remote, _session) = daemon.remote(&pairing, None);
    assert_eq!(
        remote.call(json!({"op":"hello","token":paired["token"]}))["ok"],
        true
    );
    assert_eq!(
        remote.call(json!({"op":"send","chat":chat,"text":"forever"}))["ok"],
        true
    );
    let start = Instant::now();
    assert_eq!(remote.call(json!({"op":"cancel","chat":chat}))["ok"], true);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn snapshots_preserve_text_tool_order_without_replaying_a_committed_segment() {
    let daemon = Daemon::start();
    let mut client = daemon.client();
    let chat = daemon.chat(&mut client);
    assert_eq!(
        client.call(json!({"op":"send","chat":chat,"text":"mixed"}))["ok"],
        true
    );
    let snapshot = client.wait_for(json!({"op":"chat","chat":chat}), |reply| {
        reply["segment"] == "after tool"
            && reply["interactions"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
    });
    assert_eq!(snapshot["segment"], "after tool");
    let active_history = client.call(json!({"op":"messages","chat":chat}));
    assert_eq!(
        active_history["messages"].as_array().unwrap().len(),
        1,
        "live rows must come from the snapshot until the turn ends"
    );
    assert!(
        snapshot["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["tool"] == false && item["text"] == "before tool")
    );
    assert_eq!(
        client.call(json!({"op":"respond", "chat":chat,
            "request_id":snapshot["interactions"][0]["request_id"],
            "response":{"decision":"accept"}}))["ok"],
        true
    );
    let history = client.wait_for(json!({"op":"messages","chat":chat}), |reply| {
        reply["messages"].as_array().is_some_and(|messages| {
            messages
                .iter()
                .any(|message| message["content"] == "after tool")
        })
    });
    let messages = history["messages"].as_array().unwrap();
    let before = messages
        .iter()
        .position(|message| message["content"] == "before tool")
        .unwrap();
    let tool = messages
        .iter()
        .position(|message| message["role"] == "tool")
        .unwrap();
    let after = messages
        .iter()
        .position(|message| message["content"] == "after tool")
        .unwrap();
    assert!(before < tool && tool < after);
}

#[test]
fn shell_panes_are_shared_and_outlive_their_client() {
    let daemon = Daemon::start();
    let mut first = daemon.client();
    let opened = first.call(json!({"op":"terminal-open","chat":"global:daemon-test","columns":80,"rows":24,"reuse":true}));
    assert_eq!(opened["ok"], true, "{opened}");
    let id = opened["id"].as_str().unwrap().to_owned();
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    assert_eq!(first.call(json!({"op":"terminal-input","terminal":id,"data":STANDARD.encode(b"printf 'owned-by-daemon\n'\n")}))["ok"],true);
    first.writer.shutdown(Shutdown::Both).unwrap();
    drop(first);
    thread::sleep(Duration::from_millis(100));
    let mut second = daemon.client();
    let sessions = second.call(json!({"op":"terminal-list","chat":"global:daemon-test"}));
    assert_eq!(sessions["terminals"][0]["id"], id, "{sessions}");
    let reused = second.call(json!({"op":"terminal-open","chat":"global:daemon-test","columns":80,"rows":24,"reuse":true}));
    assert_eq!(reused["id"], id);
}
