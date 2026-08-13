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

    #[test]
    fn source_builds_default_to_the_stable_channel() {
        assert!(!nightly());
        assert_eq!(data_name(), OsString::from("xd"));
        assert_eq!(app_id(), "com.restartfu.Xd");
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
