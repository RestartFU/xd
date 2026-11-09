//! Pull requests linked to a chat: what counts as one, and spotting the ones an
//! agent opens from its shell.

use std::{collections::VecDeque, sync::Mutex};

/// How many pull requests one chat keeps. Linking more is refused.
pub(crate) const MAX_PER_CHAT: usize = 32;
/// How many addresses one request may name.
pub(crate) const MAX_PER_REQUEST: usize = 16;
const MAX_URL_BYTES: usize = 2_048;

/// Where a link came from. Automatic links never bring back one the user
/// removed; a link the user or the agent asks for does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkSource {
    /// The user, from a client.
    User,
    /// The agent, through its link tool.
    Agent,
    /// The pull request of the chat's branch, or one xd's own Create PR opened.
    Branch,
    /// A `gh pr create` the agent ran.
    Command,
}

impl LinkSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::Branch => "branch",
            Self::Command => "command",
        }
    }

    /// Whether this link brings back one the user removed.
    pub(crate) fn restores(self) -> bool {
        matches!(self, Self::User | Self::Agent)
    }
}

/// The canonical address of a pull request, `https://host/owner/repo/pull/N`,
/// or `None` when `text` is not one. The host, owner and repository are
/// lowercased and anything after the number, such as `/files` or a fragment,
/// is dropped, so one pull request has one address.
pub(crate) fn normalize_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() > MAX_URL_BYTES {
        return None;
    }
    let rest = text.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let host = host.to_ascii_lowercase();
    if host.is_empty()
        || host.len() > 253
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c))
    {
        return None;
    }
    let mut parts = path.splitn(4, '/');
    let owner = parts.next()?;
    let repository = parts.next()?;
    if parts.next()? != "pull" {
        return None;
    }
    let tail = parts.next()?;
    let digits = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let (number, after) = tail.split_at(digits);
    let name = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    };
    if !name(owner)
        || !name(repository)
        || number.is_empty()
        || number.len() > 10
        || number.starts_with('0')
        || !(after.is_empty() || after.starts_with(['/', '?', '#']))
    {
        return None;
    }
    // GitHub matches owners and repositories in any case, so one pull request
    // keeps one address whichever case it was named in.
    Some(format!(
        "https://{host}/{}/{}/pull/{number}",
        owner.to_ascii_lowercase(),
        repository.to_ascii_lowercase()
    ))
}

/// The pull requests a finished shell command opened, read from its output.
///
/// Only a command that runs `gh pr create` counts, and only output lines that
/// are nothing but an address, which is how `gh` reports the new pull request.
/// A command that also lists or views pull requests could print others, so it
/// counts for nothing.
pub(crate) fn created_by(command: &str, output: &str) -> Vec<String> {
    if !creates(command) {
        return Vec::new();
    }
    let mut urls = Vec::new();
    for line in output.lines() {
        if let Some(url) = normalize_url(line)
            && !urls.contains(&url)
        {
            urls.push(url);
            if urls.len() == MAX_PER_REQUEST {
                break;
            }
        }
    }
    urls
}

/// Whether a shell command runs `gh pr create` and nothing that prints the
/// addresses of other pull requests, so the addresses it prints are new ones.
pub(crate) fn creates(command: &str) -> bool {
    let commands = shell_commands(command);
    let commands = commands
        .iter()
        .map(|words| without_gh_flags(words))
        .collect::<Vec<_>>();
    let runs = |words: &[&str]| {
        commands
            .iter()
            .any(|command| command.len() >= words.len() && command[..words.len()] == *words)
    };
    (runs(&["gh", "pr", "create"]) || runs(&["gh", "pr", "new"]))
        && !["list", "view", "status", "checkout"]
            .iter()
            .any(|verb| runs(&["gh", "pr", verb]))
        && !runs(&["gh", "search"])
        && !runs(&["gh", "api"])
}

/// A `gh` command's words without its flags, so `gh -R o/r pr create` reads
/// as `gh pr create`. Other commands are left as they are.
fn without_gh_flags(words: &[String]) -> Vec<&str> {
    let mut words = words.iter().map(String::as_str);
    if words.clone().next() != Some("gh") {
        return words.collect();
    }
    let mut kept = Vec::new();
    while let Some(word) = words.next() {
        if matches!(word, "-R" | "--repo") {
            words.next();
        } else if !word.starts_with('-') {
            kept.push(word);
        }
    }
    kept
}

/// Commands Claude ran in the background that open a pull request, by tool
/// use id, until their task finishes. A task can finish turns later, when
/// another parser reads Claude's output, so they are kept here, oldest
/// dropped first.
static BACKGROUND: Mutex<VecDeque<(String, String)>> = Mutex::new(VecDeque::new());
const MAX_BACKGROUND: usize = 64;

/// Remembers a background command that opens a pull request.
pub(crate) fn started_in_background(id: &str, command: &str) {
    if let Ok(mut waiting) = BACKGROUND.lock() {
        if waiting.len() >= MAX_BACKGROUND {
            waiting.pop_front();
        }
        waiting.push_back((id.to_owned(), command.to_owned()));
    }
}

