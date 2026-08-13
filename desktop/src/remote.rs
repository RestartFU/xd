use std::{
    path::PathBuf,
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
    let home = probe_remote_home(command)?;
    let arguments = host_arguments(command, home.to_string_lossy().as_ref());
    let mut process = Command::new(command.program());
    process
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let (handle, updates, host) = HostHandle::connect_command(process, home)
        .map_err(|error| RemoteError::Bridge(error.to_string()))?;
    Ok(SshRemoteSession {
        host: handle,
        updates,
        bridge: SshRemoteBridge { _host: host },
    })
}

fn ssh_base_arguments(command: &SshCommand) -> Vec<String> {
    let mut arguments = command.options().to_vec();
    arguments.extend([
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
    ]);
    arguments
}

fn home_probe_arguments(command: &SshCommand) -> Vec<String> {
    let mut arguments = ssh_base_arguments(command);
    arguments.extend([
        "--".into(),
        command.destination().into(),
        "printf '%s' \"$HOME\"".into(),
    ]);
    arguments
}

fn host_arguments(command: &SshCommand, home: &str) -> Vec<String> {
    let mut arguments = ssh_base_arguments(command);
    let name = channel::data_name();
    let name = name.to_string_lossy();
    arguments.extend([
        "--".into(),
        command.destination().into(),
        format!("{home}/.local/opt/{name}/libexec/xd-host"),
        "stdio".into(),
        "--data".into(),
        format!("{home}/.local/share/{name}"),
    ]);
    arguments
}

fn probe_remote_home(command: &SshCommand) -> Result<PathBuf, RemoteError> {
    let output = Command::new(command.program())
        .args(home_probe_arguments(command))
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
    let home = String::from_utf8(output.stdout)
        .map_err(|_| RemoteError::Bridge("SSH returned a non-UTF-8 home directory".into()))?;
    let home = home.trim();
    if !home.starts_with('/') || home.contains('\n') || home.contains('\r') {
        return Err(RemoteError::Bridge(
            "SSH returned an invalid remote home directory".into(),
        ));
    }
    Ok(PathBuf::from(home))
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
            home_probe_arguments(&command),
            vec![
                "-p",
                "22",
                "-i",
                "keys/zeno mc",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "--",
                "zenomc.org",
                "printf '%s' \"$HOME\"",
            ]
        );
        let name = channel::data_name();
        let name = name.to_string_lossy();
        assert_eq!(
            host_arguments(&command, "/home/zeno"),
            vec![
                "-p".into(),
                "22".into(),
                "-i".into(),
                "keys/zeno mc".into(),
                "-o".into(),
                "ServerAliveInterval=15".into(),
                "-o".into(),
                "BatchMode=yes".into(),
                "-o".into(),
                "ConnectTimeout=10".into(),
                "--".into(),
                "zenomc.org".into(),
                format!("/home/zeno/.local/opt/{name}/libexec/xd-host"),
                "stdio".into(),
                "--data".into(),
                format!("/home/zeno/.local/share/{name}"),
            ]
        );
    }
}
