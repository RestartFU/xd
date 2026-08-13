use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const TMUX_CONFIGURATION: &str = concat!(
    "set -g default-terminal screen-256color\n",
    "set -sg escape-time 0\n",
    "set -g focus-events on\n",
    "set -g mouse on\n",
    "set -g status off\n",
    "set -g allow-passthrough on\n",
    "set -g set-titles on\n",
    "set -g set-titles-string \"#{pane_title}\"\n",
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub arguments: Vec<String>,
}

impl ProcessSpec {
    pub fn new<P, I, S>(program: P, arguments: I) -> Self
    where
        P: Into<PathBuf>,
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            program: program.into(),
            arguments: arguments.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCommand {
    program: String,
    arguments: Vec<String>,
    resume_arguments: Option<Vec<String>>,
    record_codex_session: bool,
    unset_environment: Vec<String>,
    login_shell_label: Option<String>,
    user_shell: bool,
}

impl AgentCommand {
    pub fn new<P, I, S>(program: P, arguments: I) -> Self
    where
        P: Into<String>,
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            program: program.into(),
            arguments: arguments.into_iter().map(Into::into).collect(),
            resume_arguments: None,
            record_codex_session: false,
            unset_environment: Vec::new(),
            login_shell_label: None,
            user_shell: false,
        }
    }

    pub fn user_shell() -> Self {
        Self {
            program: String::new(),
            arguments: Vec::new(),
            resume_arguments: None,
            record_codex_session: false,
            unset_environment: Vec::new(),
            login_shell_label: None,
            user_shell: true,
        }
    }

    pub fn unset_environment(mut self, name: impl Into<String>) -> Self {
        self.unset_environment.push(name.into());
        self
    }

    pub fn resume_with<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.resume_arguments = Some(arguments.into_iter().map(Into::into).collect());
        self
    }

    pub fn record_codex_session(mut self) -> Self {
        self.record_codex_session = true;
        self
    }

    pub fn discover_in_user_shell(mut self, label: impl Into<String>) -> Self {
        self.login_shell_label = Some(label.into());
        self
    }

    fn shell_command(&self) -> String {
        self.shell_command_with_marker(None)
    }

    fn shell_command_with_marker(&self, marker: Option<&str>) -> String {
        if self.user_shell {
            return "exec \"${SHELL:-/bin/sh}\" -i".to_owned();
        }
        let start_arguments = match marker.filter(|_| self.record_codex_session) {
            Some(_) => {
                let mut arguments = self.arguments.clone();
                arguments.extend(["-c".to_owned(), codex_notify_override()]);
                arguments
            }
            None => self.arguments.clone(),
        };
        let command = self.command(&start_arguments);
        let command = match (marker, self.resume_arguments.as_deref()) {
            (Some(marker), Some(resume_arguments)) if self.record_codex_session => format!(
                "MARKER={marker}; export XD_AGENT_SESSION_MARKER=\"$MARKER\"; mkdir -p \"$(dirname \"$MARKER\")\" || exit; if [ -s \"$MARKER\" ]; then IFS= read -r SESSION < \"$MARKER\"; exec {} 'resume' \"$SESSION\" {}; elif [ -e \"$MARKER\" ]; then exec {}; else : > \"$MARKER\"; exec {command}; fi",
                shell_quote(&self.program),
                self.arguments
                    .iter()
                    .map(|value| shell_quote(value))
                    .collect::<Vec<_>>()
                    .join(" "),
                self.command(resume_arguments),
            ),
            (Some(marker), Some(resume_arguments)) => format!(
                "MARKER={marker}; mkdir -p \"$(dirname \"$MARKER\")\" || exit; if [ -e \"$MARKER\" ]; then exec {}; else : > \"$MARKER\"; exec {command}; fi",
                self.command(resume_arguments)
            ),
            _ => format!("exec {command}"),
        };
        let Some(label) = &self.login_shell_label else {
            return command;
        };
        let missing = format!(
            "xd: {label} is not installed or is not available on PATH. Install it, then start a new tab."
        );
        let child = format!("(trap - INT; {command})");
        let inner = format!(
            "trap '' INT; if command -v {} >/dev/null 2>&1; then if {child}; then :; else :; fi; else printf '%s\\n' {} >&2; fi; exec \"${{SHELL:-/bin/sh}}\" -i",
            shell_quote(&self.program),
            shell_quote(&missing),
        );
        format!("exec \"${{SHELL:-/bin/sh}}\" -ic {}", shell_quote(&inner))
    }

    fn command(&self, arguments: &[String]) -> String {
        let command = self
            .unset_environment
            .iter()
            .flat_map(|name| ["-u", name.as_str()])
            .chain(std::iter::once(self.program.as_str()))
            .chain(arguments.iter().map(String::as_str))
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" ");
        if self.unset_environment.is_empty() {
            command
        } else {
            format!("env {command}")
        }
    }
}

