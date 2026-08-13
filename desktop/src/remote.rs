use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use thiserror::Error;

use crate::{
    channel,
    host::{HostHandle, HostUpdate, StartedHost},
    session_host::SshCommand,
};

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("Cannot start the SSH connection: {0}")]
    Bridge(String),
}

/// Owns the SSH stdio process. Dropping it closes the remote host immediately;
/// there is no listener or background service on either machine.
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
    let output = Command::new(command.program())
        .args(probe_arguments(command))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| RemoteError::Bridge(format!("cannot launch SSH: {error}")))?;
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
    let output = Command::new(host)
        .arg("--version")
        .output()
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
    let output = Command::new(command.program())
        .args(arguments)
        .stdin(Stdio::from(file))
        .output()
        .map_err(|error| RemoteError::Bridge(format!("cannot launch SSH: {error}")))?;
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
    let installed = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if installed != local_version {
        return Err(RemoteError::Bridge(format!(
            "the remote xd host reported {installed:?} after installing {local_version:?}"
        )));
    }
    Ok(())
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
}
