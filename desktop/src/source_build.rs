use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const REPOSITORY: &str = "RestartFU/xd";
const READ_CHUNK_BYTES: usize = 1_024;
/// Git names the problem in its first lines, so this much is always enough.
const FAILURE_CAPTURE_BYTES: usize = 4_096;
const FAILURE_DETAIL_CHARS: usize = 240;
/// Locks a killed Git leaves behind, each of which fails every later run.
const STALE_GIT_LOCKS: [&str; 3] = ["shallow.lock", "index.lock", "HEAD.lock"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTarget {
    pub url: String,
    pub git_ref: String,
    pub label: String,
}

#[derive(Debug)]
pub enum SourceBuildEvent {
    Output(String),
    Finished(Result<(), String>),
}

pub struct SourceBuildRun {
    receiver: async_channel::Receiver<SourceBuildEvent>,
    control: Arc<RunControl>,
}

struct RunControl {
    cancelled: AtomicBool,
    child: Mutex<Option<Child>>,
}

/// The channels whose installs can replace themselves with a local build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BuildChannel {
    Nightly,
    Dev,
}

impl BuildChannel {
    fn current() -> Option<Self> {
        match env::var("XD_UPDATE_CHANNEL").as_deref() {
            Ok("nightly") => Some(Self::Nightly),
            Ok("dev") => Some(Self::Dev),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Nightly => "nightly",
            Self::Dev => "dev",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BuildPlatform {
    Linux,
    Macos,
}

impl SourceBuildRun {
    pub fn start(target: SourceTarget) -> Result<Self, String> {
        if !supported() {
            return Err("Source builds require an installed nightly or dev build.".into());
        }
        let channel = BuildChannel::current()
            .ok_or_else(|| "Source builds require an installed nightly or dev build.".to_owned())?;
        let checkout = checkout_dir()?;
        // Backpressure keeps a noisy or hostile build from queuing unbounded
        // output while the GPUI thread is busy painting.
        let (sender, receiver) = async_channel::bounded(64);
        let control = Arc::new(RunControl {
            cancelled: AtomicBool::new(false),
            child: Mutex::new(None),
        });
        let worker_control = control.clone();
        thread::Builder::new()
            .name("xd-source-build".into())
            .spawn(move || {
                let result =
                    build_and_install(&target, channel, &checkout, &sender, &worker_control);
                let _ = sender.send_blocking(SourceBuildEvent::Finished(result));
            })
            .map_err(|error| format!("Cannot start the source build: {error}"))?;
        Ok(Self { receiver, control })
    }

    pub fn try_recv(&self) -> Option<SourceBuildEvent> {
        self.receiver.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.control.cancelled.store(true, Ordering::Release);
        if let Ok(mut slot) = self.control.child.lock()
            && let Some(child) = slot.as_mut()
        {
            terminate_process_group(child.id());
            let _ = child.kill();
        }
    }
}

impl Drop for SourceBuildRun {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn supported() -> bool {
    build_platform().is_some()
        && BuildChannel::current().is_some_and(|channel| installed_root(channel).is_some())
}

fn build_platform() -> Option<BuildPlatform> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(BuildPlatform::Linux)
    } else if cfg!(all(
        target_os = "macos",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )) {
        Some(BuildPlatform::Macos)
    } else {
        None
    }
}

pub fn parse_target(text: &str) -> Option<SourceTarget> {
    let value = text.trim();
    if value.is_empty() {
        return None;
    }
    if value.contains("github.com/") {
        return parse_link(value);
    }
    if let Some(number) = value.strip_prefix('#') {
        return parse_pull_request(REPOSITORY, number);
    }
    if digits(value) {
        return parse_pull_request(REPOSITORY, value);
    }
    if commit(value) {
        return parse_commit(REPOSITORY, value);
    }
    parse_branch(REPOSITORY, value)
}

fn parse_link(text: &str) -> Option<SourceTarget> {
    let start = text.find("github.com/")? + "github.com/".len();
    let mut path = &text[start..];
    if let Some(cut) = [path.find('?'), path.find('#')].into_iter().flatten().min() {
        path = &path[..cut];
    }
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }
    let owner = parts[0];
    let repo_name = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    if !repo_part(owner) || !repo_part(repo_name) {
        return None;
    }
    let repository = format!("{owner}/{repo_name}");
    match parts.get(2).copied() {
        Some("pull") if parts.len() >= 4 => parse_pull_request(&repository, parts[3]),
        Some("tree") if parts.len() >= 4 => parse_branch(&repository, &parts[3..].join("/")),
        Some("commit") if parts.len() >= 4 => parse_commit(&repository, parts[3]),
        _ => None,
    }
}

fn parse_pull_request(repository: &str, number: &str) -> Option<SourceTarget> {
    if !digits(number) || number.len() > 9 {
        return None;
    }
    Some(target(
        repository,
        format!("pull request #{number}"),
        format!("refs/pull/{number}/head"),
    ))
}

fn parse_commit(repository: &str, value: &str) -> Option<SourceTarget> {
    if !commit(value) {
        return None;
    }
    Some(target(
        repository,
        format!("commit {}", &value[..value.len().min(12)]),
        value.to_owned(),
    ))
}

fn parse_branch(repository: &str, branch: &str) -> Option<SourceTarget> {
    ref_name(branch).then(|| {
        target(
            repository,
            format!("branch {branch}"),
            format!("refs/heads/{branch}"),
        )
    })
}

fn target(repository: &str, label: String, git_ref: String) -> SourceTarget {
    let label = if repository == REPOSITORY {
        label
    } else {
        format!("{label} in {repository}")
    };
    SourceTarget {
        url: format!("https://github.com/{repository}.git"),
        git_ref,
        label,
    }
}

fn commit(value: &str) -> bool {
    (7..=40).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn ref_name(value: &str) -> bool {
    if value.is_empty() || value.len() > 200 {
        return false;
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
    {
        return false;
    }
    if matches!(value.as_bytes().first(), Some(b'-' | b'/' | b'.'))
        || matches!(value.as_bytes().last(), Some(b'/' | b'.'))
    {
        return false;
    }
    !value.contains("..")
        && !value.contains("//")
        && !value.ends_with(".lock")
        && !value.contains("@{")
}

fn repo_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn checkout_dir() -> Result<PathBuf, String> {
    let cache = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .ok_or_else(|| "Cannot locate the source-build cache directory.".to_owned())?;
    let data_name = env::var("XD_DATA_NAME").unwrap_or_else(|_| "xd-nightly".into());
    Ok(cache.join(data_name).join("source"))
}

fn installed_root(channel: BuildChannel) -> Option<PathBuf> {
    let executable = fs::canonicalize(env::current_exe().ok()?).ok()?;
    let platform = build_platform()?;
    let base = PathBuf::from(env::var_os("HOME")?);
    let root = installed_root_for_executable(&executable, &base, platform, channel)?;
    let marker = match platform {
        BuildPlatform::Linux => root.join("xd.sh"),
        BuildPlatform::Macos => root.join("Contents/MacOS/xd"),
    };
    marker.is_file().then_some(root)
}

fn installed_root_for_executable(
    executable: &Path,
    base: &Path,
    platform: BuildPlatform,
    channel: BuildChannel,
) -> Option<PathBuf> {
    let expected = match platform {
        BuildPlatform::Linux => base.join(format!(".local/opt/xd-{}", channel.name())),
        BuildPlatform::Macos => base.join(format!("Applications/xd-{}.app", channel.name())),
    };
    let root = match platform {
        BuildPlatform::Linux => {
            if executable.file_name()?.to_str()? != "xd" {
                return None;
            }
            executable.parent()?.parent()?
        }
        BuildPlatform::Macos => {
            let macos = executable.parent()?;
            if executable.file_name()?.to_str()? != "xd-desktop"
                || macos.file_name()?.to_str()? != "MacOS"
                || macos.parent()?.file_name()?.to_str()? != "Contents"
            {
                return None;
            }
            macos.parent()?.parent()?
        }
    };
    (root == expected).then(|| root.to_path_buf())
}

fn build_and_install(
    target: &SourceTarget,
    channel: BuildChannel,
    checkout: &Path,
    sender: &async_channel::Sender<SourceBuildEvent>,
    control: &Arc<RunControl>,
) -> Result<(), String> {
    prepare_checkout(checkout)?;
    output(sender, "Fetching source…\n");
    run_git(
        Command::new(git_executable())
            .args(fetch_arguments(path_text(checkout)?, target))
            .env("GIT_TERMINAL_PROMPT", "0"),
        "fetch the source",
        sender,
        control,
    )?;
    output(sender, "Checking out source…\n");
    run_git(
        Command::new(git_executable()).args([
            "-C",
            path_text(checkout)?,
            "checkout",
            "-q",
            "--force",
            "--detach",
            "FETCH_HEAD",
        ]),
        "check out the source",
        sender,
        control,
    )?;
    run_git(
        Command::new(git_executable()).args(["-C", path_text(checkout)?, "clean", "-qfdx"]),
        "clean the source checkout",
        sender,
        control,
    )?;
    let platform =
        build_platform().ok_or_else(|| "Source builds are not supported here.".to_owned())?;
    let mut build = match platform {
        BuildPlatform::Linux => {
            output(
                sender,
                &format!("Building the {} bundle through Docker…\n", channel.name()),
            );
            let mut command = Command::new("./scripts/build.sh");
            match channel {
                BuildChannel::Nightly => {
                    command.args(["--build-arg", "PROFILE=nightly"]);
                }
                BuildChannel::Dev => {
                    command.env("PROFILE", "dev");
                }
            }
            command
        }
        BuildPlatform::Macos => {
            output(
                sender,
                &format!("Building the native macOS {} bundle…\n", channel.name()),
            );
            let mut command = Command::new("./scripts/build-macos.sh");
            command.env("PROFILE", channel.name());
            command
        }
    };
    build.current_dir(checkout);
    run(&mut build, "build the source", sender, control)?;
    output(
        sender,
        &format!("Installing the {} bundle…\n", channel.name()),
    );
    let mut install = Command::new("sh");
    install.current_dir(checkout);
    install.args(install_arguments(platform, channel));
    install.env("XD_ALLOW_RUNNING_INSTALL", "1");
    run(&mut install, "install the source build", sender, control)
}

/// xd's own repository is public, so fetching it needs no credentials. A user's
/// `insteadOf` rule that sends GitHub through SSH would otherwise make the update
/// depend on a key that opens without a prompt, which is why the host's update
/// check avoids `git` altogether. A build needs `git`, so the longest-match rule
/// here keeps this one URL on HTTPS. Other repositories may be private and keep
/// the user's rules.
fn fetch_arguments(checkout: &str, target: &SourceTarget) -> Vec<String> {
    let mut arguments = vec!["-C".to_owned(), checkout.to_owned()];
    if target.url == format!("https://github.com/{REPOSITORY}.git") {
        arguments.extend([
            "-c".to_owned(),
            format!("url.{0}.insteadOf={0}", target.url),
        ]);
    }
    arguments.extend(
        ["fetch", "--depth", "1", "--force"]
            .into_iter()
            .map(str::to_owned),
    );
    arguments.extend([target.url.clone(), target.git_ref.clone()]);
    arguments
}

fn install_arguments(platform: BuildPlatform, channel: BuildChannel) -> Vec<String> {
    let (script, bundle) = match platform {
        BuildPlatform::Linux => ("scripts/install.sh", "dist".to_owned()),
        BuildPlatform::Macos => (
            "scripts/install-macos.sh",
            format!("dist/macos/xd-{}.app", channel.name()),
        ),
    };
    let mut arguments = vec![script.to_owned()];
    if channel == BuildChannel::Dev {
        arguments.push("--dev".to_owned());
    }
    arguments.extend(["--from".to_owned(), bundle]);
    arguments
}

fn prepare_checkout(checkout: &Path) -> Result<(), String> {
    match fs::symlink_metadata(checkout) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err("The source-build cache path is not a regular directory.".into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(checkout)
                .map_err(|error| format!("Cannot create the source checkout: {error}"))?;
        }
        Err(error) => return Err(format!("Cannot inspect the source checkout: {error}")),
    }
    let git_dir = checkout.join(".git");
    match fs::symlink_metadata(&git_dir) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            clear_stale_locks(&git_dir)?;
            // An interrupted init leaves a directory Git does not recognize.
            if git_dir.join("HEAD").is_file() {
                Ok(())
            } else {
                init_checkout(checkout)
            }
        }
        Ok(_) => Err("The source-build Git directory is unsafe.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => init_checkout(checkout),
        Err(error) => Err(format!("Cannot inspect the source checkout: {error}")),
    }
}

/// Stopping a build or quitting kills Git outright, which leaves its locks in
/// place. The app runs one build at a time, so these are not its own live locks.
/// A second running copy of this install is not excluded, but two builds would
/// already overwrite each other in this shared checkout.
fn clear_stale_locks(git_dir: &Path) -> Result<(), String> {
    for lock in STALE_GIT_LOCKS {
        match fs::remove_file(git_dir.join(lock)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot clear the stale {lock}: {error}")),
        }
    }
    Ok(())
}

fn init_checkout(checkout: &Path) -> Result<(), String> {
    let status = Command::new(git_executable())
        .args(["init", "-q"])
        .arg(checkout)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("Cannot initialize the source checkout: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "Git could not initialize the source checkout.".into())
}

/// Runs a Git step, whose first error line says why far better than its exit
/// code. Build scripts stay on [`run`]: their stderr is progress, not a reason.
fn run_git(
    command: &mut Command,
    action: &str,
    sender: &async_channel::Sender<SourceBuildEvent>,
    control: &Arc<RunControl>,
) -> Result<(), String> {
    let stderr = Arc::new(Mutex::new(Vec::new()));
    run_capturing(command, action, sender, control, Some(&stderr))
}

fn run(
    command: &mut Command,
    action: &str,
    sender: &async_channel::Sender<SourceBuildEvent>,
    control: &Arc<RunControl>,
) -> Result<(), String> {
    run_capturing(command, action, sender, control, None)
}

fn run_capturing(
    command: &mut Command,
    action: &str,
    sender: &async_channel::Sender<SourceBuildEvent>,
    control: &Arc<RunControl>,
    captured_stderr: Option<&Arc<Mutex<Vec<u8>>>>,
) -> Result<(), String> {
    if control.cancelled.load(Ordering::Acquire) {
        return Err("Source build stopped.".into());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Cannot {action}: {error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let readers = [
        stdout.map(|reader| (OutputReader::Stdout(reader), None)),
        stderr.map(|reader| (OutputReader::Stderr(reader), captured_stderr.cloned())),
    ]
    .into_iter()
    .flatten()
    .map(|(reader, capture)| spawn_reader(reader, sender.clone(), capture))
    .collect::<Vec<_>>();
    *control
        .child
        .lock()
        .map_err(|_| "Source-build process state is unavailable.".to_owned())? = Some(child);

    let status = wait_for_child(control)?;
    for reader in readers {
        let _ = reader.join();
    }
    if control.cancelled.load(Ordering::Acquire) {
        return Err("Source build stopped.".into());
    }
    if status.success() {
        return Ok(());
    }
    let detail = captured_stderr
        .and_then(|stderr| stderr.lock().ok())
        .and_then(|stderr| failure_detail(&String::from_utf8_lossy(&stderr)));
    Err(match detail {
        Some(detail) => format!("Could not {action}: {detail}"),
        None => format!("Could not {action} ({}).", status_text(status)),
    })
}

/// The first line of Git's stderr that states the failure, without the advice
/// and notices that can come before it.
fn failure_detail(stderr: &str) -> Option<String> {
    let line = stderr.lines().map(str::trim).find(|line| {
        let lower = line.to_ascii_lowercase();
        !line.is_empty() && !lower.starts_with("warning:") && !lower.starts_with("hint:")
    })?;
    let line = line.strip_prefix("fatal: ").unwrap_or(line);
    let line = line.strip_prefix("error: ").unwrap_or(line);
    Some(line.chars().take(FAILURE_DETAIL_CHARS).collect())
}

fn git_executable() -> PathBuf {
    PathBuf::from("git")
}

enum OutputReader {
    Stdout(std::process::ChildStdout),
    Stderr(std::process::ChildStderr),
}

impl Read for OutputReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Stdout(reader) => reader.read(buffer),
            Self::Stderr(reader) => reader.read(buffer),
        }
    }
}

fn spawn_reader(
    mut reader: OutputReader,
    sender: async_channel::Sender<SourceBuildEvent>,
    capture: Option<Arc<Mutex<Vec<u8>>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; READ_CHUNK_BYTES];
        loop {
            let Ok(count) = reader.read(&mut buffer) else {
                break;
            };
            if count == 0 {
                break;
            }
            if let Some(mut captured) = capture.as_ref().and_then(|capture| capture.lock().ok()) {
                let room = FAILURE_CAPTURE_BYTES.saturating_sub(captured.len());
                captured.extend_from_slice(&buffer[..count.min(room)]);
            }
            output(&sender, &String::from_utf8_lossy(&buffer[..count]));
        }
    })
}

