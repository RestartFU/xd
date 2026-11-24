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
    sync::{
        Mutex, OnceLock, PoisonError, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub use crate::origin_hosting::Hosted;
use crate::origin_hosting;

/// One file is carried only up to this size.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// A whole profile is refused past this size.
const MAX_PROFILE_BYTES: u64 = 512 * 1024 * 1024;
/// Only text files up to this size have the origin's home rewritten in them.
const MAX_REWRITE_BYTES: usize = 4 * 1024 * 1024;
const STATE: &str = "state.json";
/// The active origin, named so no origin id can take its place: ids never
/// start with a dot.
const ACTIVE: &str = ".active";
/// The global Git configuration a profile's processes read, beside its home.
const GIT_CONFIG: &str = "gitconfig";
/// What hosted programs start, in the profile home. `.xd` there is the
/// runner's, never the origin's.
const ORIGIN_EXEC: &str = ".xd/origin-exec";

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
    /// Programs the runner starts on the origin, by id. Only ids and labels
    /// reach the runner; the origin keeps the commands.
    pub hosted: BTreeMap<String, Hosted>,
    /// Codex hook trust keys the origin's own config honours.
    pub codex_trusted_hooks: Vec<String>,
    /// The origin's login environment, which hosted programs run with.
    pub login_env: HashMap<String, String>,
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
            "origin_hosted": self.hosted.iter().map(|(id, hosted)| json!({
                "id": id,
                "label": hosted.label,
            })).collect::<Vec<_>>(),
            "codex_trusted_hooks": self.codex_trusted_hooks,
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
        incomplete: HashSet::new(),
        whole: HashSet::new(),
        codex_home: codex_home.to_path_buf(),
    };
    for entry in CLAUDE_ENTRIES {
        collector.add_root(&claude_home.join(entry), &format!(".claude/{entry}"))?;
    }
    for entry in CODEX_ENTRIES {
        collector.add_root(&codex_home.join(entry), &format!(".codex/{entry}"))?;
    }
    for entry in HOME_ENTRIES {
        collector.add_root(&home.join(entry), entry)?;
    }
    collector.add_claude_json(claude_json)?;
    collector.add_referenced()?;
    collector.host_unportable()?;
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
    // A shell that could not be read is an error, never a profile without
    // variables: pushing that would remove the ones the runner holds.
    let variables = crate::user_environment::login_environment()?;
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
    collected.login_env = variables;
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
    /// Files carried without the folder they need, which cannot run on a
    /// runner as they are.
    incomplete: HashSet<String>,
    /// Folders carried as a whole, which a dependency on a folder needs.
    whole: HashSet<String>,
    codex_home: PathBuf,
}

