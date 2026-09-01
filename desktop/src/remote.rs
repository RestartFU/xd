#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    channel,
    host::{HostHandle, HostUpdate, StartedHost},
    session_host::SshCommand,
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("Cannot start the SSH connection: {0}")]
    Bridge(String),
}

/// Owns the SSH stdio process. Dropping it closes the remote host immediately.
/// Each connection gets a direct host so SSH startup cannot depend on a
/// separately detached broker being healthy on the remote machine.
pub struct SshRemoteBridge {
    _host: StartedHost,
}

pub struct SshRemoteSession {
    host: HostHandle,
    updates: async_channel::Receiver<HostUpdate>,
    bridge: SshRemoteBridge,
}

impl SshRemoteSession {
    pub fn into_parts(
        self,
    ) -> (
        HostHandle,
        async_channel::Receiver<HostUpdate>,
        SshRemoteBridge,
    ) {
        (self.host, self.updates, self.bridge)
    }
}

pub fn connect_ssh(command: &SshCommand) -> Result<SshRemoteSession, RemoteError> {
    let local_host = local_host_executable()?;
    let local_version = host_version(&local_host)?;
    let remote = probe_remote(command)?;
    if remote.host_version.as_deref() != Some(local_version.as_str()) {
        deploy_remote_host(command, &local_host, &local_version, &remote)?;
    }
    let arguments = host_arguments(command, &remote.home);
    let mut process = Command::new(command.program());
    process
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (handle, updates, host) = HostHandle::connect_command(process, remote.home)
        .map_err(|error| RemoteError::Bridge(error.to_string()))?;
    Ok(SshRemoteSession {
        host: handle,
        updates,
        bridge: SshRemoteBridge { _host: host },
    })
}

fn ssh_base_arguments(command: &SshCommand) -> Vec<String> {
    let mut arguments = command.connection_options();
    arguments.extend([
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
    ]);
    arguments
}

fn probe_arguments(command: &SshCommand) -> Vec<String> {
    let mut arguments = ssh_base_arguments(command);
    let relative_host = remote_host_relative_path();
    let script = format!(
        "printf 'XD_HOME=%s\\n' \"$HOME\"; printf 'XD_SYSTEM='; uname -s; printf 'XD_ARCH='; uname -m; host=\"$HOME\"/{}; if [ -x \"$host\" ]; then printf 'XD_HOST_VERSION='; \"$host\" --version 2>/dev/null || true; fi",
        shell_quote(&relative_host.to_string_lossy()),
    );
    arguments.extend(["--".into(), command.destination().into(), script]);
    arguments
}

fn host_arguments(command: &SshCommand, home: &Path) -> Vec<String> {
    let mut arguments = ssh_base_arguments(command);
    let host = home.join(remote_host_relative_path());
    let data = home.join(".local/share").join(channel::data_name());
    let script = format!(
        "exec {} stdio --data {}",
        shell_quote(&host.to_string_lossy()),
        shell_quote(&data.to_string_lossy()),
    );
    arguments.extend(["--".into(), command.destination().into(), script]);
    arguments
}

#[derive(Debug, PartialEq, Eq)]
struct RemoteInfo {
    home: PathBuf,
    system: String,
    architecture: String,
    host_version: Option<String>,
}