fn wait_for_child(control: &Arc<RunControl>) -> Result<ExitStatus, String> {
    loop {
        let status = {
            let mut slot = control
                .child
                .lock()
                .map_err(|_| "Source-build process state is unavailable.".to_owned())?;
            let child = slot
                .as_mut()
                .ok_or_else(|| "Source-build process state was lost.".to_owned())?;
            if control.cancelled.load(Ordering::Acquire) {
                terminate_process_group(child.id());
                let _ = child.kill();
            }
            child
                .try_wait()
                .map_err(|error| format!("Cannot wait for the source build: {error}"))?
        };
        if let Some(status) = status {
            let _ = control.child.lock().map(|mut slot| slot.take());
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn terminate_process_group(pid: u32) {
    #[cfg(unix)]
    let _ = Command::new("kill")
        .args(["-KILL", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn output(sender: &async_channel::Sender<SourceBuildEvent>, text: &str) {
    let _ = sender.send_blocking(SourceBuildEvent::Output(text.to_owned()));
}

fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "The source-build cache path is not valid UTF-8.".to_owned())
}

fn status_text(status: ExitStatus) -> String {
    status
        .code()
        .map(|code| format!("exit {code}"))
        .unwrap_or_else(|| "terminated".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetching_xd_itself_ignores_rewrites_that_other_repositories_keep() {
        let own = parse_target("abcdef0123456789").unwrap();
        let arguments = fetch_arguments("/checkout", &own);
        assert_eq!(
            arguments[..4],
            [
                "-C",
                "/checkout",
                "-c",
                "url.https://github.com/RestartFU/xd.git.insteadOf=https://github.com/RestartFU/xd.git",
            ]
        );
        assert_eq!(
            arguments[4..],
            [
                "fetch",
                "--depth",
                "1",
                "--force",
                "https://github.com/RestartFU/xd.git",
                "abcdef0123456789",
            ]
        );

        let other = parse_target("https://github.com/example/project/tree/main").unwrap();
        assert!(
            !fetch_arguments("/checkout", &other)
                .iter()
                .any(|argument| argument == "-c")
        );
    }

    #[test]
    fn failure_detail_names_the_reason_rather_than_the_preamble() {
        assert_eq!(
            failure_detail(
                "Warning: Permanently added 'github.com' to known hosts.\n\
                 git@github.com: Permission denied (publickey).\n\
                 fatal: Could not read from remote repository.\n"
            )
            .as_deref(),
            Some("git@github.com: Permission denied (publickey).")
        );
        assert_eq!(
            failure_detail("hint: try again\nfatal: couldn't find remote ref abc\n").as_deref(),
            Some("couldn't find remote ref abc")
        );
        assert_eq!(failure_detail("\n  \nwarning: noise\n"), None);
        assert_eq!(
            failure_detail(&"é".repeat(FAILURE_DETAIL_CHARS * 2))
                .unwrap()
                .chars()
                .count(),
            FAILURE_DETAIL_CHARS
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_git_failure_reports_what_git_said_and_a_build_failure_keeps_its_exit_code() {
        let (sender, _receiver) = async_channel::bounded(64);
        let control = Arc::new(RunControl {
            cancelled: AtomicBool::new(false),
            child: Mutex::new(None),
        });
        let failing = || {
            let mut command = Command::new("sh");
            command.args(["-c", "echo 'fatal: no route' >&2; exit 128"]);
            command
        };
        assert_eq!(
            run_git(&mut failing(), "fetch the source", &sender, &control),
            Err("Could not fetch the source: no route".to_owned())
        );
        assert_eq!(
            run(&mut failing(), "build the source", &sender, &control),
            Err("Could not build the source (exit 128).".to_owned())
        );
    }

    #[test]
    fn a_checkout_wrecked_by_an_interrupted_git_recovers() {
        let checkout = env::temp_dir().join(format!("xd-source-{}", std::process::id()));
        let _ = fs::remove_dir_all(&checkout);
        let git_dir = checkout.join(".git");
        fs::create_dir_all(&checkout).unwrap();

        // An interrupted init left a directory that is not yet a repository.
        fs::create_dir(&git_dir).unwrap();
        prepare_checkout(&checkout).unwrap();
        assert!(git_dir.join("HEAD").is_file());

        // A killed fetch left its lock, which Git would refuse to get past.
        for lock in STALE_GIT_LOCKS {
            fs::write(git_dir.join(lock), "").unwrap();
        }
        prepare_checkout(&checkout).unwrap();
        for lock in STALE_GIT_LOCKS {
            assert!(!git_dir.join(lock).exists(), "kept {lock}");
        }

        fs::remove_dir_all(&checkout).unwrap();
    }

    fn installed_nightly_root_for_executable(
        executable: &Path,
        base: &Path,
        platform: BuildPlatform,
    ) -> Option<PathBuf> {
        installed_root_for_executable(executable, base, platform, BuildChannel::Nightly)
    }

    #[test]
    fn a_dev_install_builds_and_installs_as_dev() {
        let home = Path::new("/home/person");
        assert_eq!(
            installed_root_for_executable(
                Path::new("/home/person/.local/opt/xd-dev/bin/xd"),
                home,
                BuildPlatform::Linux,
                BuildChannel::Dev,
            ),
            Some(PathBuf::from("/home/person/.local/opt/xd-dev"))
        );
        assert_eq!(
            installed_root_for_executable(
                Path::new("/home/person/.local/opt/xd-nightly/bin/xd"),
                home,
                BuildPlatform::Linux,
                BuildChannel::Dev,
            ),
            None
        );
        assert_eq!(
            install_arguments(BuildPlatform::Linux, BuildChannel::Dev),
            ["scripts/install.sh", "--dev", "--from", "dist"]
        );
        assert_eq!(
            install_arguments(BuildPlatform::Macos, BuildChannel::Dev),
            [
                "scripts/install-macos.sh",
                "--dev",
                "--from",
                "dist/macos/xd-dev.app"
            ]
        );
        assert_eq!(
            install_arguments(BuildPlatform::Linux, BuildChannel::Nightly),
            ["scripts/install.sh", "--from", "dist"]
        );
        let commit = "77af5848ea1eb3941752a3de6a31ee9da5ed8a2f";
        assert_eq!(parse_target(commit).unwrap().git_ref, commit);
    }

    #[test]
    fn parses_branches_pull_requests_commits_and_links() {
        assert_eq!(
            parse_target("feature/desktop").unwrap(),
            SourceTarget {
                url: "https://github.com/RestartFU/xd.git".into(),
                git_ref: "refs/heads/feature/desktop".into(),
                label: "branch feature/desktop".into(),
            }
        );
        assert_eq!(parse_target("#128").unwrap().git_ref, "refs/pull/128/head");
        assert_eq!(parse_target("128").unwrap().label, "pull request #128");
        assert_eq!(
            parse_target("abcdef0123456789").unwrap().label,
            "commit abcdef012345"
        );
        let linked =
            parse_target("https://github.com/example/project/tree/fix/ui?tab=readme").unwrap();
        assert_eq!(linked.url, "https://github.com/example/project.git");
        assert_eq!(linked.git_ref, "refs/heads/fix/ui");
        assert_eq!(linked.label, "branch fix/ui in example/project");
        assert_eq!(
            parse_target("https://github.com/example/project/pull/42#discussion")
                .unwrap()
                .git_ref,
            "refs/pull/42/head"
        );
    }

    #[test]
    fn rejects_shell_text_invalid_refs_and_ambiguous_links() {
        for value in [
            "",
            "--upload-pack=bad",
            "branch; touch /tmp/no",
            "branch name",
            "../main",
            "main//nested",
            "main.lock",
            "https://github.com/owner/repo",
            "https://github.com/-owner/repo/tree/main",
            "#1234567890",
        ] {
            assert!(parse_target(value).is_none(), "accepted {value:?}");
        }
    }

    #[test]
    fn recognizes_only_installed_nightlies() {
        let linux_home = Path::new("/home/person");
        let linux = Path::new("/home/person/.local/opt/xd-nightly/bin/xd");
        assert_eq!(
            installed_nightly_root_for_executable(linux, linux_home, BuildPlatform::Linux),
            Some(PathBuf::from("/home/person/.local/opt/xd-nightly"))
        );

        let macos_home = Path::new("/Users/person");
        let macos =
            Path::new("/Users/person/Applications/xd-nightly.app/Contents/MacOS/xd-desktop");
        assert_eq!(
            installed_nightly_root_for_executable(macos, macos_home, BuildPlatform::Macos),
            Some(PathBuf::from("/Users/person/Applications/xd-nightly.app"))
        );

        for (executable, home, platform) in [
            (
                "/tmp/xd-nightly/bin/xd",
                "/home/person",
                BuildPlatform::Linux,
            ),
            (
                "/Users/person/Applications/xd.app/Contents/MacOS/xd-desktop",
                "/Users/person",
                BuildPlatform::Macos,
            ),
            (
                "/Users/person/Applications/xd-nightly.app/Contents/MacOS/other",
                "/Users/person",
                BuildPlatform::Macos,
            ),
            (
                "/home/person/.local/opt/xd-nightly/bin/other",
                "/home/person",
                BuildPlatform::Linux,
            ),
        ] {
            assert!(
                installed_nightly_root_for_executable(
                    Path::new(executable),
                    Path::new(home),
                    platform,
                )
                .is_none()
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn output_reader_chunks_are_bounded_and_active_processes_cancel() {
        assert_eq!(READ_CHUNK_BYTES, 1_024);
        let (sender, receiver) = async_channel::bounded(64);
        let control = Arc::new(RunControl {
            cancelled: AtomicBool::new(false),
            child: Mutex::new(None),
        });
        let worker_control = control.clone();
        let worker = thread::spawn(move || {
            let mut command = Command::new("sh");
            command.args(["-c", "printf started; sleep 30"]);
            run(
                &mut command,
                "run cancellation fixture",
                &sender,
                &worker_control,
            )
        });
        let child_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < child_deadline
            && control.child.lock().is_ok_and(|child| child.is_none())
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(control.child.lock().is_ok_and(|child| child.is_some()));
        let run = SourceBuildRun { receiver, control };
        let output_deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut saw_output = false;
        while std::time::Instant::now() < output_deadline {
            if matches!(run.try_recv(), Some(SourceBuildEvent::Output(_))) {
                saw_output = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(saw_output);
        run.cancel();
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error, "Source build stopped.");
    }
}
