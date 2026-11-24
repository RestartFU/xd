//! `xd-host profile`: push this machine's origin profile to a runner on the
//! same machine through its local socket. Paired runners get the same
//! operations over TLS from the desktop; this is the spike's way in.

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{net::UnixStream, process::CommandExt},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{SyncSender, TrySendError, sync_channel},
    },
    thread,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use xd_host::origin_profile::{self, Hosted};

use crate::{HostOptions, daemon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProfileAction {
    Push,
    Pull,
    Status,
    Show,
    /// Push, then stay linked so the runner can run hosted programs here.
    Link,
}

/// Input and output travel in pieces of at most this many bytes.
const CHUNK: usize = 64 * 1024;

/// A large first push carries every skill and plugin in one frame.
const PUSH_TIMEOUT: Duration = Duration::from_secs(600);

pub(crate) fn default_origin() -> String {
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default();
    let name = name
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let name = name.trim_start_matches('.');
    if origin_profile::valid_origin(name) {
        name.to_owned()
    } else {
        "origin".into()
    }
}

pub(crate) fn run(options: HostOptions, action: ProfileAction, origin: &str) -> Result<(), String> {
    match action {
        ProfileAction::Show => {
            let collected = origin_profile::collect_local().map_err(|e| e.to_string())?;
            println!("{}", summary(&collected));
            Ok(())
        }
        ProfileAction::Status => print(request(&options, json!({"op": "origin-profile-status"}))?),
        ProfileAction::Pull => {
            let mut reply = request(&options, json!({"op": "origin-profile-pull", "origin": origin}))?;
            // Sign-ins are listed, never printed.
            if let Some(changed) = reply["changed"].as_array_mut() {
                for file in changed {
                    if let Some(file) = file.as_object_mut() {
                        file.remove("data");
                    }
                }
            }
            print(reply)
        }
        ProfileAction::Push => push(&options, origin).map(|_| ()),
        ProfileAction::Link => link(&options, origin),
    }
}

fn push(options: &HostOptions, origin: &str) -> Result<origin_profile::Collected, String> {
    let collected = origin_profile::collect_local().map_err(|e| e.to_string())?;
    eprintln!("{}", summary(&collected));
    let manifest = collected.manifest();
    let offer = request(
        options,
        json!({"op": "origin-profile-offer", "origin": origin, "manifest": manifest}),
    )?;
    if offer["ok"] != true {
        return Err(offer["error"].as_str().unwrap_or("The runner refused the profile.").into());
    }
    let wanted = offer["missing"]
        .as_array()
        .map(|missing| {
            missing
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let blobs = collected.blobs(&wanted).map_err(|e| e.to_string())?;
    let bytes = collected
        .files
        .iter()
        .filter(|file| wanted.contains(&file.sha256))
        .map(|file| file.size)
        .sum::<u64>();
    eprintln!(
        "sending {} of {} files ({} KiB) to origin {origin}",
        wanted.len(),
        collected.files.len(),
        bytes / 1024
    );
    let reply = request(
        options,
        json!({"op": "origin-profile-push", "origin": origin, "manifest": manifest,
            "blobs": blobs, "activate": true}),
    )?;
    print(reply)?;
    Ok(collected)
}

/// One connection's writing side, shared by the threads that answer on it.
type Writer = Arc<Mutex<UnixStream>>;

fn send(writer: &Writer, body: Value) -> std::io::Result<()> {
    let mut line = body.to_string();
    line.push('\n');
    writer
        .lock()
        .map_err(|_| std::io::Error::other("connection lock"))?
        .write_all(line.as_bytes())
}

/// Pushes the profile, then keeps a link open so the runner can run this
/// origin's hosted MCP servers and hooks here. Reconnects when the runner
/// restarts.
fn link(options: &HostOptions, origin: &str) -> Result<(), String> {
    loop {
        match push(options, origin).and_then(|collected| serve_link(options, origin, collected)) {
            Ok(()) => eprintln!("xd-host: the runner closed the link; reconnecting"),
            Err(error) => eprintln!("xd-host: {error}; retrying"),
        }
        thread::sleep(Duration::from_secs(3));
    }
}

struct Running {
    input: Option<SyncSender<Vec<u8>>>,
    pid: i32,
}

/// Input waiting for a hosted program, in chunks. A program that stops
/// reading is stopped once this many are queued, rather than queuing more.
const QUEUED_INPUT: usize = 64;

fn serve_link(
    options: &HostOptions,
    origin: &str,
    collected: origin_profile::Collected,
) -> Result<(), String> {
    let stream = daemon::connect(options, false, false).map_err(|e| e.to_string())?;
    let writer: Writer = Arc::new(Mutex::new(stream.try_clone().map_err(|e| e.to_string())?));
    send(&writer, json!({"op": "origin-link", "origin": origin, "_xd_request": 1}))
        .map_err(|e| e.to_string())?;
    eprintln!(
        "linked to the runner as {origin}; hosting {} program(s)",
        collected.hosted.len()
    );
    let hosted = Arc::new(collected.hosted);
    let environment = Arc::new(collected.login_env);
    let home = collected.origin_home;
    let running: Arc<Mutex<HashMap<u64, Running>>> = Arc::default();
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("_xd_request") == Some(&json!(1)) && message["ok"] != true {
            return Err(message["error"].as_str().unwrap_or("The runner refused the link.").into());
        }
        let Some(exec) = message["exec"].as_u64() else {
            continue;
        };
        match message["event"].as_str() {
            Some("origin-exec-start") => {
                let command = message["command"].as_str().unwrap_or_default();
                match hosted.get(command) {
                    Some(program) => {
                        if let Err(error) = start(&writer, &running, exec, program, &environment, &home) {
                            let _ = send(&writer, json!({"op": "origin-exec-exit", "exec": exec,
                                "code": 127, "error": error}));
                        }
                    }
                    // Only what this origin's own configuration offered runs.
                    None => {
                        let _ = send(&writer, json!({"op": "origin-exec-exit", "exec": exec,
                            "code": 127, "error": "The origin does not offer that program."}));
                    }
                }
            }
            Some("origin-exec-input") => {
                let data = STANDARD
                    .decode(message["data"].as_str().unwrap_or_default())
                    .unwrap_or_default();
                let program = running
                    .lock()
                    .ok()
                    .and_then(|running| running.get(&exec).map(|r| (r.input.clone(), r.pid)));
                if let Some((Some(input), pid)) = program
                    && let Err(TrySendError::Full(_)) = input.try_send(data)
                {
                    eprintln!("xd-host: a hosted program stopped reading its input; stopping it");
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
            }
            Some("origin-exec-eof") => {
                if let Ok(mut running) = running.lock()
                    && let Some(program) = running.get_mut(&exec)
                {
                    program.input = None;
                }
            }
            Some("origin-exec-kill") => {
                if let Some(pid) = running.lock().ok().and_then(|r| r.get(&exec).map(|p| p.pid)) {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
            }
            _ => {}
        }
    }
    // The runner went: what it started here has nobody to talk to.
    if let Ok(running) = running.lock() {
        for program in running.values() {
            unsafe {
                libc::kill(-program.pid, libc::SIGKILL);
            }
        }
    }
    Ok(())
}

fn start(
    writer: &Writer,
    running: &Arc<Mutex<HashMap<u64, Running>>>,
    exec: u64,
    program: &Hosted,
    environment: &HashMap<String, String>,
    home: &str,
) -> Result<(), String> {
    let mut command = Command::new(&program.program);
    command
        .args(&program.args)
        .env_clear()
        .envs(environment)
        .envs(&program.env)
        .current_dir(program.cwd.as_deref().unwrap_or(home))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn().map_err(|e| format!("{}: {e}", program.label))?;
    let pid = child.id() as i32;
    let mut stdin = child.stdin.take().ok_or("no input")?;
    let (input, inputs) = sync_channel::<Vec<u8>>(QUEUED_INPUT);
    thread::spawn(move || {
        for data in inputs {
            if stdin.write_all(&data).and_then(|()| stdin.flush()).is_err() {
                break;
            }
        }
    });
    let pumps = [
        (child.stdout.take().map(|out| Box::new(out) as Box<dyn Read + Send>), "stdout"),
        (child.stderr.take().map(|err| Box::new(err) as Box<dyn Read + Send>), "stderr"),
    ]
    .into_iter()
    .filter_map(|(reader, stream)| Some((reader?, stream)))
    .map(|(mut reader, stream)| {
        let writer = writer.clone();
        thread::spawn(move || {
            let mut buffer = vec![0; CHUNK];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0
                    || send(&writer, json!({"op": "origin-exec-output", "exec": exec,
                        "stream": stream, "data": STANDARD.encode(&buffer[..read])}))
                    .is_err()
                {
                    break;
                }
            }
        })
    })
    .collect::<Vec<_>>();
    running
        .lock()
        .map_err(|_| "running programs lock")?
        .insert(exec, Running { input: Some(input), pid });
    let writer = writer.clone();
    let running = running.clone();
    thread::spawn(move || {
        let status = child.wait();
        for pump in pumps {
            let _ = pump.join();
        }
        if let Ok(mut running) = running.lock() {
            running.remove(&exec);
        }
        let code = status.ok().and_then(|status| status.code()).unwrap_or(1);
        let _ = send(&writer, json!({"op": "origin-exec-exit", "exec": exec, "code": code}));
    });
    Ok(())
}

/// Runs the program the origin hosts as `command`, through this runner, with
/// this process's standard input and output as its own. Returns its exit code.
pub(crate) fn origin_exec(options: HostOptions, origin: &str, command: &str) -> Result<u8, String> {
    let stream = daemon::connect(&options, false, false).map_err(|e| e.to_string())?;
    let writer: Writer = Arc::new(Mutex::new(stream.try_clone().map_err(|e| e.to_string())?));
    send(&writer, json!({"op": "origin-exec-start", "origin": origin, "command": command,
        "_xd_request": 1}))
    .map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(stream).lines();
    // A program that finishes at once can answer before the start reply
    // arrives, so its output is kept until the reply says whose it is.
    let mut early = Vec::new();
    let exec = loop {
        let line = lines
            .next()
            .ok_or("The runner closed the connection.")?
            .map_err(|e| e.to_string())?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("_xd_request") == Some(&json!(1)) {
            if message["ok"] != true {
                return Err(message["error"].as_str().unwrap_or("The runner refused.").into());
            }
            break message["exec"].as_u64().ok_or("The runner gave no exec.")?;
        }
        if matches!(message["event"].as_str(), Some("origin-exec-output" | "origin-exec-exit")) {
            early.push(message);
        }
    };
    {
        let writer = writer.clone();
        thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buffer = vec![0; CHUNK];
            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) | Err(_) => {
                        let _ = send(&writer, json!({"op": "origin-exec-input", "exec": exec}));
                        break;
                    }
                    Ok(read) => {
                        let body = json!({"op": "origin-exec-input", "exec": exec,
                            "data": STANDARD.encode(&buffer[..read])});
                        if send(&writer, body).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let later = lines
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok());
    for message in early.into_iter().chain(later) {
        if message["exec"].as_u64() != Some(exec) {
            continue;
        }
        match message["event"].as_str() {
            Some("origin-exec-output") => {
                let data = STANDARD
                    .decode(message["data"].as_str().unwrap_or_default())
                    .unwrap_or_default();
                let target: &mut dyn Write = if message["stream"] == "stderr" {
                    &mut stderr
                } else {
                    &mut stdout
                };
                if target.write_all(&data).and_then(|()| target.flush()).is_err() {
                    return Ok(1);
                }
            }
            Some("origin-exec-exit") => {
                if let Some(error) = message["error"].as_str() {
                    eprintln!("xd-host: {error}");
                }
                return Ok(message["code"].as_i64().unwrap_or(1).clamp(0, 255) as u8);
            }
            _ => {}
        }
    }
    Err("The runner closed the connection.".into())
}

fn summary(collected: &origin_profile::Collected) -> String {
    let secrets = collected
        .files
        .iter()
        .filter(|file| file.secret)
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    let mut lines = vec![
        format!("origin home: {}", collected.origin_home),
        format!(
            "files: {} ({} KiB), sign-ins and keys: {}",
            collected.files.len(),
            collected.total_bytes() / 1024,
            secrets.join(", ")
        ),
        format!(
            "variables: {}",
            collected.env.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
    ];
    let mut tops = collected
        .files
        .iter()
        .map(|file| {
            let mut parts = file.path.splitn(3, '/');
            let first = parts.next().unwrap_or_default();
            match parts.next() {
                Some(second) if parts.next().is_some() => format!("{first}/{second}/"),
                Some(second) => format!("{first}/{second}"),
                None => first.to_owned(),
            }
        })
        .collect::<Vec<_>>();
    tops.dedup();
    lines.push(format!("carries: {}", tops.join(" ")));
    lines.extend(collected.notes.iter().map(|note| format!("note: {note}")));
    lines.join("\n")
}

fn request(options: &HostOptions, body: Value) -> Result<Value, String> {
    let mut stream = daemon::connect(options, false, false).map_err(|e| e.to_string())?;
    daemon::request_within(&mut stream, body, PUSH_TIMEOUT).map_err(|e| e.to_string())
}

fn print(reply: Value) -> Result<(), String> {
    println!("{}", serde_json::to_string_pretty(&reply).unwrap_or_default());
    if reply["ok"] == true {
        Ok(())
    } else {
        Err(reply["error"].as_str().unwrap_or("The runner refused.").to_owned())
    }
}