fn probe_remote(command: &SshCommand) -> Result<RemoteInfo, RemoteError> {
    let mut process = Command::new(command.program());
    process.args(probe_arguments(command)).stdin(Stdio::null());
    let output = bounded_output(&mut process, COMMAND_TIMEOUT, COMMAND_OUTPUT_LIMIT)
        .map_err(|error| RemoteError::Bridge(format!("cannot probe SSH: {error}")))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(RemoteError::Bridge(if error.is_empty() {
            format!("SSH exited with {}", output.status)
        } else {
            error
        }));
    }
    let output = String::from_utf8(output.stdout)
        .map_err(|_| RemoteError::Bridge("SSH returned non-UTF-8 platform details".into()))?;
    let field = |prefix: &str| {
        output
            .lines()
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let home = field("XD_HOME=")
        .ok_or_else(|| RemoteError::Bridge("SSH returned no remote home directory".into()))?;
    if !home.starts_with('/') || home.contains('\n') || home.contains('\r') {
        return Err(RemoteError::Bridge(
            "SSH returned an invalid remote home directory".into(),
        ));
    }
    Ok(RemoteInfo {
        home: PathBuf::from(home),
        system: field("XD_SYSTEM=")
            .ok_or_else(|| RemoteError::Bridge("SSH returned no remote system".into()))?
            .to_owned(),
        architecture: field("XD_ARCH=")
            .ok_or_else(|| RemoteError::Bridge("SSH returned no remote architecture".into()))?
            .to_owned(),
        host_version: field("XD_HOST_VERSION=").map(str::to_owned),
    })
}

fn local_host_executable() -> Result<PathBuf, RemoteError> {
    let path = env::var_os("XD_HOST_EXECUTABLE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            env::current_exe().ok().and_then(|executable| {
                executable
                    .parent()
                    .map(|parent| parent.join("xd-host"))
                    .filter(|path| path.is_file())
            })
        })
        .ok_or_else(|| {
            RemoteError::Bridge("cannot find the xd host executable to send remotely".into())
        })?;
    Ok(path)
}

fn host_version(host: &Path) -> Result<String, RemoteError> {
    let mut command = Command::new(host);
    command.arg("--version").stdin(Stdio::null());
    let output = bounded_output(&mut command, VERSION_TIMEOUT, COMMAND_OUTPUT_LIMIT)
        .map_err(|error| RemoteError::Bridge(format!("cannot inspect xd host: {error}")))?;
    if !output.status.success() {
        return Err(RemoteError::Bridge(format!(
            "cannot inspect xd host: exited with {}",
            output.status
        )));
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|version| version.trim().to_owned())
        .filter(|version| !version.is_empty())
        .ok_or_else(|| RemoteError::Bridge("xd host returned no version".into()))
}

fn deploy_remote_host(
    command: &SshCommand,
    local_host: &Path,
    local_version: &str,
    remote: &RemoteInfo,
) -> Result<(), RemoteError> {
    if !same_platform(&remote.system, &remote.architecture) {
        return Err(RemoteError::Bridge(format!(
            "the remote machine is {} {}, but this xd host is for {} {}",
            remote.system,
            remote.architecture,
            env::consts::OS,
            env::consts::ARCH,
        )));
    }
    let directory = remote.home.join(remote_host_relative_directory());
    let destination = directory.join("xd-host");
    let script = format!(
        "set -eu; umask 077; directory={}; destination={}; mkdir -p \"$directory\"; temporary=\"$directory/.xd-host.$$\"; trap 'rm -f \"$temporary\"' EXIT HUP INT TERM; cat > \"$temporary\"; chmod 700 \"$temporary\"; \"$temporary\" --version; mv -f \"$temporary\" \"$destination\"; trap - EXIT HUP INT TERM",
        shell_quote(&directory.to_string_lossy()),
        shell_quote(&destination.to_string_lossy()),
    );
    let mut arguments = ssh_base_arguments(command);
    arguments.extend(["--".into(), command.destination().into(), script]);
    let file = fs::File::open(local_host)
        .map_err(|error| RemoteError::Bridge(format!("cannot read xd host: {error}")))?;
    let mut process = Command::new(command.program());
    process.args(arguments).stdin(Stdio::from(file));
    let output = bounded_output(&mut process, COMMAND_TIMEOUT, COMMAND_OUTPUT_LIMIT)
        .map_err(|error| RemoteError::Bridge(format!("cannot install over SSH: {error}")))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(RemoteError::Bridge(if error.is_empty() {
            format!(
                "cannot install the remote xd host: SSH exited with {}",
                output.status
            )
        } else {
            format!("cannot install the remote xd host: {error}")
        }));
    }
    if !installed_version_matches(&output.stdout, local_version) {
        let installed = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        return Err(RemoteError::Bridge(format!(
            "the remote xd host reported {installed:?} after installing {local_version:?}"
        )));
    }
    Ok(())
}

