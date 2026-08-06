use std::{
    collections::HashMap,
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
    pub favorite_models: Vec<String>,
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
            favorite_models: Vec::new(),
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
        let path = settings_path();
        match fs::symlink_metadata(&path) {
            Ok(_) => load_from(&path).unwrap_or_default(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let settings = import_legacy_settings().unwrap_or_default();
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

const LEGACY_DCONF_PATHS: [&str; 2] = ["/com/restartfu/XdNightly/", "/com/restartfu/Hy/"];
const DCONF_OUTPUT_LIMIT: u64 = 256 * 1024;
const DCONF_TIMEOUT: Duration = Duration::from_millis(750);

fn import_legacy_settings() -> Option<AppSettings> {
    LEGACY_DCONF_PATHS
        .iter()
        .find_map(|path| read_dconf(path).and_then(|dump| import_legacy_dump(&dump)))
}

fn read_dconf(path: &str) -> Option<String> {
    let mut child = Command::new("dconf")
        .args(["dump", path])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let reader = match thread::Builder::new()
        .name("xd-dev-dconf-import".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(DCONF_OUTPUT_LIMIT + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        }) {
        Ok(reader) => reader,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let deadline = Instant::now() + DCONF_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let bytes = reader.join().ok()?.ok()?;
    if !status?.success() || bytes.is_empty() || bytes.len() as u64 > DCONF_OUTPUT_LIMIT {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn import_legacy_dump(dump: &str) -> Option<AppSettings> {
    let values = dump
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('[') && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect::<HashMap<_, _>>();
    let mut settings = AppSettings::default();
    let mut imported = false;

    if let Some(width) = bounded_u16(values.get("window-width").copied(), 8_192) {
        settings.window_width = width;
        imported = true;
    }
    if let Some(height) = bounded_u16(values.get("window-height").copied(), 8_192) {
        settings.window_height = height;
        imported = true;
    }
    if let Some(maximized) = variant_bool(values.get("window-maximized").copied()) {
        settings.window_maximized = maximized;
        imported = true;
    }
    if let Some(width) = bounded_u16(values.get("sidebar-width").copied(), 2_048) {
        settings.sidebar_width = width;
        imported = true;
    }
    if let Some(width) = bounded_u16(values.get("diff-width").copied(), 4_096) {
        settings.diff_width = width;
        imported = true;
    }
    if let Some(height) = bounded_u16(values.get("terminal-height").copied(), 4_096) {
        settings.terminal_height = height;
        imported = true;
    }
    if let Some(favorites) = values
        .get("favorite-models")
        .and_then(|value| variant_strings(value))
    {
        settings.favorite_models = favorites
            .into_iter()
            .filter(|value| !value.is_empty() && value.len() <= 256)
            .take(128)
            .collect();
        imported = true;
    }
    if let Some(active) = values
        .get("active-chat")
        .and_then(|value| variant_string(value))
        .and_then(|value| value.strip_prefix("local:").map(str::to_owned))
        .filter(|value| !value.is_empty() && value.len() <= 256)
    {
        settings.last_chat = Some(active);
        imported = true;
    }
    if let Some(backend) = values
        .get("git-writing-backend")
        .and_then(|value| variant_string(value))
    {
        settings.git_writer = match backend.as_str() {
            "claude" => GitWriter::Claude,
            "codex" => GitWriter::Codex,
            _ => GitWriter::Chat,
        };
        imported = true;
    }
    if settings.git_writer != GitWriter::Chat
        && let Some(model) = values
            .get("git-writing-model")
            .and_then(|value| variant_string(value))
            .filter(|value| !value.is_empty() && value.len() <= 256)
    {
        settings.git_writer_model = Some(model);
        imported = true;
    }
    if let Some(states) = values
        .get("pane-state")
        .and_then(|value| variant_u32_map(value))
    {
        settings.pane_states = states
            .into_iter()
            .filter(|(key, _)| !key.is_empty() && key.len() <= 512)
            .take(512)
            .map(|(key, state)| (key, (state & 0b0101) as u8))
            .collect();
        imported = true;
    }

    imported.then_some(settings)
}

fn bounded_u16(value: Option<&str>, maximum: u16) -> Option<u16> {
    value?
        .parse::<u16>()
        .ok()
        .filter(|value| *value > 0 && *value <= maximum)
}

fn variant_bool(value: Option<&str>) -> Option<bool> {
    match value? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn variant_string(value: &str) -> Option<String> {
    let mut cursor = 0;
    let result = quoted_string(value.trim(), &mut cursor)?;
    (value.trim()[cursor..].trim().is_empty()).then_some(result)
}

fn variant_strings(value: &str) -> Option<Vec<String>> {
    let value = value.trim();
    if !value.starts_with('[') || !value.ends_with(']') {
        return None;
    }
    let mut cursor = 1;
    let end = value.len() - 1;
    let mut strings = Vec::new();
    loop {
        skip_ascii_whitespace(value, &mut cursor);
        if cursor == end {
            return Some(strings);
        }
        strings.push(quoted_string(value, &mut cursor)?);
        skip_ascii_whitespace(value, &mut cursor);
        match value.as_bytes().get(cursor) {
            Some(b',') => cursor += 1,
            _ if cursor == end => return Some(strings),
            _ => return None,
        }
    }
}

fn variant_u32_map(value: &str) -> Option<HashMap<String, u32>> {
    let value = value.trim();
    if !value.starts_with('{') || !value.ends_with('}') {
        return None;
    }
    let mut cursor = 1;
    let end = value.len() - 1;
    let mut values = HashMap::new();
    loop {
        skip_ascii_whitespace(value, &mut cursor);
        if cursor == end {
            return Some(values);
        }
        let key = quoted_string(value, &mut cursor)?;
        skip_ascii_whitespace(value, &mut cursor);
        if value.as_bytes().get(cursor) != Some(&b':') {
            return None;
        }
        cursor += 1;
        skip_ascii_whitespace(value, &mut cursor);
        if value[cursor..].starts_with("uint32") {
            cursor += "uint32".len();
            skip_ascii_whitespace(value, &mut cursor);
        }
        let start = cursor;
        while value.as_bytes().get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if start == cursor {
            return None;
        }
        let state = value[start..cursor].parse().ok()?;
        values.insert(key, state);
        skip_ascii_whitespace(value, &mut cursor);
        match value.as_bytes().get(cursor) {
            Some(b',') => cursor += 1,
            _ if cursor == end => return Some(values),
            _ => return None,
        }
    }
}

fn quoted_string(value: &str, cursor: &mut usize) -> Option<String> {
    skip_ascii_whitespace(value, cursor);
    let quote = *value.as_bytes().get(*cursor)?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    *cursor += 1;
    let mut bytes = Vec::new();
    while let Some(byte) = value.as_bytes().get(*cursor).copied() {
        *cursor += 1;
        if byte == quote {
            return String::from_utf8(bytes).ok();
        }
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        let escaped = value.as_bytes().get(*cursor).copied()?;
        *cursor += 1;
        bytes.push(match escaped {
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'\\' => b'\\',
            b'\'' => b'\'',
            b'"' => b'"',
            _ => return None,
        });
    }
    None
}

fn skip_ascii_whitespace(value: &str, cursor: &mut usize) {
    while value
        .as_bytes()
        .get(*cursor)
        .is_some_and(u8::is_ascii_whitespace)
    {
        *cursor += 1;
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
            favorite_models: vec!["claude/claude-opus-5".into()],
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

    #[test]
    fn imports_only_compatible_legacy_dconf_settings() {
        let settings = import_legacy_dump(
            r#"
[/]
window-width=1440
window-height=900
window-maximized=true
sidebar-width=312
diff-width=540
terminal-height=280
favorite-models=['codex/gpt-5.4', 'claude/claude-opus-4-6']
active-chat='local:chat-123'
git-writing-backend='codex'
git-writing-model='gpt-5.4'
expanded-folders=['folder-open']
pane-state={'local/chat-123': uint32 5, 'remote/host:4001/chat': uint32 3}
"#,
        )
        .unwrap();
        assert_eq!(settings.window_width, 1440);
        assert_eq!(settings.window_height, 900);
        assert!(settings.window_maximized);
        assert_eq!(settings.sidebar_width, 312);
        assert_eq!(settings.diff_width, 540);
        assert_eq!(settings.terminal_height, 280);
        assert_eq!(settings.favorite_models.len(), 2);
        assert_eq!(settings.last_chat.as_deref(), Some("chat-123"));
        assert_eq!(settings.git_writer, GitWriter::Codex);
        assert_eq!(settings.git_writer_model.as_deref(), Some("gpt-5.4"));
        assert_eq!(settings.pane_states["local/chat-123"], 5);
        assert_eq!(settings.pane_states["remote/host:4001/chat"], 1);
        assert!(settings.collapsed_folders.is_empty());
    }

    #[test]
    fn legacy_variant_parser_handles_escapes_and_rejects_malformed_values() {
        assert_eq!(
            variant_strings(r#"['one', 'two\'s', "three"]"#).unwrap(),
            ["one", "two's", "three"]
        );
        assert!(variant_strings("['unterminated]").is_none());
        assert!(variant_u32_map("{'chat': uint32 nope}").is_none());
        assert!(import_legacy_dump("[/]\nunknown='value'").is_none());
    }
}
