use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
pub use xd_desktop::theme::ThemePreset;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AccentPreset {
    #[default]
    Blue,
    Purple,
    Green,
    Orange,
    Pink,
    Red,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitWriter {
    #[default]
    Chat,
    Claude,
    Codex,
    Jcode,
}

impl GitWriter {
    pub fn backend(self) -> Option<&'static str> {
        match self {
            Self::Chat => None,
            Self::Claude => Some("claude"),
            Self::Codex => Some("codex"),
            Self::Jcode => Some("jcode"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct AppSettings {
    pub theme: ThemePreset,
    pub accent: AccentPreset,
    pub notifications: bool,
    pub speech: bool,
    pub allow_all_permissions: bool,
    pub git_writer: GitWriter,
    pub git_writer_model: Option<String>,
    pub build_source: String,
    pub favorite_models: Vec<String>,
    /// The exact SSH connection command used for remote mode. XD parses this
    /// into arguments and appends its own terminal session command; it is
    /// never evaluated by a shell.
    pub remote_ssh_command: Option<String>,
    pub active_connection: Option<String>,
    /// Last selected chat per host connection. `last_chat` remains as a
    /// backwards-compatible fallback for existing local settings files.
    pub last_chats: HashMap<String, String>,
    pub last_chat: Option<String>,
    /// Collapsed workspace folders per host connection. The legacy flat
    /// list below remains the fallback for the local host.
    pub collapsed_folder_sets: HashMap<String, Vec<String>>,
    pub collapsed_folders: Vec<String>,
    pub collapsed_diff_files: HashMap<String, Vec<String>>,
    pub expanded_file_directories: HashMap<String, Vec<String>>,
    pub sidebar_width: u16,
    pub sidebar_files_height: u16,
    pub diff_width: u16,
    pub terminal_height: u16,
    pub window_width: u16,
    pub window_height: u16,
    pub window_maximized: bool,
    pub pane_states: HashMap<String, u8>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreset::Dark,
            accent: AccentPreset::Blue,
            notifications: true,
            speech: false,
            allow_all_permissions: false,
            git_writer: GitWriter::Chat,
            git_writer_model: None,
            build_source: String::new(),
            favorite_models: Vec::new(),
            remote_ssh_command: None,
            active_connection: None,
            last_chats: HashMap::new(),
            last_chat: None,
            collapsed_folder_sets: HashMap::new(),
            collapsed_folders: Vec::new(),
            collapsed_diff_files: HashMap::new(),
            expanded_file_directories: HashMap::new(),
            sidebar_width: 272,
            sidebar_files_height: 280,
            diff_width: 460,
            terminal_height: 320,
            window_width: 1180,
            window_height: 780,
            window_maximized: false,
            pane_states: HashMap::new(),
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let path = settings_path();
        match fs::symlink_metadata(&path) {
            Ok(_) => load_from(&path).unwrap_or_default(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let settings = Self::default();
                let _ = save_to(&path, &settings);
                settings
            }
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        save_to(&settings_path(), self)
    }
}

fn settings_path() -> PathBuf {
    if let Some(path) = env::var_os("XD_SETTINGS_PATH").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from(".config"));
    let data_name = xd_desktop::channel::data_name();
    config_home.join(data_name).join("settings.json")
}

fn load_from(path: &Path) -> Result<AppSettings, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AppSettings::default());
        }
        Err(error) => return Err(format!("Cannot read app settings: {error}")),
    };
    serde_json::from_slice(&bytes).map_err(|error| format!("Cannot parse app settings: {error}"))
}

