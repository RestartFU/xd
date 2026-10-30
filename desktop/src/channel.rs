use std::{
    env,
    ffi::{OsStr, OsString},
    path::Path,
    process::Command,
};

pub fn nightly() -> bool {
    option_env!("XD_BUILD_PROFILE") == Some("nightly")
}

pub fn data_name() -> OsString {
    env::var_os("XD_DATA_NAME")
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| if nightly() { "xd-nightly" } else { "xd" }.into())
}

pub fn app_id() -> String {
    env::var("XD_APP_ID").unwrap_or_else(|_| {
        if nightly() {
            "com.restartfu.Xd.Nightly"
        } else {
            "com.restartfu.Xd"
        }
        .into()
    })
}

pub fn installed_restart_path() -> Option<std::path::PathBuf> {
    restart_path_for_host(Path::new(&env::var_os("XD_HOST_EXECUTABLE")?))
}

fn restart_path_for_host(host: &Path) -> Option<std::path::PathBuf> {
    let libexec = host.parent()?;
    if libexec.file_name()? != "libexec" {
        return None;
    }
    let root = libexec.parent()?;
    if cfg!(target_os = "macos") {
        Some(root.parent()?.join("MacOS/xd"))
    } else {
        Some(root.join("xd.sh"))
    }
}

/// Keep the executable path out of shell source, including paths containing
/// whitespace or shell metacharacters. The helper outlives the current app.
pub fn restart_command(launcher: &Path, parent: u32) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "while kill -0 \"$1\" 2>/dev/null; do sleep 0.1; done; exec \"$2\"",
            "xd-restart",
        ])
        .arg(parent.to_string())
        .arg(launcher);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

fn host_update_channel(configured: Option<OsString>, nightly_build: bool) -> OsString {
    if configured.as_deref() == Some(OsStr::new("dev")) {
        "dev".into()
    } else if nightly_build {
        "nightly".into()
    } else {
        "release".into()
    }
}

pub fn configure_host(command: &mut Command, _launcher: &Path) {
    command.env("XD_DATA_NAME", data_name()).env(
        "XD_UPDATE_CHANNEL",
        host_update_channel(env::var_os("XD_UPDATE_CHANNEL"), nightly()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn restart_executes_launcher_paths_literally() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let root = env::temp_dir().join(format!("xd-restart-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let launcher = root.join("launcher with spaces; and $shell 'quotes'");
        fs::write(&launcher, "#!/bin/sh\nprintf ready\n").unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
        // An impossible parent PID makes the helper proceed immediately.
        let output = restart_command(&launcher, i32::MAX as u32)
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        assert_eq!(output.stdout, b"ready");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn build_profile_selects_the_default_channel_identity() {
        let nightly_build = option_env!("XD_BUILD_PROFILE") == Some("nightly");
        assert_eq!(nightly(), nightly_build);
        assert_eq!(
            data_name(),
            OsString::from(if nightly_build { "xd-nightly" } else { "xd" })
        );
        assert_eq!(
            app_id(),
            if nightly_build {
                "com.restartfu.Xd.Nightly"
            } else {
                "com.restartfu.Xd"
            }
        );
    }

    #[test]
    fn host_update_channel_preserves_explicit_dev_identity() {
        assert_eq!(
            host_update_channel(Some(OsString::from("dev")), true),
            OsString::from("dev")
        );
        assert_eq!(
            host_update_channel(Some(OsString::from("nightly")), false),
            OsString::from("release")
        );
        assert_eq!(
            host_update_channel(Some(OsString::from("release")), true),
            OsString::from("nightly")
        );
        assert_eq!(host_update_channel(None, true), OsString::from("nightly"));
        assert_eq!(host_update_channel(None, false), OsString::from("release"));
    }
}
