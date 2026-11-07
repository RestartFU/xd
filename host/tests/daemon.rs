use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use xd_transport::{TlsReader, TlsSession, TlsWriter};

struct Daemon {
    root: PathBuf,
    socket: PathBuf,
    child: Child,
}
/// Where a daemon's stderr goes, so a failing test can show why it died.
fn daemon_log(root: &Path) -> Stdio {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("daemon-stderr.log"))
        .unwrap()
        .into()
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
if sys.argv[1:2] == ['exec']:
    with pathlib.Path(__file__).with_suffix('.titles').open('a') as log:
        log.write(json.dumps(sys.argv) + '\n')
    print(json.dumps({'type':'thread.started','thread_id':'title-thread'}), flush=True)
    print(json.dumps({'type':'item.completed','item':{'id':'item','type':'agent_message','text':json.dumps({'title':'Generated title'})}}), flush=True)
    print(json.dumps({'type':'turn.completed','usage':{}}), flush=True)
    sys.exit(0)
for line in sys.stdin:
    frame = json.loads(line)
    with pathlib.Path(__file__).with_suffix('.requests').open('a') as log:
        log.write(json.dumps({'pid': os.getpid(), 'argv': sys.argv, 'home': os.environ.get('CODEX_HOME'), 'frame': frame}) + '\n')
    method = frame.get('method')
    if method == 'initialize': print(json.dumps({'id':1,'result':{}}), flush=True)
    elif method == 'model/list':
        time.sleep(0.3)
        print(json.dumps({'id':2,'result':{'data':[{'model':'cli-default','isDefault':True,'defaultReasoningEffort':'low'},{'model':'gpt-5.5','defaultReasoningEffort':'medium'}]}}), flush=True)
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
        command.env("XD_DISABLE_AUTO_UPDATE", "1");
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
            .stderr(daemon_log(&root))
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
    /// Starts a new daemon on the same data after this one was stopped.
    fn restart(&mut self) {
        self.child = Command::new(env!("CARGO_BIN_EXE_xd-host"))
            .env("XD_DISABLE_AUTO_UPDATE", "1")
            .args(["daemon", "--data"])
            .arg(&self.root)
            .env("XD_CODEX_EXECUTABLE", self.root.join("codex"))
            .env("HOME", &self.root)
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(daemon_log(&self.root))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&self.socket).is_err() {
            assert!(Instant::now() < deadline, "daemon restart timed out");
            thread::sleep(Duration::from_millis(10));
        }
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
        if thread::panicking() {
            eprintln!("daemon exit status: {:?}", self.child.try_wait());
            eprintln!(
                "daemon stderr:\n{}",
                fs::read_to_string(self.root.join("daemon-stderr.log")).unwrap_or_default()
            );
        }
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
    #[track_caller]
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
    #[track_caller]
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
    #[track_caller]
    fn frame(&mut self) -> Value {
        let mut line = String::new();
        assert!(
            self.reader.read_line(&mut line).unwrap() > 0,
            "daemon disconnected"
        );
        serde_json::from_str(&line).unwrap()
    }
    #[track_caller]
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
fn reports_assistant_updates_and_refuses_unknown_requests() {
    let daemon = Daemon::start();
    let mut client = daemon.client();
    let reply = client.call(json!({"op":"agent-cli-updates"}));
    assert_eq!(reply["ok"], true, "{reply}");
    let providers = reply["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 2);
    assert!(
        providers
            .iter()
            .all(|provider| { provider["state"] == "idle" && provider["available"] == false })
    );
    let refused =
        client.call(json!({"op":"agent-cli-updates", "action":"install", "provider":"jcode"}));
    assert_eq!(refused["ok"], false, "{refused}");
    let refused = client.call(json!({"op":"agent-cli-updates", "action":"nope"}));
    assert_eq!(refused["ok"], false, "{refused}");
}

#[test]
fn a_client_that_leaves_at_once_does_not_stop_the_daemon() {
    let daemon = Daemon::start();
    for _ in 0..20 {
        drop(UnixStream::connect(&daemon.socket).unwrap());
    }
    // Long enough for the accept loop to reach every closed connection.
    thread::sleep(Duration::from_millis(200));
    let reply = daemon.client().call(json!({"op":"ping"}));
    assert_eq!(reply["ok"], true, "{reply}");
}

#[test]
fn a_cut_short_turn_can_be_resumed_exactly_once() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client();
    let chat = daemon.chat(&mut client);
    let resumable = |client: &mut Client<UnixStream, UnixStream>| {
        client.call(json!({"op":"chat", "chat":chat}))["resumable"] == true
    };
    let continues = |client: &mut Client<UnixStream, UnixStream>| {
        client.call(json!({"op":"messages", "chat":chat}))["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message["role"] == "user" && message["content"] == "Continue where you left off."
            })
            .count()
    };
    let idle = |client: &mut Client<UnixStream, UnixStream>| {
        client.wait_for(json!({"op":"chat", "chat":chat}), |reply| {
            reply["working"] == false
        });
    };

    // A turn that ran to its end has nothing to resume.
    let reply = client.call(json!({"op":"send", "chat":chat, "text":"hello"}));
    assert_eq!(reply["ok"], true, "{reply}");
    idle(&mut client);
    assert!(!resumable(&mut client));
    let refused = client.call(json!({"op":"resume-turn", "chat":chat}));
    assert_eq!(refused["ok"], false, "{refused}");

    // Stop offers it, and a running turn does not.
    let reply = client.call(json!({"op":"send", "chat":chat, "text":"forever"}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(!resumable(&mut client));
    assert_eq!(client.call(json!({"op":"cancel", "chat":chat}))["ok"], true);
    idle(&mut client);
    assert!(resumable(&mut client));

    // Resuming spends the offer, so a second client pressing it adds nothing.
    let reply = client.call(json!({"op":"resume-turn", "chat":chat}));
    assert_eq!(reply["queued"], false, "{reply}");
    let refused = client.call(json!({"op":"resume-turn", "chat":chat}));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(continues(&mut client), 1);
    idle(&mut client);
    assert!(!resumable(&mut client));
    assert_eq!(continues(&mut client), 1);

    // A crash offers it too, and the offer survives the restart.
    let reply = client.call(json!({"op":"send", "chat":chat, "text":"forever"}));
    assert_eq!(reply["ok"], true, "{reply}");
    drop(client);
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    daemon.restart();
    let mut client = daemon.client();
    idle(&mut client);
    assert!(resumable(&mut client));
    // Typing a message instead moves the chat on.
    let reply = client.call(json!({"op":"send", "chat":chat, "text":"hello"}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(!resumable(&mut client));
    idle(&mut client);
    assert!(!resumable(&mut client));
}

#[test]
fn a_restart_continues_cut_off_turns_only_when_asked() {
    let continued = |client: &mut Client<UnixStream, UnixStream>, chat: &str| {
        client.call(json!({"op":"messages", "chat":chat}))["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["role"] == "user"
                    && message["content"]
                        .as_str()
                        .unwrap()
                        .starts_with("Continue where")
            })
    };
    let mut daemon = Daemon::start();
    let mut client = daemon.client();

    // A crash leaves the turn looking as if it were still running.
    let crashed = daemon.chat(&mut client);
    let handled = daemon.chat(&mut client);
    for chat in [&crashed, &handled] {
        let reply = client.call(json!({"op":"send", "chat":chat, "text":"forever"}));
        assert_eq!(reply["ok"], true, "{reply}");
    }
    let draft = client.call(json!({"op":"set-draft", "chat":crashed, "text":"unsent"}));
    assert_eq!(draft["ok"], true, "{draft}");
    drop(client);
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    // A second restart before the desktop answers keeps them waiting.
    daemon.restart();
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    daemon.restart();
    let mut client = daemon.client();
    // Picked up by another client first, so it is not continued a second time.
    assert_eq!(
        client.call(json!({"op":"send", "chat":handled, "text":"hello"}))["ok"],
        true
    );
    let reply = client.call(json!({"op":"resume-interrupted", "resume":true}));
    assert_eq!(reply["resumed"], json!([crashed]), "{reply}");
    assert!(!continued(&mut client, &handled));
    assert_eq!(
        client.call(json!({"op":"chat", "chat":crashed}))["draft"],
        "unsent"
    );
    assert!(continued(&mut client, &crashed));
    let again = client.call(json!({"op":"resume-interrupted", "resume":true}));
    assert_eq!(again["resumed"], json!([]), "{again}");

    // A clean stop ends the turn first, but it still never finished.
    client.wait_for(json!({"op":"chat", "chat":crashed}), |reply| {
        reply["working"] == false
    });
    let stopped = daemon.chat(&mut client);
    assert_eq!(
        client.call(json!({"op":"send", "chat":stopped, "text":"forever"}))["ok"],
        true
    );
    assert_eq!(client.call(json!({"op":"daemon-stop"}))["ok"], true);
    drop(client);
    daemon.child.wait().unwrap();
    daemon.restart();
    let mut client = daemon.client();
    let reply = client.call(json!({"op":"resume-interrupted", "resume":true}));
    assert_eq!(reply["resumed"], json!([stopped]), "{reply}");
    assert!(continued(&mut client, &stopped));

    // With the setting off the interruption is dropped, and asking later does nothing.
    client.wait_for(json!({"op":"chat", "chat":stopped}), |reply| {
        reply["working"] == false
    });
    let declined = daemon.chat(&mut client);
    assert_eq!(
        client.call(json!({"op":"send", "chat":declined, "text":"forever"}))["ok"],
        true
    );
    drop(client);
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    daemon.restart();
    let mut client = daemon.client();
    let reply = client.call(json!({"op":"resume-interrupted", "resume":false}));
    assert_eq!(reply["resumed"], json!([]), "{reply}");
    let later = client.call(json!({"op":"resume-interrupted", "resume":true}));
    assert_eq!(later["resumed"], json!([]), "{later}");
    assert!(!continued(&mut client, &declined));
}

#[test]
fn a_paired_device_continues_turns_a_restart_cut_off() {
    let mut daemon = Daemon::start();
    let mut local = daemon.client();
    let chat = daemon.chat(&mut local);
    let pairing = local.call(json!({"op":"peer-pairing", "bind":"127.0.0.1", "port":0}));
    assert_eq!(pairing["ok"], true, "{pairing}");
    let (mut remote, session) = daemon.remote(&pairing, None);

    // Before it authenticates, a device has no say.
    let refused = remote.call(json!({"op":"resume-interrupted", "resume":true}));
    assert_eq!(refused["ok"], false, "{refused}");
    let paired = remote.call(json!({"op":"pair", "code":pairing["code"], "name":"Test device"}));
    assert_eq!(paired["ok"], true, "{paired}");
    let sent = remote.call(json!({"op":"send", "chat":chat, "text":"forever"}));
    assert_eq!(sent["ok"], true, "{sent}");
    drop(remote);
    drop(session);
    drop(local);
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    daemon.restart();

    // The daemon restores its listener before it starts serving the socket.
    assert_eq!(daemon.client().call(json!({"op":"ping"}))["ok"], true);
    let (mut remote, _session) = daemon.remote(&pairing, None);
    let hello = remote.call(json!({"op":"hello", "token":paired["token"]}));
    assert_eq!(hello["ok"], true, "{hello}");
    let reply = remote.call(json!({"op":"resume-interrupted", "resume":true}));
    assert_eq!(reply["resumed"], json!([chat]), "{reply}");
    let messages = remote.call(json!({"op":"messages", "chat":chat}));
    assert!(
        messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "user"
                && message["content"]
                    .as_str()
                    .unwrap()
                    .starts_with("Continue where")),
        "{messages}"
    );
    // Answered once, so another device connecting later does nothing.
    let again = remote.call(json!({"op":"resume-interrupted", "resume":true}));
    assert_eq!(again["resumed"], json!([]), "{again}");
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
    // Only processes that opened a thread ran turns: the host also starts short
    // ones to ask the assistant for its models.
    let requests = || -> Vec<Value> {
        let frames: Vec<Value> = fs::read_to_string(daemon.root.join("codex.requests"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let turn_processes: Vec<Value> = frames
            .iter()
            .filter(|frame| {
                matches!(
                    frame["frame"]["method"].as_str(),
                    Some("thread/start" | "thread/resume")
                )
            })
            .map(|frame| frame["pid"].clone())
            .collect();
        frames
            .into_iter()
            .filter(|frame| turn_processes.contains(&frame["pid"]))
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
}

#[test]
fn a_first_message_names_the_chat_and_later_ones_do_not() {
    let daemon = Daemon::start();
    let mut client = daemon.client();
    let chat = daemon.chat(&mut client);
    let settings = client.call(json!({
        "op":"set-chat-title-settings", "backend":"codex", "model":"gpt-5.5",
    }));
    assert_eq!(settings["models"]["codex"], "gpt-5.5", "{settings}");

    assert_eq!(
        client.call(json!({"op":"send", "chat":chat, "text":"fix the scroll"}))["ok"],
        true
    );
    let named = client.wait_for(json!({"op":"chat", "chat":chat}), |reply| {
        reply["title"] == "Generated title" && reply["working"] == false
    });
    assert_eq!(named["session_title"], "Generated title");

    let titles = fs::read_to_string(daemon.root.join("codex.titles")).unwrap();
    let runs = titles
        .lines()
        .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(runs.len(), 1, "{titles}");
    assert!(
        runs[0].windows(2).any(|pair| pair == ["-m", "gpt-5.5"]),
        "{titles}"
    );
    assert!(runs[0].contains(&"--ephemeral".to_owned()), "{titles}");
    assert!(
        runs[0]
            .iter()
            .any(|argument| argument.contains("fix the scroll")),
        "{titles}"
    );

    assert_eq!(
        client.call(json!({"op":"send", "chat":chat, "text":"and the header"}))["ok"],
        true
    );
    client.wait_for(json!({"op":"chat", "chat":chat}), |reply| {
        reply["working"] == false
    });
    let titles = fs::read_to_string(daemon.root.join("codex.titles")).unwrap();
    assert_eq!(titles.lines().count(), 1, "{titles}");

    let settings = client.call(json!({"op":"set-chat-title-settings", "enabled":false}));
    assert_eq!(settings["enabled"], false, "{settings}");
    assert_eq!(
        client.call(json!({"op":"chat-title-settings"}))["enabled"],
        false
    );
}

#[test]
fn a_first_turn_waits_for_the_default_the_assistant_reports() {
    let daemon = Daemon::start();
    let mut client = daemon.client();
    // Nothing has been discovered yet, and the send races the probe this starts.
    let chat = daemon.chat(&mut client);
    assert_eq!(
        client.call(json!({"op":"send", "chat":chat, "text":"hello"}))["ok"],
        true
    );
    client.wait_for(json!({"op":"chat", "chat":chat}), |reply| {
        reply["working"] == false
    });
    let requests = fs::read_to_string(daemon.root.join("codex.requests")).unwrap();
    let thread = requests
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|frame| frame["frame"]["method"] == "thread/start")
        .unwrap();
    assert_eq!(thread["frame"]["params"]["model"], "cli-default");
}

#[test]
fn a_first_turn_leaves_a_remembered_model_the_assistant_has_retired() {
    let daemon = Daemon::start();
    let mut client = daemon.client();
    let earlier = daemon.chat(&mut client);
    // Built in, so it is accepted, but the assistant does not offer it.
    assert_eq!(
        client.call(json!({"op":"set-option", "chat":earlier, "option":"model",
            "value":"gpt-5.3-codex-spark"}))["ok"],
        true
    );
    assert_eq!(
        client.call(json!({"op":"set-option", "chat":earlier, "option":"effort",
            "value":"high"}))["ok"],
        true
    );
    // The earlier chat's probe has not answered yet, so the pick is not checked.
    let chat = daemon.chat(&mut client);
    assert_eq!(
        client.call(json!({"op":"send", "chat":chat, "text":"hello"}))["ok"],
        true
    );
    client.wait_for(json!({"op":"chat", "chat":chat}), |reply| {
        reply["working"] == false
    });
    let requests = fs::read_to_string(daemon.root.join("codex.requests")).unwrap();
    let thread = requests
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|frame| frame["frame"]["method"] == "thread/start")
        .unwrap();
    assert_eq!(thread["frame"]["params"]["model"], "cli-default");
}

#[test]
fn a_first_turn_waits_for_the_default_effort_of_a_model_already_chosen() {
    let daemon = Daemon::start();
    let mut client = daemon.client();
    let folder = client.call(json!({"op":"new-folder", "name":"Project"}));
    // A built-in model is accepted and pinned without anything being discovered.
    let settings = client.call(json!({"op":"set-folder-settings", "folder":folder["id"],
        "backend":null, "model":"gpt-5.5", "workdir":null, "repo":null}));
    assert_eq!(settings["ok"], true, "{settings}");
    let chat = client.call(json!({"op":"new-chat", "folder":folder["id"]}));
    assert_eq!(chat["ok"], true, "{chat}");
    let chat = chat["id"].as_str().unwrap();
    assert_eq!(
        client.call(json!({"op":"send", "chat":chat, "text":"hello"}))["ok"],
        true
    );
    client.wait_for(json!({"op":"chat", "chat":chat}), |reply| {
        reply["working"] == false
    });
    let requests = fs::read_to_string(daemon.root.join("codex.requests")).unwrap();
    let turn = requests
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|frame| frame["frame"]["method"] == "turn/start")
        .unwrap();
    assert_eq!(turn["frame"]["params"]["effort"], "medium");
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
                    .env("XD_DISABLE_AUTO_UPDATE", "1")
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

/// Stands in for the previous build's daemon, which a test binary cannot be.
struct StaleDaemon {
    root: PathBuf,
    socket: PathBuf,
    requests: Arc<Mutex<Vec<Value>>>,
    info: Arc<Mutex<Value>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl StaleDaemon {
    fn start(allow_restart: bool) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("xd-stale-{}", uuid::Uuid::new_v4()));
        let data = root.join("data");
        fs::create_dir_all(data.join("Workspaces")).unwrap();
        let data = fs::canonicalize(data).unwrap();
        let digest = format!(
            "{:x}",
            Sha256::digest(data.join("chats.db").as_os_str().as_encoded_bytes())
        );
        let runtime = PathBuf::from(format!("/tmp/xd-{}", unsafe { libc::geteuid() }));
        fs::create_dir_all(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = runtime.join(format!("daemon-{}.sock", &digest[..24]));
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let info = Arc::new(Mutex::new(json!({
            "ok": true,
            "version": "0.0.0-stale",
            "protocol": 2,
            "build": "stale",
            "pid": 1,
            "workspaces": data.join("Workspaces"),
        })));
        let thread = {
            let (requests, stop, socket) = (requests.clone(), stop.clone(), socket.clone());
            let info = info.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let Ok((stream, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    let mut writer = stream.try_clone().unwrap();
                    for line in BufReader::new(stream).lines() {
                        let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
                        requests.lock().unwrap().push(request.clone());
                        let restart = request["action"] == "restart";
                        let mut reply = match request["op"].as_str() {
                            Some("daemon-info") => info.lock().unwrap().clone(),
                            Some("host-update") if restart && !allow_restart => {
                                json!({"ok": false, "error": "Close terminal tabs"})
                            }
                            Some("host-update") => {
                                json!({"ok": true, "supported": true, "state": "idle"})
                            }
                            _ => json!({"ok": false}),
                        };
                        reply["_xd_request"] = request["_xd_request"].clone();
                        writeln!(writer, "{reply}").unwrap();
                        if restart && allow_restart {
                            stop.store(true, Ordering::Release);
                            let _ = fs::remove_file(&socket);
                            return;
                        }
                    }
                }
            })
        };
        Self {
            root,
            socket,
            requests,
            info,
            stop,
            thread: Some(thread),
        }
    }
    fn report(&self, field: &str, value: Value) {
        self.info.lock().unwrap()[field] = value;
    }
    /// A host binary laid out the way the installer lays it out.
    fn install_host(&self) -> PathBuf {
        let libexec = self.root.join(".local/opt/xd/libexec");
        fs::create_dir_all(&libexec).unwrap();
        let installer = libexec.join("install.sh");
        fs::write(&installer, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&installer, fs::Permissions::from_mode(0o755)).unwrap();
        let host = libexec.join("xd-host");
        // A link cannot hit ETXTBSY from a copy still open during another fork.
        fs::hard_link(env!("CARGO_BIN_EXE_xd-host"), &host).unwrap();
        fs::canonicalize(host).unwrap()
    }
    fn bridge(&self, host: &Path) -> Child {
        Command::new(host)
            .args(["stdio", "--data"])
            .arg(self.root.join("data"))
            .env("HOME", &self.root)
            .env("SHELL", "/bin/sh")
            .env("XD_DISABLE_AUTO_UPDATE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
    fn rejection(&self, host: &Path) -> String {
        let output = self.bridge(host).wait_with_output().unwrap();
        assert!(!output.status.success());
        String::from_utf8(output.stderr).unwrap()
    }
    fn restart_requests(&self) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["action"] == "restart")
            .count()
    }
}
impl Drop for StaleDaemon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.thread.take().unwrap().join();
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assert_bridge_replaces(stale: &StaleDaemon, host: &Path) {
    let mut bridge = stale.bridge(host);
    let mut stdin = bridge.stdin.take().unwrap();
    let mut stdout = BufReader::new(bridge.stdout.take().unwrap());
    writeln!(stdin, "{}", json!({"op":"daemon-info","_xd_request":1})).unwrap();
    let info = loop {
        let mut line = String::new();
        if stdout.read_line(&mut line).unwrap() == 0 {
            let mut error = String::new();
            let mut stderr = bridge.stderr.take().unwrap();
            stderr.read_to_string(&mut error).unwrap();
            panic!("bridge closed: {error}");
        }
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["_xd_request"] == 1 {
            break frame;
        }
    };
    assert_eq!(stale.restart_requests(), 1);
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"), "{info}");
    assert_eq!(info["executable"], host.to_str().unwrap());
    // Only cleanup: this test isn't about the stop acknowledgement.
    writeln!(stdin, "{}", json!({"op":"daemon-stop"})).unwrap();
    bridge.wait().unwrap();
}

#[test]
fn bridge_replaces_an_idle_daemon_left_behind_by_an_update() {
    let stale = StaleDaemon::start(true);
    let host = stale.install_host();
    stale.report("executable", json!(host));
    assert_bridge_replaces(&stale, &host);
}

#[test]
fn bridge_replaces_an_idle_daemon_from_before_executable_was_reported() {
    let stale = StaleDaemon::start(true);
    let host = stale.install_host();
    // A real process started from the installed path, identified only by its
    // command line. Its own database keeps it apart from the bridge's daemon.
    let other = stale.root.join("other");
    fs::create_dir_all(other.join("Workspaces")).unwrap();
    let mut legacy = Command::new(&host)
        .arg("daemon")
        .arg("--database")
        .arg(other.join("chats.db"))
        .arg("--workspaces")
        .arg(other.join("Workspaces"))
        .env("HOME", &stale.root)
        .env("XD_DISABLE_AUTO_UPDATE", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    stale.report("pid", json!(legacy.id()));
    assert_bridge_replaces(&stale, &host);
    legacy.kill().unwrap();
    legacy.wait().unwrap();
}

#[test]
fn bridge_keeps_a_busy_daemon_running_and_explains_how_to_stop_it() {
    let stale = StaleDaemon::start(false);
    let host = stale.install_host();
    stale.report("executable", json!(host));
    let error = stale.rejection(&host);
    assert_eq!(stale.restart_requests(), 1);
    assert!(error.contains("chats or terminals are active"), "{error}");
    assert!(error.contains("daemon-stop --data"), "{error}");
}

fn assert_bridge_leaves_running(stale: &StaleDaemon, host: &Path) {
    let error = stale.rejection(host);
    assert_eq!(stale.restart_requests(), 0);
    assert!(error.contains("different xd daemon version"), "{error}");
}

#[test]
fn bridge_never_restarts_a_daemon_it_cannot_replace() {
    // A development build sharing the data directory of an installed daemon.
    let stale = StaleDaemon::start(true);
    let host = stale.install_host();
    stale.report("executable", json!(host));
    assert_bridge_leaves_running(&stale, Path::new(env!("CARGO_BIN_EXE_xd-host")));
    drop(stale);

    // An installed bridge whose daemon came from another install.
    let stale = StaleDaemon::start(true);
    let host = stale.install_host();
    stale.report("executable", json!("/elsewhere/libexec/xd-host"));
    assert_bridge_leaves_running(&stale, &host);
    drop(stale);

    // An older daemon whose command line doesn't name this install (PID 1).
    let stale = StaleDaemon::start(true);
    let host = stale.install_host();
    assert_bridge_leaves_running(&stale, &host);
}

#[test]
fn bridge_never_restarts_a_daemon_serving_other_workspaces() {
    let stale = StaleDaemon::start(true);
    let host = stale.install_host();
    stale.report("executable", json!(host));
    stale.report("workspaces", json!("/elsewhere/Workspaces"));
    let error = stale.rejection(&host);
    assert_eq!(stale.restart_requests(), 0);
    assert!(error.contains("different workspace directory"), "{error}");
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
