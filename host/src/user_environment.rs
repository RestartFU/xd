//! Login-shell discovery for user-installed tools. The capture and path merge
//! follow T3 Code's shared shell utilities; see data/licenses/t3code-LICENSE.
use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::{OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

const NAMES: &[&str] = &["PATH", "CODEX_HOME", "CLAUDE_CONFIG_DIR", "SSH_AUTH_SOCK"];
const LIMIT: u64 = 64 * 1024;
static ENVIRONMENT: OnceLock<HashMap<String, OsString>> = OnceLock::new();

pub(crate) fn values() -> &'static HashMap<String, OsString> {
    ENVIRONMENT.get_or_init(discover)
}

pub(crate) fn apply(command: &mut Command) {
    command.envs(values());
}

fn discover() -> HashMap<String, OsString> {
    let mut captured = HashMap::new();
    let mut shells = Vec::new();
    if let Some(shell) = env::var_os("SHELL").filter(|shell| !shell.is_empty()) {
        shells.push(shell);
    }
    shells.push(
        if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        }
        .into(),
    );
    shells.dedup();
    for shell in shells {
        if let Some(values) = probe(Path::new(&shell), Duration::from_secs(5)) {
            captured = values;
            break;
        }
    }
    let mut result = HashMap::new();
    let inherited = env::var_os("PATH");
    if let Some(path) = merge_paths(captured.get("PATH"), inherited.as_ref()) {
        result.insert("PATH".into(), path);
    }
    // Explicit configuration from the parent wins. Never create an app-owned
    // provider home or copy credential files into the workspace.
    for name in &NAMES[1..] {
        if env::var_os(name).is_none()
            && let Some(value) = captured.remove(*name)
        {
            result.insert((*name).into(), value);
        }
    }
    result
}

fn merge_paths(preferred: Option<&OsString>, inherited: Option<&OsString>) -> Option<OsString> {
    let mut paths = Vec::new();
    for value in [preferred, inherited].into_iter().flatten() {
        for path in env::split_paths(value) {
            if !path.as_os_str().is_empty() && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    (!paths.is_empty())
        .then(|| env::join_paths(paths).ok())
        .flatten()
}

fn capture_command(shell: &Path) -> String {
    let nu = shell.file_stem().is_some_and(|name| name == "nu");
    NAMES.iter().map(|name| {
        if nu {
            let value = if *name == "PATH" {
                "if ($env.PATH? | describe | str starts-with 'list') { $env.PATH | str join (char esep) } else { $env.PATH? | default '' }".to_owned()
            } else {
                format!("$env.{name}? | default ''")
            };
            format!("print '__XD_{name}_START__'; print ({value}); print '__XD_{name}_END__'")
        } else {
            format!("printf '%s\\n' '__XD_{name}_START__'; printenv {name} || true; printf '%s\\n' '__XD_{name}_END__'")
        }
    }).collect::<Vec<_>>().join("; ")
}

fn parse_capture(output: &str) -> HashMap<String, OsString> {
    NAMES
        .iter()
        .filter_map(|name| {
            let start = format!("__XD_{name}_START__\n");
            let end = format!("\n__XD_{name}_END__");
            let value = output.split_once(&start)?.1.split_once(&end)?.0;
            (!value.is_empty()).then(|| ((*name).to_owned(), OsString::from(value)))
        })
        .collect()
}

fn probe(shell: &Path, timeout: Duration) -> Option<HashMap<String, OsString>> {
    let mut command = Command::new(shell);
    command
        .args(["-ilc", &capture_command(shell)])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(LIMIT + 1).read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    let deadline = Instant::now() + timeout;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => break false,
        }
    };
    // Startup scripts may leave children holding the capture pipe open.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    let bytes = receiver.recv_timeout(Duration::from_millis(100)).ok()?;
    if !success || bytes.len() > LIMIT as usize {
        return None;
    }
    let values = parse_capture(std::str::from_utf8(&bytes).ok()?);
    values.contains_key("PATH").then_some(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_path_precedes_inherited_extras_without_empty_entries() {
        assert_eq!(
            merge_paths(
                Some(&"/tools:/bin::/tools".into()),
                Some(&"/bin:/extra".into())
            ),
            Some(OsString::from("/tools:/bin:/extra"))
        );
    }

    #[test]
    fn capture_ignores_shell_banners_and_preserves_spaces() {
        let values = parse_capture(
            "Welcome\n__XD_PATH_START__\n/tools with spaces:/bin\n__XD_PATH_END__\n__XD_CODEX_HOME_START__\n/home/user/custom codex\n__XD_CODEX_HOME_END__\n",
        );
        assert_eq!(values["PATH"], "/tools with spaces:/bin");
        assert_eq!(values["CODEX_HOME"], "/home/user/custom codex");
        assert!(!values.contains_key("CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn login_shell_probe_captures_environment_without_credentials() {
        let values = probe(Path::new("/bin/sh"), Duration::from_secs(2)).unwrap();
        assert!(!values["PATH"].is_empty());
        assert!(values.keys().all(|name| NAMES.contains(&name.as_str())));
    }
}
