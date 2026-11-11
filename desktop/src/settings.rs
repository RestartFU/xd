use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
pub use xd_desktop::theme::ThemePreset;

use crate::browser::BrowserSession;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct BrowserState {
    pub open: bool,
    pub session: BrowserSession,
}

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
    Claude,
    Codex,
    /// Also what a writer xd no longer offers, such as JCode, falls back to,
    /// so an older settings file still loads. Serde needs it last for that.
    #[default]
    #[serde(other)]
    Chat,
}

impl GitWriter {
    pub fn backend(self) -> Option<&'static str> {
        match self {
            Self::Chat => None,
            Self::Claude => Some("claude"),
            Self::Codex => Some("codex"),
        }
    }
}

/// How a project's last new chat was set up, so the next one starts the same
/// way: the agent and model, and the worktree and branch it runs in.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct NewChatChoices {
    /// The agent's protocol name, such as `codex` or `claude`.
    pub agent: Option<String>,
    /// The model, or none for the agent's default.
    pub model: Option<String>,
    pub new_worktree: bool,
    /// The checkout a chat in the current worktree runs in.
    pub checkout: Option<String>,
    /// The ref a new worktree branches from.
    pub base: Option<String>,
    pub start_from_remote: bool,
    pub effort: Option<String>,
    pub access: Option<String>,
    pub plan: Option<bool>,
}

