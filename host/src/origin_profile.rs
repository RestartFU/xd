//! Origin profiles: the environment a runner borrows from the device that
//! sends it work.
//!
//! The origin collects its agent configuration, sign-ins, Git and GitHub CLI
//! setup, SSH identity and login-shell variables into a manifest of files
//! addressed by content. A runner keeps one profile per origin under
//! `DATA/origins/<origin>/home`, laid out like the origin's home, and runs
//! that origin's processes with `CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
//! `GIT_CONFIG_GLOBAL`, `GH_CONFIG_DIR` and `GIT_SSH_COMMAND` pointed into it.
//! The runner's own home is never touched.
//!
//! The profile home is live: the CLIs write their transcripts, caches and
//! refreshed sign-ins into it. Applying a manifest therefore only replaces the
//! files the origin manages, and a sign-in the runner refreshed since the last
//! push is kept and offered back to the origin rather than overwritten.
//!
//! See docs/runners.md.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs, io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// One file is carried only up to this size.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// A whole profile is refused past this size.
const MAX_PROFILE_BYTES: u64 = 512 * 1024 * 1024;
/// Only text files up to this size have the origin's home rewritten in them.
const MAX_REWRITE_BYTES: usize = 4 * 1024 * 1024;
const STATE: &str = "state.json";
const ACTIVE: &str = "active";

/// Keys of the origin's `.claude.json` a runner takes. The rest is the CLI's
/// own bookkeeping (start counts, caches, per-path project state) and stays
/// with the machine that wrote it.
const CLAUDE_JSON_KEYS: &[&str] = &[
    "mcpServers",
    "oauthAccount",
    "userID",
    "hasCompletedOnboarding",
    "lastOnboardingVersion",
    "primaryApiKey",
    "customApiKeyResponses",
    "bypassPermissionsModeAccepted",
    "theme",
];

/// Files and folders of a provider home that a runner takes. Everything else
/// (transcripts, sessions, caches, databases, logs) is the origin's own.
const CLAUDE_ENTRIES: &[&str] = &[
    "CLAUDE.md",
    "settings.json",
    "keybindings.json",
    ".credentials.json",
    "skills",
    "agents",
    "commands",
    "plugins",
    "hooks",
    "output-styles",
    "rules",
];
const CODEX_ENTRIES: &[&str] = &[
    "AGENTS.md",
    "config.toml",
    "hooks.json",
    "auth.json",
    "skills",
    "prompts",
    "rules",
    "plugins",
];
/// Home-relative files and folders outside the provider homes.
const HOME_ENTRIES: &[&str] = &[
    ".gitconfig",
    ".git-credentials",
    ".config/git/config",
    ".config/git/ignore",
    ".config/git/attributes",
    ".config/gh/config.yml",
    ".config/gh/hosts.yml",
    ".agents/skills",
    ".ssh",
];

/// Login-shell variables that describe the machine or session rather than the
/// user, or that a profile sets itself.
const DENIED_VARIABLES: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PWD",
    "OLDPWD",
    "SHLVL",
    "_",
    "TERM",
    "COLORTERM",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "MAIL",
    "HOSTNAME",
    "HOSTTYPE",
    "MACHTYPE",
    "OSTYPE",
    "LS_COLORS",
    "LESSOPEN",
    "LESSCLOSE",
    "MOTD_SHOWN",
    "INVOCATION_ID",
    "JOURNAL_STREAM",
    "MANAGERPID",
    "NOTIFY_SOCKET",
    "SYSTEMD_EXEC_PID",
    "CLAUDECODE",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "GIT_CONFIG_GLOBAL",
    "GH_CONFIG_DIR",
    "GIT_SSH_COMMAND",
    "SSH_AUTH_SOCK",
];
const DENIED_PREFIXES: &[&str] = &[
    "XDG_",
    "DBUS_",
    "SSH_",
    "TMUX",
    "TERM_",
    "KITTY_",
    "VTE_",
    "BASH_FUNC_",
    "XD_",
    "CLAUDE_CODE_",
    "CODEX_",
    "T3_",
    "BUILDPOOL",
];

// ---------------------------------------------------------------------------
// The origin side: collecting a profile.
// ---------------------------------------------------------------------------

/// Where a collected file's bytes come from.
#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    Bytes(Vec<u8>),
}

impl Source {
    fn read(&self) -> io::Result<Vec<u8>> {
        match self {
            Self::File(path) => fs::read(path),
            Self::Bytes(bytes) => Ok(bytes.clone()),
        }
    }
}

/// How a runner writes a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// The origin's copy replaces the runner's.
    Replace,
    /// The origin's top-level keys are written over the runner's JSON object,
    /// which keeps whatever the runner's CLI added.
    JsonKeys,
}

#[derive(Clone, Debug)]
pub struct CollectedFile {
    /// Relative to the origin's home, `/`-separated.
    pub path: String,
    pub mode: u32,
    pub sha256: String,
    pub size: u64,
    pub secret: bool,
    pub merge: Merge,
    pub source: Source,
}

/// A profile ready to offer to a runner.
#[derive(Debug, Default)]
pub struct Collected {
    pub origin_home: String,
    pub files: Vec<CollectedFile>,
    pub env: BTreeMap<String, String>,
    /// What was left out and why, for the person pushing.
    pub notes: Vec<String>,
}

impl Collected {
    pub fn manifest(&self) -> Value {
        json!({
            "origin_home": self.origin_home,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "files": self.files.iter().map(|file| json!({
                "path": file.path,
                "mode": file.mode,
                "sha256": file.sha256,
                "size": file.size,
                "secret": file.secret,
                "merge": match file.merge { Merge::Replace => "replace", Merge::JsonKeys => "json-keys" },
            })).collect::<Vec<_>>(),
            "env": self.env,
        })
    }

