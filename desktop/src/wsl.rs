//! Linux services behind the native Windows desktop. WSL owns project paths,
//! agent processes and persistent tmux sessions; Windows owns the UI/browser.

use std::{env, path::PathBuf};

pub(crate) const HOST_SCRIPT: &str = r#"set -eu
umask 077
payload=$(wslpath -u "$1") || { printf '%s\n' 'xd: Cannot locate the bundled host inside WSL.' >&2; exit 1; }
[ -f "$payload" ] || { printf '%s\n' 'xd: The bundled Linux host is missing. Install xd again.' >&2; exit 1; }
[ "$(uname -m)" = x86_64 ] || { printf '%s\n' 'xd: This Windows build requires an x86_64 WSL distribution.' >&2; exit 1; }
for dependency in git tmux; do
    command -v "$dependency" >/dev/null 2>&1 || { printf 'xd: Install %s in your WSL distribution (Ubuntu: sudo apt install git tmux), then reopen xd.\n' "$dependency" >&2; exit 1; }
done
export XD_DATA_NAME="$2" XD_UPDATE_CHANNEL="$3" XD_APP_BRIDGE=1
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

/// A path as Linux inside WSL names it, from however Windows shows it.
///
/// Projects live in WSL, and what someone has to hand is usually how Explorer
/// names the folder: `\\wsl.localhost\Ubuntu\home\me\project` for one
/// inside the distribution, `C:\Users\me\project` for one on a Windows
/// drive, either in the quotes "Copy as path" adds. The host knows neither
/// form, and said only that the directory did not exist. A Linux path, and
/// anything else, is passed on as typed.
///
/// Asking WSL takes running a program, so callers ask [`is_windows_path`]
/// first and call this off the UI thread only when it says so.
///
/// WSL is asked only when the answer matters. `host_distribution` tells which
/// distribution xd's host runs in: a folder named in another one is refused,
/// because the same path there is a different folder, or none, and agents
/// would be set to work on the wrong files. `mounted` tells where that
/// distribution mounts a Windows path: under `/mnt` unless its
/// `/etc/wsl.conf` says elsewhere, so it is not for xd to guess.
pub fn linux_path(
    typed: &str,
    host_distribution: impl FnOnce() -> Option<String>,
    mounted: impl FnOnce(&str) -> Option<String>,
) -> Result<String, String> {
    let trimmed = typed.trim();
    let path = trimmed
        .strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
        .unwrap_or(trimmed);
    let slashed = path.replace('\\', "/");

    // \\wsl.localhost\<distribution>\... and its older name, \\wsl$\...
    for share in ["//wsl.localhost/", "//wsl$/"] {
        // `get`, because the cut may fall inside a character of some other path.
        if let Some(head) = slashed.get(..share.len())
            && head.eq_ignore_ascii_case(share)
        {
            let (distribution, inside) = slashed[share.len()..]
                .split_once('/')
                .unwrap_or((&slashed[share.len()..], ""));
            return match host_distribution() {
                Some(host) if host.eq_ignore_ascii_case(distribution) => {
                    Ok(format!("/{}", inside.trim_end_matches('/')))
                }
                Some(host) => Err(format!(
                    "That folder is in the WSL distribution {distribution}, but xd runs in {host}. Choose a folder in {host}, or start xd with XD_WSL_DISTRIBUTION set to {distribution}."
                )),
                None => Err(
                    "xd could not tell which WSL distribution it runs in. Enter the folder's Linux path instead, such as /home/you/project."
                        .into(),
                ),
            };
        }
    }

    // X:\... is a Windows drive, mounted somewhere inside the distribution.
    let mut characters = slashed.chars();
    if let (Some(drive), Some(':')) = (characters.next(), characters.next())
        && drive.is_ascii_alphabetic()
    {
        let rest = characters.as_str();
        if rest.is_empty() || rest.starts_with('/') {
            let windows = format!("{drive}:{}", rest.replace('/', "\\"));
            // A bare drive is its root, not the directory last used on it.
            let windows = if rest.is_empty() {
                format!("{windows}\\")
            } else {
                windows
            };
            return mounted(&windows).ok_or_else(|| {
                "xd could not ask WSL where that Windows folder is. Enter its Linux path instead, such as /mnt/c/Users/you/project."
                    .to_owned()
            });
        }
    }
    Ok(typed.to_owned())
}