impl Collector {
    /// Carries `source` at `path`, and notes a folder as carried whole.
    fn add_root(&mut self, source: &Path, path: &str) -> io::Result<()> {
        if fs::metadata(source).is_ok_and(|metadata| metadata.is_dir()) {
            self.whole.insert(path.to_owned());
        }
        self.add_tree(source, path)
    }

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
                        self.add_root(&root, &root_relative)?;
                    }
                    None => {
                        let root_relative = root
                            .strip_prefix(&self.home)
                            .map(|path| path.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.collected.notes.push(format!(
                            "carried only ~/{relative}: the rest of ~/{root_relative} is over {} MiB or {} files",
                            MAX_BUNDLE_BYTES / (1024 * 1024),
                            MAX_BUNDLE_FILES
                        ));
                        self.incomplete.insert(relative.clone());
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
                            self.add_root(&settings, &format!(".config/{name}"))?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl Collector {
    /// Whether everything `text` names in the origin's home came along whole,
    /// along with what those files name in turn (a wrapper that starts a
    /// Python environment). Paths that do not exist are not counted.
    fn portable(&self, text: &str) -> bool {
        let home = &self.collected.origin_home;
        // A program named by a path outside the home and the system's own
        // folders (Homebrew, /opt, Nix…) cannot be known to exist on a runner.
        let program = text.split_whitespace().next().unwrap_or_default();
        if program.starts_with('/')
            && !program.starts_with(&format!("{home}/"))
            && !SYSTEM_PROGRAM_FOLDERS
                .iter()
                .any(|folder| program.starts_with(folder))
        {
            return false;
        }
        let carried = |relative: &str, directory: bool| {
            if directory {
                self.whole.iter().any(|root| {
                    relative == root || relative.starts_with(&format!("{root}/"))
                })
            } else {
                self.seen.contains(relative) && !self.incomplete.contains(relative)
            }
        };
        let relative = |absolute: &str| {
            absolute
                .strip_prefix(home.as_str())
                .map(|rest| rest.trim_start_matches('/').to_owned())
                .filter(|rest| !rest.is_empty())
        };
        for absolute in home_paths(text, home) {
            let Some(path) = relative(&absolute) else {
                continue;
            };
            let Ok(metadata) = fs::metadata(self.home.join(&path)) else {
                continue;
            };
            if !carried(&path, metadata.is_dir()) {
                return false;
            }
            if metadata.is_file()
                && metadata.len() <= 1024 * 1024
                && let Ok(inner) = fs::read_to_string(self.home.join(&path))
            {
                for named in home_paths(&inner, home) {
                    let Some(named) = relative(&named) else {
                        continue;
                    };
                    if let Ok(metadata) = fs::metadata(self.home.join(&named))
                        && !carried(&named, metadata.is_dir())
                    {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn file_text(&self, path: &str) -> io::Result<Option<String>> {
        let Some(file) = self.collected.files.iter().find(|file| file.path == path) else {
            return Ok(None);
        };
        Ok(String::from_utf8(file.source.read()?).ok())
    }

    fn replace_file(&mut self, path: &str, bytes: Vec<u8>) {
        if let Some(file) = self.collected.files.iter_mut().find(|file| file.path == path) {
            file.sha256 = sha256(&bytes);
            file.size = bytes.len() as u64;
            file.source = Source::Bytes(bytes);
        }
    }

    /// Hosts `hosted` on the origin unless `portable`, returning the command
    /// line the runner's configuration uses instead.
    fn host(&mut self, hosted: Hosted) -> (String, String) {
        let id = hosted.id();
        self.collected.notes.push(format!(
            "runs on the origin: {} (its files cannot run on a runner)",
            hosted.label
        ));
        self.collected.hosted.insert(id.clone(), hosted);
        (format!("{}/.xd/origin-exec", self.collected.origin_home), id)
    }

    /// Points every MCP server and hook that cannot run on a runner at the
    /// origin, and notes which Codex hooks the origin trusts.
    fn host_unportable(&mut self) -> io::Result<()> {
        // Claude's MCP servers.
        if let Some(text) = self.file_text(".claude/.claude.json")? {
            let mut settings: Value = serde_json::from_str(&text).map_err(io::Error::other)?;
            let mut changed = false;
            if let Some(servers) = settings["mcpServers"].as_object_mut() {
                for (name, server) in servers.iter_mut() {
                    if let Some(hosted) = json_server(server, &format!("Claude MCP server {name}"))
                        && !self.portable(&hosted_text(&hosted))
                    {
                        let (command, id) = self.host(hosted);
                        point_json_server(server, command, id);
                        changed = true;
                    }
                }
            }
            if changed {
                self.replace_file(
                    ".claude/.claude.json",
                    serde_json::to_vec_pretty(&settings).map_err(io::Error::other)?,
                );
            }
        }
        // Codex's trust is read from the hooks as the origin wrote them.
        let codex_hooks_file = self.codex_home.join("hooks.json");
        if let (Some(config), Some(hooks)) = (
            self.file_text(".codex/config.toml")?,
            self.file_text(".codex/hooks.json")?,
        ) {
            self.collected.codex_trusted_hooks = origin_hosting::trusted_codex_hooks(
                &config,
                &hooks,
                &codex_hooks_file.to_string_lossy(),
            );
        }
        // Hooks, which Claude and Codex both run through the shell.
        for (path, tool) in [(".claude/settings.json", "Claude"), (".codex/hooks.json", "Codex")] {
            let Some(text) = self.file_text(path)? else {
                continue;
            };
            let Ok(mut settings) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let mut changed = false;
            let codex_file = codex_hooks_file.to_string_lossy().into_owned();
            if let Some(events) = settings["hooks"].as_object_mut() {
                for (event, groups) in events.iter_mut() {
                    let label = origin_hosting::event_label(event);
                    for (group_index, group) in groups.as_array_mut().into_iter().flatten().enumerate() {
                        let handlers = group["hooks"].as_array_mut().into_iter().flatten();
                        for (handler_index, handler) in handlers.enumerate() {
                            let Some(command) = handler["command"].as_str().map(str::to_owned) else {
                                continue;
                            };
                            if handler["type"].as_str().unwrap_or("command") != "command"
                                || self.portable(&command)
                            {
                                continue;
                            }
                            // Codex runs only the hooks the user approved and left
                            // on; any other stays as it is, and stays untrusted.
                            if tool == "Codex" {
                                let key = format!("{codex_file}:{label}:{group_index}:{handler_index}");
                                if !self.collected.codex_trusted_hooks.contains(&key) {
                                    continue;
                                }
                            }
                            let (program, id) = self.host(Hosted {
                                program: "/bin/sh".into(),
                                args: vec!["-c".into(), command],
                                env: BTreeMap::new(),
                                cwd: None,
                                label: format!("{tool} {event} hook"),
                            });
                            handler["command"] = json!(format!("{program} {id}"));
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                self.replace_file(path, serde_json::to_vec_pretty(&settings).map_err(io::Error::other)?);
            }
        }
        // Codex's MCP servers.
        if let Some(config) = self.file_text(".codex/config.toml")?
            && let Ok(mut document) = config.parse::<toml_edit::DocumentMut>()
        {
            let mut changed = false;
            let names = document
                .get("mcp_servers")
                .and_then(toml_edit::Item::as_table_like)
                .map(|servers| servers.iter().map(|(name, _)| name.to_owned()).collect::<Vec<_>>())
                .unwrap_or_default();
            for name in names {
                let server = &document["mcp_servers"][name.as_str()];
                let Some(program) = server.get("command").and_then(toml_edit::Item::as_str) else {
                    continue;
                };
                let strings = |key: &str| {
                    server
                        .get(key)
                        .and_then(toml_edit::Item::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| item.as_str().map(str::to_owned))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                };
                let env = server
                    .get("env")
                    .and_then(toml_edit::Item::as_table_like)
                    .map(|env| {
                        env.iter()
                            .filter_map(|(key, value)| Some((key.to_owned(), value.as_str()?.to_owned())))
                            .collect::<BTreeMap<_, _>>()
                    })
                    .unwrap_or_default();
                let hosted = Hosted {
                    program: program.to_owned(),
                    args: strings("args"),
                    env,
                    cwd: server.get("cwd").and_then(toml_edit::Item::as_str).map(str::to_owned),
                    label: format!("Codex MCP server {name}"),
                };
                if self.portable(&hosted_text(&hosted)) {
                    continue;
                }
                let (command, id) = self.host(hosted);
                let server = &mut document["mcp_servers"][name.as_str()];
                server["command"] = toml_edit::value(command);
                let mut args = toml_edit::Array::new();
                args.push(id);
                server["args"] = toml_edit::value(args);
                if let Some(table) = server.as_table_like_mut() {
                    // They are the origin's to give the program, not the runner's.
                    table.remove("env");
                    table.remove("env_vars");
                    table.remove("cwd");
                }
                changed = true;
            }
            if changed {
                self.replace_file(".codex/config.toml", document.to_string().into_bytes());
            }
        }
        Ok(())
    }
}

/// A stdio MCP server in Claude's JSON settings, as a hosted program.
fn json_server(server: &Value, label: &str) -> Option<Hosted> {
    if !matches!(server["type"].as_str(), None | Some("stdio")) {
        return None;
    }
    Some(Hosted {
        program: server["command"].as_str()?.to_owned(),
        args: server["args"]
            .as_array()
            .map(|args| args.iter().filter_map(|arg| arg.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
        env: server["env"]
            .as_object()
            .map(|env| {
                env.iter()
                    .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default(),
        cwd: server["cwd"].as_str().map(str::to_owned),
        label: label.to_owned(),
    })
}

fn point_json_server(server: &mut Value, command: String, id: String) {
    if let Some(server) = server.as_object_mut() {
        server.remove("env");
        server.remove("cwd");
        server.insert("type".into(), json!("stdio"));
        server.insert("command".into(), json!(command));
        server.insert("args".into(), json!([id]));
    }
}

/// Everything a hosted program names, for checking what it needs.
fn hosted_text(hosted: &Hosted) -> String {
    let mut parts = vec![hosted.program.clone()];
    parts.extend(hosted.args.iter().cloned());
    parts.extend(hosted.env.values().cloned());
    parts.extend(hosted.cwd.iter().cloned());
    parts.join("\n")
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

/// Folders whose programs every runner is taken to have.
const SYSTEM_PROGRAM_FOLDERS: &[&str] = &["/usr/bin/", "/bin/", "/usr/sbin/", "/sbin/"];

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

/// The bytes in `root` a profile would carry, or `None` past the bundle
/// limits. Counts what `Collector::add_tree` would carry: linked files and
/// folders are followed, and only a link back into a folder being read stops.
fn measure(root: &Path) -> Option<u64> {
    let mut total = (0, 0);
    let mut chain = HashSet::new();
    measure_folder(root, 0, &mut chain, &mut total)?;
    Some(total.0)
}

fn measure_folder(
    folder: &Path,
    depth: usize,
    chain: &mut HashSet<PathBuf>,
    total: &mut (u64, usize),
) -> Option<()> {
    let real = fs::canonicalize(folder).ok()?;
    if depth > 24 || !chain.insert(real.clone()) {
        return Some(());
    }
    let result = (|| {
        for entry in fs::read_dir(folder).ok()?.filter_map(Result::ok) {
            if skipped_name(entry.file_name().to_str().unwrap_or("")) {
                continue;
            }
            // Follows a link, like the collector; a link to nothing is left out.
            let Ok(metadata) = fs::metadata(entry.path()) else {
                continue;
            };
            if metadata.is_dir() {
                measure_folder(&entry.path(), depth + 1, chain, total)?;
            } else if metadata.is_file() {
                total.0 += metadata.len();
                total.1 += 1;
                if total.1 > MAX_BUNDLE_FILES || total.0 > MAX_BUNDLE_BYTES {
                    return None;
                }
            }
        }
        Some(())
    })();
    chain.remove(&real);
    result
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
    ) || (path.starts_with(".ssh/") && !ssh_public(name))
}

/// Files in `~/.ssh` that hold nothing secret. Everything else there is
/// treated as a key, whatever it is called.
fn ssh_public(name: &str) -> bool {
    name.ends_with(".pub")
        || name.starts_with("known_hosts")
        || matches!(name, "config" | "authorized_keys" | "environment")
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
    /// Applies or reads back one profile at a time. Never held while waiting
    /// on the locks below, and never taken by a process start.
    applying: Mutex<()>,
    /// Held exclusively while a push puts its files and state in place, and
    /// shared while a process start reads the active profile, so a process
    /// never starts on a profile half replaced. Taken before `active` and
    /// `cached`, never after.
    commit: RwLock<()>,
    active: Mutex<Option<String>>,
    /// The active origin's variables, which every process start reads, with
    /// the generation they were read at.
    cached: Mutex<Option<(u64, Vec<(String, String)>)>>,
    /// Advanced by every push, so variables read during one are not kept.
    generation: AtomicU64,
}

static INSTALLED: OnceLock<Profiles> = OnceLock::new();
/// How `xd-host` reaches the daemon that runs here, for `origin-exec`.
static DAEMON_ARGUMENTS: OnceLock<Vec<String>> = OnceLock::new();

/// Records the arguments that name this daemon (`--database … --workspaces
/// …`), so the programs it writes reach it and not another one.
pub fn set_daemon_arguments(arguments: Vec<String>) {
    let _ = DAEMON_ARGUMENTS.set(arguments);
}

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
    /// For a merged JSON file, the top-level keys the origin owns in it.
    keys: Vec<String>,
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
                "keys": managed.keys,
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
                                    keys: managed["keys"]
                                        .as_array()
                                        .map(|keys| {
                                            keys.iter()
                                                .filter_map(Value::as_str)
                                                .map(str::to_owned)
                                                .collect()
                                        })
                                        .unwrap_or_default(),
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
        if path == ".xd" || path.starts_with(".xd/") {
            return Err(format!("{path} is the runner's own."));
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
    for entry in &entries {
        let mut path = entry.path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            if seen.contains(parent) {
                return Err(format!("The manifest names {parent} as a file and a folder."));
            }
            path = parent;
        }
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
            applying: Mutex::new(()),
            commit: RwLock::new(()),
            active: Mutex::new(active),
            cached: Mutex::new(None),
            generation: AtomicU64::new(0),
        }
    }

    /// Waits only for a push's commit, never for the rest of a push, and
    /// takes the other locks one at a time.
    fn active_environment(&self) -> Vec<(String, String)> {
        let _commit = self.commit.read().unwrap_or_else(PoisonError::into_inner);
        let generation = self.generation.load(Ordering::Acquire);
        if let Ok(cached) = self.cached.lock()
            && let Some((at, environment)) = cached.as_ref()
            && *at == generation
        {
            return environment.clone();
        }
        let environment = self
            .active()
            .and_then(|origin| self.environment(&origin).ok())
            .unwrap_or_default();
        if let Ok(mut cached) = self.cached.lock()
            && self.generation.load(Ordering::Acquire) == generation
        {
            *cached = Some((generation, environment.clone()));
        }
        environment
    }

    pub fn active(&self) -> Option<String> {
        self.active.lock().ok()?.clone()
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
        match read_regular(&path) {
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

    /// The hashes of `manifest` the runner does not hold for their path yet,
    /// or holds no longer because the file is gone from the profile home.
    pub fn missing(&self, origin: &str, manifest: &Value) -> Result<Vec<String>, String> {
        let state = self.load(origin)?;
        let home = self.home(origin)?;
        let mut missing = entries(manifest)?
            .into_iter()
            .filter(|entry| needs_body(&state, &home, entry))
            .map(|entry| entry.sha256)
            .collect::<Vec<_>>();
        missing.sort();
        missing.dedup();
        Ok(missing)
    }

    /// Applies `manifest` with the bodies in `blobs` (base64, by hash) and
    /// returns what changed.
    ///
    /// Everything is checked, rendered and staged before the live home is
    /// touched, so a refused push changes nothing. The staged files then go
    /// in together with the new state under the commit lock, which process
    /// starts wait on, so a process starts on one whole profile; and if any
    /// step of that fails, what was already moved goes back.
    pub fn apply(
        &self,
        origin: &str,
        manifest: &Value,
        blobs: &Map<String, Value>,
        activate: bool,
    ) -> Result<Value, String> {
        let _applying = self
            .applying
            .lock()
            .map_err(|_| "The profile store is unavailable.")?;
        let directory = self.origin_directory(origin)?;
        let home = directory.join("home");
        for path in [&self.root, &directory, &home] {
            private_directory(path).map_err(|e| e.to_string())?;
        }
        let target = home.to_string_lossy().into_owned();
        let origin_home = manifest["origin_home"]
            .as_str()
            .filter(|home| home.starts_with('/'))
            .ok_or("The manifest has no origin home.")?
            .trim_end_matches('/')
            .to_owned();
        let entries = entries(manifest)?;
        let mut state = self.load(origin)?;

        // Every body is checked before anything is written.
        let mut missing = Vec::new();
        let mut bodies = HashMap::new();
        for entry in &entries {
            if !needs_body(&state, &home, entry) {
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
            bodies.insert(entry.path.clone(), bytes);
        }
        if !missing.is_empty() {
            missing.sort();
            missing.dedup();
            return Ok(json!({"ok": false, "error": "Some file bodies are missing.", "missing": missing}));
        }

        // Then what each file becomes.
        let mut written = Vec::new();
        let mut kept = Vec::new();
        let mut conflicts = Vec::new();
        let mut files = BTreeMap::new();
        let mut rendered: BTreeMap<String, (Vec<u8>, u32)> = BTreeMap::new();
        let mut modes = Vec::new();
        for entry in &entries {
            let previous = state.files.get(&entry.path).cloned();
            let on_disk = read_inside(&home, &entry.path).map(|bytes| sha256(&bytes));
            let runner_changed = previous
                .as_ref()
                .is_some_and(|managed| on_disk.as_deref() != Some(managed.applied.as_str()));
            let mode = if entry.secret {
                0o600
            } else {
                (entry.mode & 0o755) | 0o600
            };
            let Some(bytes) = bodies.remove(&entry.path) else {
                // The origin has not changed it since the last push. Its mode
                // still follows the origin's, and a key is always private.
                if let Some(managed) = previous {
                    if runner_changed && managed.secret {
                        kept.push(entry.path.clone());
                    }
                    if entry.secret || !runner_changed {
                        modes.push((entry.path.clone(), mode));
                    }
                    files.insert(entry.path.clone(), managed);
                }
                continue;
            };
            if runner_changed && entry.secret {
                // Both sides changed a sign-in. The origin's is the one the
                // person just pushed; the runner's is reported.
                conflicts.push(entry.path.clone());
            }
            let previous_keys = previous.map(|managed| managed.keys).unwrap_or_default();
            let (body, keys) =
                self.render(entry, bytes, &home, &origin_home, &target, &previous_keys)?;
            files.insert(
                entry.path.clone(),
                Managed {
                    origin: entry.sha256.clone(),
                    applied: sha256(&body),
                    secret: entry.secret,
                    keys,
                },
            );
            rendered.insert(entry.path.clone(), (body, mode));
            written.push(entry.path.clone());
        }

        // Codex trusts hooks by their hash as rewritten here, and only hooks
        // exactly as the origin sent them: a hooks file the runner changed
        // keeps whatever trust it had.
        let trusted = manifest["codex_trusted_hooks"]
            .as_array()
            .map(|keys| {
                keys.iter()
                    .filter_map(Value::as_str)
                    .map(|key| rewrite_home(key, &origin_home, &target))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let as_applied = |path: &str| -> Option<String> {
            let bytes = match rendered.get(path) {
                Some((body, _)) => body.clone(),
                None => read_inside(&home, path)
                    .filter(|bytes| files.get(path).is_some_and(|m: &Managed| sha256(bytes) == m.applied))?,
            };
            String::from_utf8(bytes).ok()
        };
        if !trusted.is_empty()
            && let Some(hooks) = as_applied(".codex/hooks.json")
            && let Some(config) = as_applied(".codex/config.toml")
            && let Some(updated) = origin_hosting::retrust_codex_hooks(&config, &hooks, &trusted)
            && let Some(managed) = files.get_mut(".codex/config.toml")
        {
            managed.applied = sha256(updated.as_bytes());
            let mode = rendered.get(".codex/config.toml").map_or(0o600, |(_, mode)| *mode);
            rendered.insert(".codex/config.toml".into(), (updated.into_bytes(), mode));
        }
        // What starts an origin's hosted programs.
        let exec = self.origin_exec_script(origin).map_err(|e| e.to_string())?;
        if read_inside(&home, ORIGIN_EXEC).as_deref() != Some(exec.as_bytes()) {
            rendered.insert(ORIGIN_EXEC.into(), (exec.into_bytes(), 0o700));
        }

        // Then staged, outside the home, with what stands in the way of each
        // file found now: a file the origin dropped where a folder must go, or
        // the reverse. Such a thing is moved aside only when the runner never
        // changed it; anything else in the way refuses the push.
        let _ = remove_stale_staging(&directory);
        let staging = directory.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
        private_directory(&staging).map_err(|e| e.to_string())?;
        let mut plan = Plan::default();
        let keep = entries.iter().map(|entry| entry.path.as_str()).collect::<HashSet<_>>();
        let obsolete = |path: &str| {
            !keep.contains(path)
                && state.files.get(path).is_some_and(|managed| {
                    read_inside(&home, path).is_some_and(|bytes| sha256(&bytes) == managed.applied)
                })
        };
        let planned = (|| -> Result<(), String> {
            for (index, (path, (body, mode))) in rendered.iter().enumerate() {
                plan_place(&home, path, &obsolete, &mut plan.blockers)?;
                let temporary = staging.join(format!("write-{index}"));
                create_file(&temporary, body, *mode)
                    .map_err(|e| format!("Cannot write {path}: {e}"))?;
                plan.writes.push((temporary, home.join(path), Some(path.clone())));
            }
            match git_config(&home, &files) {
                Some(text) => {
                    if read_regular(&directory.join(GIT_CONFIG)).ok().as_deref()
                        != Some(text.as_bytes())
                    {
                        let temporary = staging.join("gitconfig");
                        create_file(&temporary, text.as_bytes(), 0o600)
                            .map_err(|e| e.to_string())?;
                        plan.writes.push((temporary, directory.join(GIT_CONFIG), None));
                    }
                }
                None => plan.unused.push(directory.join(GIT_CONFIG)),
            }
            Ok(())
        })();
        if let Err(error) = planned {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }

        let previous_files = std::mem::replace(&mut state.files, files);
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

        let _commit = self.commit.write().unwrap_or_else(PoisonError::into_inner);
        let mut undo = Vec::new();
        let committed = commit(&home, &staging, &plan, &mut undo)
            .map_err(|e| format!("Cannot apply the profile: {e}"))
            .and_then(|()| self.save(origin, &state))
            .and_then(|()| {
                if activate {
                    write_atomic(&self.root.join(ACTIVE), origin.as_bytes(), 0o600)
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
            });
        if let Err(error) = committed {
            for step in undo.into_iter().rev() {
                step.revert();
            }
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        for (path, mode) in modes {
            let _ = set_mode_inside(&home, &path, mode);
        }
        let mut removed = Vec::new();
        for (path, managed) in &previous_files {
            if state.files.contains_key(path) {
                continue;
            }
            if plan.blockers.iter().any(|blocker| {
                path == blocker || path.starts_with(&format!("{blocker}/"))
            }) {
                removed.push(path.clone());
                continue;
            }
            // A file the runner changed since is its own now.
            if read_inside(&home, path).is_some_and(|bytes| sha256(&bytes) == managed.applied) {
                let _ = fs::remove_file(home.join(path));
                removed.push(path.clone());
            }
        }
        for path in &plan.unused {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&staging);
        if activate && let Ok(mut active) = self.active.lock() {
            *active = Some(origin.to_owned());
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut cached) = self.cached.lock() {
            *cached = None;
        }
        drop(_commit);
        removed.sort();
        Ok(json!({
            "ok": true,
            "origin": origin,
            "home": target,
            "written": written,
            "removed": removed,
            "kept_runner_changes": kept,
            "conflicts": conflicts,
            "active": self.active().as_deref() == Some(origin),
        }))
    }

    /// `home/.xd/origin-exec`, which hosted MCP servers and hooks start in
    /// place of their real command: it asks this runner to run the program
    /// on the origin.
    fn origin_exec_script(&self, origin: &str) -> io::Result<String> {
        let executable = std::env::current_exe()?;
        let data = self.root.parent().unwrap_or(&self.root);
        let daemon = DAEMON_ARGUMENTS.get().cloned().unwrap_or_else(|| {
            vec!["--data".into(), data.to_string_lossy().into_owned()]
        });
        Ok(format!(
            "#!/bin/sh\nexec {} origin-exec {} --origin {} \"$@\"\n",
            shell_quote(&executable.to_string_lossy()),
            daemon.iter().map(|argument| shell_quote(argument)).collect::<Vec<_>>().join(" "),
            shell_quote(origin),
        ))
    }

    /// The file as the runner writes it, and for a merged JSON file the keys
    /// the origin owns in it now. A key the origin owned before and dropped
    /// goes; the runner's own keys stay.
    fn render(
        &self,
        entry: &Entry,
        bytes: Vec<u8>,
        home: &Path,
        origin_home: &str,
        target: &str,
        previous_keys: &[String],
    ) -> Result<(Vec<u8>, Vec<String>), String> {
        if entry.secret {
            return Ok((bytes, Vec::new()));
        }
        let bytes = match String::from_utf8(bytes) {
            Ok(text) if text.len() <= MAX_REWRITE_BYTES => {
                rewrite_home(&text, origin_home, target).into_bytes()
            }
            Ok(text) => text.into_bytes(),
            Err(error) => error.into_bytes(),
        };
        if entry.merge != Merge::JsonKeys {
            return Ok((bytes, Vec::new()));
        }
        let incoming = match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(incoming)) => incoming,
            _ => return Err(format!("{} is not a JSON object.", entry.path)),
        };
        let mut merged = read_inside(home, &entry.path)
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| match value {
                Value::Object(existing) => Some(existing),
                _ => None,
            })
            .unwrap_or_default();
        for key in previous_keys {
            if !incoming.contains_key(key) {
                merged.remove(key);
            }
        }
        let keys = incoming.keys().cloned().collect();
        merged.extend(incoming);
        let body = serde_json::to_vec_pretty(&Value::Object(merged)).map_err(|e| e.to_string())?;
        Ok((body, keys))
    }
    /// Sign-ins the runner's CLIs refreshed since the last push, for the
    /// origin to take back so the two sides keep one current login.
    pub fn pull(&self, origin: &str) -> Result<Value, String> {
        let _applying = self
            .applying
            .lock()
            .map_err(|_| "The profile store is unavailable.")?;
        let state = self.load(origin)?;
        let home = self.home(origin)?;
        let mut changed = Vec::new();
        for (path, managed) in state.files.iter().filter(|(_, managed)| managed.secret) {
            let Some(bytes) = read_inside(&home, path) else {
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
        if git_config(&home, &state.files).is_some() {
            let config = self.origin_directory(origin)?.join(GIT_CONFIG);
            environment.push(("GIT_CONFIG_GLOBAL".into(), config.to_string_lossy().into_owned()));
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
        if !ssh_public(name) && !name.contains('/') {
            parts.push("-i".into());
            parts.push(shell_quote(&ssh.join(name).to_string_lossy()));
        }
    }
    parts.join(" ")
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r#"'\''"#))
}

/// Whether a push must carry `entry`'s body: the runner has not applied that
/// hash at that path, or what it wrote is gone from the home.
fn needs_body(state: &State, home: &Path, entry: &Entry) -> bool {
    state
        .files
        .get(&entry.path)
        .is_none_or(|managed| managed.origin != entry.sha256)
        || !parents_inside(home, &entry.path)
        || fs::symlink_metadata(home.join(&entry.path)).is_err()
}

/// The global Git configuration for a profile holding `files`: the origin's
/// XDG configuration and then its `~/.gitconfig`, in the order Git itself
/// reads them, and the ignore and attributes files Git would find through
/// `XDG_CONFIG_HOME`, which a profile does not move.
fn git_config(home: &Path, files: &BTreeMap<String, Managed>) -> Option<String> {
    let quoted = |path: &str| {
        let path = home.join(path).to_string_lossy().into_owned();
        format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
    };
    let mut text = String::new();
    let mut core = String::new();
    for (path, key) in [
        (".config/git/ignore", "excludesFile"),
        (".config/git/attributes", "attributesFile"),
    ] {
        if files.contains_key(path) {
            core.push_str(&format!("\t{key} = {}\n", quoted(path)));
        }
    }
    if !core.is_empty() {
        text.push_str("[core]\n");
        text.push_str(&core);
    }
    for path in [".config/git/config", ".gitconfig"] {
        if files.contains_key(path) {
            text.push_str(&format!("[include]\n\tpath = {}\n", quoted(path)));
        }
    }
    (!text.is_empty()).then_some(text)
}

/// A push's changes to the live profile, prepared before any is made.
#[derive(Default)]
struct Plan {
    /// Staged files, where each goes, and that place under the home.
    writes: Vec<(PathBuf, PathBuf, Option<String>)>,
    /// Files and folders the origin dropped that stand where a new file or
    /// folder goes, home-relative. Moved aside before the writes.
    blockers: Vec<String>,
    /// Files this push leaves unneeded, removed once it is in.
    unused: Vec<PathBuf>,
}

/// How to take back one step of a commit.
enum Undo {
    /// Put `aside` back at `place`.
    Restore { aside: PathBuf, place: PathBuf },
    Remove(PathBuf),
    RemoveFolder(PathBuf),
}

impl Undo {
    fn revert(self) {
        let _ = match self {
            Undo::Restore { aside, place } => fs::rename(aside, place),
            Undo::Remove(place) => fs::remove_file(place),
            Undo::RemoveFolder(place) => fs::remove_dir(place),
        };
    }
}

/// Checks what stands at and above `relative` in `home` before a file is
/// written there. A file where a folder must go, or a folder where the file
/// must go, is planned to move aside when the origin dropped it and the
/// runner never changed it (`obsolete`); anything else refuses the push.
fn plan_place(
    home: &Path,
    relative: &str,
    obsolete: &dyn Fn(&str) -> bool,
    blockers: &mut Vec<String>,
) -> Result<(), String> {
    let parts = relative.split('/').collect::<Vec<_>>();
    for end in 1..parts.len() {
        let parent = parts[..end].join("/");
        if blockers.contains(&parent) {
            return Ok(());
        }
        match fs::symlink_metadata(home.join(&parent)) {
            Err(_) => return Ok(()),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("~/{parent} in the profile is a link; {relative} cannot go under it."));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) if obsolete(&parent) => {
                blockers.push(parent);
                return Ok(());
            }
            Ok(_) => {
                return Err(format!("~/{parent} on the runner is in the way of ~/{relative}."));
            }
        }
    }
    let place = home.join(relative);
    if fs::symlink_metadata(&place).is_ok_and(|metadata| metadata.is_dir()) {
        let mut pending = vec![(place, relative.to_owned())];
        while let Some((folder, folder_relative)) = pending.pop() {
            for child in fs::read_dir(&folder).map_err(|e| e.to_string())? {
                let child = child.map_err(|e| e.to_string())?;
                let name = child.file_name().to_string_lossy().into_owned();
                let child_relative = format!("{folder_relative}/{name}");
                let kind = child.file_type().map_err(|e| e.to_string())?;
                if kind.is_dir() {
                    pending.push((child.path(), child_relative));
                } else if !kind.is_file() || !obsolete(&child_relative) {
                    return Err(format!(
                        "~/{child_relative} on the runner is in the way of ~/{relative}."
                    ));
                }
            }
        }
        blockers.push(relative.to_owned());
    }
    Ok(())
}

/// Puts `plan` in place, recording in `undo` how to take each step back.
fn commit(home: &Path, staging: &Path, plan: &Plan, undo: &mut Vec<Undo>) -> io::Result<()> {
    for (index, blocker) in plan.blockers.iter().enumerate() {
        let aside = staging.join(format!("aside-{index}"));
        let place = home.join(blocker);
        fs::rename(&place, &aside)?;
        undo.push(Undo::Restore { aside, place });
    }
    for (index, (temporary, destination, relative)) in plan.writes.iter().enumerate() {
        if let Some(relative) = relative {
            let mut current = home.to_path_buf();
            let parts = relative.split('/').collect::<Vec<_>>();
            for part in &parts[..parts.len() - 1] {
                current.push(part);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                    Ok(_) => {
                        return Err(io::Error::other(format!("{} is not a folder", current.display())));
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        fs::create_dir(&current)?;
                        undo.push(Undo::RemoveFolder(current.clone()));
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        // The file it replaces stays reachable until the push is in, and the
        // replacement itself is one rename, so the path never goes missing.
        if fs::symlink_metadata(destination).is_ok() {
            let aside = staging.join(format!("previous-{index}"));
            fs::hard_link(destination, &aside)?;
            fs::rename(temporary, destination)?;
            undo.push(Undo::Restore { aside, place: destination.clone() });
        } else {
            fs::rename(temporary, destination)?;
            undo.push(Undo::Remove(destination.clone()));
        }
    }
    Ok(())
}

/// Removes what an interrupted push left staged.
fn remove_stale_staging(directory: &Path) -> io::Result<()> {
    for entry in fs::read_dir(directory)?.filter_map(Result::ok) {
        if entry.file_name().to_string_lossy().starts_with(".staging-")
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

/// Sets the mode of the regular file `relative` under `home`, never through
/// a link.
fn set_mode_inside(home: &Path, relative: &str, mode: u32) -> io::Result<()> {
    if !parents_inside(home, relative) {
        return Ok(());
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(home.join(relative))?;
    if file.metadata()?.is_file() {
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// `path` as a private folder, made if missing, refused if it is a link.
fn private_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(path)?,
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::other(format!("{} is not a folder", path.display())));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// A regular file's bytes, never through a link.
fn read_regular(path: &Path) -> io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(format!("{} is not a file", path.display())));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The bytes of `relative` under `home`, when it and every folder above it
/// are real: what a profile reads back is never something a link points at.
fn read_inside(home: &Path, relative: &str) -> Option<Vec<u8>> {
    parents_inside(home, relative)
        .then(|| read_regular(&home.join(relative)).ok())
        .flatten()
}

/// Whether every folder above `relative` under `home` is a real folder.
fn parents_inside(home: &Path, relative: &str) -> bool {
    let mut current = home.to_path_buf();
    let parts = relative.split('/').collect::<Vec<_>>();
    parts[..parts.len().saturating_sub(1)].iter().all(|part| {
        current.push(part);
        fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.is_dir())
    })
}

fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let temporary = stage_file(path, bytes, mode)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

/// Writes `bytes` to a new private file beside `path` and returns its name.
fn stage_file(path: &Path, bytes: &[u8], mode: u32) -> io::Result<PathBuf> {
    let directory = path.parent().unwrap_or(Path::new("."));
    let temporary = directory.join(format!(
        ".{}.xd-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file"),
        uuid::Uuid::new_v4().simple()
    ));
    create_file(&temporary, bytes, mode)?;
    Ok(temporary)
}

/// Writes `bytes` to the new file `path`, removing it again on failure.
fn create_file(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    use std::io::Write as _;
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)?;
        file.write_all(bytes)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
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
        assert!(secret_path(".ssh/deploy-key"), "a key is a key whatever its name");
        assert!(!secret_path(".ssh/known_hosts") && !secret_path(".ssh/config"));
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
    fn hosts_what_cannot_run_on_a_runner_on_the_origin() {
        let home = origin_home();
        // A wrapper in bin that starts a Python environment, as an MCP server
        // and as a Codex hook, and a hook whose folder stays behind.
        write(
            &home.join(".local/bin/wrapped"),
            &format!("#!/bin/sh\nexec {}/Projects/py/.venv/bin/py-mcp\n", home.display()),
        );
        // Two Codex hooks that cannot run on a runner: one the user approved,
        // one never approved.
        let codex_hooks = format!(
            r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"{h}/.local/bin/wrapped --stop"}}]}}],"SessionStart":[{{"hooks":[{{"type":"command","command":"{h}/.local/bin/wrapped --start"}}]}}]}}}}"#,
            h = home.display()
        );
        write(&home.join(".codex/hooks.json"), &codex_hooks);
        let parsed: Value = serde_json::from_str(&codex_hooks).unwrap();
        let stop = &parsed["hooks"]["Stop"][0];
        let approved = origin_hosting::codex_hook_hash("stop", stop, &stop["hooks"][0]).unwrap();
        write(
            &home.join(".codex/config.toml"),
            &format!(
                "[mcp_servers.wrapped]\ncommand = \"{h}/.local/bin/wrapped\"\nenv = {{ TOKEN = \"t\" }}\n\n\
                 [mcp_servers.web]\nurl = \"https://x\"\n\n\
                 [hooks.state.\"{h}/.codex/hooks.json:stop:0:0\"]\ntrusted_hash = \"{approved}\"\n",
                h = home.display()
            ),
        );
        let collected = collect_from(&home);
        let file = |path: &str| {
            let file = collected.files.iter().find(|file| file.path == path).unwrap();
            String::from_utf8(file.source.read().unwrap()).unwrap()
        };
        let exec = format!("{}/.xd/origin-exec", home.display());

        let claude: Value = serde_json::from_str(&file(".claude/.claude.json")).unwrap();
        let py = &claude["mcpServers"]["py"];
        assert_eq!(py["command"], exec.as_str());
        let id = py["args"][0].as_str().unwrap();
        let hosted = &collected.hosted[id];
        assert!(hosted.program.ends_with("/Projects/py/.venv/bin/py-mcp"));
        // Servers that came along whole, and remote ones, stay as they were.
        assert!(claude["mcpServers"]["chat"]["command"].as_str().unwrap().ends_with("/.local/bin/chat-mcp"));
        assert_eq!(claude["mcpServers"]["lens"]["url"], "https://x");

        let codex = file(".codex/config.toml");
        assert!(codex.contains(&format!("command = \"{exec}\"")), "{codex}");
        assert!(!codex.contains("TOKEN"), "the origin keeps the server's variables");
        let wrapped = collected
            .hosted
            .values()
            .find(|hosted| hosted.label == "Codex MCP server wrapped")
            .unwrap();
        assert_eq!(wrapped.env.get("TOKEN").map(String::as_str), Some("t"));
        assert!(codex.contains("[mcp_servers.web]"));

        let hooks: Value = serde_json::from_str(&file(".codex/hooks.json")).unwrap();
        let command = hooks["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
        let hook_id = command.strip_prefix(&format!("{exec} ")).unwrap();
        assert_eq!(collected.hosted[hook_id].program, "/bin/sh");
        assert!(collected.hosted[hook_id].args[1].ends_with("/.local/bin/wrapped --stop"));
        // The hook nobody approved is not offered: it stays as written, and
        // stays untrusted on the runner as on the origin.
        let start = hooks["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str().unwrap();
        assert!(start.ends_with("/.local/bin/wrapped --start"), "{start}");
        assert!(!collected.hosted.values().any(|hosted| hosted.args.iter().any(|arg| arg.ends_with("--start"))));
        // The Claude hook's folder came along, so it runs on the runner.
        assert!(file(".claude/settings.json").contains("/.local/share/hooks/dispatch.py"));
        let manifest = collected.manifest();
        assert_eq!(manifest["origin_hosted"].as_array().unwrap().len(), 3);
        assert!(!manifest.to_string().contains("py-mcp"), "commands stay on the origin");
    }

    #[test]
    fn hosts_programs_a_runner_cannot_be_known_to_have() {
        let home = origin_home();
        write(&home.join("data/partial/kept.txt"), "x\n");
        write(
            &home.join(".claude.json"),
            &format!(
                r#"{{"mcpServers":{{"brew":{{"command":"/opt/homebrew/bin/tool"}},"npx":{{"command":"npx","args":["-y","pkg"]}},"system":{{"command":"/usr/bin/env","args":["true"]}},"data":{{"command":"npx","args":["{}/data/partial"]}}}}}}"#,
                home.display()
            ),
        );
        let collected = collect_from(&home);
        let file = collected.files.iter().find(|file| file.path == ".claude/.claude.json").unwrap();
        let claude: Value = serde_json::from_slice(&file.source.read().unwrap()).unwrap();
        let hosted = |name: &str| {
            claude["mcpServers"][name]["command"]
                .as_str()
                .unwrap()
                .ends_with("/.xd/origin-exec")
        };
        assert!(hosted("brew"), "a Homebrew program");
        assert!(hosted("data"), "a folder that was not carried");
        assert!(!hosted("npx"), "a program found on the runner's PATH");
        assert!(!hosted("system"));
    }

    #[test]
    fn a_runner_writes_its_origin_exec_and_retrusts_codex_hooks() {
        let home = origin_home();
        let hooks = format!(
            r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"{}/.local/share/hooks/dispatch.py"}}]}}]}}}}"#,
            home.display()
        );
        write(&home.join(".codex/hooks.json"), &hooks);
        let parsed: Value = serde_json::from_str(&hooks).unwrap();
        let group = &parsed["hooks"]["Stop"][0];
        let hash = origin_hosting::codex_hook_hash("stop", group, &group["hooks"][0]).unwrap();
        write(
            &home.join(".codex/config.toml"),
            &format!(
                "[hooks.state.\"{}/.codex/hooks.json:stop:0:0\"]\ntrusted_hash = \"{hash}\"\n",
                home.display()
            ),
        );
        let collected = collect_from(&home);
        assert_eq!(collected.codex_trusted_hooks.len(), 1);
        let profiles = Profiles::open(scratch("runner"));
        push(&profiles, "laptop", &collected);
        let profile = profiles.home("laptop").unwrap();
        let script = fs::read_to_string(profile.join(".xd/origin-exec")).unwrap();
        assert!(script.contains(" origin-exec ") && script.contains("--origin 'laptop'"));
        let runner_hooks = fs::read_to_string(profile.join(".codex/hooks.json")).unwrap();
        let runner: Value = serde_json::from_str(&runner_hooks).unwrap();
        let group = &runner["hooks"]["Stop"][0];
        let expected = origin_hosting::codex_hook_hash("stop", group, &group["hooks"][0]).unwrap();
        assert_ne!(expected, hash, "the command moved into the profile");
        let config = fs::read_to_string(profile.join(".codex/config.toml")).unwrap();
        assert!(config.contains(&expected), "{config}");
        assert!(config.contains(&format!("{}/.codex/hooks.json:stop:0:0", profile.display())));
        // The rewrite is the runner's own, not a change it offers back.
        let again = push(&profiles, "laptop", &collect_from(&home));
        assert_eq!(again["written"], json!([]));
        assert!(profiles.pull("laptop").unwrap()["changed"].as_array().unwrap().is_empty());
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
        assert_eq!(get("GIT_CONFIG_GLOBAL"), Some(profile.with_file_name(GIT_CONFIG).to_string_lossy().into()));
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
    fn a_refused_push_changes_nothing() {
        let profiles = Profiles::open(scratch("runner"));
        let file = |path: &str, body: &[u8]| json!({"path": path, "sha256": sha256(body)});
        let blob = |body: &[u8]| (sha256(body), json!(STANDARD.encode(body)));
        let first = json!({"origin_home": "/home/a", "files": [
            file(".claude/.credentials.json", b"login-1"), file("b", b"b-1")]});
        profiles
            .apply("laptop", &first, &Map::from_iter([blob(b"login-1"), blob(b"b-1")]), false)
            .unwrap();
        let home = profiles.home("laptop").unwrap();
        let second = json!({"origin_home": "/home/a", "files": [
            file(".claude/.credentials.json", b"login-2"), file("b", b"b-2")]});
        // A body missing after a good one…
        let reply = profiles
            .apply("laptop", &second, &Map::from_iter([blob(b"login-2")]), false)
            .unwrap();
        assert_eq!(reply["ok"], false);
        // …or one that does not match its hash.
        let mut wrong = Map::from_iter([blob(b"login-2")]);
        wrong.insert(sha256(b"b-2"), json!(STANDARD.encode(b"other")));
        assert!(profiles.apply("laptop", &second, &wrong, false).is_err());
        assert_eq!(fs::read(home.join(".claude/.credentials.json")).unwrap(), b"login-1");
        assert_eq!(fs::read(home.join("b")).unwrap(), b"b-1");
        assert!(profiles.pull("laptop").unwrap()["changed"].as_array().unwrap().is_empty());
        let leftovers = fs::read_dir(home.join(".claude"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".xd-"))
            .count();
        assert_eq!(leftovers, 0, "no staged files are left behind");
    }

    /// Pushes `files` (path, body, mode) as a whole manifest, with only the
    /// bodies the runner asks for.
    fn push_files(profiles: &Profiles, files: &[(&str, &[u8], u32)]) -> Result<Value, String> {
        let manifest = json!({"origin_home": "/home/a", "files": files.iter().map(|(path, body, mode)| {
            let mut file = json!({"path": path, "sha256": sha256(body), "mode": mode});
            if path.ends_with(".claude.json") {
                file["merge"] = json!("json-keys");
            }
            file
        }).collect::<Vec<_>>()});
        let missing = profiles.missing("laptop", &manifest)?;
        let blobs = files
            .iter()
            .filter(|(_, body, _)| missing.contains(&sha256(body)))
            .map(|(_, body, _)| (sha256(body), json!(STANDARD.encode(body))))
            .collect::<Map<_, _>>();
        profiles.apply("laptop", &manifest, &blobs, true)
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_file_gone_from_the_runner_is_sent_again() {
        let profiles = Profiles::open(scratch("runner"));
        push_files(&profiles, &[("notes/a", b"a", 0o644)]).unwrap();
        let home = profiles.home("laptop").unwrap();
        fs::remove_file(home.join("notes/a")).unwrap();
        let reply = push_files(&profiles, &[("notes/a", b"a", 0o644)]).unwrap();
        assert_eq!(reply["written"], json!(["notes/a"]), "{reply}");
        assert_eq!(fs::read(home.join("notes/a")).unwrap(), b"a");
    }

    #[test]
    fn modes_follow_the_origin_and_keys_stay_private() {
        let profiles = Profiles::open(scratch("runner"));
        push_files(&profiles, &[("bin/run", b"#!/bin/sh\n", 0o644), (".ssh/id", b"KEY", 0o600)]).unwrap();
        let home = profiles.home("laptop").unwrap();
        assert_eq!(mode(&home.join("bin/run")), 0o644);
        fs::set_permissions(home.join(".ssh/id"), fs::Permissions::from_mode(0o644)).unwrap();
        let reply =
            push_files(&profiles, &[("bin/run", b"#!/bin/sh\n", 0o755), (".ssh/id", b"KEY", 0o644)]).unwrap();
        assert_eq!(reply["written"], json!([]), "only modes changed: {reply}");
        assert_eq!(mode(&home.join("bin/run")), 0o755);
        assert_eq!(mode(&home.join(".ssh/id")), 0o600);
    }

    #[test]
    fn a_file_and_a_folder_trade_places_unless_the_runner_owns_what_is_in_the_way() {
        let profiles = Profiles::open(scratch("runner"));
        let home = profiles.home("laptop").unwrap();
        push_files(&profiles, &[("tool", b"one file", 0o644), ("keep", b"k", 0o644)]).unwrap();
        let reply = push_files(&profiles, &[("tool/main", b"main", 0o644), ("keep", b"k", 0o644)]).unwrap();
        assert_eq!(reply["removed"], json!(["tool"]), "{reply}");
        assert_eq!(fs::read(home.join("tool/main")).unwrap(), b"main");
        let reply = push_files(&profiles, &[("tool", b"one file", 0o644), ("keep", b"k", 0o644)]).unwrap();
        assert_eq!(reply["removed"], json!(["tool/main"]), "{reply}");
        assert_eq!(fs::read(home.join("tool")).unwrap(), b"one file");

        // A file of the runner's own where a folder must go refuses the
        // whole push.
        write(&home.join("cache"), "the runner's");
        let refused = push_files(
            &profiles,
            &[("tool", b"two", 0o644), ("cache/x", b"x", 0o644), ("keep", b"k", 0o644)],
        );
        assert!(refused.unwrap_err().contains("in the way"));
        assert_eq!(fs::read(home.join("tool")).unwrap(), b"one file");
        assert_eq!(fs::read_to_string(home.join("cache")).unwrap(), "the runner's");
        let staged = fs::read_dir(home.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".staging-"))
            .count();
        assert_eq!(staged, 0);
    }

    #[test]
    fn settings_keys_the_origin_drops_go_and_the_runners_stay() {
        let profiles = Profiles::open(scratch("runner"));
        let home = profiles.home("laptop").unwrap();
        push_files(&profiles, &[(".claude/.claude.json", br#"{"mcpServers":1,"theme":"dark"}"#, 0o600)]).unwrap();
        let path = home.join(".claude/.claude.json");
        let mut settings: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        settings["numStartups"] = json!(3);
        fs::write(&path, settings.to_string()).unwrap();
        push_files(&profiles, &[(".claude/.claude.json", br#"{"mcpServers":2}"#, 0o600)]).unwrap();
        let settings: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(settings, json!({"mcpServers": 2, "numStartups": 3}));
    }

    #[test]
    fn git_reads_both_of_the_origins_global_configurations() {
        let profiles = Profiles::open(scratch("runner"));
        push_files(
            &profiles,
            &[
                (".config/git/config", b"[user]\n\tname = Xdg\n[core]\n\teditor = vi\n", 0o644),
                (".gitconfig", b"[user]\n\tname = Home\n", 0o644),
            ],
        )
        .unwrap();
        let environment = profiles.environment("laptop").unwrap();
        let global = environment
            .iter()
            .find(|(name, _)| name == "GIT_CONFIG_GLOBAL")
            .map(|(_, value)| value.clone())
            .unwrap();
        let get = |key: &str| {
            let output = std::process::Command::new("git")
                .args(["config", "--global", "--includes", "--get", key])
                .env("GIT_CONFIG_GLOBAL", &global)
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        // ~/.gitconfig is read after the XDG file, so it wins where both set
        // a key, and the XDG file's other keys still apply.
        assert_eq!(get("user.name"), "Home");
        assert_eq!(get("core.editor"), "vi");
    }

    #[test]
    fn a_bundle_is_measured_through_its_links() {
        let root = scratch("bundle");
        let large = scratch("large");
        fs::File::create(large.join("blob"))
            .unwrap()
            .set_len(MAX_BUNDLE_BYTES + 1)
            .unwrap();
        write(&root.join("main.py"), "print()\n");
        assert!(measure(&root).is_some());
        std::os::unix::fs::symlink(&large, root.join("data")).unwrap();
        assert!(measure(&root).is_none());
        // A loop is not counted twice, and does not end the measure.
        let looped = scratch("loop");
        write(&looped.join("a"), "a");
        std::os::unix::fs::symlink(&looped, looped.join("again")).unwrap();
        assert_eq!(measure(&looped), Some(1));
    }

    #[test]
    fn an_origin_named_like_the_active_marker_is_its_own() {
        let root = scratch("runner");
        let profiles = Profiles::open(root.clone());
        let manifest = json!({"origin_home": "/home/a", "files": [{"path": "a", "sha256": sha256(b"a")}]});
        let blobs = Map::from_iter([(sha256(b"a"), json!(STANDARD.encode(b"a")))]);
        profiles.apply("active", &manifest, &blobs, true).unwrap();
        assert!(root.join("active/home/a").is_file());
        assert_eq!(Profiles::open(root).active().as_deref(), Some("active"));
        assert!(!valid_origin(ACTIVE));
    }

    #[test]
    fn refuses_a_path_named_as_a_file_and_a_folder_or_the_runners_own() {
        let profiles = Profiles::open(scratch("runner"));
        for paths in [vec!["a", "a/b"], vec![".xd/origin-exec"]] {
            let manifest = json!({"origin_home": "/home/a", "files": paths.iter().map(|path| {
                json!({"path": path, "sha256": sha256(path.as_bytes())})
            }).collect::<Vec<_>>()});
            assert!(profiles.missing("laptop", &manifest).is_err(), "{paths:?}");
        }
    }

    #[test]
    fn reads_back_nothing_a_link_points_at() {
        let profiles = Profiles::open(scratch("runner"));
        let body = b"login-1";
        let manifest = json!({"origin_home": "/home/a", "files": [
            {"path": ".claude/.credentials.json", "sha256": sha256(body)}]});
        profiles
            .apply("laptop", &manifest, &Map::from_iter([(sha256(body), json!(STANDARD.encode(body)))]), false)
            .unwrap();
        let home = profiles.home("laptop").unwrap();
        let elsewhere = scratch("elsewhere").join("secret");
        fs::write(&elsewhere, "someone else's").unwrap();
        fs::remove_file(home.join(".claude/.credentials.json")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.join(".claude/.credentials.json")).unwrap();
        assert!(profiles.pull("laptop").unwrap()["changed"].as_array().unwrap().is_empty());
    }

    #[test]
    fn never_writes_through_a_link_in_the_profile_home() {
        let profiles = Profiles::open(scratch("runner"));
        let outside = scratch("outside");
        let manifest = |body: &[u8]| {
            json!({"origin_home": "/home/a", "files": [{"path": ".claude/skills/x/SKILL.md", "sha256": sha256(body)}]})
        };
        let blobs = |body: &[u8]| Map::from_iter([(sha256(body), json!(STANDARD.encode(body)))]);
        profiles.apply("laptop", &manifest(b"one"), &blobs(b"one"), false).unwrap();
        let home = profiles.home("laptop").unwrap();
        fs::remove_dir_all(home.join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join(".claude/skills")).unwrap();
        assert!(profiles.apply("laptop", &manifest(b"two"), &blobs(b"two"), false).is_err());
        assert!(!outside.join("x").exists());
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