impl Default for NewChatChoices {
    fn default() -> Self {
        Self {
            agent: None,
            model: None,
            new_worktree: false,
            checkout: None,
            base: None,
            start_from_remote: true,
            effort: None,
            access: None,
            plan: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct AppSettings {
    pub theme: ThemePreset,
    pub accent: AccentPreset,
    pub notifications: bool,
    pub allow_all_permissions: bool,
    /// Off by default: it starts agents working without anyone asking.
    pub auto_resume_chats: bool,
    pub git_writer: GitWriter,
    pub git_writer_model: Option<String>,
    pub build_source: String,
    pub favorite_models: Vec<String>,
    /// Legacy SSH command retained for migration of per-host UI state.
    /// New remote connections use pinned daemon credentials.
    pub remote_ssh_command: Option<String>,
    pub remote_credentials: Option<xd_transport::RemoteCredentials>,
    pub active_connection: Option<String>,
    /// Last selected chat per host connection. `last_chat` remains as a
    /// backwards-compatible fallback for existing local settings files.
    pub last_chats: HashMap<String, String>,
    pub last_chat: Option<String>,
    /// Collapsed workspace folders per host connection. The legacy flat
    /// list below remains the fallback for the local host.
    pub collapsed_folder_sets: HashMap<String, Vec<String>>,
    pub collapsed_folders: Vec<String>,
    /// Sessions folded over the sessions their agents started, per host
    /// connection.
    pub collapsed_session_sets: HashMap<String, Vec<String>>,
    /// Whether the sidebar's Settled shelf is open on this device.
    pub settled_shelf_expanded: bool,
    pub collapsed_diff_files: HashMap<String, Vec<String>>,
    pub expanded_file_directories: HashMap<String, Vec<String>>,
    pub catalog_width: u16,
    pub sidebar_files_height: u16,
    pub diff_width: u16,
    pub terminal_height: u16,
    pub browser_width: u16,
    /// Last browser address per connection and project.
    pub browser_urls: HashMap<String, String>,
    /// Open browser tabs and their selection per connection and chat.
    pub browser_sessions: HashMap<String, BrowserState>,
    pub window_width: u16,
    pub window_height: u16,
    pub window_maximized: bool,
    pub pane_states: HashMap<String, u8>,
    /// Active and hidden chat tabs within each connection's shared sessions.
    pub session_chats: HashMap<String, String>,
    pub hidden_chat_tabs: HashMap<String, Vec<String>>,
    /// The Usage page's last view: `cost`, `tokens`, or `limits`.
    pub usage_metric: String,
    pub usage_window_days: u32,
    pub usage_hidden_providers: Vec<String>,
    /// The choices of each project's last new chat, per connection and project.
    pub new_chat_choices: HashMap<String, NewChatChoices>,
    #[serde(skip)]
    pub new_chat_drafts: HashMap<String, serde_json::Value>,
    #[serde(skip)]
    load_error: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreset::Dark,
            accent: AccentPreset::Blue,
            notifications: true,
            allow_all_permissions: false,
            auto_resume_chats: false,
            git_writer: GitWriter::Chat,
            git_writer_model: None,
            build_source: String::new(),
            favorite_models: Vec::new(),
            remote_ssh_command: None,
            remote_credentials: None,
            active_connection: None,
            last_chats: HashMap::new(),
            last_chat: None,
            collapsed_folder_sets: HashMap::new(),
            collapsed_folders: Vec::new(),
            collapsed_session_sets: HashMap::new(),
            settled_shelf_expanded: false,
            collapsed_diff_files: HashMap::new(),
            expanded_file_directories: HashMap::new(),
            catalog_width: 256,
            sidebar_files_height: 280,
            diff_width: 460,
            terminal_height: 320,
            browser_width: 460,
            browser_urls: HashMap::new(),
            browser_sessions: HashMap::new(),
            window_width: 1180,
            window_height: 780,
            window_maximized: false,
            pane_states: HashMap::new(),
            session_chats: HashMap::new(),
            hidden_chat_tabs: HashMap::new(),
            usage_metric: "limits".into(),
            usage_window_days: 30,
            usage_hidden_providers: Vec::new(),
            new_chat_choices: HashMap::new(),
            new_chat_drafts: HashMap::new(),
            load_error: None,
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let path = settings_path();
        match fs::symlink_metadata(&path) {
            Ok(_) => Self::load_preserving(&path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let settings = Self::default();
                let _ = save_to(&path, &settings);
                settings
            }
            Err(error) => Self {
                load_error: Some(format!("Cannot inspect app settings: {error}")),
                ..Self::default()
            },
        }
    }

    /// Saves in the background: the write syncs to disk, which can take a
    /// while on a busy disk, and switching chats or navigating the browser
    /// saves. Only the newest settings are written, in order; a failure is
    /// reported through [`take_save_error`], and [`flush_saves`] waits for
    /// the write before the app exits.
    pub fn save(&self) -> Result<(), String> {
        // Persistence is tested through save_preserving with explicit paths.
        // GPUI fixtures must not write to the user's settings or each other.
        if cfg!(test) {
            return Ok(());
        }
        if let Some(error) = &self.load_error {
            return Err(format!(
                "App settings were not saved because the existing file is invalid: {error}"
            ));
        }
        let bytes = encode(self)?;
        save_writer()
            .send(SaveJob::Write(settings_path(), bytes))
            .map_err(|_| "Cannot save app settings: the writer stopped.".to_owned())
    }

    fn load_preserving(path: &Path) -> Self {
        load_from(path).unwrap_or_else(|error| Self {
            load_error: Some(error),
            ..Self::default()
        })
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    #[cfg(test)]
    fn save_preserving(&self, path: &Path) -> Result<(), String> {
        if let Some(error) = &self.load_error {
            return Err(format!(
                "App settings were not saved because the existing file is invalid: {error}"
            ));
        }
        save_to(path, self)
    }
}

pub(super) fn settings_path() -> PathBuf {
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

enum SaveJob {
    Write(PathBuf, Vec<u8>),
    Flush(mpsc::Sender<()>),
}

static SAVE_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// The last background save that failed, once.
pub fn take_save_error() -> Option<String> {
    SAVE_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Waits, for a short while, until every saved setting is on disk.
pub fn flush_saves() {
    if cfg!(test) {
        return;
    }
    let (reply, done) = mpsc::channel();
    if save_writer().send(SaveJob::Flush(reply)).is_ok() {
        // Every save the app made is on disk before it goes, as when saving
        // was synchronous, however long the disk takes.
        let _ = done.recv();
    }
}

fn save_writer() -> &'static mpsc::Sender<SaveJob> {
    static WRITER: OnceLock<mpsc::Sender<SaveJob>> = OnceLock::new();
    WRITER.get_or_init(|| {
        let (sender, jobs) = mpsc::channel();
        std::thread::Builder::new()
            .name("xd-settings-writer".into())
            .spawn(move || run_save_writer(jobs))
            .expect("start the settings writer");
        sender
    })
}

/// Writes saves in order. Saves that queue up behind a slow write collapse
/// into the newest one for each file.
fn run_save_writer(jobs: mpsc::Receiver<SaveJob>) {
    let mut pending: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let write_pending = |pending: &mut Vec<(PathBuf, Vec<u8>)>| {
        for (path, bytes) in pending.drain(..) {
            if let Err(error) = write_bytes(&path, &bytes) {
                *SAVE_ERROR
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
            }
        }
    };
    loop {
        let job = if pending.is_empty() {
            match jobs.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        } else {
            match jobs.try_recv() {
                Ok(job) => job,
                Err(mpsc::TryRecvError::Empty) => {
                    write_pending(&mut pending);
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    write_pending(&mut pending);
                    return;
                }
            }
        };
        match job {
            SaveJob::Write(path, bytes) => {
                pending.retain(|(queued, _)| queued != &path);
                pending.push((path, bytes));
            }
            SaveJob::Flush(reply) => {
                write_pending(&mut pending);
                let _ = reply.send(());
            }
        }
    }
}

fn encode(settings: &AppSettings) -> Result<Vec<u8>, String> {
    serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("Cannot encode app settings: {error}"))
}

fn save_to(path: &Path, settings: &AppSettings) -> Result<(), String> {
    write_bytes(path, &encode(settings)?)
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "App settings need a parent directory.".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("Cannot create app settings: {error}"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".settings-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("Cannot prepare app settings: {error}"))?;
        file.write_all(bytes)
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
    fn a_retired_git_writer_falls_back_to_the_chat_assistant() {
        let settings: AppSettings =
            serde_json::from_str(r#"{"git_writer":"jcode","theme":"light"}"#).unwrap();
        assert_eq!(settings.git_writer, GitWriter::Chat);
        assert_eq!(settings.theme, ThemePreset::Light);
    }

    #[test]
    fn background_saves_write_the_newest_settings_before_a_flush_returns() {
        let directory = env::temp_dir().join(format!(
            "xd-settings-writer-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("settings.json");
        let (sender, jobs) = mpsc::channel();
        let writer = std::thread::spawn(move || run_save_writer(jobs));
        sender
            .send(SaveJob::Write(path.clone(), b"first".to_vec()))
            .unwrap();
        sender
            .send(SaveJob::Write(path.clone(), b"second".to_vec()))
            .unwrap();
        let (reply, done) = mpsc::channel();
        sender.send(SaveJob::Flush(reply)).unwrap();
        done.recv().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");

        sender
            .send(SaveJob::Write(path.clone(), b"third".to_vec()))
            .unwrap();
        drop(sender);
        writer.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"third", "written before exiting");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_settings_are_not_replaced_by_defaults() {
        let directory = env::temp_dir().join(format!(
            "xd-malformed-settings-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("settings.json");
        fs::create_dir_all(&directory).unwrap();
        let malformed = br#"{"theme":"dark""#;
        fs::write(&path, malformed).unwrap();

        let settings = AppSettings::load_preserving(&path);
        assert!(settings.load_error().is_some());
        assert!(settings.save_preserving(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), malformed);

        fs::remove_dir_all(directory).unwrap();
    }

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
            session_chats: HashMap::from([("local/session/workspace".into(), "review".into())]),
            hidden_chat_tabs: HashMap::from([(
                "local/session/workspace".into(),
                vec!["implementation".into()],
            )]),
            usage_metric: "tokens".into(),
            usage_window_days: 7,
            usage_hidden_providers: vec!["codex".into()],
            theme: ThemePreset::Light,
            accent: AccentPreset::Purple,
            notifications: false,
            allow_all_permissions: true,
            auto_resume_chats: true,
            git_writer: GitWriter::Claude,
            git_writer_model: Some("claude-opus-5".into()),
            build_source: "#128".into(),
            favorite_models: vec!["claude/claude-opus-5".into()],
            remote_ssh_command: Some("ssh dev.example -p 22".into()),
            remote_credentials: None,
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
            collapsed_session_sets: HashMap::from([("local".into(), vec!["session:lead".into()])]),
            settled_shelf_expanded: true,
            collapsed_diff_files: HashMap::from([(
                "local/chat-restore/working".into(),
                vec!["desktop/src/main.rs".into()],
            )]),
            expanded_file_directories: HashMap::from([(
                "remote/dev.example:4001/chat-remote".into(),
                vec!["desktop".into(), "desktop/src".into()],
            )]),
            catalog_width: 318,
            sidebar_files_height: 336,
            diff_width: 512,
            terminal_height: 280,
            browser_width: 540,
            browser_urls: HashMap::from([(
                "local/project/folder-a".into(),
                "http://localhost:3000/".into(),
            )]),
            browser_sessions: HashMap::from([
                (
                    "local/chat/chat-restore".into(),
                    BrowserState {
                        open: true,
                        session: BrowserSession {
                            tabs: vec![
                                Some("http://localhost:3000/".into()),
                                Some("https://example.com/".into()),
                                None,
                            ],
                            active_tab: 1,
                        },
                    },
                ),
                (
                    "local/chat/chat-closed".into(),
                    BrowserState {
                        open: false,
                        session: BrowserSession {
                            tabs: vec![Some("https://example.com/closed".into())],
                            active_tab: 0,
                        },
                    },
                ),
            ]),
            window_width: 1440,
            window_height: 900,
            window_maximized: true,
            pane_states: HashMap::from([
                ("local/chat-restore".into(), 5),
                ("remote/dev.example:4001/chat-remote".into(), 1),
            ]),
            new_chat_choices: HashMap::from([(
                "local/project/folder-a".into(),
                NewChatChoices {
                    agent: Some("claude".into()),
                    model: Some("claude-opus-5".into()),
                    new_worktree: true,
                    checkout: None,
                    base: Some("refs/remotes/origin/dev".into()),
                    start_from_remote: true,
                    ..Default::default()
                },
            )]),
            new_chat_drafts: HashMap::new(),
            load_error: None,
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
    fn remote_start_defaults_on_and_preserves_saved_choices() {
        assert!(NewChatChoices::default().start_from_remote);
        let legacy: NewChatChoices = serde_json::from_str(r#"{"new_worktree":true}"#).unwrap();
        assert!(legacy.start_from_remote);
        for enabled in [false, true] {
            let saved: NewChatChoices =
                serde_json::from_value(serde_json::json!({"start_from_remote": enabled})).unwrap();
            assert_eq!(saved.start_from_remote, enabled);
            let restored: NewChatChoices =
                serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
            assert_eq!(restored, saved);
        }
    }

    #[test]
    fn settings_without_a_theme_use_the_dark_default() {
        let settings: AppSettings =
            serde_json::from_str(r#"{"accent":"green","notifications":true}"#).unwrap();

        assert_eq!(settings.theme, ThemePreset::Dark);
    }

    #[test]
    fn auto_resume_is_off_unless_chosen_and_survives_serialization() {
        let defaults = serde_json::to_value(AppSettings::default()).unwrap();
        assert_eq!(defaults["auto_resume_chats"], false);
        let older: AppSettings = serde_json::from_str(r#"{"speech":true}"#).unwrap();
        assert!(!older.auto_resume_chats);
        let enabled: AppSettings = serde_json::from_str(r#"{"auto_resume_chats":true}"#).unwrap();
        assert_eq!(
            serde_json::to_value(enabled).unwrap()["auto_resume_chats"],
            true
        );
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
    fn removed_experiment_mode_is_ignored_in_existing_settings() {
        let settings: AppSettings =
            serde_json::from_str(r#"{"experiment_mode":true,"allow_all_permissions":true}"#)
                .unwrap();
        assert!(settings.allow_all_permissions);
        assert!(
            serde_json::to_value(settings)
                .unwrap()
                .get("experiment_mode")
                .is_none()
        );
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