/// Whether `typed` names a folder the way Windows does, so that
/// [`linux_path`] has something to ask WSL about.
pub fn is_windows_path(typed: &str) -> bool {
    // Nothing is asked for a path that would come back as it was typed.
    linux_path(typed, || None, |_| None).as_deref() != Ok(typed)
}

/// How long WSL gets to answer a question about a path. It answers in well
/// under a second once it is running; a distribution still starting takes a
/// few.
#[cfg(windows)]
const ANSWER_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// What a short WSL command printed, or `None` if it failed or took longer
/// than [`ANSWER_WAIT`], in which case it is stopped.
#[cfg(windows)]
fn answer(mut command: std::process::Command) -> Option<String> {
    use std::{io::Read as _, process::Stdio, time::Instant};

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + ANSWER_WAIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    // A path is far smaller than a pipe holds, so it is all there to read.
    let mut printed = String::new();
    child.stdout.take()?.read_to_string(&mut printed).ok()?;
    status.success().then(|| printed.trim().to_owned())
}

/// The WSL distribution xd's host runs in, as WSL itself names it there.
///
/// Asked of WSL once and remembered; a failure is asked again next time, in
/// case WSL was only still starting.
#[cfg(windows)]
pub fn host_distribution() -> Option<String> {
    use std::sync::Mutex;

    static NAME: Mutex<Option<String>> = Mutex::new(None);
    let mut name = NAME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if name.is_none() {
        *name = answer(command(r#"printf '%s' "$WSL_DISTRO_NAME""#, &[]))
            .filter(|reported| !reported.is_empty());
    }
    name.clone()
}

/// Where the distribution xd's host runs in mounts a Windows path, asked of
/// that distribution's own `wslpath`.
#[cfg(windows)]
pub fn mounted_path(windows: &str) -> Option<String> {
    let path = answer(command(r#"wslpath -u "$1""#, &[windows.to_owned()]))?;
    // `/mnt/c/` for a drive's root: the folder without the slash.
    let path = match path.trim_end_matches('/') {
        "" => path,
        trimmed => trimmed.to_owned(),
    };
    path.starts_with('/').then_some(path)
}

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
    // Have wsl.exe write its own messages as UTF-8 rather than UTF-16.
    command.env("WSL_UTF8", "1");
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
        "The Linux host that xd runs in WSL is missing from its installation. Install xd again."
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

    /// How a distribution with the default settings mounts a Windows path.
    fn under_mnt(windows: &str) -> Option<String> {
        let (drive, rest) = windows.split_once(':')?;
        let rest = rest.replace('\\', "/");
        Some(format!(
            "/mnt/{}{}",
            drive.to_ascii_lowercase(),
            rest.trim_end_matches('/')
        ))
    }

    fn in_ubuntu(typed: &str) -> Result<String, String> {
        linux_path(typed, || Some("Ubuntu".into()), under_mnt)
    }

    #[test]
    fn windows_names_for_a_folder_become_the_path_linux_knows_it_by() {
        for (typed, linux) in [
            // Inside the distribution, as Explorer's address bar shows it.
            (
                r"\\wsl.localhost\Ubuntu\home\me\project",
                "/home/me/project",
            ),
            (r"\\wsl$\Ubuntu\home\me\project\", "/home/me/project"),
            (r"\\WSL.LOCALHOST\ubuntu\srv", "/srv"),
            ("//wsl.localhost/Ubuntu/home/me", "/home/me"),
            (r"\\wsl.localhost\Ubuntu", "/"),
            (r"\\wsl.localhost\Ubuntu\", "/"),
            // On a Windows drive, where the distribution mounts it.
            (r"C:\Users\me\project", "/mnt/c/Users/me/project"),
            ("d:/work/site/", "/mnt/d/work/site"),
            (r"C:\", "/mnt/c"),
            ("C:", "/mnt/c"),
            // What "Copy as path" puts on the clipboard.
            (
                r#""C:\Users\A Person\my project""#,
                "/mnt/c/Users/A Person/my project",
            ),
            (r"  C:\Users\me  ", "/mnt/c/Users/me"),
        ] {
            assert_eq!(in_ubuntu(typed).as_deref(), Ok(linux), "{typed}");
        }
    }

    #[test]
    fn only_a_windows_name_for_a_folder_is_worth_asking_wsl_about() {
        for windows in [
            r"C:\Users\me\project",
            "d:/work",
            "C:",
            r"\\wsl.localhost\Ubuntu\home\me",
            r"\\wsl$\Debian\srv",
            r#""C:\Users\A Person\my project""#,
        ] {
            assert!(is_windows_path(windows), "{windows}");
        }
        for linux in [
            "/home/me/project",
            "~/project",
            "project",
            "",
            "c:project",
            r"\\server\share\project",
        ] {
            assert!(!is_windows_path(linux), "{linux}");
        }
    }

    #[test]
    fn a_drive_is_wherever_the_distribution_mounts_it() {
        // WSL is asked with the path as Windows writes it, whatever was typed.
        let asked = std::cell::RefCell::new(Vec::new());
        let custom_root = |windows: &str| {
            asked.borrow_mut().push(windows.to_owned());
            // `[automount] root = /` in /etc/wsl.conf.
            Some("/c/Users/me/project".to_owned())
        };
        assert_eq!(
            linux_path(
                "\"c:/Users/me/project/\"",
                || panic!("a drive needs no distribution"),
                custom_root
            )
            .as_deref(),
            Ok("/c/Users/me/project")
        );
        assert_eq!(*asked.borrow(), [r"c:\Users\me\project\"]);
        // A bare drive is asked about as its root.
        let root = |windows: &str| (windows == r"D:\").then(|| "/mnt/d".to_owned());
        assert_eq!(linux_path("D:", || None, root).as_deref(), Ok("/mnt/d"));
        // No answer is no licence to guess /mnt.
        let error = linux_path(r"C:\work", || None, |_| None).unwrap_err();
        assert!(error.contains("/mnt/c/Users/you/project"), "{error}");
    }

    #[test]
    fn a_folder_in_another_distribution_is_refused_not_looked_up_in_this_one() {
        // /home/me/project in Debian is not /home/me/project in Ubuntu.
        let error = in_ubuntu(r"\\wsl.localhost\Debian\home\me\project").unwrap_err();
        assert!(
            error.contains("Debian") && error.contains("Ubuntu"),
            "{error}"
        );
        assert!(error.contains("XD_WSL_DISTRIBUTION"), "{error}");
        // Not knowing where xd runs is no licence to guess.
        let error = linux_path(r"\\wsl$\Ubuntu\home\me", || None, under_mnt).unwrap_err();
        assert!(error.contains("/home/you/project"), "{error}");
    }

    #[test]
    fn paths_linux_already_knows_are_left_as_typed() {
        for typed in [
            "/home/me/project",
            "~/project",
            "project",
            "",
            // Not a drive: a relative name that happens to hold a colon.
            "c:project",
            "notes:2024/plan",
            // Any script is a name; none of it is cut mid-character.
            "проекты/мой-проект",
            "/home/me/プロジェクト/サイト",
            // Another machine's share is not this distribution.
            r"\\server\share\project",
        ] {
            assert_eq!(
                linux_path(
                    typed,
                    || panic!("a Linux path needs no distribution"),
                    |_| panic!("a Linux path is mounted nowhere")
                )
                .as_deref(),
                Ok(typed),
                "{typed}"
            );
        }
    }

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
            (bin.join("uname"), "#!/bin/sh\nprintf '%s\\n' x86_64\n"),
            (
                payload.clone(),
                "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$XD_DATA_NAME\" \"$XD_UPDATE_CHANNEL\" \"$XD_APP_BRIDGE\" \"$@\"\ncat\n",
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
                "{}\nxd-nightly\nnightly\n1\nstdio\n--data\n{}/.local/share/xd-nightly\n",
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