/// The pull requests a background command opened, read from its output file
/// once its task has finished. Nothing for any other task.
pub(crate) fn finished_in_background(id: &str, output_file: Option<&str>) -> Vec<String> {
    let command = BACKGROUND.lock().ok().and_then(|mut waiting| {
        let index = waiting.iter().position(|(waiting, _)| waiting == id)?;
        waiting.remove(index).map(|(_, command)| command)
    });
    let Some(command) = command else {
        return Vec::new();
    };
    output_file
        .and_then(crate::delegation::read_output)
        .map(|output| created_by(&command, &output))
        .unwrap_or_default()
}

/// The words of each simple command in a shell script, past variable
/// assignments and wrappers such as `env` or `timeout 60`. The shell's
/// quoting decides what is a command: `echo "a; gh pr create"` runs `echo`
/// alone, and a heredoc's body runs nothing.
fn shell_commands(script: &str) -> Vec<Vec<String>> {
    crate::delegation::simple_commands(script)
        .into_iter()
        .filter_map(|words| {
            let mut words = words.into_iter().peekable();
            loop {
                let word = words.peek()?;
                let assignment = word.split_once('=').is_some_and(|(name, _)| {
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                });
                if assignment
                    || matches!(
                        word.as_str(),
                        "env"
                            | "time"
                            | "nohup"
                            | "command"
                            | "exec"
                            | "do"
                            | "then"
                            | "else"
                            | "!"
                    )
                {
                    words.next();
                } else if word == "timeout" {
                    words.next();
                    words.next();
                } else {
                    break;
                }
            }
            let words = words.collect::<Vec<_>>();
            (!words.is_empty()).then_some(words)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalized_to_one_per_pull_request() {
        for (text, expected) in [
            (
                "https://github.com/restartfu/xd/pull/180",
                Some("https://github.com/restartfu/xd/pull/180"),
            ),
            (
                " https://GitHub.com/RestartFU/XD/pull/180/files?w=1 ",
                Some("https://github.com/restartfu/xd/pull/180"),
            ),
            (
                "https://github.example:8443/o.r/r-e_p/pull/9#discussion",
                Some("https://github.example:8443/o.r/r-e_p/pull/9"),
            ),
            ("http://github.com/o/r/pull/1", None),
            ("https://github.com/o/r/issues/1", None),
            ("https://github.com/o/r/pull/", None),
            ("https://github.com/o/r/pull/012", None),
            ("https://github.com/o/r/pull/12abc", None),
            ("https://github.com/../r/pull/12", None),
            ("https://github.com/o/r/pull/1 and more", None),
            ("https://git hub.com/o/r/pull/1", None),
        ] {
            assert_eq!(normalize_url(text).as_deref(), expected, "{text}");
        }
    }

    #[test]
    fn only_a_create_command_links_the_address_it_prints() {
        let output =
            "Creating pull request for feat into master\n\nhttps://github.com/o/r/pull/7\n";
        let created = vec!["https://github.com/o/r/pull/7".to_owned()];
        for command in [
            "gh pr create --fill",
            "git push -u origin HEAD && gh pr create --title t --body-file /tmp/b",
            "cd repo; GH_PROMPT_DISABLED=1 gh pr create -f",
            "env FOO=1 timeout 60 gh pr create -f",
            "if true; then gh pr new -f; fi",
        ] {
            assert_eq!(created_by(command, output), created, "{command}");
        }
        for command in [
            "echo 'gh pr create'",
            "gh pr list",
            "gh pr create -f && gh pr view --json url -q .url",
            "printf %s \"gh pr create\"",
            "git log",
        ] {
            assert!(created_by(command, output).is_empty(), "{command}");
        }
    }

    #[test]
    fn flags_before_the_subcommand_do_not_hide_a_create() {
        let output = "https://github.com/o/other/pull/2\n";
        for command in [
            "gh --repo o/other pr create --fill",
            "gh -R o/other pr create --fill",
            "gh pr -R o/other create --fill",
            "gh -R \"o/other\" pr create --title \"Fix it\"",
            "gh --repo='o/other' pr create --fill",
        ] {
            assert_eq!(created_by(command, output).len(), 1, "{command}");
        }
        assert!(created_by("gh -R o/other pr list", output).is_empty());
        let heredoc = "gh pr create --title \"Docs\" --body-file - <<'EOF'\nInspect it with:\ngh pr view\nEOF";
        assert_eq!(
            created_by(heredoc, output).len(),
            1,
            "a heredoc body runs nothing"
        );
    }

    #[test]
    fn a_background_create_is_read_from_its_output_once_it_finishes() {
        let output = std::env::temp_dir().join(format!("xd-pr-{}.output", uuid::Uuid::new_v4()));
        std::fs::write(&output, "https://github.com/o/r/pull/4\n").unwrap();
        let output_file = output.to_str();
        assert!(finished_in_background("toolu_unknown", output_file).is_empty());
        started_in_background("toolu_bg", "gh pr create --fill");
        assert_eq!(
            finished_in_background("toolu_bg", output_file),
            ["https://github.com/o/r/pull/4"]
        );
        assert!(
            finished_in_background("toolu_bg", output_file).is_empty(),
            "read once"
        );
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn addresses_inside_other_text_are_not_taken() {
        assert!(
            created_by(
                "gh pr create -f",
                "a pull request for branch \"x\" already exists:\nsee https://github.com/o/r/pull/3 here"
            )
            .is_empty()
        );
        assert_eq!(
            created_by(
                "gh pr create -f",
                "https://github.com/o/r/pull/3\nhttps://github.com/o/r/pull/3\n"
            ),
            vec!["https://github.com/o/r/pull/3".to_owned()]
        );
    }
}