fn installed_version_matches(output: &[u8], expected: &str) -> bool {
    String::from_utf8_lossy(output)
        .lines()
        .any(|line| line.trim() == expected)
}

fn bounded_output(
    command: &mut Command,
    timeout: Duration,
    output_limit: usize,
) -> Result<Output, String> {
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "cannot capture command output".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "cannot capture command errors".to_owned())?;
    let stdout = thread::spawn(move || read_bounded(stdout, output_limit));
    let stderr = thread::spawn(move || read_bounded(stderr, output_limit));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                terminate_command(&mut child);
                let _ = stdout.join();
                let _ = stderr.join();
                return Err(format!(
                    "command timed out after {} seconds",
                    timeout.as_secs_f32()
                ));
            }
            Err(error) => {
                terminate_command(&mut child);
                let _ = stdout.join();
                let _ = stderr.join();
                return Err(format!("cannot wait for command: {error}"));
            }
        }
    };
    while (!stdout.is_finished() || !stderr.is_finished()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !stdout.is_finished() || !stderr.is_finished() {
        terminate_command(&mut child);
        let _ = stdout.join();
        let _ = stderr.join();
        return Err(format!(
            "command timed out after {} seconds",
            timeout.as_secs_f32()
        ));
    }
    let (stdout, stdout_truncated) = stdout
        .join()
        .map_err(|_| "command output reader failed".to_owned())?;
    let (stderr, stderr_truncated) = stderr
        .join()
        .map_err(|_| "command error reader failed".to_owned())?;
    if stdout_truncated || stderr_truncated {
        return Err(format!(
            "command produced too much output (limit: {output_limit} bytes per stream)"
        ));
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn terminate_command(child: &mut Child) {
    #[cfg(unix)]
    let killed_group = libc::pid_t::try_from(child.id())
        .is_ok_and(|pid| unsafe { libc::kill(-pid, libc::SIGKILL) } == 0);
    #[cfg(not(unix))]
    let killed_group = false;
    if !killed_group {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn read_bounded(mut input: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut truncated = false;
    loop {
        let Ok(count) = input.read(&mut buffer) else {
            break;
        };
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..count.min(remaining)]);
        truncated |= count > remaining;
    }
    (kept, truncated)
}

fn remote_host_relative_directory() -> PathBuf {
    PathBuf::from(".local/share")
        .join(channel::data_name())
        .join("runtime/v1")
}

fn remote_host_relative_path() -> PathBuf {
    remote_host_relative_directory().join("xd-host")
}

fn same_platform(system: &str, architecture: &str) -> bool {
    let system_matches = match env::consts::OS {
        "linux" => system.eq_ignore_ascii_case("linux"),
        "macos" => system.eq_ignore_ascii_case("darwin"),
        other => system.eq_ignore_ascii_case(other),
    };
    let architecture_matches = match env::consts::ARCH {
        "aarch64" => matches!(architecture, "aarch64" | "arm64"),
        "x86_64" => matches!(architecture, "x86_64" | "amd64"),
        other => architecture == other,
    };
    system_matches && architecture_matches
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_transport_runs_the_host_over_stdio_without_a_socket_forward() {
        let command = SshCommand::parse(
            r#"ssh zenomc.org -p 22 -i "keys/zeno mc" -o ServerAliveInterval=15"#,
        )
        .unwrap();
        assert_eq!(
            probe_arguments(&command),
            vec![
                "-p",
                "22",
                "-i",
                "keys/zeno mc",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPersist=10m",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ControlPath=~/.ssh/xd-%C",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "--",
                "zenomc.org",
                "printf 'XD_HOME=%s\\n' \"$HOME\"; printf 'XD_SYSTEM='; uname -s; printf 'XD_ARCH='; uname -m; host=\"$HOME\"/'.local/share/xd/runtime/v1/xd-host'; if [ -x \"$host\" ]; then printf 'XD_HOST_VERSION='; \"$host\" --version 2>/dev/null || true; fi",
            ]
        );
        assert_eq!(
            host_arguments(&command, Path::new("/home/zeno")),
            Vec::<String>::from([
                "-p".into(),
                "22".into(),
                "-i".into(),
                "keys/zeno mc".into(),
                "-o".into(),
                "ServerAliveInterval=15".into(),
                "-o".into(),
                "ControlMaster=auto".into(),
                "-o".into(),
                "ControlPersist=10m".into(),
                "-o".into(),
                "ServerAliveCountMax=3".into(),
                "-o".into(),
                "ControlPath=~/.ssh/xd-%C".into(),
                "-o".into(),
                "BatchMode=yes".into(),
                "-o".into(),
                "ConnectTimeout=10".into(),
                "--".into(),
                "zenomc.org".into(),
                "exec '/home/zeno/.local/share/xd/runtime/v1/xd-host' stdio --data '/home/zeno/.local/share/xd'".into(),
            ])
        );
    }

    #[test]
    fn platform_names_accept_the_native_uname_spellings() {
        let (system, architecture) = match (env::consts::OS, env::consts::ARCH) {
            ("linux", "x86_64") => ("Linux", "x86_64"),
            ("linux", "aarch64") => ("Linux", "aarch64"),
            ("macos", "x86_64") => ("Darwin", "x86_64"),
            ("macos", "aarch64") => ("Darwin", "arm64"),
            pair => pair,
        };
        assert!(same_platform(system, architecture));
        assert!(!same_platform("Plan9", architecture));
    }

    #[test]
    fn deployment_version_check_ignores_remote_shell_banner_output() {
        let expected = "xd-host 0.1.10 (742f275)";
        assert!(installed_version_matches(
            b"Welcome to the build server\nxd-host 0.1.10 (742f275)\n",
            expected,
        ));
        assert!(!installed_version_matches(
            b"Welcome to the build server\nxd-host 0.1.9 (6ab3faf)\n",
            expected,
        ));
    }

    #[test]
    fn remote_commands_have_a_wall_clock_timeout() {
        let mut command = Command::new("sleep");
        command.arg("30");
        let started = std::time::Instant::now();
        let error =
            bounded_output(&mut command, std::time::Duration::from_millis(50), 1024).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn remote_timeout_kills_descendants_that_retain_output_pipes() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let descendant_file = env::temp_dir().join(format!(
            "xd-remote-timeout-{}-{nonce}.pid",
            std::process::id()
        ));
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!(
                "sleep 30 & descendant=$!; printf '%s\\n' \"$descendant\" > {}; exit 0",
                shell_quote(&descendant_file.to_string_lossy())
            ),
        ]);
        let (done, result) = std::sync::mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            done.send(bounded_output(
                &mut command,
                Duration::from_millis(50),
                1024,
            ))
            .unwrap();
        });

        let returned = result.recv_timeout(Duration::from_secs(1));
        if returned.is_err()
            && let Ok(pid) = fs::read_to_string(&descendant_file)
                .unwrap_or_default()
                .trim()
                .parse::<u32>()
        {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        worker.join().unwrap();
        let _ = fs::remove_file(descendant_file);

        let error = returned
            .expect("timeout waited for a descendant-held output pipe")
            .expect_err("command should time out");
        assert!(error.contains("timed out"), "{error}");
    }

    #[test]
    fn remote_command_output_is_bounded() {
        let mut command = Command::new("sh");
        command.args(["-c", "yes x | head -c 65536"]);
        let error =
            bounded_output(&mut command, std::time::Duration::from_secs(2), 1024).unwrap_err();
        assert!(error.contains("too much output"), "{error}");
    }
}
