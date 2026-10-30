//! Linux services behind the native Windows desktop. WSL owns project paths,
//! agent processes and persistent tmux sessions; Windows owns the UI/browser.

use std::{env, path::PathBuf};

pub(crate) const HOST_SCRIPT: &str = r#"set -eu
umask 077
payload=$(wslpath -u "$1") || { printf '%s\n' 'xd: Cannot locate the bundled host inside WSL.' >&2; exit 1; }
[ -f "$payload" ] || { printf '%s\n' 'xd: The bundled Linux host is missing. Extract the complete Windows ZIP.' >&2; exit 1; }
[ "$(uname -m)" = x86_64 ] || { printf '%s\n' 'xd: This Windows build requires an x86_64 WSL distribution.' >&2; exit 1; }
for dependency in git tmux; do
    command -v "$dependency" >/dev/null 2>&1 || { printf 'xd: Install %s in your WSL distribution (Ubuntu: sudo apt install git tmux), then reopen xd.\n' "$dependency" >&2; exit 1; }
done
export XD_DATA_NAME="$2" XD_UPDATE_CHANNEL="$3"
data="$HOME/.local/share/$XD_DATA_NAME"
runtime="$data/runtime/v1"
mkdir -p "$runtime"
temporary=$(mktemp "$runtime/.xd-host.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cp "$payload" "$temporary"
chmod 700 "$temporary"
mv -f "$temporary" "$runtime/xd-host"
trap - EXIT HUP INT TERM
cd "$HOME"
exec "$runtime/xd-host" stdio --data "$data"
"#;

#[cfg(windows)]
const VERSION_SCRIPT: &str = r#"set -eu
payload=$(wslpath -u "$1")
exec "$payload" --version
"#;

pub(crate) fn executable() -> PathBuf {
    PathBuf::from("wsl.exe")
}

pub(crate) fn distribution() -> Option<String> {
    env::var("XD_WSL_DISTRIBUTION")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub(crate) fn shell_arguments(
    script: &str,
    parameters: &[String],
    distribution: Option<&str>,
) -> Vec<String> {
    let mut arguments = Vec::new();
    if let Some(distribution) = distribution {
        arguments.extend(["--distribution".into(), distribution.into()]);
    }
    arguments.extend([
        "--exec".into(),
        "sh".into(),
        "-c".into(),
        script.into(),
        "xd".into(),
    ]);
    arguments.extend_from_slice(parameters);
    arguments
}

#[cfg(windows)]
pub(crate) fn command(script: &str, parameters: &[String]) -> std::process::Command {
    use std::os::windows::process::CommandExt;

    let mut command = std::process::Command::new(executable());
    command.args(shell_arguments(
        script,
        parameters,
        distribution().as_deref(),
    ));
    // The desktop has its own native window. Stdio services should not create
    // a second console; terminal views use ConPTY separately.
    command.creation_flags(0x0800_0000);
    command
}

#[cfg(windows)]
pub(crate) fn payload() -> Result<PathBuf, String> {
    let configured = env::var_os("XD_WSL_HOST_EXECUTABLE")
        .or_else(|| env::var_os("XD_HOST_EXECUTABLE"))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let path = configured.or_else(|| {
        env::current_exe().ok().and_then(|executable| {
            executable
                .parent()
                .map(|parent| parent.join("libexec/xd-host-linux"))
        })
    });
    path.filter(|path| path.is_file()).ok_or_else(|| {
        "The bundled WSL host is missing. Extract the complete Windows ZIP, including libexec/xd-host-linux."
            .into()
    })
}

#[cfg(windows)]
pub(crate) fn payload_argument(path: &std::path::Path) -> String {
    // Rust's canonical Windows paths may use the extended-length prefix,
    // which wslpath does not require. Keep the ordinary drive/UNC spelling.
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        value.strip_prefix(r"\\?\").unwrap_or(&value).to_owned()
    }
}

#[cfg(windows)]
pub(crate) fn version_command(path: &std::path::Path) -> std::process::Command {
    command(VERSION_SCRIPT, &[payload_argument(path)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_arguments_keep_paths_and_shell_metacharacters_in_separate_parameters() {
        let parameters = vec![
            r"C:\Users\A Person\xd\libexec\xd-host-linux".into(),
            "project's name; echo unexpected".into(),
        ];
        assert_eq!(
            shell_arguments("printf '%s\\n' \"$1\"", &parameters, Some("xd-ci-smoke")),
            vec![
                "--distribution",
                "xd-ci-smoke",
                "--exec",
                "sh",
                "-c",
                "printf '%s\\n' \"$1\"",
                "xd",
                &parameters[0],
                &parameters[1],
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn wsl_host_installs_in_the_linux_home_and_keeps_stdio_clean() {
        use std::{fs, os::unix::fs::PermissionsExt, process::Command};

        let directory = env::temp_dir().join(format!(
            "xd-wsl-host-script-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin = directory.join("bin");
        let home = directory.join("Linux Home");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&home).unwrap();
        let payload = directory.join("Windows payload's host");
        for (path, content) in [
            (bin.join("wslpath"), "#!/bin/sh\nprintf '%s\\n' \"$2\"\n"),
            (bin.join("tmux"), "#!/bin/sh\nexit 0\n"),
            (
                payload.clone(),
                "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$XD_DATA_NAME\" \"$XD_UPDATE_CHANNEL\" \"$@\"\ncat\n",
            ),
        ] {
            fs::write(&path, content).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let output = Command::new("sh")
            .args(["-c", HOST_SCRIPT, "xd"])
            .arg(&payload)
            .args(["xd-nightly", "nightly"])
            .env("HOME", &home)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        assert!(output.stderr.is_empty());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "{}\nxd-nightly\nnightly\nstdio\n--data\n{}/.local/share/xd-nightly\n",
                home.display(),
                home.display(),
            )
        );
        assert!(
            home.join(".local/share/xd-nightly/runtime/v1/xd-host")
                .is_file()
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
