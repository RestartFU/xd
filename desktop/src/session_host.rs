use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

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
    unset_environment: Vec<String>,
    login_shell_label: Option<String>,
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
            unset_environment: Vec::new(),
            login_shell_label: None,
        }
    }

    pub fn unset_environment(mut self, name: impl Into<String>) -> Self {
        self.unset_environment.push(name.into());
        self
    }

    pub fn discover_in_user_shell(mut self, label: impl Into<String>) -> Self {
        self.login_shell_label = Some(label.into());
        self
    }

    fn shell_command(&self) -> String {
        let command = self
            .unset_environment
            .iter()
            .flat_map(|name| ["-u", name.as_str()])
            .chain(std::iter::once(self.program.as_str()))
            .chain(self.arguments.iter().map(String::as_str))
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" ");
        let command = if self.unset_environment.is_empty() {
            command
        } else {
            format!("env {command}")
        };
        let Some(label) = &self.login_shell_label else {
            return format!("exec {command}");
        };
        let missing = format!(
            "xd: {label} is not installed or is not available on PATH. Install it, then start a new tab."
        );
        let inner = format!(
            "if command -v {} >/dev/null 2>&1; then exec {command}; else printf '%s\\n' {} >&2; exec \"${{SHELL:-/bin/sh}}\" -l; fi",
            shell_quote(&self.program),
            shell_quote(&missing),
        );
        format!("exec \"${{SHELL:-/bin/sh}}\" -lic {}", shell_quote(&inner))
    }
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
        self.options
            .iter()
            .cloned()
            .chain(["--".to_owned(), self.destination.clone()])
            .collect()
    }

    pub fn options(&self) -> &[String] {
        &self.options
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }
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
            HostTarget::Local { tmux, runtime } => ProcessSpec::new(
                tmux,
                [
                    "-S".to_owned(),
                    runtime.join("tmux.sock").to_string_lossy().into_owned(),
                    "-f".to_owned(),
                    runtime.join("tmux.conf").to_string_lossy().into_owned(),
                    "new-session".to_owned(),
                    "-A".to_owned(),
                    "-s".to_owned(),
                    session,
                    "-c".to_owned(),
                    workdir.to_string_lossy().into_owned(),
                    agent.shell_command(),
                ],
            ),
            HostTarget::Ssh {
                command,
                remote_runtime,
            } => {
                let runtime = format!("$HOME/{remote_runtime}");
                let data_name = crate::channel::data_name();
                let remote = format!(
                    "RUNTIME=\"{runtime}\"; mkdir -p \"$RUNTIME\" || exit; CONF=\"$RUNTIME/tmux.conf\"; [ -f \"$CONF\" ] || printf '%s\\n' 'set -g default-terminal screen-256color' 'set -sg escape-time 0' 'set -g focus-events on' 'set -g mouse off' > \"$CONF\"; TMUX=\"$HOME/.local/opt/{}/libexec/tmux\"; [ -x \"$TMUX\" ] || TMUX=tmux; exec \"$TMUX\" -S \"$RUNTIME/tmux.sock\" -f \"$CONF\" new-session -A -s {} -c {} {}",
                    data_name.to_string_lossy(),
                    shell_quote(&session),
                    shell_quote(&workdir.to_string_lossy()),
                    shell_quote(&agent.shell_command()),
                );
                let mut arguments = command.options.clone();
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
    use std::path::{Path, PathBuf};

    use super::{AgentCommand, ProcessSpec, SessionHost, SshCommand};

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
                "--",
                "zenomc.org",
            ]
        );
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
    fn external_agents_are_discovered_in_the_users_login_shell() {
        let command = AgentCommand::new("codex", ["resume", "session-1"])
            .discover_in_user_shell("Codex")
            .shell_command();

        assert!(command.starts_with("exec \"${SHELL:-/bin/sh}\" -lic "));
        assert!(
            command.contains("command -v '\"'\"'codex'\"'\"'"),
            "{command}"
        );
        assert!(
            command
                .contains("exec '\"'\"'codex'\"'\"' '\"'\"'resume'\"'\"' '\"'\"'session-1'\"'\"'"),
            "{command}"
        );
        assert!(command.contains("Codex is not installed or is not available on PATH"));
        assert!(command.contains("exec \"${SHELL:-/bin/sh}\" -l"));
    }

    #[test]
    fn external_agent_environment_is_removed_after_login_shell_setup() {
        let command = AgentCommand::new("claude", ["--dangerously-skip-permissions"])
            .unset_environment("TMUX")
            .discover_in_user_shell("Claude Code")
            .shell_command();

        assert!(command.contains(
            "exec env '\"'\"'-u'\"'\"' '\"'\"'TMUX'\"'\"' '\"'\"'claude'\"'\"' '\"'\"'--dangerously-skip-permissions'\"'\"'"
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
        assert_eq!(
            &spec.arguments[..5],
            ["-p", "22", "-tt", "--", "zenomc.org"]
        );
        let remote = &spec.arguments[5];
        assert!(remote.contains("mkdir -p \"$RUNTIME\""));
        assert!(remote.contains(&format!(
            "TMUX=\"$HOME/.local/opt/{}/libexec/tmux\"",
            crate::channel::data_name().to_string_lossy()
        )));
        assert!(remote.contains("-S \"$RUNTIME/tmux.sock\" -f \"$CONF\""));
        assert!(remote.contains("new-session -A -s 'xd-f2c795031a96bd79'"));
    }
}
