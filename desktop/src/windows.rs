use std::path::{Path, PathBuf};

/// Set portable-app defaults before GPUI or host threads start. Existing
/// overrides remain available for custom installations and CI fixtures.
#[cfg(target_os = "windows")]
pub fn configure_environment() {
    use std::env;

    let home = env::var_os("USERPROFILE").map(PathBuf::from);
    if let Some(home) = home {
        let roaming = env::var_os("APPDATA").map(PathBuf::from);
        let local = env::var_os("LOCALAPPDATA").map(PathBuf::from);
        for (key, path) in profile_paths(&home, roaming.as_deref(), local.as_deref()) {
            set_default(key, path);
        }
        set_default("HOME", home);
    }
    if let Ok(executable) = env::current_exe()
        && let Some(root) = executable.parent()
    {
        set_default("XD_APP_ROOT", root);
    }
    set_default(
        "XD_UPDATE_CHANNEL",
        if xd_desktop::channel::nightly() {
            "nightly"
        } else {
            "release"
        },
    );
}

#[cfg(target_os = "windows")]
fn set_default(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    if std::env::var_os(key).is_none_or(|value| value.is_empty()) {
        // Called only by main, before any threads exist.
        unsafe { std::env::set_var(key, value) };
    }
}

fn profile_paths(
    home: &Path,
    roaming: Option<&Path>,
    local: Option<&Path>,
) -> [(&'static str, PathBuf); 3] {
    let roaming = roaming
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join("AppData/Roaming"));
    let local = local
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join("AppData/Local"));
    [
        ("XDG_CONFIG_HOME", roaming),
        ("XDG_DATA_HOME", local.clone()),
        ("XDG_CACHE_HOME", local.join("Cache")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_launch_uses_user_profile_directories_without_shell_initialization() {
        let home = Path::new("C:/Users/Example User");
        let fallback = profile_paths(home, None, None);
        assert_eq!(fallback[0].1, home.join("AppData/Roaming"));
        assert_eq!(fallback[1].1, home.join("AppData/Local"));
        let roaming = Path::new("D:/Roaming");
        let local = Path::new("D:/Local");
        let configured = profile_paths(home, Some(roaming), Some(local));
        assert_eq!(configured[0].1, roaming);
        assert_eq!(configured[1].1, local);
        assert_eq!(configured[2].1, local.join("Cache"));
    }
}