fn codex_notify_override() -> String {
    let script = "marker=$XD_AGENT_SESSION_MARKER; payload=$1; [ -n \"$marker\" ] || exit 0; id=$(printf '%s\\n' \"$payload\" | sed -n 's/.*\"thread-id\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p'); case \"$id\" in ''|*[!A-Za-z0-9_-]*) exit 0;; esac; umask 077; tmp=\"$marker.$$\"; printf '%s\\n' \"$id\" > \"$tmp\" && mv \"$tmp\" \"$marker\"";
    let command = serde_json::to_string(&["sh", "-c", script, "xd-codex-recorder"])
        .expect("static Codex recorder command should serialize");
    format!("notify={command}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshCommand {
    program: PathBuf,
    options: Vec<String>,
    destination: String,
}

impl SshCommand {
    pub fn parse(input: &str) -> Result<Self, String> {
        let words = split_command_line(input)?;
        let Some(program) = words.first() else {
            return Err("Enter an SSH command.".into());
        };
        let program = PathBuf::from(program);
        let executable = program
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default();
        if executable != "ssh" {
            return Err("The remote command must start with ssh.".into());
        }

        let mut options = Vec::new();
        let mut destination = None;
        let mut index = 1;
        while index < words.len() {
            let word = &words[index];
            if word == "--" {
                index += 1;
                if destination.is_some() || index >= words.len() {
                    return Err("The SSH command has an invalid destination.".into());
                }
                destination = Some(words[index].clone());
            } else if word.starts_with('-') && word != "-" {
                let option = ssh_option(word)?;
                options.push(word.clone());
                if option.needs_value && option.attached_value.is_none() {
                    index += 1;
                    let value = words
                        .get(index)
                        .ok_or_else(|| format!("SSH option {word} needs a value."))?;
                    options.push(value.clone());
                }
            } else if destination.is_none() {
                destination = Some(word.clone());
            } else {
                return Err(
                    "Only an SSH connection may be entered; remove the remote command.".into(),
                );
            }
            index += 1;
        }

        let destination = destination
            .filter(|destination| !destination.trim().is_empty())
            .ok_or_else(|| "The SSH command needs a destination.".to_owned())?;
        Ok(Self {
            program,
            options,
            destination,
        })
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn connection_arguments(&self) -> Vec<String> {
        let mut arguments = self.connection_options();
        arguments.extend(["--".to_owned(), self.destination.clone()]);
        arguments
    }

    pub fn connection_options(&self) -> Vec<String> {
        let mut arguments = self.options.clone();
        if !has_ssh_config_option(&arguments, "ControlMaster") {
            arguments.extend(["-o".into(), "ControlMaster=auto".into()]);
        }
        if !has_ssh_config_option(&arguments, "ControlPersist") {
            arguments.extend(["-o".into(), "ControlPersist=10m".into()]);
        }
        if !has_ssh_config_option(&arguments, "ControlPath")
            && !arguments
                .iter()
                .any(|argument| argument == "-S" || argument.starts_with("-S"))
        {
            arguments.extend(["-o".into(), "ControlPath=~/.ssh/xd-%C".into()]);
        }
        arguments
    }

    pub fn options(&self) -> &[String] {
        &self.options
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }
}

fn has_ssh_config_option(arguments: &[String], name: &str) -> bool {
    arguments.iter().enumerate().any(|(index, argument)| {
        let option = if argument == "-o" {
            arguments.get(index + 1).map(String::as_str)
        } else {
            argument.strip_prefix("-o")
        };
        option
            .and_then(|option| option.split('=').next())
            .is_some_and(|option| option.eq_ignore_ascii_case(name))
    })
}

struct SshOption<'a> {
    needs_value: bool,
    attached_value: Option<&'a str>,
}

fn ssh_option(word: &str) -> Result<SshOption<'_>, String> {
    if matches!(
        word,
        "-N" | "-T" | "-G" | "-s" | "-f" | "-D" | "-L" | "-M" | "-O" | "-R" | "-W"
    ) {
        return Err(format!(
            "SSH option {word} cannot be used for an interactive XD session."
        ));
    }
    let option = word.as_bytes().get(1).copied().map(char::from);
    let Some(option) = option else {
        return Err(format!("Invalid SSH option {word}."));
    };
    if "NTGsfDLMORW".contains(option) {
        return Err(format!(
            "SSH option -{option} cannot be used for an interactive XD session."
        ));
    }
    let needs_value = "BbcDEeFIiJLlmOoPpRSwW".contains(option);
    let is_flag = "46AaCKkMnqVvXxYyt".contains(option);
    if !needs_value && !is_flag {
        return Err(format!("Unsupported SSH option {word}."));
    }
    let attached_value = (word.len() > 2).then(|| &word[2..]);
    if !needs_value && word[2..].chars().any(|character| character != option) {
        return Err(format!("Unsupported combined SSH option {word}."));
    }
    Ok(SshOption {
        needs_value,
        attached_value,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HostTarget {
    Local {
        tmux: PathBuf,
        runtime: PathBuf,
    },
    Ssh {
        command: SshCommand,
        remote_runtime: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionHost {
    target: HostTarget,
}

impl SessionHost {
    pub fn local(tmux: PathBuf, runtime: PathBuf) -> Self {
        Self {
            target: HostTarget::Local { tmux, runtime },
        }
    }

    pub fn ssh(command: SshCommand, remote_runtime: impl Into<String>) -> Self {
        Self {
            target: HostTarget::Ssh {
                command,
                remote_runtime: remote_runtime.into().trim_matches('/').to_owned(),
            },
        }
    }

    pub fn attach(&self, id: &str, workdir: &Path, agent: &AgentCommand) -> ProcessSpec {
        let session = session_name(id);
        match &self.target {
            HostTarget::Local { tmux, runtime } => {
                let marker = agent.resume_arguments.as_ref().map(|_| {
                    runtime
                        .join("agent-sessions")
                        .join(format!("{session}.started"))
                });
                if let Some(marker) = marker.as_ref()
                    && Command::new(tmux)
                        .args(["-S"])
                        .arg(runtime.join("tmux.sock"))
                        .args(["has-session", "-t", &session])
                        .status()
                        .is_ok_and(|status| status.success())
                    && !marker.exists()
                {
                    let _ = marker.parent().map(fs::create_dir_all);
                    let _ = fs::write(marker, []);
                }
                ProcessSpec::new(
                    tmux,
                    [
                        "-S".to_owned(),
                        runtime.join("tmux.sock").to_string_lossy().into_owned(),
                        "-f".to_owned(),
                        runtime.join("tmux.conf").to_string_lossy().into_owned(),
                        "start-server".to_owned(),
                        ";".to_owned(),
                        "source-file".to_owned(),
                        runtime.join("tmux.conf").to_string_lossy().into_owned(),
                        ";".to_owned(),
                        "new-session".to_owned(),
                        "-A".to_owned(),
                        "-s".to_owned(),
                        session.clone(),
                        "-c".to_owned(),
                        workdir.to_string_lossy().into_owned(),
                        agent.shell_command_with_marker(
                            marker
                                .as_ref()
                                .map(|marker| shell_quote(&marker.to_string_lossy()))
                                .as_deref(),
                        ),
                    ],
                )
            }
            HostTarget::Ssh {
                command,
                remote_runtime,
            } => {
                let runtime = format!("$HOME/{remote_runtime}");
                let data_name = crate::channel::data_name();
                let configuration = TMUX_CONFIGURATION
                    .lines()
                    .map(shell_quote)
                    .collect::<Vec<_>>()
                    .join(" ");
                let remote = format!(
                    "RUNTIME=\"{runtime}\"; mkdir -p \"$RUNTIME/agent-sessions\" || exit; CONF=\"$RUNTIME/tmux.conf\"; printf '%s\\n' {configuration} > \"$CONF\" || exit; TMUX=\"$HOME/.local/opt/{}/libexec/tmux\"; [ -x \"$TMUX\" ] || TMUX=tmux; MARKER=\"$RUNTIME/agent-sessions/{session}.started\"; if \"$TMUX\" -S \"$RUNTIME/tmux.sock\" has-session -t {} 2>/dev/null && [ ! -e \"$MARKER\" ]; then : > \"$MARKER\"; fi; exec \"$TMUX\" -S \"$RUNTIME/tmux.sock\" -f \"$CONF\" start-server \\; source-file \"$CONF\" \\; new-session -A -s {} -c {} {}",
                    data_name.to_string_lossy(),
                    shell_quote(&session),
                    shell_quote(&session),
                    shell_quote(&workdir.to_string_lossy()),
                    shell_quote(
                        &agent.shell_command_with_marker(
                            agent
                                .resume_arguments
                                .as_ref()
                                .map(|_| format!(
                                    "\"$HOME/{remote_runtime}/agent-sessions/{session}.started\""
                                ))
                                .as_deref(),
                        )
                    ),
                );
                let mut arguments = command.connection_options();
                arguments.extend([
                    "-tt".to_owned(),
                    "--".to_owned(),
                    command.destination.clone(),
                    remote,
                ]);
                ProcessSpec::new(&command.program, arguments)
            }
        }
    }

    pub fn kill_process(&self, id: &str) -> ProcessSpec {
        let session = session_name(id);
        match &self.target {
            HostTarget::Local { tmux, runtime } => ProcessSpec::new(
                tmux,
                [
                    "-S".to_owned(),
                    runtime.join("tmux.sock").to_string_lossy().into_owned(),
                    "kill-session".to_owned(),
                    "-t".to_owned(),
                    session,
                ],
            ),
            HostTarget::Ssh {
                command,
                remote_runtime,
            } => {
                let runtime = format!("$HOME/{remote_runtime}");
                let remote = format!(
                    "RUNTIME=\"{runtime}\"; TMUX=\"$HOME/.local/opt/{}/libexec/tmux\"; [ -x \"$TMUX\" ] || TMUX=tmux; exec \"$TMUX\" -S \"$RUNTIME/tmux.sock\" kill-session -t {}",
                    crate::channel::data_name().to_string_lossy(),
                    shell_quote(&session),
                );
                let mut arguments = command.connection_options();
                arguments.extend(["--".to_owned(), command.destination.clone(), remote]);
                ProcessSpec::new(&command.program, arguments)
            }
        }
    }
}

fn session_name(id: &str) -> String {
    let hash = id
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("xd-{hash:016x}")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn split_command_line(input: &str) -> Result<Vec<String>, String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = Quote::None;
    let mut escaped = false;
    let mut started = false;
    for character in input.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            started = true;
            continue;
        }
        match (quote, character) {
            (Quote::None, '\\') | (Quote::Double, '\\') => escaped = true,
            (Quote::None, '\'') => {
                quote = Quote::Single;
                started = true;
            }
            (Quote::Single, '\'') => quote = Quote::None,
            (Quote::None, '"') => {
                quote = Quote::Double;
                started = true;
            }
            (Quote::Double, '"') => quote = Quote::None,
            (Quote::None, character) if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (_, character) => {
                word.push(character);
                started = true;
            }
        }
    }
    if escaped || quote != Quote::None {
        return Err("The SSH command has an unfinished quote or escape.".into());
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        thread,
        time::{Duration, Instant},
    };

    use super::{
        AgentCommand, ProcessSpec, SessionHost, SshCommand, TMUX_CONFIGURATION, shell_quote,
    };

    fn detached(mut spec: ProcessSpec) -> ProcessSpec {
        let index = spec
            .arguments
            .iter()
            .position(|argument| argument == "new-session")
            .expect("tmux new-session command");
        spec.arguments.insert(index + 1, "-d".into());
        spec.arguments.retain(|argument| argument != "-A");
        spec
    }

    fn wait_for_lines(path: &Path, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let lines = fs::read_to_string(path)
                .map(|contents| contents.lines().count())
                .unwrap_or(0);
            if lines >= expected {
                return;
            }
            assert!(Instant::now() < deadline, "agent launch was not recorded");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_session_to_close(runtime: &Path, terminal_id: &str) {
        let session = super::session_name(terminal_id);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Command::new("tmux")
            .args(["-S"])
            .arg(runtime.join("tmux.sock"))
            .args(["has-session", "-t", &session])
            .status()
            .is_ok_and(|status| status.success())
        {
            assert!(Instant::now() < deadline, "tmux session did not close");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn codex_notification_records_the_exact_thread_id_atomically() {
        let directory =
            std::env::temp_dir().join(format!("xd-codex-recorder-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let marker = directory.join("session");
        let override_value = super::codex_notify_override();
        let command: Vec<String> =
            serde_json::from_str(override_value.strip_prefix("notify=").unwrap()).unwrap();
        let status = Command::new(&command[0])
            .args(&command[1..])
            .env("XD_AGENT_SESSION_MARKER", &marker)
            .arg(r#"{"type":"agent-turn-complete","thread-id":"thread-exact-1"}"#)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(fs::read_to_string(marker).unwrap(), "thread-exact-1\n");
    }

    #[test]
    fn codex_resume_reads_the_recorded_thread_instead_of_using_last() {
        let directory =
            std::env::temp_dir().join(format!("xd-codex-resume-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let marker = directory.join("session");
        fs::write(&marker, "thread-exact-2\n").unwrap();
        let log = directory.join("arguments");
        let executable = directory.join("codex");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n",
                shell_quote(&log.to_string_lossy())
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();

        let command = AgentCommand::new(executable.to_string_lossy(), ["--no-alt-screen"])
            .resume_with(["resume", "--last", "--no-alt-screen"])
            .record_codex_session()
            .shell_command_with_marker(Some(&shell_quote(&marker.to_string_lossy())));
        assert!(
            Command::new("/bin/sh")
                .args(["-c", &command])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            fs::read_to_string(log).unwrap(),
            "resume\nthread-exact-2\n--no-alt-screen\n"
        );
    }

    #[test]
    fn managed_tmux_configuration_is_accepted_by_tmux() {
        assert!(TMUX_CONFIGURATION.contains("set -g allow-passthrough on"));
        let directory =
            std::env::temp_dir().join(format!("xd-tmux-configuration-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let configuration = directory.join("tmux.conf");
        fs::write(&configuration, TMUX_CONFIGURATION).unwrap();
        let socket = format!("xd-configuration-test-{}", std::process::id());

        let output = Command::new("tmux")
            .args(["-L", &socket, "-f"])
            .arg(&configuration)
            .args(["start-server", ";", "source-file"])
            .arg(&configuration)
            .args([";", "new-session", "-d", "-s", "check", "true"])
            .output()
            .unwrap();
        let _ = Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .output();

        assert!(
            output.status.success(),
            "tmux rejected the managed configuration: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn pasted_ssh_commands_are_parsed_without_a_shell_and_options_may_follow_the_host() {
        let command = SshCommand::parse(
            r#"ssh zenomc.org -p 22 -i "keys/zeno mc" -o ServerAliveInterval=15"#,
        )
        .unwrap();

        assert_eq!(command.program(), Path::new("ssh"));
        assert_eq!(
            command.connection_arguments(),
            [
                "-p",
                "22",
                "-i",
                "keys/zeno mc",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPersist=10m",
                "-o",
                "ControlPath=~/.ssh/xd-%C",
                "--",
                "zenomc.org",
            ]
        );
    }

    #[test]
    fn ssh_connections_are_multiplexed_across_host_and_terminal_processes() {
        let command = SshCommand::parse("ssh -p 2222 user@example.com").unwrap();
        let arguments = command.connection_arguments();

        assert!(
            arguments
                .windows(2)
                .any(|pair| { pair == ["-o", "ControlMaster=auto"] })
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| { pair == ["-o", "ControlPersist=10m"] })
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| { pair == ["-o", "ControlPath=~/.ssh/xd-%C"] })
        );
    }

    #[test]
    fn explicit_ssh_multiplexing_options_are_not_overridden() {
        let command = SshCommand::parse(
            "ssh -o ControlMaster=no -oControlPersist=1m -S~/.ssh/custom-%C host",
        )
        .unwrap();
        let arguments = command.connection_arguments();

        assert_eq!(
            arguments
                .iter()
                .filter(|argument| argument.contains("ControlMaster="))
                .count(),
            1
        );
        assert_eq!(
            arguments
                .iter()
                .filter(|argument| argument.contains("ControlPersist="))
                .count(),
            1
        );
        assert!(!arguments.iter().any(|argument| argument.contains("xd-%C")));
    }

    #[test]
    fn remote_terminal_attach_and_cleanup_use_persisted_ssh_connection_options() {
        let host = SessionHost::ssh(
            SshCommand::parse("ssh user@example.com").unwrap(),
            ".local/share/xd/sessions",
        );
        let attach = host.attach(
            "terminal",
            Path::new("/workspace"),
            &AgentCommand::user_shell(),
        );
        let cleanup = host.kill_process("terminal");

        for spec in [attach, cleanup] {
            assert!(
                spec.arguments
                    .windows(2)
                    .any(|pair| pair == ["-o", "ControlMaster=auto"])
            );
            assert!(
                spec.arguments
                    .windows(2)
                    .any(|pair| pair == ["-o", "ControlPersist=10m"])
            );
            assert!(
                spec.arguments
                    .windows(2)
                    .any(|pair| pair == ["-o", "ControlPath=~/.ssh/xd-%C"])
            );
        }
    }

    #[test]
    fn ssh_setup_rejects_non_ssh_programs_and_remote_commands() {
        assert!(SshCommand::parse("bash -c whoami").is_err());
        assert!(SshCommand::parse("ssh host.example whoami").is_err());
        assert!(SshCommand::parse("ssh").is_err());
        assert!(SshCommand::parse("ssh -W other:22 host.example").is_err());
        assert!(SshCommand::parse("ssh -L 4001:localhost:4001 host.example").is_err());
    }

    #[test]
    fn local_sessions_attach_through_the_private_bundled_tmux_runtime() {
        let host = SessionHost::local(
            PathBuf::from("/app/libexec/tmux"),
            PathBuf::from("/data/xd/runtime"),
        );
        let spec = host.attach(
            "chat id/with punctuation",
            Path::new("/work/tree with spaces"),
            &AgentCommand::new("codex", ["resume", "agent's-session"]),
        );

        assert_eq!(
            spec,
            ProcessSpec::new(
                "/app/libexec/tmux",
                [
                    "-S",
                    "/data/xd/runtime/tmux.sock",
                    "-f",
                    "/data/xd/runtime/tmux.conf",
                    "start-server",
                    ";",
                    "source-file",
                    "/data/xd/runtime/tmux.conf",
                    ";",
                    "new-session",
                    "-A",
                    "-s",
                    "xd-d77cc6beab2f469d",
                    "-c",
                    "/work/tree with spaces",
                    "exec 'codex' 'resume' 'agent'\"'\"'s-session'",
                ],
            )
        );
    }

    #[test]
    fn interactive_terminals_use_the_configured_user_shell() {
        assert_eq!(
            AgentCommand::user_shell().shell_command(),
            "exec \"${SHELL:-/bin/sh}\" -i"
        );
    }

    #[test]
    fn interrupting_an_agent_returns_to_a_live_shell() {
        let socket = format!("xd-agent-interrupt-test-{}", std::process::id());
        let command = AgentCommand::new("sleep", ["30"])
            .discover_in_user_shell("test agent")
            .shell_command();
        let started = Command::new("tmux")
            .args(["-L", &socket, "new-session", "-d", "-s", "check", &command])
            .status()
            .unwrap();
        assert!(started.success());

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let output = Command::new("tmux")
                .args([
                    "-L",
                    &socket,
                    "display-message",
                    "-pt",
                    "check:0.0",
                    "#{pane_current_command}",
                ])
                .output()
                .unwrap();
            if output.stdout == b"sleep\n" || Instant::now() >= deadline {
                assert_eq!(output.stdout, b"sleep\n", "agent did not start");
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        let interrupted = Command::new("tmux")
            .args(["-L", &socket, "send-keys", "-t", "check:0.0", "C-c"])
            .status()
            .unwrap();
        assert!(interrupted.success());
        thread::sleep(Duration::from_millis(100));
        let alive = Command::new("tmux")
            .args(["-L", &socket, "has-session", "-t", "check"])
            .status()
            .unwrap();
        let _ = Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .output();

        assert!(
            alive.success(),
            "Ctrl+C closed the persistent terminal pane"
        );
    }

    #[test]
    fn closing_a_terminal_kills_its_persistent_tmux_session() {
        let directory = std::env::temp_dir().join(format!(
            "xd-terminal-close-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("tmux.conf"), TMUX_CONFIGURATION).unwrap();
        let host = SessionHost::local(PathBuf::from("tmux"), directory.clone());
        let terminal_id = "chat:claude:close-test";
        let spec =
            detached(host.attach(terminal_id, &directory, &AgentCommand::new("sleep", ["30"])));
        assert!(
            Command::new(&spec.program)
                .args(&spec.arguments)
                .status()
                .unwrap()
                .success()
        );
        let session = super::session_name(terminal_id);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !Command::new("tmux")
            .args(["-S"])
            .arg(directory.join("tmux.sock"))
            .args(["has-session", "-t", &session])
            .status()
            .is_ok_and(|status| status.success())
        {
            assert!(Instant::now() < deadline, "tmux session did not start");
            thread::sleep(Duration::from_millis(10));
        }

        let cleanup = host.kill_process(terminal_id);
        assert!(
            Command::new(cleanup.program)
                .args(cleanup.arguments)
                .status()
                .unwrap()
                .success()
        );

        assert!(
            !Command::new("tmux")
                .args(["-S"])
                .arg(directory.join("tmux.sock"))
                .args(["has-session", "-t", &session])
                .status()
                .is_ok_and(|status| status.success()),
            "closing the tab left an orphaned tmux session"
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn recreating_a_dead_agent_terminal_resumes_its_backend_session() {
        let directory = std::env::temp_dir().join(format!(
            "xd-terminal-resume-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("tmux.conf"), TMUX_CONFIGURATION).unwrap();
        let log = directory.join("launches");
        let agent = directory.join("agent");
        fs::write(
            &agent,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n",
                super::shell_quote(&log.to_string_lossy())
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&agent).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o700);
        }
        fs::set_permissions(&agent, permissions).unwrap();
        let host = SessionHost::local(PathBuf::from("tmux"), directory.clone());
        let command =
            AgentCommand::new(agent.to_string_lossy(), ["new"]).resume_with(["resume", "--last"]);

        for launch in 1..=2 {
            let spec = detached(host.attach("chat:codex", &directory, &command));
            let status = Command::new(&spec.program)
                .args(&spec.arguments)
                .status()
                .unwrap();
            assert!(status.success());
            wait_for_lines(&log, launch);
            wait_for_session_to_close(&directory, "chat:codex");
        }

        assert_eq!(fs::read_to_string(&log).unwrap(), "new\nresume --last\n");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn reattaching_a_legacy_terminal_marks_it_for_future_resume() {
        let directory = std::env::temp_dir().join(format!(
            "xd-terminal-legacy-resume-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("tmux.conf"), TMUX_CONFIGURATION).unwrap();
        let host = SessionHost::local(PathBuf::from("tmux"), directory.clone());
        let terminal_id = "chat:codex:legacy";
        let legacy =
            detached(host.attach(terminal_id, &directory, &AgentCommand::new("sleep", ["30"])));
        assert!(
            Command::new(&legacy.program)
                .args(&legacy.arguments)
                .status()
                .unwrap()
                .success()
        );
        thread::sleep(Duration::from_millis(100));

        let log = directory.join("launches");
        let agent = directory.join("agent");
        fs::write(
            &agent,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n",
                super::shell_quote(&log.to_string_lossy())
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&agent).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o700);
        }
        fs::set_permissions(&agent, permissions).unwrap();
        let resumable =
            AgentCommand::new(agent.to_string_lossy(), ["new"]).resume_with(["resume", "--last"]);

        let _reattach = host.attach(terminal_id, &directory, &resumable);
        let cleanup = host.kill_process(terminal_id);
        assert!(
            Command::new(cleanup.program)
                .args(cleanup.arguments)
                .status()
                .unwrap()
                .success()
        );

        let recreated = detached(host.attach(terminal_id, &directory, &resumable));
        assert!(
            Command::new(&recreated.program)
                .args(&recreated.arguments)
                .status()
                .unwrap()
                .success()
        );
        wait_for_lines(&log, 1);
        assert_eq!(fs::read_to_string(&log).unwrap(), "resume --last\n");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn external_agents_are_discovered_in_the_users_interactive_shell() {
        let command = AgentCommand::new("codex", ["resume", "session-1"])
            .discover_in_user_shell("Codex")
            .shell_command();

        assert!(command.starts_with("exec \"${SHELL:-/bin/sh}\" -ic "));
        assert!(command.contains("trap '\"'\"''\"'\"' INT"), "{command}");
        assert!(command.contains("trap - INT; exec"), "{command}");
        assert!(
            command.contains("command -v '\"'\"'codex'\"'\"'"),
            "{command}"
        );
        assert!(
            command.contains("'\"'\"'codex'\"'\"' '\"'\"'resume'\"'\"' '\"'\"'session-1'\"'\"'"),
            "{command}"
        );
        assert!(
            !command.contains("then exec '\"'\"'codex'\"'\"'"),
            "the discovery shell must survive when Ctrl+C exits the agent: {command}"
        );
        assert!(command.contains("Codex is not installed or is not available on PATH"));
        assert!(command.contains("exec \"${SHELL:-/bin/sh}\" -i"));
    }

    #[test]
    fn external_agent_environment_is_removed_after_login_shell_setup() {
        let command = AgentCommand::new("claude", ["--dangerously-skip-permissions"])
            .unset_environment("TMUX")
            .discover_in_user_shell("Claude Code")
            .shell_command();

        assert!(command.contains(
            "env '\"'\"'-u'\"'\"' '\"'\"'TMUX'\"'\"' '\"'\"'claude'\"'\"' '\"'\"'--dangerously-skip-permissions'\"'\"'"
        ), "{command}");
    }

    #[test]
    fn remote_sessions_use_ssh_and_the_tmux_shipped_with_the_remote_install() {
        let host = SessionHost::ssh(
            SshCommand::parse("ssh zenomc.org -p 22").unwrap(),
            ".local/share/xd/runtime/v1",
        );
        let spec = host.attach(
            "chat-1",
            Path::new("/srv/project"),
            &AgentCommand::new("claude", ["--resume", "session-1"]),
        );

        assert_eq!(spec.program, PathBuf::from("ssh"));
        assert_eq!(&spec.arguments[..2], ["-p", "22"]);
        for option in [
            "ControlMaster=auto",
            "ControlPersist=10m",
            "ControlPath=~/.ssh/xd-%C",
        ] {
            assert!(
                spec.arguments.windows(2).any(|pair| pair == ["-o", option]),
                "missing {option}: {:?}",
                spec.arguments
            );
        }
        assert!(
            spec.arguments
                .windows(3)
                .any(|args| args == ["-tt", "--", "zenomc.org"])
        );
        let remote = spec.arguments.last().unwrap();
        assert!(remote.contains("mkdir -p \"$RUNTIME/agent-sessions\""));
        assert!(
            !remote.contains("[ -f \"$CONF\" ]"),
            "the managed tmux configuration must be refreshed on every attach: {remote}"
        );
        assert!(remote.contains("'set -g status off'"), "{remote}");
        assert!(remote.contains("'set -g mouse on'"), "{remote}");
        assert!(remote.contains("'set -g set-titles on'"), "{remote}");
        assert!(
            remote.contains("'set -g set-titles-string \"#{pane_title}\"'"),
            "{remote}"
        );
        assert!(remote.contains(&format!(
            "TMUX=\"$HOME/.local/opt/{}/libexec/tmux\"",
            crate::channel::data_name().to_string_lossy()
        )));
        assert!(remote.contains("-S \"$RUNTIME/tmux.sock\" -f \"$CONF\""));
        assert!(remote.contains("new-session -A -s 'xd-f2c795031a96bd79'"));
    }
}
