use std::{
    env, fs,
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const REPOSITORY: &str = "RestartFU/xd";
const READ_CHUNK_BYTES: usize = 1_024;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BuildPlatform {
    Linux,
    Macos,
}

impl SourceBuildRun {
    pub fn start(target: SourceTarget) -> Result<Self, String> {
        if !supported() {
            return Err("Source builds require an installed nightly on Linux or macOS.".into());
        }
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
                let result = build_and_install(&target, &checkout, &sender, &worker_control);
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
        && env::var("XD_UPDATE_CHANNEL").as_deref() == Ok("nightly")
        && installed_nightly_root().is_some()
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

fn installed_nightly_root() -> Option<PathBuf> {
    let executable = fs::canonicalize(env::current_exe().ok()?).ok()?;
    let home = PathBuf::from(env::var_os("HOME")?);
    let platform = build_platform()?;
    let root = installed_nightly_root_for_executable(&executable, &home, platform)?;
    let marker = match platform {
        BuildPlatform::Linux => root.join("xd.sh"),
        BuildPlatform::Macos => root.join("Contents/MacOS/xd"),
    };
    marker.is_file().then_some(root)
}

fn installed_nightly_root_for_executable(
    executable: &Path,
    home: &Path,
    platform: BuildPlatform,
) -> Option<PathBuf> {
    let expected = match platform {
        BuildPlatform::Linux => home.join(".local/opt/xd-nightly"),
        BuildPlatform::Macos => home.join("Applications/xd-nightly.app"),
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
    checkout: &Path,
    sender: &async_channel::Sender<SourceBuildEvent>,
    control: &Arc<RunControl>,
) -> Result<(), String> {
    prepare_checkout(checkout)?;
    output(sender, "Fetching source…\n");
    run(
        Command::new("git")
            .args([
                "-C",
                path_text(checkout)?,
                "fetch",
                "--depth",
                "1",
                "--force",
            ])
            .arg(&target.url)
            .arg(&target.git_ref),
        "fetch the source",
        sender,
        control,
    )?;
    output(sender, "Checking out source…\n");
    run(
        Command::new("git").args([
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
    run(
        Command::new("git").args(["-C", path_text(checkout)?, "clean", "-qfdx"]),
        "clean the source checkout",
        sender,
        control,
    )?;
    let platform =
        build_platform().ok_or_else(|| "Source builds are not supported here.".to_owned())?;
    let mut build = match platform {
        BuildPlatform::Linux => {
            output(sender, "Building the nightly bundle through Docker…\n");
            let mut command = Command::new("./scripts/build.sh");
            command.args(["--build-arg", "PROFILE=nightly"]);
            command
        }
        BuildPlatform::Macos => {
            output(sender, "Building the native macOS nightly bundle…\n");
            let mut command = Command::new("./scripts/build-macos.sh");
            command.env("PROFILE", "nightly");
            command
        }
    };
    build.current_dir(checkout);
    run(&mut build, "build the source", sender, control)?;
    output(sender, "Installing the nightly bundle…\n");
    let mut install = Command::new("sh");
    install.current_dir(checkout);
    match platform {
        BuildPlatform::Linux => {
            install.args(["scripts/install.sh", "--from", "dist"]);
        }
        BuildPlatform::Macos => {
            install.args([
                "scripts/install-macos.sh",
                "--from",
                "dist/macos/xd-nightly.app",
            ]);
        }
    }
    install.env("XD_ALLOW_RUNNING_INSTALL", "1");
    run(&mut install, "install the source build", sender, control)
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
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err("The source-build Git directory is unsafe.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let status = Command::new("git")
                .args(["init", "-q"])
                .arg(checkout)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .status()
                .map_err(|error| format!("Cannot initialize the source checkout: {error}"))?;
            status
                .success()
                .then_some(())
                .ok_or_else(|| "Git could not initialize the source checkout.".into())
        }
        Err(error) => Err(format!("Cannot inspect the source checkout: {error}")),
    }
}

fn run(
    command: &mut Command,
    action: &str,
    sender: &async_channel::Sender<SourceBuildEvent>,
    control: &Arc<RunControl>,
) -> Result<(), String> {
    if control.cancelled.load(Ordering::Acquire) {
        return Err("Source build stopped.".into());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Cannot {action}: {error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let readers = [
        stdout.map(OutputReader::Stdout),
        stderr.map(OutputReader::Stderr),
    ]
    .into_iter()
    .flatten()
    .map(|reader| spawn_reader(reader, sender.clone()))
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
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("Could not {action} ({}).", status_text(status)))
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
    let _ = Command::new("kill")
        .args(["-KILL", &format!("-{pid}")])
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
    fn recognizes_only_installed_linux_and_macos_nightlies() {
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