    /// The base64 bodies of the files whose hashes are in `wanted`.
    pub fn blobs(&self, wanted: &HashSet<String>) -> io::Result<Map<String, Value>> {
        let mut blobs = Map::new();
        for file in &self.files {
            if wanted.contains(&file.sha256) && !blobs.contains_key(&file.sha256) {
                let bytes = file.source.read()?;
                if sha256(&bytes) != file.sha256 {
                    return Err(io::Error::other(format!(
                        "{} changed while it was being sent; push again",
                        file.path
                    )));
                }
                blobs.insert(file.sha256.clone(), Value::String(STANDARD.encode(bytes)));
            }
        }
        Ok(blobs)
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

/// Collects this machine's profile. `claude_home` and `codex_home` are the
/// provider homes the user's CLIs use; `variables` is the login shell's
/// environment.
pub fn collect(
    home: &Path,
    claude_home: &Path,
    codex_home: &Path,
    claude_json: &Path,
    variables: &HashMap<String, String>,
) -> io::Result<Collected> {
    let origin_home = home.to_string_lossy().trim_end_matches('/').to_owned();
    let mut collector = Collector {
        home: home.to_path_buf(),
        collected: Collected {
            origin_home: origin_home.clone(),
            ..Collected::default()
        },
        seen: HashSet::new(),
        visited: HashSet::new(),
    };
    for entry in CLAUDE_ENTRIES {
        collector.add_tree(&claude_home.join(entry), &format!(".claude/{entry}"))?;
    }
    for entry in CODEX_ENTRIES {
        collector.add_tree(&codex_home.join(entry), &format!(".codex/{entry}"))?;
    }
    for entry in HOME_ENTRIES {
        collector.add_tree(&home.join(entry), entry)?;
    }
    collector.add_claude_json(claude_json)?;
    collector.add_referenced()?;
    if collector.collected.total_bytes() > MAX_PROFILE_BYTES {
        return Err(io::Error::other(format!(
            "the profile is {} MiB, over the {} MiB limit",
            collector.collected.total_bytes() / (1024 * 1024),
            MAX_PROFILE_BYTES / (1024 * 1024)
        )));
    }
    for (name, value) in variables {
        if denied_variable(name) {
            continue;
        }
        // Paths into the origin's home name its toolchains (GOPATH, CARGO_HOME,
        // NVM_DIR…), which a runner has at its own paths or not at all.
        if mentions_home(value, &origin_home) {
            collector
                .collected
                .notes
                .push(format!("left out {name}: it points into the origin's home"));
            continue;
        }
        collector
            .collected
            .env
            .insert(name.clone(), value.clone());
    }
    collector
        .collected
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(collector.collected)
}

/// Collects this machine's own profile, from the provider homes and login
/// shell its CLIs use.
pub fn collect_local() -> io::Result<Collected> {
    let variables = crate::user_environment::login_environment();
    let lookup = |name: &str| {
        variables
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
            .filter(|value| !value.is_empty())
    };
    let home = PathBuf::from(lookup("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?);
    let claude_configured = lookup("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let claude_home = claude_configured
        .clone()
        .unwrap_or_else(|| home.join(".claude"));
    let claude_json = match claude_configured {
        Some(directory) => directory.join(".claude.json"),
        None => home.join(".claude.json"),
    };
    let codex_home = lookup("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let mut collected = collect(&home, &claude_home, &codex_home, &claude_json, &variables)?;
    if cfg!(target_os = "macos") && !collected.files.iter().any(|f| f.path == ".claude/.credentials.json") {
        collected
            .notes
            .push("Claude's sign-in is in the macOS keychain, which this build does not read yet".into());
    }
    Ok(collected)
}

struct Collector {
    home: PathBuf,
    collected: Collected,
    seen: HashSet<String>,
    /// Linked folders being read, so a link loop ends.
    visited: HashSet<PathBuf>,
}

impl Collector {
    fn depth(&self, path: &str) -> usize {
        path.matches('/').count()
    }

    fn add_tree(&mut self, source: &Path, path: &str) -> io::Result<()> {
        let metadata = match fs::symlink_metadata(source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if skipped_name(name) {
            return Ok(());
        }
        // Linked folders are followed, so a skill linked in from a checkout
        // comes along, even when two homes link the same one. Only a link
        // back into a folder being read (a loop) stops.
        let mut entered = None;
        let metadata = if metadata.file_type().is_symlink() {
            match fs::metadata(source) {
                Ok(target) => {
                    if target.is_dir() {
                        let Ok(real) = fs::canonicalize(source) else {
                            return Ok(());
                        };
                        if !self.visited.insert(real.clone()) {
                            return Ok(());
                        }
                        entered = Some(real);
                    }
                    target
                }
                Err(_) => {
                    self.collected
                        .notes
                        .push(format!("left out {path}: a link to nothing"));
                    return Ok(());
                }
            }
        } else {
            metadata
        };
        if metadata.is_dir() && self.depth(path) > 24 {
            return Ok(());
        }
        if metadata.is_dir() {
            let mut children = fs::read_dir(source)?
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect::<Vec<_>>();
            children.sort();
            let result = children.iter().try_for_each(|child| {
                let Some(child_name) = child.to_str() else {
                    return Ok(());
                };
                self.add_tree(&source.join(child_name), &format!("{path}/{child_name}"))
            });
            if let Some(real) = entered {
                self.visited.remove(&real);
            }
            return result;
        }
        if metadata.is_file() {
            return self.add_file(source, path, metadata.len(), metadata.permissions().mode());
        }
        Ok(())
    }

    fn add_file(&mut self, source: &Path, path: &str, size: u64, mode: u32) -> io::Result<()> {
        self.add_file_within(source, path, size, mode, MAX_FILE_BYTES)
    }

    fn add_file_within(
        &mut self,
        source: &Path,
        path: &str,
        size: u64,
        mode: u32,
        limit: u64,
    ) -> io::Result<()> {
        if !self.seen.insert(path.to_owned()) {
            return Ok(());
        }
        if size > limit {
            self.collected.notes.push(format!(
                "left out {path}: {} MiB is over the per-file limit",
                size / (1024 * 1024)
            ));
            return Ok(());
        }
        let bytes = fs::read(source)?;
        self.collected.files.push(CollectedFile {
            path: path.to_owned(),
            mode: mode & 0o777,
            sha256: sha256(&bytes),
            size: bytes.len() as u64,
            secret: secret_path(path),
            merge: Merge::Replace,
            source: Source::File(source.to_path_buf()),
        });
        Ok(())
    }

    /// Takes the shareable part of `.claude.json`, which holds the MCP servers
    /// and which account is signed in.
    fn add_claude_json(&mut self, source: &Path) -> io::Result<()> {
        let bytes = match fs::read(source) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let Ok(Value::Object(settings)) = serde_json::from_slice::<Value>(&bytes) else {
            self.collected
                .notes
                .push("left out .claude.json: it is not a JSON object".into());
            return Ok(());
        };
        let kept = settings
            .into_iter()
            .filter(|(key, _)| CLAUDE_JSON_KEYS.contains(&key.as_str()))
            .collect::<Map<_, _>>();
        let bytes = serde_json::to_vec_pretty(&Value::Object(kept)).map_err(io::Error::other)?;
        let path = ".claude/.claude.json".to_owned();
        self.seen.insert(path.clone());
        self.collected.files.push(CollectedFile {
            path,
            mode: 0o600,
            sha256: sha256(&bytes),
            size: bytes.len() as u64,
            secret: false,
            merge: Merge::JsonKeys,
            source: Source::Bytes(bytes),
        });
        Ok(())
    }

    /// Hooks, MCP servers and status lines name programs and scripts by
    /// absolute path. Those inside the origin's home come along so the
    /// rewritten configuration still finds them.
    fn add_referenced(&mut self) -> io::Result<()> {
        let home = self.collected.origin_home.clone();
        let configuration = self
            .collected
            .files
            .iter()
            .filter(|file| {
                !file.secret
                    && matches!(
                        file.path.as_str(),
                        ".claude/settings.json"
                            | ".claude/.claude.json"
                            | ".codex/config.toml"
                            | ".codex/hooks.json"
                            | ".gitconfig"
                            | ".config/git/config"
                            | ".ssh/config"
                    )
            })
            .map(|file| file.source.read())
            .collect::<io::Result<Vec<_>>>()?;
        let mut referenced = Vec::new();
        for bytes in configuration {
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            referenced.extend(home_paths(&text, &home));
        }
        referenced.sort();
        referenced.dedup();
        for absolute in referenced {
            let Some(relative) = absolute
                .strip_prefix(&home)
                .map(|rest| rest.trim_start_matches('/').to_owned())
            else {
                continue;
            };
            if relative.is_empty() || self.seen.contains(&relative) {
                continue;
            }
            let source = self.home.join(&relative);
            let Ok(metadata) = fs::metadata(&source) else {
                continue;
            };
            // Folders named in configuration are projects and data (Codex's
            // per-project trust, a launcher's data folder), not something a
            // runner can use at the origin's path.
            if !metadata.is_file() {
                continue;
            }
            if relative
                .split('/')
                .any(|part| matches!(part, ".venv" | "venv" | "site-packages"))
            {
                self.collected.notes.push(format!(
                    "left out ~/{relative}: a Python environment only works on the machine that made it"
                ));
                continue;
            }
            match self.bundle(&source) {
                Some(root) => match measure(&root) {
                    Some(_) => {
                        let root_relative = root
                            .strip_prefix(&self.home)
                            .map(|path| path.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.add_tree(&root, &root_relative)?;
                    }
                    None => {
                        let root_relative = root
                            .strip_prefix(&self.home)
                            .map(|path| path.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.collected.notes.push(format!(
                            "carried only ~/{relative}: the rest of ~/{root_relative} is over {} MiB or {} files, so it may not work on a runner",
                            MAX_BUNDLE_BYTES / (1024 * 1024),
                            MAX_BUNDLE_FILES
                        ));
                        self.add_file_within(
                            &source,
                            &relative,
                            metadata.len(),
                            metadata.permissions().mode(),
                            MAX_REFERENCED_BYTES,
                        )?;
                    }
                },
                None => {
                    self.add_file_within(
                        &source,
                        &relative,
                        metadata.len(),
                        metadata.permissions().mode(),
                        MAX_REFERENCED_BYTES,
                    )?;
                    // A tool in a shared bin folder keeps its settings in
                    // ~/.config/<tool>, like a token an MCP server reads.
                    if let Some(name) = source.file_name().and_then(|name| name.to_str()) {
                        let settings = self.home.join(".config").join(name);
                        if settings.is_dir() && measure(&settings).is_some() {
                            self.add_tree(&settings, &format!(".config/{name}"))?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// The folder a referenced script belongs to: the nearest one above it that
/// holds a project (a checkout, a package), else its own folder, unless that
/// is a shared `bin` folder where each program stands alone. Never the home
/// itself.
impl Collector {
    fn bundle(&self, file: &Path) -> Option<PathBuf> {
        let parent = file.parent()?;
        let mut folder = Some(parent);
        while let Some(current) = folder {
            if current == self.home || !current.starts_with(&self.home) {
                break;
            }
            if PROJECT_MARKERS
                .iter()
                .any(|marker| current.join(marker).exists())
            {
                return Some(current.to_path_buf());
            }
            folder = current.parent();
        }
        if parent == self.home || parent.file_name().is_some_and(|name| name == "bin") {
            return None;
        }
        Some(parent.to_path_buf())
    }
}

const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "package.json",
    "pyproject.toml",
    "Cargo.toml",
    "go.mod",
];
/// A referenced program's whole folder comes along up to these sizes.
const MAX_BUNDLE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_BUNDLE_FILES: usize = 5000;
/// A referenced program on its own, such as a runtime an MCP server starts.
const MAX_REFERENCED_BYTES: u64 = 192 * 1024 * 1024;

/// The bytes in `root` a profile would carry, or `None` past the bundle limits.
fn measure(root: &Path) -> Option<u64> {
    let mut bytes = 0;
    let mut files = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(&folder).ok()?.filter_map(Result::ok) {
            let name = entry.file_name();
            if skipped_name(name.to_str().unwrap_or("")) {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files += 1;
                bytes += entry.metadata().map_or(0, |metadata| metadata.len());
                if files > MAX_BUNDLE_FILES || bytes > MAX_BUNDLE_BYTES {
                    return None;
                }
            }
        }
    }
    Some(bytes)
}

fn skipped_name(name: &str) -> bool {
    name.is_empty()
        || name == ".DS_Store"
        || name == ".git"
        || name == ".venv"
        || name == "__pycache__"
        || name.ends_with(".lock")
        || name.ends_with(".old")
        || name.contains(".tmp")
        || name.ends_with(".bak")
        || name.contains(".bak-")
        || name.contains(".before-")
}

/// Sign-ins and keys: never rewritten, always private, and offered back to the
/// origin when the runner's CLI refreshes them.
fn secret_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        path,
        ".claude/.credentials.json"
            | ".codex/auth.json"
            | ".git-credentials"
            | ".config/gh/hosts.yml"
    ) || (path.starts_with(".ssh/")
        && name.starts_with("id_")
        && !name.ends_with(".pub"))
}

fn denied_variable(name: &str) -> bool {
    DENIED_VARIABLES.contains(&name)
        || DENIED_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
        || name.ends_with("_PID")
}

/// Whether `value` names a path inside `home`.
fn mentions_home(value: &str, home: &str) -> bool {
    !home_paths(value, home).is_empty() || value == home
}

/// Absolute paths inside `home` that appear in `text`.
fn home_paths(text: &str, home: &str) -> Vec<String> {
    let mut found = Vec::new();
    if home.is_empty() || home == "/" {
        return found;
    }
    let mut rest = text;
    let mut offset = 0;
    while let Some(index) = rest.find(home) {
        let start = offset + index;
        let after = start + home.len();
        let preceded = text[..start]
            .chars()
            .next_back()
            .is_none_or(|before| !path_character(before));
        if preceded && text[after..].starts_with('/') {
            let end = text[after..]
                .find(|character: char| !path_character(character))
                .map_or(text.len(), |length| after + length);
            found.push(text[start..end].trim_end_matches(['.', ':']).to_owned());
        }
        offset = after;
        rest = &text[after..];
    }
    found
}

fn path_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '/' | '.' | '_' | '-' | '+' | '@' | '~')
}

/// Replaces the origin's home with `target` wherever it starts a path.
fn rewrite_home(text: &str, home: &str, target: &str) -> String {
    if home.is_empty() || home == "/" {
        return text.to_owned();
    }
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    let mut consumed = 0;
    while let Some(index) = rest.find(home) {
        let start = consumed + index;
        let after = start + home.len();
        let preceded = text[..start]
            .chars()
            .next_back()
            .is_none_or(|before| !path_character(before));
        let followed = text[after..]
            .chars()
            .next()
            .is_none_or(|next| next == '/' || !path_character(next));
        result.push_str(&text[consumed..start]);
        result.push_str(if preceded && followed { target } else { home });
        consumed = after;
        rest = &text[after..];
    }
    result.push_str(&text[consumed..]);
    result
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------
// The runner side: applying profiles and running processes under them.
// ---------------------------------------------------------------------------

/// Where a runner keeps its origins' profiles: `DATA/origins`.
#[derive(Debug)]
pub struct Profiles {
    root: PathBuf,
    /// Applies one manifest at a time, and guards the active origin.
    lock: Mutex<Option<String>>,
    /// The active origin's variables, which every process start reads.
    /// Cleared by each push.
    cached: Mutex<Option<Vec<(String, String)>>>,
}

static INSTALLED: OnceLock<Profiles> = OnceLock::new();

/// Makes `root` this process's profile store and loads its active origin.
pub fn install(root: PathBuf) -> &'static Profiles {
    INSTALLED.get_or_init(|| Profiles::open(root))
}

pub fn installed() -> Option<&'static Profiles> {
    INSTALLED.get()
}

/// The variables every process runs with under the active origin's profile,
/// if one is active.
pub fn active_environment() -> Vec<(String, String)> {
    installed()
        .map(Profiles::active_environment)
        .unwrap_or_default()
}

pub fn valid_origin(origin: &str) -> bool {
    !origin.is_empty()
        && origin.len() <= 64
        && !origin.starts_with('.')
        && origin
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[derive(Debug, Default, Clone, PartialEq)]
struct Managed {
    /// The origin's hash of the file, before any rewrite.
    origin: String,
    /// The hash of what the runner wrote.
    applied: String,
    secret: bool,
}

#[derive(Debug, Default)]
struct State {
    origin_home: String,
    files: BTreeMap<String, Managed>,
    env: BTreeMap<String, String>,
    pushed_at_ms: u64,
}

impl State {
    fn to_json(&self) -> Value {
        json!({
            "origin_home": self.origin_home,
            "pushed_at_ms": self.pushed_at_ms,
            "env": self.env,
            "files": self.files.iter().map(|(path, managed)| (path.clone(), json!({
                "origin": managed.origin,
                "applied": managed.applied,
                "secret": managed.secret,
            }))).collect::<Map<_, _>>(),
        })
    }

    fn from_json(value: &Value) -> Self {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        Self {
            origin_home: text(&value["origin_home"]),
            pushed_at_ms: value["pushed_at_ms"].as_u64().unwrap_or(0),
            env: value["env"]
                .as_object()
                .map(|env| {
                    env.iter()
                        .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
            files: value["files"]
                .as_object()
                .map(|files| {
                    files
                        .iter()
                        .map(|(path, managed)| {
                            (
                                path.clone(),
                                Managed {
                                    origin: text(&managed["origin"]),
                                    applied: text(&managed["applied"]),
                                    secret: managed["secret"].as_bool().unwrap_or(false),
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// A manifest entry as a runner reads it.
struct Entry {
    path: String,
    mode: u32,
    sha256: String,
    secret: bool,
    merge: Merge,
}

fn entries(manifest: &Value) -> Result<Vec<Entry>, String> {
    let files = manifest["files"]
        .as_array()
        .ok_or("The manifest has no file list.")?;
    let mut seen = HashSet::new();
    let mut entries = Vec::with_capacity(files.len());
    for file in files {
        let path = file["path"].as_str().ok_or("A manifest file has no path.")?;
        if !safe_relative(path) {
            return Err(format!("The manifest names an unsafe path: {path}"));
        }
        if !seen.insert(path) {
            return Err(format!("The manifest names {path} twice."));
        }
        let sha256 = file["sha256"]
            .as_str()
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| format!("{path} has no valid hash."))?;
        entries.push(Entry {
            path: path.to_owned(),
            mode: file["mode"].as_u64().unwrap_or(0o644) as u32 & 0o777,
            sha256: sha256.to_ascii_lowercase(),
            // The runner decides what is secret too, so a manifest cannot
            // have a key rewritten or left world-readable.
            secret: file["secret"].as_bool().unwrap_or(false) || secret_path(path),
            merge: match file["merge"].as_str() {
                Some("json-keys") => Merge::JsonKeys,
                _ => Merge::Replace,
            },
        });
    }
    Ok(entries)
}

/// A relative path with no `..`, no empty part, and nothing absolute.
fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

impl Profiles {
    fn open(root: PathBuf) -> Self {
        let active = fs::read_to_string(root.join(ACTIVE))
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|origin| valid_origin(origin));
        Self {
            root,
            lock: Mutex::new(active),
            cached: Mutex::new(None),
        }
    }

    fn active_environment(&self) -> Vec<(String, String)> {
        let Ok(mut cached) = self.cached.lock() else {
            return Vec::new();
        };
        if let Some(environment) = cached.as_ref() {
            return environment.clone();
        }
        let environment = self
            .active()
            .and_then(|origin| self.environment(&origin).ok())
            .unwrap_or_default();
        *cached = Some(environment.clone());
        environment
    }

    pub fn active(&self) -> Option<String> {
        self.lock.lock().ok()?.clone()
    }

    fn origin_directory(&self, origin: &str) -> Result<PathBuf, String> {
        if !valid_origin(origin) {
            return Err("That origin id is not valid.".into());
        }
        Ok(self.root.join(origin))
    }

    pub fn home(&self, origin: &str) -> Result<PathBuf, String> {
        Ok(self.origin_directory(origin)?.join("home"))
    }

    fn load(&self, origin: &str) -> Result<State, String> {
        let path = self.origin_directory(origin)?.join(STATE);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .map(|value| State::from_json(&value))
                .map_err(|error| format!("The profile state is unreadable: {error}")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn save(&self, origin: &str, state: &State) -> Result<(), String> {
        let directory = self.origin_directory(origin)?;
        let bytes = serde_json::to_vec_pretty(&state.to_json()).map_err(|e| e.to_string())?;
        write_atomic(&directory.join(STATE), &bytes, 0o600).map_err(|e| e.to_string())
    }

    /// The hashes of `manifest` the runner does not hold for their path yet.
    pub fn missing(&self, origin: &str, manifest: &Value) -> Result<Vec<String>, String> {
        let state = self.load(origin)?;
        let mut missing = entries(manifest)?
            .into_iter()
            .filter(|entry| {
                state
                    .files
                    .get(&entry.path)
                    .is_none_or(|managed| managed.origin != entry.sha256)
            })
            .map(|entry| entry.sha256)
            .collect::<Vec<_>>();
        missing.sort();
        missing.dedup();
        Ok(missing)
    }

    /// Applies `manifest` with the bodies in `blobs` (base64, by hash) and
    /// returns what changed.
    pub fn apply(
        &self,
        origin: &str,
        manifest: &Value,
        blobs: &Map<String, Value>,
        activate: bool,
    ) -> Result<Value, String> {
        let mut active = self
            .lock
            .lock()
            .map_err(|_| "The profile store is unavailable.")?;
        let directory = self.origin_directory(origin)?;
        for path in [&self.root, &directory] {
            fs::create_dir_all(path).map_err(|e| e.to_string())?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let home = directory.join("home");
        fs::create_dir_all(&home).map_err(|e| e.to_string())?;
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        let target = home.to_string_lossy().into_owned();
        let origin_home = manifest["origin_home"]
            .as_str()
            .filter(|home| home.starts_with('/'))
            .ok_or("The manifest has no origin home.")?
            .trim_end_matches('/')
            .to_owned();
        let entries = entries(manifest)?;
        let mut state = self.load(origin)?;
        let mut written = Vec::new();
        let mut kept = Vec::new();
        let mut conflicts = Vec::new();
        let mut missing = Vec::new();
        let mut files = BTreeMap::new();
        for entry in &entries {
            let previous = state.files.get(&entry.path).cloned();
            let destination = home.join(&entry.path);
            let on_disk = fs::read(&destination).ok().map(|bytes| sha256(&bytes));
            let runner_changed = previous
                .as_ref()
                .is_some_and(|managed| on_disk.as_deref() != Some(managed.applied.as_str()));
            if let Some(managed) = previous
                .as_ref()
                .filter(|managed| managed.origin == entry.sha256)
            {
                // The origin has not changed it since the last push.
                if runner_changed && managed.secret {
                    kept.push(entry.path.clone());
                }
                files.insert(entry.path.clone(), managed.clone());
                continue;
            }
            let Some(body) = blobs.get(&entry.sha256).and_then(Value::as_str) else {
                missing.push(entry.sha256.clone());
                continue;
            };
            let bytes = STANDARD
                .decode(body)
                .map_err(|_| format!("The body of {} is not base64.", entry.path))?;
            if sha256(&bytes) != entry.sha256 {
                return Err(format!("The body of {} does not match its hash.", entry.path));
            }
            if runner_changed && entry.secret {
                // Both sides changed a sign-in. The origin's is the one the
                // person just pushed; the runner's is offered back by `pull`
                // until this write replaces it, so it is reported.
                conflicts.push(entry.path.clone());
            }
            let body = self.render(entry, bytes, &destination, &origin_home, &target)?;
            let mode = if entry.secret {
                0o600
            } else {
                (entry.mode & 0o755) | 0o600
            };
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            write_atomic(&destination, &body, mode).map_err(|e| e.to_string())?;
            files.insert(
                entry.path.clone(),
                Managed {
                    origin: entry.sha256.clone(),
                    applied: sha256(&body),
                    secret: entry.secret,
                },
            );
            written.push(entry.path.clone());
        }
        if !missing.is_empty() {
            missing.sort();
            missing.dedup();
            return Ok(json!({"ok": false, "error": "Some file bodies are missing.", "missing": missing}));
        }
        let mut removed = Vec::new();
        for (path, managed) in &state.files {
            if files.contains_key(path) {
                continue;
            }
            let destination = home.join(path);
            let on_disk = fs::read(&destination).ok().map(|bytes| sha256(&bytes));
            // A file the runner changed since is its own now.
            if on_disk.as_deref() == Some(managed.applied.as_str()) {
                let _ = fs::remove_file(&destination);
                removed.push(path.clone());
            }
        }
        state.files = files;
        state.origin_home = origin_home;
        state.env = manifest["env"]
            .as_object()
            .map(|env| {
                env.iter()
                    .filter(|(name, _)| !denied_variable(name))
                    .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        state.pushed_at_ms = now_ms();
        self.save(origin, &state)?;
        if let Ok(mut cached) = self.cached.lock() {
            *cached = None;
        }
        if activate {
            write_atomic(&self.root.join(ACTIVE), origin.as_bytes(), 0o600)
                .map_err(|e| e.to_string())?;
            *active = Some(origin.to_owned());
        }
        Ok(json!({
            "ok": true,
            "origin": origin,
            "home": target,
            "written": written,
            "removed": removed,
            "kept_runner_changes": kept,
            "conflicts": conflicts,
            "active": active.as_deref() == Some(origin),
        }))
    }

    fn render(
        &self,
        entry: &Entry,
        bytes: Vec<u8>,
        destination: &Path,
        origin_home: &str,
        target: &str,
    ) -> Result<Vec<u8>, String> {
        if entry.secret {
            return Ok(bytes);
        }
        let bytes = match String::from_utf8(bytes) {
            Ok(text) if text.len() <= MAX_REWRITE_BYTES => {
                rewrite_home(&text, origin_home, target).into_bytes()
            }
            Ok(text) => text.into_bytes(),
            Err(error) => error.into_bytes(),
        };
        if entry.merge != Merge::JsonKeys {
            return Ok(bytes);
        }
        let incoming = match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(incoming)) => incoming,
            _ => return Err(format!("{} is not a JSON object.", entry.path)),
        };
        let mut merged = fs::read(destination)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| match value {
                Value::Object(existing) => Some(existing),
                _ => None,
            })
            .unwrap_or_default();
        merged.extend(incoming);
        serde_json::to_vec_pretty(&Value::Object(merged)).map_err(|e| e.to_string())
    }

    /// Sign-ins the runner's CLIs refreshed since the last push, for the
    /// origin to take back so the two sides keep one current login.
    pub fn pull(&self, origin: &str) -> Result<Value, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "The profile store is unavailable.")?;
        let state = self.load(origin)?;
        let home = self.home(origin)?;
        let mut changed = Vec::new();
        for (path, managed) in state.files.iter().filter(|(_, managed)| managed.secret) {
            let Ok(bytes) = fs::read(home.join(path)) else {
                continue;
            };
            let hash = sha256(&bytes);
            if hash != managed.applied {
                changed.push(json!({"path": path, "sha256": hash, "data": STANDARD.encode(bytes)}));
            }
        }
        Ok(json!({"ok": true, "origin": origin, "changed": changed}))
    }

    pub fn status(&self) -> Value {
        let active = self.active();
        let mut origins = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.filter_map(Result::ok) {
                let Some(origin) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !valid_origin(&origin) || !entry.path().is_dir() {
                    continue;
                }
                let Ok(state) = self.load(&origin) else {
                    continue;
                };
                origins.push(json!({
                    "origin": origin,
                    "origin_home": state.origin_home,
                    "files": state.files.len(),
                    "variables": state.env.len(),
                    "pushed_at_ms": state.pushed_at_ms,
                    "active": active.as_deref() == Some(origin.as_str()),
                }));
            }
        }
        json!({"ok": true, "active": active, "origins": origins})
    }

    /// What a process run for `origin` gets in its environment.
    pub fn environment(&self, origin: &str) -> Result<Vec<(String, String)>, String> {
        let state = self.load(origin)?;
        let home = self.home(origin)?;
        let mut environment = state
            .env
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();
        let has = |prefix: &str| {
            state
                .files
                .keys()
                .any(|path| path == prefix || path.starts_with(&format!("{prefix}/")))
        };
        let at = |path: &str| home.join(path).to_string_lossy().into_owned();
        if has(".claude") {
            environment.push(("CLAUDE_CONFIG_DIR".into(), at(".claude")));
        }
        if has(".codex") {
            environment.push(("CODEX_HOME".into(), at(".codex")));
        }
        if has(".gitconfig") {
            environment.push(("GIT_CONFIG_GLOBAL".into(), at(".gitconfig")));
        }
        if has(".config/gh") {
            environment.push(("GH_CONFIG_DIR".into(), at(".config/gh")));
        }
        if has(".ssh") {
            environment.push(("GIT_SSH_COMMAND".into(), ssh_command(&home, &state)));
        }
        Ok(environment)
    }
}

/// `ssh` reading the profile's configuration, known hosts and keys. OpenSSH
/// expands `~` from the password database rather than `HOME`, so every path
/// is given in full.
fn ssh_command(home: &Path, state: &State) -> String {
    let ssh = home.join(".ssh");
    let mut parts = vec!["ssh".to_owned()];
    let config = ssh.join("config");
    parts.push("-F".into());
    parts.push(if state.files.contains_key(".ssh/config") {
        shell_quote(&config.to_string_lossy())
    } else {
        "/dev/null".into()
    });
    parts.push("-o".into());
    parts.push(shell_quote(&format!(
        "UserKnownHostsFile={}",
        ssh.join("known_hosts").to_string_lossy()
    )));
    for path in state.files.keys() {
        let Some(name) = path.strip_prefix(".ssh/") else {
            continue;
        };
        if name.starts_with("id_") && !name.ends_with(".pub") && !name.contains('/') {
            parts.push("-i".into());
            parts.push(shell_quote(&ssh.join(name).to_string_lossy()));
        }
    }
    parts.join(" ")
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r#"'\''"#))
}

fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    use std::io::Write as _;
    let directory = path.parent().unwrap_or(Path::new("."));
    let temporary = directory.join(format!(
        ".{}.xd-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file"),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("xd-origin-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn origin_home() -> PathBuf {
        let home = scratch("home");
        write(&home.join(".claude/CLAUDE.md"), "Always be kind.\n");
        write(
            &home.join(".claude/settings.json"),
            &format!(
                r#"{{"hooks":{{"PreToolUse":[{{"hooks":[{{"type":"command","command":"/usr/bin/python3 {}/.local/share/hooks/dispatch.py"}}]}}]}}}}"#,
                home.display()
            ),
        );
        write(&home.join(".claude/.credentials.json"), r#"{"token":"origin-1"}"#);
        write(&home.join(".claude/skills/review/SKILL.md"), "# Review\n");
        // A skill linked in from a checkout, and a link loop inside it.
        write(&home.join("Projects/tools/linked/SKILL.md"), "# Linked\n");
        std::os::unix::fs::symlink(
            home.join("Projects/tools/linked"),
            home.join(".claude/skills/linked"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            home.join("Projects/tools/linked"),
            home.join("Projects/tools/linked/again"),
        )
        .unwrap();
        write(&home.join(".codex/skills/own/SKILL.md"), "# Own\n");
        std::os::unix::fs::symlink(
            home.join("Projects/tools/linked"),
            home.join(".codex/skills/linked"),
        )
        .unwrap();
        write(&home.join(".claude/projects/huge/transcript.jsonl"), "{}\n");
        write(&home.join(".codex/config.toml"), "model = \"gpt\"\n");
        write(&home.join(".codex/auth.json"), r#"{"token":"codex"}"#);
        write(&home.join(".codex/sessions/a.jsonl"), "{}\n");
        write(&home.join(".local/share/hooks/dispatch.py"), "print('hi')\n");
        write(&home.join(".local/share/hooks/policy/rules.toml"), "deny = []\n");
        write(&home.join(".local/bin/chat-mcp"), "#!/bin/sh\n");
        write(&home.join(".local/bin/unrelated"), "#!/bin/sh\n");
        write(&home.join(".config/chat-mcp/token"), "secret\n");
        write(&home.join("Projects/py/.venv/bin/py-mcp"), "#!/bin/sh\n");
        write(&home.join(".gitconfig"), "[user]\n\tname = Someone\n");
        write(&home.join(".ssh/id_ed25519"), "PRIVATE\n");
        write(&home.join(".ssh/id_ed25519.pub"), "PUBLIC\n");
        write(&home.join(".ssh/known_hosts"), "github.com ssh-ed25519 AAAA\n");
        write(
            &home.join(".claude.json"),
            &format!(
                r#"{{"mcpServers":{{"lens":{{"type":"http","url":"https://x"}},"chat":{{"command":"{home}/.local/bin/chat-mcp"}},"py":{{"command":"{home}/Projects/py/.venv/bin/py-mcp"}}}},"numStartups":40,"projects":{{"/a":{{}}}}}}"#,
                home = home.display()
            ),
        );
        home
    }

    fn collect_from(home: &Path) -> Collected {
        let variables = HashMap::from([
            ("GH_TOKEN".to_owned(), "ghp_x".to_owned()),
            ("PATH".to_owned(), "/bin".to_owned()),
            ("GOPATH".to_owned(), format!("{}/go", home.display())),
            ("CLAUDECODE".to_owned(), "1".to_owned()),
        ]);
        collect(
            home,
            &home.join(".claude"),
            &home.join(".codex"),
            &home.join(".claude.json"),
            &variables,
        )
        .unwrap()
    }

    fn push(profiles: &Profiles, origin: &str, collected: &Collected) -> Value {
        let manifest = collected.manifest();
        let wanted = profiles
            .missing(origin, &manifest)
            .unwrap()
            .into_iter()
            .collect::<HashSet<_>>();
        let blobs = collected.blobs(&wanted).unwrap();
        profiles.apply(origin, &manifest, &blobs, true).unwrap()
    }

    #[test]
    fn collects_configuration_and_sign_ins_but_not_transcripts() {
        let home = origin_home();
        let collected = collect_from(&home);
        let paths = collected
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<HashSet<_>>();
        for expected in [
            ".claude/CLAUDE.md",
            ".claude/settings.json",
            ".claude/.credentials.json",
            ".claude/skills/review/SKILL.md",
            ".claude/skills/linked/SKILL.md",
            ".codex/skills/linked/SKILL.md",
            ".claude/.claude.json",
            ".codex/config.toml",
            ".codex/auth.json",
            ".gitconfig",
            ".ssh/id_ed25519",
            ".ssh/known_hosts",
            ".local/share/hooks/dispatch.py",
        ] {
            assert!(paths.contains(expected), "missing {expected}: {paths:?}");
        }
        // A hook's folder comes along; a tool in bin brings its settings but
        // not its neighbours; a Python environment stays behind.
        assert!(paths.contains(".local/share/hooks/policy/rules.toml"));
        assert!(paths.contains(".local/bin/chat-mcp"));
        assert!(paths.contains(".config/chat-mcp/token"));
        assert!(!paths.contains(".local/bin/unrelated"));
        assert!(!paths.iter().any(|path| path.contains(".venv")));
        assert!(collected.notes.iter().any(|note| note.contains("Python environment")));
        assert!(!paths.iter().any(|path| path.contains("projects/")));
        assert!(!paths.iter().any(|path| path.contains("sessions/")));
        let secret = |path: &str| collected.files.iter().find(|f| f.path == path).unwrap().secret;
        assert!(secret(".claude/.credentials.json"));
        assert!(secret(".ssh/id_ed25519"));
        assert!(!secret(".ssh/id_ed25519.pub"));
        assert_eq!(collected.env.get("GH_TOKEN").map(String::as_str), Some("ghp_x"));
        assert!(!collected.env.contains_key("PATH"));
        assert!(!collected.env.contains_key("GOPATH"));
        assert!(!collected.env.contains_key("CLAUDECODE"));
        let Source::Bytes(settings) = &collected
            .files
            .iter()
            .find(|f| f.path == ".claude/.claude.json")
            .unwrap()
            .source
        else {
            panic!("filtered settings are built in memory");
        };
        let settings: Value = serde_json::from_slice(settings).unwrap();
        assert!(settings["mcpServers"]["lens"].is_object());
        assert!(settings.get("numStartups").is_none());
        assert!(settings.get("projects").is_none());
    }

    #[test]
    fn applies_a_profile_with_the_origin_home_rewritten_into_it() {
        let home = origin_home();
        let runner = scratch("runner");
        let profiles = Profiles::open(runner.clone());
        let reply = push(&profiles, "laptop", &collect_from(&home));
        assert_eq!(reply["ok"], true, "{reply}");
        let profile = profiles.home("laptop").unwrap();
        let settings = fs::read_to_string(profile.join(".claude/settings.json")).unwrap();
        assert!(settings.contains(&format!(
            "{}/.local/share/hooks/dispatch.py",
            profile.display()
        )));
        assert!(!settings.contains(&format!("{}/.local", home.display())));
        assert!(profile.join(".local/share/hooks/dispatch.py").is_file());
        let mode = |path: &str| fs::metadata(profile.join(path)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(".claude/.credentials.json"), 0o600);
        assert_eq!(mode(".ssh/id_ed25519"), 0o600);
        let environment = profiles.environment("laptop").unwrap();
        let get = |name: &str| {
            environment
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(get("CLAUDE_CONFIG_DIR"), Some(profile.join(".claude").to_string_lossy().into()));
        assert_eq!(get("CODEX_HOME"), Some(profile.join(".codex").to_string_lossy().into()));
        assert_eq!(get("GIT_CONFIG_GLOBAL"), Some(profile.join(".gitconfig").to_string_lossy().into()));
        assert_eq!(get("GH_TOKEN").as_deref(), Some("ghp_x"));
        let ssh = get("GIT_SSH_COMMAND").unwrap();
        assert!(ssh.contains("-F /dev/null"), "{ssh}");
        assert!(ssh.contains(&format!("-i '{}'", profile.join(".ssh/id_ed25519").display())));
        assert!(!ssh.contains("id_ed25519.pub"));
        assert_eq!(profiles.active().as_deref(), Some("laptop"));
        assert_eq!(Profiles::open(runner).active().as_deref(), Some("laptop"));
    }

    #[test]
    fn a_second_push_sends_only_what_changed_and_keeps_runner_state() {
        let home = origin_home();
        let profiles = Profiles::open(scratch("runner"));
        push(&profiles, "laptop", &collect_from(&home));
        let profile = profiles.home("laptop").unwrap();
        // The runner's CLI writes its own transcript and bookkeeping.
        write(&profile.join(".claude/projects/run/t.jsonl"), "{}\n");
        let mut settings: Value =
            serde_json::from_slice(&fs::read(profile.join(".claude/.claude.json")).unwrap()).unwrap();
        settings["numStartups"] = json!(3);
        fs::write(profile.join(".claude/.claude.json"), settings.to_string()).unwrap();

        write(&home.join(".claude/CLAUDE.md"), "Always be kinder.\n");
        write(
            &home.join(".claude.json"),
            r#"{"mcpServers":{"lens":{"type":"http","url":"https://y"}}}"#,
        );
        let collected = collect_from(&home);
        let missing = profiles.missing("laptop", &collected.manifest()).unwrap();
        assert_eq!(missing.len(), 2, "{missing:?}");
        assert_eq!(
            profiles
                .active_environment()
                .iter()
                .find(|(name, _)| name == "GH_TOKEN")
                .map(|(_, value)| value.as_str()),
            Some("ghp_x")
        );
        let reply = push(&profiles, "laptop", &collected);
        assert_eq!(reply["written"].as_array().unwrap().len(), 2, "{reply}");
        assert_eq!(
            fs::read_to_string(profile.join(".claude/CLAUDE.md")).unwrap(),
            "Always be kinder.\n"
        );
        let merged: Value =
            serde_json::from_slice(&fs::read(profile.join(".claude/.claude.json")).unwrap()).unwrap();
        assert_eq!(merged["mcpServers"]["lens"]["url"], "https://y");
        assert_eq!(merged["numStartups"], 3);
        assert!(profile.join(".claude/projects/run/t.jsonl").is_file());
    }

    #[test]
    fn a_sign_in_the_runner_refreshed_is_kept_and_offered_back() {
        let home = origin_home();
        let profiles = Profiles::open(scratch("runner"));
        push(&profiles, "laptop", &collect_from(&home));
        let profile = profiles.home("laptop").unwrap();
        fs::write(profile.join(".claude/.credentials.json"), r#"{"token":"runner-2"}"#).unwrap();

        let reply = push(&profiles, "laptop", &collect_from(&home));
        assert_eq!(reply["kept_runner_changes"], json!([".claude/.credentials.json"]));
        assert_eq!(
            fs::read_to_string(profile.join(".claude/.credentials.json")).unwrap(),
            r#"{"token":"runner-2"}"#
        );
        let pulled = profiles.pull("laptop").unwrap();
        let changed = pulled["changed"].as_array().unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0]["path"], ".claude/.credentials.json");
        assert_eq!(
            STANDARD.decode(changed[0]["data"].as_str().unwrap()).unwrap(),
            br#"{"token":"runner-2"}"#
        );

        // When the origin also refreshed, its push wins and says so.
        write(&home.join(".claude/.credentials.json"), r#"{"token":"origin-3"}"#);
        let reply = push(&profiles, "laptop", &collect_from(&home));
        assert_eq!(reply["conflicts"], json!([".claude/.credentials.json"]));
        assert_eq!(
            fs::read_to_string(profile.join(".claude/.credentials.json")).unwrap(),
            r#"{"token":"origin-3"}"#
        );
        assert!(profiles.pull("laptop").unwrap()["changed"].as_array().unwrap().is_empty());
    }

    #[test]
    fn removed_files_go_unless_the_runner_changed_them() {
        let home = origin_home();
        let profiles = Profiles::open(scratch("runner"));
        push(&profiles, "laptop", &collect_from(&home));
        let profile = profiles.home("laptop").unwrap();
        fs::write(profile.join(".codex/config.toml"), "model = \"mine\"\n").unwrap();
        fs::remove_dir_all(home.join(".claude/skills")).unwrap();
        fs::remove_file(home.join(".codex/config.toml")).unwrap();
        let reply = push(&profiles, "laptop", &collect_from(&home));
        assert_eq!(reply["removed"], json!([".claude/skills/linked/SKILL.md", ".claude/skills/review/SKILL.md"]), "{reply}");
        assert!(!profile.join(".claude/skills/review/SKILL.md").exists());
        assert!(profile.join(".codex/config.toml").is_file());
    }

    #[test]
    fn refuses_unsafe_manifests_and_origin_ids() {
        let profiles = Profiles::open(scratch("runner"));
        let manifest = json!({"origin_home": "/home/a", "files": [{"path": "../escape", "sha256": "0".repeat(64)}]});
        assert!(profiles.apply("laptop", &manifest, &Map::new(), false).is_err());
        let manifest = json!({"origin_home": "/home/a", "files": [{"path": ".ssh//x", "sha256": "0".repeat(64)}]});
        assert!(profiles.apply("laptop", &manifest, &Map::new(), false).is_err());
        let manifest = json!({"origin_home": "/home/a", "files": [{"path": "a", "sha256": sha256(b"x")}]});
        let blobs = Map::from_iter([(sha256(b"x"), json!(STANDARD.encode(b"y")))]);
        assert!(profiles.apply("laptop", &manifest, &blobs, false).is_err());
        for origin in ["", "..", ".hidden", "a/b", &"x".repeat(65)] {
            assert!(!valid_origin(origin), "{origin}");
        }
        let reply = profiles
            .apply("laptop", &json!({"origin_home": "/home/a", "files": [{"path": "a", "sha256": sha256(b"x")}]}), &Map::new(), false)
            .unwrap();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["missing"], json!([sha256(b"x")]));
    }

    #[test]
    fn rewrites_the_home_only_where_it_starts_a_path() {
        let text = "/home/a/x /home/ab/y x/home/a/z \"/home/a\"";
        assert_eq!(
            rewrite_home(text, "/home/a", "/r/h"),
            "/r/h/x /home/ab/y x/home/a/z \"/r/h\""
        );
        assert_eq!(
            home_paths("cmd /home/a/bin/tool --flag=/home/a/f.txt, /home/ab/no", "/home/a"),
            vec!["/home/a/bin/tool", "/home/a/f.txt"]
        );
    }
}