fn save_to(path: &Path, settings: &AppSettings) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "App settings need a parent directory.".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("Cannot create app settings: {error}"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".settings-{}-{nonce}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("Cannot encode app settings: {error}"))?;
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("Cannot prepare app settings: {error}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("Cannot write app settings: {error}"))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("Cannot replace app settings: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use xd_desktop::theme::ThemePreset;

    #[test]
    fn settings_round_trip_and_unknown_fields_are_ignored() {
        let directory = env::temp_dir().join(format!(
            "xd-settings-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("settings.json");
        let settings = AppSettings {
            theme: ThemePreset::Warm,
            accent: AccentPreset::Purple,
            notifications: false,
            speech: true,
            allow_all_permissions: true,
            git_writer: GitWriter::Claude,
            git_writer_model: Some("claude-opus-5".into()),
            build_source: "#128".into(),
            favorite_models: vec!["claude/claude-opus-5".into()],
            remote_ssh_command: Some("ssh dev.example -p 22".into()),
            active_connection: Some("remote/dev.example:4001".into()),
            last_chats: HashMap::from([
                ("local".into(), "chat-restore".into()),
                ("remote/dev.example:4001".into(), "chat-remote".into()),
            ]),
            last_chat: Some("chat-restore".into()),
            collapsed_folder_sets: HashMap::from([
                ("local".into(), vec!["folder-a".into(), "folder-b".into()]),
                (
                    "remote/dev.example:4001".into(),
                    vec!["folder-remote".into()],
                ),
            ]),
            collapsed_folders: vec!["folder-a".into(), "folder-b".into()],
            collapsed_diff_files: HashMap::from([(
                "local/chat-restore/working".into(),
                vec!["desktop/src/main.rs".into()],
            )]),
            expanded_file_directories: HashMap::from([(
                "remote/dev.example:4001/chat-remote".into(),
                vec!["desktop".into(), "desktop/src".into()],
            )]),
            sidebar_width: 318,
            sidebar_files_height: 336,
            diff_width: 512,
            terminal_height: 280,
            window_width: 1440,
            window_height: 900,
            window_maximized: true,
            pane_states: HashMap::from([
                ("local/chat-restore".into(), 5),
                ("remote/dev.example:4001/chat-remote".into(), 1),
            ]),
        };
        save_to(&path, &settings).unwrap();
        assert_eq!(load_from(&path).unwrap(), settings);
        fs::write(
            &path,
            br#"{"accent":"green","notifications":true,"future":1}"#,
        )
        .unwrap();
        assert_eq!(load_from(&path).unwrap().accent, AccentPreset::Green);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn settings_without_a_theme_use_the_dark_default() {
        let settings: AppSettings =
            serde_json::from_str(r#"{"accent":"green","notifications":true}"#).unwrap();

        assert_eq!(settings.theme, ThemePreset::Dark);
    }

    #[test]
    fn all_permissions_is_safe_by_default_and_survives_serialization() {
        let defaults = serde_json::to_value(AppSettings::default()).unwrap();
        assert_eq!(defaults["allow_all_permissions"], false);

        let enabled: AppSettings =
            serde_json::from_str(r#"{"allow_all_permissions":true}"#).unwrap();
        let enabled = serde_json::to_value(enabled).unwrap();
        assert_eq!(enabled["allow_all_permissions"], true);
    }

    #[test]
    fn remote_ssh_command_is_optional_and_survives_serialization() {
        let defaults: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(defaults.remote_ssh_command, None);

        let configured: AppSettings =
            serde_json::from_str(r#"{"remote_ssh_command":"ssh zenomc.org -p 22"}"#).unwrap();
        let saved = serde_json::to_value(configured).unwrap();
        assert_eq!(saved["remote_ssh_command"], "ssh zenomc.org -p 22");
    }

    #[test]
    fn collapsed_diff_files_survive_settings_round_trip() {
        let directory = env::temp_dir().join(format!(
            "xd-diff-settings-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("settings.json");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            &path,
            br#"{
                "collapsed_diff_files": {
                    "local/chat-one/working": ["src/main.rs", "README.md"]
                }
            }"#,
        )
        .unwrap();

        let settings = load_from(&path).unwrap();
        save_to(&path, &settings).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            saved["collapsed_diff_files"]["local/chat-one/working"],
            serde_json::json!(["src/main.rs", "README.md"])
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn connection_ui_state_survives_settings_round_trip() {
        let directory = env::temp_dir().join(format!(
            "xd-connection-state-settings-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("settings.json");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            &path,
            br#"{
                "active_connection": "remote/dev.example:4001",
                "last_chats": {
                    "local": "local-chat",
                    "remote/dev.example:4001": "remote-chat"
                },
                "collapsed_folder_sets": {
                    "local": ["local-folder"],
                    "remote/dev.example:4001": ["remote-folder"]
                }
            }"#,
        )
        .unwrap();

        let settings = load_from(&path).unwrap();
        save_to(&path, &settings).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

        assert_eq!(saved["active_connection"], "remote/dev.example:4001");
        assert_eq!(
            saved["last_chats"]["remote/dev.example:4001"],
            "remote-chat"
        );
        assert_eq!(
            saved["collapsed_folder_sets"]["remote/dev.example:4001"],
            serde_json::json!(["remote-folder"])
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
