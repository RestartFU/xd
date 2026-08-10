use std::{env, ffi::OsString, path::Path, process::Command};

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

pub fn configure_daemon(command: &mut Command, _launcher: &Path) {
    command.env("XD_DATA_NAME", data_name()).env(
        "XD_UPDATE_CHANNEL",
        if nightly() { "nightly" } else { "release" },
    );

    configure_background(command);

    #[cfg(windows)]
    if let Some(bin) = _launcher.parent() {
        command
            .env("XD_TLS_PROXY_EXECUTABLE", bin.join("xd-tls-proxy.exe"))
            .env(
                "XD_CODEX_EXECUTABLE",
                bin.join("codex-package/bin/codex.exe"),
            )
            .env("XD_CLAUDE_EXECUTABLE", bin.join("claude.exe"))
            .env(
                "XD_CLAUDE_PROXY_EXECUTABLE",
                bin.join("claude-code-proxy.exe"),
            )
            .env("XD_WHISPER_SERVER", bin.join("whisper-server-bin.exe"));
        let mut paths = vec![bin.to_owned()];
        if let Some(root) = bin.parent() {
            paths.push(root.join("git/cmd"));
        }
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        if let Ok(path) = env::join_paths(paths) {
            command.env("PATH", path);
        }
    }
}

pub fn configure_background(_command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        _command.creation_flags(background_creation_flags(true));
    }
}

#[cfg(any(windows, test))]
fn background_creation_flags(windows: bool) -> u32 {
    if windows { 0x0800_0000 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_builds_default_to_the_stable_channel() {
        assert!(!nightly());
        assert_eq!(data_name(), OsString::from("xd"));
        assert_eq!(app_id(), "com.restartfu.Xd");
    }

    #[test]
    fn background_daemons_use_the_windows_no_console_flag() {
        assert_eq!(background_creation_flags(true), 0x0800_0000);
        assert_eq!(background_creation_flags(false), 0);
    }
}
