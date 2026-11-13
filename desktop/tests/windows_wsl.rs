#![cfg(windows)]

use std::{sync::mpsc, thread, time::Duration};

use xd_desktop::host::{HostHandle, HostUpdate, RequestKind};
use xd_desktop::{
    session_host::{AgentCommand, SessionHost},
    session_runtime::{SessionEvent, SessionEventKind, SessionRuntime},
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

/// A repository named the way Explorer names it is looked up by the
/// distribution itself: which one it is, and where it mounts a drive.
#[test]
#[ignore = "requires an initialized WSL distribution with git and tmux"]
fn wsl_says_where_windows_folders_are() {
    use xd_desktop::wsl::{host_distribution, linux_path, mounted_path};

    let distribution = host_distribution().expect("WSL names the distribution it runs");
    if let Ok(selected) = std::env::var("XD_WSL_DISTRIBUTION") {
        assert_eq!(distribution, selected);
    }
    // The Windows folder is certain to exist; how it is mounted is WSL's say.
    let windows = std::env::var("SystemRoot").expect("Windows sets SystemRoot");
    let mounted = mounted_path(&windows).expect("WSL mounts the system drive");
    assert!(
        mounted.starts_with('/') && !mounted.ends_with('/'),
        "{mounted}"
    );
    assert!(
        mounted.to_ascii_lowercase().ends_with("/windows"),
        "{mounted}"
    );
    assert_eq!(
        linux_path(&format!("\"{windows}\\\""), host_distribution, mounted_path).as_deref(),
        Ok(mounted.as_str())
    );
    // A folder inside this distribution, and one in a distribution that is not.
    assert_eq!(
        linux_path(
            &format!(r"\\wsl.localhost\{distribution}\etc"),
            host_distribution,
            mounted_path
        )
        .as_deref(),
        Ok("/etc")
    );
    assert!(
        linux_path(
            r"\\wsl.localhost\xd-no-such-distribution\etc",
            host_distribution,
            mounted_path
        )
        .is_err()
    );
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
    let mut lifecycle = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    receive_output_until(&receiver, deadline, &mut output, &mut lifecycle, |output| {
        output.contains("xd-wsl-ready")
    });
    runtime.resize(&terminal_id, 100, 40).unwrap();
    // WSL delivers the ConPTY resize to the Linux PTY asynchronously.
    thread::sleep(Duration::from_millis(100));
    runtime.input(&terminal_id, b"bridge-input\r").unwrap();
    receive_output_until(&receiver, deadline, &mut output, &mut lifecycle, |output| {
        output.contains("xd-wsl-input:bridge-input") && output.contains("40 100")
    });
    runtime.kill(&terminal_id).unwrap();
}

fn receive_output_until(
    receiver: &mpsc::Receiver<SessionEvent>,
    deadline: std::time::Instant,
    output: &mut String,
    lifecycle: &mut Vec<SessionEventKind>,
    expected: impl Fn(&str) -> bool,
) {
    while !expected(output) {
        let event = receiver
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .unwrap_or_else(|error| {
                panic!("WSL terminal stalled: {error}; output={output:?}; events={lifecycle:?}")
            });
        match event.kind {
            SessionEventKind::Output { data, .. } => {
                output.push_str(&String::from_utf8_lossy(&data));
            }
            kind => {
                let closed = matches!(kind, SessionEventKind::Closed { .. });
                lifecycle.push(kind);
                assert!(
                    !closed,
                    "WSL terminal closed early; output={output:?}; events={lifecycle:?}"
                );
            }
        }
    }
}
