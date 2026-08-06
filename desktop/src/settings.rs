use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

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

impl AccentPreset {
    pub const ALL: [Self; 6] = [
        Self::Blue,
        Self::Purple,
        Self::Green,
        Self::Orange,
        Self::Pink,
        Self::Red,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Blue => "Blue",
            Self::Purple => "Purple",
            Self::Green => "Green",
            Self::Orange => "Orange",
            Self::Pink => "Pink",
            Self::Red => "Red",
        }
    }

    pub fn color(self) -> u32 {
        match self {
            Self::Blue => 0x6b8cff,
            Self::Purple => 0xa77bff,
            Self::Green => 0x42b883,
            Self::Orange => 0xe98949,
            Self::Pink => 0xe66da8,
            Self::Red => 0xe56870,
        }
    }

    pub fn hover_color(self) -> u32 {
        match self {
            Self::Blue => 0x7b98ff,
            Self::Purple => 0xb38cff,
            Self::Green => 0x52c493,
            Self::Orange => 0xf19a5f,
            Self::Pink => 0xee7db5,
            Self::Red => 0xed7880,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitWriter {
    #[default]
    Chat,
    Claude,
    Codex,
}

impl GitWriter {
    pub const ALL: [Self; 3] = [Self::Chat, Self::Claude, Self::Codex];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Use chat model",
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    pub fn backend(self) -> Option<&'static str> {
        match self {
            Self::Chat => None,
            Self::Claude => Some("claude"),
            Self::Codex => Some("codex"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct AppSettings {
    pub accent: AccentPreset,
    pub notifications: bool,
    pub speech: bool,
    pub git_writer: GitWriter,
    pub git_writer_model: Option<String>,
    pub last_chat: Option<String>,
    pub collapsed_folders: Vec<String>,
    pub sidebar_width: u16,
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
            accent: AccentPreset::Blue,
            notifications: true,
            speech: false,
            git_writer: GitWriter::Chat,
            git_writer_model: None,
            last_chat: None,
            collapsed_folders: Vec::new(),
            sidebar_width: 272,
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
        load_from(&settings_path()).unwrap_or_default()
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
    config_home.join("xd-dev/settings.json")
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

    #[test]
    fn settings_round_trip_and_unknown_fields_are_ignored() {
        let directory = env::temp_dir().join(format!(
            "xd-dev-settings-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("settings.json");
        let settings = AppSettings {
            accent: AccentPreset::Purple,
            notifications: false,
            speech: true,
            git_writer: GitWriter::Claude,
            git_writer_model: Some("claude-opus-5".into()),
            last_chat: Some("chat-restore".into()),
            collapsed_folders: vec!["folder-a".into(), "folder-b".into()],
            sidebar_width: 318,
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
}
