#![cfg(windows)]

use std::{sync::mpsc, thread, time::Duration};

use xd_desktop::host::{HostHandle, HostUpdate, RequestKind};
use xd_desktop::{
    session_host::{AgentCommand, SessionHost},
    session_runtime::{SessionEventKind, SessionRuntime},
};

/// Release CI supplies an isolated distribution and the bundled Linux host.
#[test]
#[ignore = "requires an initialized WSL distribution with git and tmux"]
fn wsl_host_stdio_round_trip() {
    let (host, updates, _process) = HostHandle::start_local().expect("start the WSL host bridge");
    host.tree().expect("send a tree request");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(update) = updates.recv_blocking() {
            match update {
                HostUpdate::Reply {
                    kind: RequestKind::Tree,
                    body,
                    ..
                } => {
                    let _ = sender.send(Ok(body));
                    break;
                }
                HostUpdate::Disconnected { message } => {
                    let _ = sender.send(Err(message));
                    break;
                }
                _ => {}
            }
        }
    });
    let body = receiver
        .recv_timeout(Duration::from_secs(60))
        .expect("receive a framed response from WSL")
        .expect("the WSL host stayed connected");
    assert_eq!(body.get("ok").and_then(|value| value.as_bool()), Some(true));
    assert!(body.get("folders").is_some_and(|value| value.is_array()));
}

#[test]
#[ignore = "requires an initialized WSL distribution with git and tmux"]
fn wsl_terminal_streams_input_and_updates_its_geometry() {
    let host = SessionHost::local("tmux".into(), std::path::PathBuf::new());
    let terminal_id = format!("windows-conpty-smoke-{}", std::process::id());
    let spec = host.attach(
        &terminal_id,
        std::path::Path::new("~"),
        &AgentCommand::new(
            "sh",
            ["-c", "printf 'xd-wsl-ready\\n'; read -r line; printf 'xd-wsl-input:%s\\n' \"$line\"; stty size; sleep 30"],
        ),
    );
    let (runtime, events) = SessionRuntime::new();
    runtime
        .open(
            "windows-smoke",
            &terminal_id,
            "Terminal",
            None,
            spec,
            Some(host.kill_process(&terminal_id)),
            80,
            24,
        )
        .expect("open WSL through ConPTY");
    struct Cleanup(SessionRuntime, String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.kill(&self.1);
        }
    }
    let _cleanup = Cleanup(runtime.clone(), terminal_id.clone());
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(event) = events.recv_blocking() {
            if sender.send(event).is_err() {
                break;
            }
        }
    });
    let mut output = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !output.contains("xd-wsl-ready") {
        let event = receiver
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("receive WSL terminal output");
        if let SessionEventKind::Output { data, .. } = event.kind {
            output.push_str(&String::from_utf8_lossy(&data));
        }
    }
    runtime.resize(&terminal_id, 100, 40).unwrap();
    // WSL delivers the ConPTY resize to the Linux PTY asynchronously.
    thread::sleep(Duration::from_millis(100));
    runtime.input(&terminal_id, b"bridge-input\n").unwrap();
    while !output.contains("xd-wsl-input:bridge-input") || !output.contains("40 100") {
        let event = receiver
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .unwrap_or_else(|error| panic!("WSL input/resize did not arrive: {error}; {output}"));
        if let SessionEventKind::Output { data, .. } = event.kind {
            output.push_str(&String::from_utf8_lossy(&data));
        }
    }
    runtime.kill(&terminal_id).unwrap();
}
