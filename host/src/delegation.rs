//! Assistants started from a shell.
//!
//! An agent can delegate without its own subagent tool by running another
//! assistant headlessly: Claude runs `codex exec`, or the codex-opussy skill's
//! `codex-agent` script, and Codex runs `claude -p`. Each of those is a
//! delegated agent rather than a command, so the parsers turn them into
//! subagent records instead of shell rows. Recognising one only needs the
//! command line, so a parser can tell when the call starts and again when it
//! finishes without remembering anything in between.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use crate::subagent::{SubagentPatch, SubagentStatus};

/// An assistant a shell command starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegation {
    pub provider: &'static str,
    pub agent: String,
    pub model: Option<String>,
    pub prompt: Option<String>,
}

impl Delegation {
    /// The record for the call as it starts. `launcher` is the assistant whose
    /// shell runs it; `description` is what that assistant said the call is for.
    pub fn started(&self, key: &str, launcher: &str, description: Option<&str>) -> SubagentPatch {
        let description = description
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                self.prompt
                    .as_deref()
                    .and_then(|prompt| prompt.lines().map(str::trim).find(|line| !line.is_empty()))
                    .map(str::to_owned)
            });
        SubagentPatch {
            provider: Some(self.provider.into()),
            agent: Some(self.agent.clone()),
            launcher: Some(launcher.into()),
            model: self.model.clone(),
            description,
            prompt: self.prompt.clone(),
            status: Some(SubagentStatus::Running),
            ..SubagentPatch::new(key)
        }
    }
}

/// The assistant `command` starts, if it starts one.
pub fn detect(command: &str) -> Option<Delegation> {
    detect_within(command, 0)
}

fn detect_within(script: &str, depth: usize) -> Option<Delegation> {
    if depth > 2 {
        return None;
    }
    segments(script)
        .iter()
        .filter(|segment| !segment.defined)
        .find_map(|segment| {
            classify(segment, depth).or_else(|| {
                segment
                    .substitutions
                    .iter()
                    .find_map(|inner| detect_within(inner, depth + 1))
            })
        })
}

/// The commands an expanding heredoc body runs through `$(…)` or backticks.
fn expansions(text: &str) -> Vec<String> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut found = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 2,
            '$' if chars.get(index + 1) == Some(&'(') => {
                let end = substitution_end(&chars, index + 2);
                found.push(chars[index + 2..end].iter().collect());
                index = end + 1;
            }
            '`' => {
                let start = index + 1;
                let mut end = start;
                while end < chars.len() && chars[end] != '`' {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                let end = end.min(chars.len());
                found.push(chars[start..end].iter().collect());
                index = end + 1;
            }
            _ => index += 1,
        }
    }
    found
}

/// Where the `$(…)` whose body starts at `start` closes. The body is a
/// script of its own, so its quotes and nested substitutions are its own too.
fn substitution_end(chars: &[char], start: usize) -> usize {
    let mut depth = 1;
    let mut index = start;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 1,
            '\'' => {
                index += 1;
                while index < chars.len() && chars[index] != '\'' {
                    index += 1;
                }
            }
            '"' => {
                index += 1;
                while index < chars.len() && chars[index] != '"' {
                    if chars[index] == '\\' {
                        index += 1;
                    } else if chars[index] == '$' && chars.get(index + 1) == Some(&'(') {
                        index = substitution_end(chars, index + 2);
                    }
                    index += 1;
                }
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return index;
                }
            }
            _ => {}
        }
        index += 1;
    }
    chars.len()
}

/// What a delegated agent reported, read from its command's output.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub text: String,
    pub failed: bool,
    pub model: Option<String>,
}

/// The agent's report without the status footer `codex-agent` appends, and
/// what that footer says about how the run went.
pub fn report(output: &str) -> Report {
    let lines = output.trim_end().lines().collect::<Vec<_>>();
    let footer = lines
        .last()
        .and_then(|line| line.trim().strip_prefix("[codex-opussy] "));
    let Some(footer) = footer else {
        return Report {
            text: output.trim().to_owned(),
            ..Report::default()
        };
    };
    let mut body = &lines[..lines.len() - 1];
    if body.last().is_some_and(|line| line.trim() == "---") {
        body = &body[..body.len() - 1];
    }
    let status = footer.split(" · ").next().unwrap_or_default().trim();
    Report {
        text: body.join("\n").trim().to_owned(),
        failed: status != "ok",
        model: footer
            .split_whitespace()
            .find_map(|word| word.strip_prefix("model="))
            .filter(|model| !model.is_empty())
            .map(str::to_owned),
    }
}

/// The end of a backgrounded command's output file, where its report is.
/// Only a regular file is read: opening a pipe would wait for a writer.
pub fn read_output(path: &str) -> Option<String> {
    const LIMIT: u64 = 64 * 1024;
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(LIMIT);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(LIMIT).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // A read that starts mid-file starts mid-line, unless that line is all
    // there is.
    let text = match text.split_once('\n') {
        Some((_, rest)) if start > 0 && !rest.trim().is_empty() => rest,
        _ => &text,
    };
    Some(text.to_owned())
}

/// One simple command: its words, and what a heredoc or here-string feeds it.
#[derive(Debug, Default)]
struct Segment {
    words: Vec<String>,
    stdin: Option<String>,
    /// Part of a function's body, which defining the function does not run.
    defined: bool,
    /// Commands run by substitutions inside double quotes, which the words
    /// keep as text.
    substitutions: Vec<String>,
}

struct PendingHeredoc {
    delimiter: String,
    strip_tabs: bool,
    /// An unquoted delimiter lets the body run substitutions.
    expands: bool,
    segment: usize,
}

/// Splits a shell script into simple commands. Enough of the shell's quoting
/// to read a command line, not an interpreter: expansions stay as written.
// The last segment's flush resets state nothing reads afterwards.
#[allow(unused_assignments)]
fn segments(script: &str) -> Vec<Segment> {
    #[derive(PartialEq)]
    enum Redirect {
        None,
        File,
        Heredoc(bool),
        HereString,
    }
    let chars = script.chars().collect::<Vec<_>>();
    let mut done: Vec<Segment> = Vec::new();
    let mut current = Segment::default();
    let mut word = String::new();
    let mut in_word = false;
    // The word so far was quoted or escaped, so braces in it are text.
    let mut quoted_word = false;
    let mut redirect = Redirect::None;
    let mut heredocs: Vec<PendingHeredoc> = Vec::new();
    // `name()` or `function name` was just read; its body is the next `{`.
    let mut function_pending = false;
    let mut braces = 0usize;
    // The brace depth at which the function body being read ends.
    let mut function_body: Option<usize> = None;
    // The same for a body written as a subshell, `name() ( … )`.
    let mut parens = 0usize;
    let mut function_subshell: Option<usize> = None;
    let mut index = 0;

    macro_rules! finish_word {
        () => {
            if in_word {
                let finished = std::mem::take(&mut word);
                match std::mem::replace(&mut redirect, Redirect::None) {
                    Redirect::None if quoted_word => current.words.push(finished),
                    Redirect::None => {
                        if finished == "{" {
                            braces += 1;
                            if function_pending && function_body.is_none() {
                                function_body = Some(braces);
                            }
                            function_pending = false;
                        } else if finished == "}" {
                            if function_body == Some(braces) {
                                function_body = None;
                            }
                            braces = braces.saturating_sub(1);
                        } else if finished == "function" && current.words.is_empty() {
                            function_pending = true;
                        }
                        current.words.push(finished);
                    }
                    Redirect::File => {}
                    Redirect::Heredoc(strip_tabs) => heredocs.push(PendingHeredoc {
                        delimiter: finished,
                        strip_tabs,
                        expands: !quoted_word,
                        segment: done.len(),
                    }),
                    Redirect::HereString => current.stdin = Some(finished),
                }
                in_word = false;
                quoted_word = false;
            }
        };
    }
    macro_rules! finish_segment {
        () => {
            finish_word!();
            redirect = Redirect::None;
            let mut segment = std::mem::take(&mut current);
            segment.defined = function_body.is_some() || function_subshell.is_some();
            if !segment.words.is_empty() || segment.stdin.is_some() {
                done.push(segment);
            } else {
                // Nothing to attach a heredoc to: its body goes nowhere.
                for heredoc in heredocs.iter_mut().filter(|h| h.segment == done.len()) {
                    heredoc.segment = usize::MAX;
                }
            }
        };
    }

    while index < chars.len() {
        let character = chars[index];
        match character {
            '\'' => {
                in_word = true;
                quoted_word = true;
                index += 1;
                while index < chars.len() && chars[index] != '\'' {
                    word.push(chars[index]);
                    index += 1;
                }
                index += 1;
            }
            '"' => {
                in_word = true;
                quoted_word = true;
                index += 1;
                while index < chars.len() && chars[index] != '"' {
                    if chars[index] == '\\'
                        && matches!(chars.get(index + 1), Some('"' | '\\' | '$' | '`' | '\n'))
                    {
                        // An escaped `$(` or backtick is text.
                        index += 1;
                        if chars[index] != '\n' {
                            word.push(chars[index]);
                        }
                        index += 1;
                    } else if chars[index] == '$' && chars.get(index + 1) == Some(&'(') {
                        // A substitution runs a command, whose quotes are its own.
                        let end = substitution_end(&chars, index + 2);
                        current
                            .substitutions
                            .push(chars[index + 2..end].iter().collect());
                        word.extend(&chars[index..(end + 1).min(chars.len())]);
                        index = end + 1;
                    } else if chars[index] == '`' {
                        let start = index + 1;
                        let mut end = start;
                        while end < chars.len() && chars[end] != '`' {
                            end += if chars[end] == '\\' { 2 } else { 1 };
                        }
                        let end = end.min(chars.len());
                        current
                            .substitutions
                            .push(chars[start..end].iter().collect());
                        word.extend(&chars[index..(end + 1).min(chars.len())]);
                        index = end + 1;
                    } else {
                        word.push(chars[index]);
                        index += 1;
                    }
                }
                index += 1;
            }
            '\\' => {
                if let Some(&next) = chars.get(index + 1)
                    && next != '\n'
                {
                    word.push(next);
                    in_word = true;
                    quoted_word = true;
                }
                index += 2;
            }
            ' ' | '\t' | '\r' => {
                finish_word!();
                index += 1;
            }
            '#' if !in_word => {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
            }
            '\n' => {
                finish_segment!();
                index += 1;
                for heredoc in std::mem::take(&mut heredocs) {
                    let mut body = Vec::new();
                    while index < chars.len() {
                        let end = chars[index..]
                            .iter()
                            .position(|c| *c == '\n')
                            .map_or(chars.len(), |offset| index + offset);
                        let line = chars[index..end].iter().collect::<String>();
                        index = (end + 1).min(chars.len());
                        let line = if heredoc.strip_tabs {
                            line.trim_start_matches('\t').to_owned()
                        } else {
                            line
                        };
                        if line.trim_end_matches('\r') == heredoc.delimiter {
                            break;
                        }
                        body.push(line);
                    }
                    if let Some(segment) = done.get_mut(heredoc.segment) {
                        let body = body.join("\n");
                        if heredoc.expands {
                            segment.substitutions.extend(expansions(&body));
                        }
                        segment.stdin = Some(body);
                    }
                }
            }
            '&' if chars.get(index + 1) == Some(&'>') => {
                // `&>file` sends both streams to a file.
                finish_word!();
                index += 1;
            }
            // A substitution runs a command but stays part of the word it is in,
            // so the words around it are still this command's.
            '$' if chars.get(index + 1) == Some(&'(') => {
                let end = substitution_end(&chars, index + 2);
                current
                    .substitutions
                    .push(chars[index + 2..end].iter().collect());
                word.extend(&chars[index..(end + 1).min(chars.len())]);
                in_word = true;
                index = end + 1;
            }
            '`' => {
                let start = index + 1;
                let mut end = start;
                while end < chars.len() && chars[end] != '`' {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                let end = end.min(chars.len());
                current
                    .substitutions
                    .push(chars[start..end].iter().collect());
                word.extend(&chars[index..(end + 1).min(chars.len())]);
                in_word = true;
                index = end + 1;
            }
            '(' if chars[index + 1..].iter().find(|c| !matches!(c, ' ' | '\t')) == Some(&')') => {
                // `name()` defines a function rather than running anything.
                finish_segment!();
                function_pending = true;
                index += 1;
                while index < chars.len() && chars[index] != ')' {
                    index += 1;
                }
                index += 1;
            }
            '(' => {
                finish_segment!();
                parens += 1;
                if function_pending && function_body.is_none() && function_subshell.is_none() {
                    function_subshell = Some(parens);
                }
                function_pending = false;
                index += 1;
            }
            ')' => {
                finish_segment!();
                if function_subshell == Some(parens) {
                    function_subshell = None;
                }
                parens = parens.saturating_sub(1);
                index += 1;
            }
            ';' | '&' | '|' => {
                finish_segment!();
                index += 1;
                while index < chars.len() && matches!(chars[index], ';' | '&' | '|') {
                    index += 1;
                }
            }
            '<' | '>' => {
                // A descriptor written right before the operator is part of it.
                if in_word && !word.is_empty() && word.chars().all(|c| c.is_ascii_digit()) {
                    word.clear();
                    in_word = false;
                } else {
                    finish_word!();
                }
                if character == '<' && chars.get(index + 1) == Some(&'<') {
                    if chars.get(index + 2) == Some(&'<') {
                        index += 3;
                        redirect = Redirect::HereString;
                    } else {
                        index += 2;
                        let strip_tabs = chars.get(index) == Some(&'-');
                        if strip_tabs {
                            index += 1;
                        }
                        redirect = Redirect::Heredoc(strip_tabs);
                    }
                } else {
                    index += 1;
                    while index < chars.len() && matches!(chars[index], '>' | '&' | '|') {
                        index += 1;
                    }
                    redirect = Redirect::File;
                }
            }
            _ => {
                word.push(character);
                in_word = true;
                index += 1;
            }
        }
    }
    finish_segment!();
    done
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

fn classify(segment: &Segment, depth: usize) -> Option<Delegation> {
    let mut words = segment.words.as_slice();
    // Past keywords, variable assignments and commands that run another.
    loop {
        let first = words.first()?.as_str();
        if is_assignment(first)
            || matches!(
                first,
                "{" | "!" | "if" | "then" | "else" | "elif" | "do" | "while" | "until"
            )
        {
            words = &words[1..];
            continue;
        }
        match basename(first) {
            "nohup" | "exec" | "command" | "builtin" | "time" => words = &words[1..],
            "env" | "stdbuf" | "nice" | "setsid" => {
                words = &words[1..];
                while words
                    .first()
                    .is_some_and(|word| word.starts_with('-') || is_assignment(word))
                {
                    let takes_value = matches!(words[0].as_str(), "-u" | "-n" | "-C" | "-S");
                    words = words
                        .get(if takes_value { 2 } else { 1 }..)
                        .unwrap_or_default();
                }
            }
            "timeout" => {
                words = &words[1..];
                while words.first().is_some_and(|word| word.starts_with('-')) {
                    let takes_value =
                        matches!(words[0].as_str(), "-k" | "-s" | "--kill-after" | "--signal");
                    words = words
                        .get(if takes_value { 2 } else { 1 }..)
                        .unwrap_or_default();
                }
                // The duration.
                words = words.get(1..)?;
            }
            _ => break,
        }
    }
    let program = basename(&words[0]);
    let args = &words[1..];
    let stdin = segment.stdin.as_deref();
    match program {
        "bash" | "sh" | "zsh" | "dash" => {
            // Options come before the first operand: with `-c` that is the
            // script, otherwise a script file whose arguments follow.
            let mut command = false;
            let mut from_stdin = false;
            let mut index = 0;
            while let Some(arg) = args.get(index) {
                match arg.as_str() {
                    "--" => {
                        index += 1;
                        break;
                    }
                    // `-n` reads the script without running it.
                    "--noexec" => return None,
                    "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file" => {
                        if arg == "-o" && args.get(index + 1).is_some_and(|o| o == "noexec") {
                            return None;
                        }
                        index += 1;
                    }
                    flag if flag.starts_with("--") => {}
                    flag if flag.len() > 1 && (flag.starts_with('-') || flag.starts_with('+')) => {
                        if flag.starts_with('-') && flag.contains('n') {
                            return None;
                        }
                        command |= flag.starts_with('-') && flag.contains('c');
                        from_stdin |= flag.starts_with('-') && flag.contains('s');
                    }
                    _ => break,
                }
                index += 1;
            }
            if command {
                return detect_within(args.get(index)?, depth + 1);
            }
            // With no script file, or with `-s`, the shell runs what it reads,
            // as with `bash <<'EOF'`.
            if from_stdin || index >= args.len() {
                return detect_within(stdin?, depth + 1);
            }
            None
        }
        "codex-agent" => codex_agent(args, stdin),
        "codex" => codex(args, stdin),
        "claude" => claude(args, stdin),
        _ => None,
    }
}

fn prompt_text(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Splits `--flag=value` so both spellings read the same.
fn option(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
        _ => (arg, None),
    }
}

/// The codex-opussy skill's wrapper: `codex-agent [options] - <<'EOF'`.
fn codex_agent(args: &[String], stdin: Option<&str>) -> Option<Delegation> {
    let mut name = None;
    let mut model = None;
    let mut prompt = None;
    let mut index = 0;
    while index < args.len() {
        let (flag, inline) = option(&args[index]);
        let mut value = || {
            inline.map(str::to_owned).or_else(|| {
                index += 1;
                args.get(index).cloned()
            })
        };
        match flag {
            "-h" | "--help" | "--print-config" => return None,
            "-n" | "--name" => name = value(),
            "-m" | "--model" => model = value(),
            "-e" | "--effort" | "-s" | "--sandbox" | "-C" | "--cd" | "-t" | "--timeout"
            | "--add-dir" => {
                value();
            }
            "--" => {
                prompt = prompt_text(&args[index + 1..].join(" "));
                break;
            }
            "-" => prompt = stdin.and_then(prompt_text),
            other if other.starts_with('-') => {}
            _ => {
                prompt = prompt_text(&args[index..].join(" "));
                break;
            }
        }
        index += 1;
    }
    Some(Delegation {
        provider: "Codex",
        agent: name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Codex agent".into()),
        model,
        prompt: prompt.or_else(|| stdin.and_then(prompt_text)),
    })
}

/// Reads options and positional words, skipping the values of `valued` options.
/// Returns `None` when a flag asks for help or a version instead of a run.
fn scan<'a>(
    args: &'a [String],
    valued: &[&str],
    model_flags: &[&str],
) -> Option<(Vec<&'a str>, Option<String>, bool)> {
    let mut positional = Vec::new();
    let mut model = None;
    let mut dash = false;
    let mut index = 0;
    while index < args.len() {
        let (flag, inline) = option(&args[index]);
        if matches!(flag, "-h" | "--help" | "-V" | "--version") {
            return None;
        }
        if flag == "-" {
            dash = true;
        } else if flag == "--" {
            positional.extend(args[index + 1..].iter().map(String::as_str));
            break;
        } else if flag.starts_with('-') {
            if inline.is_none() && valued.contains(&flag) {
                index += 1;
                if model_flags.contains(&flag) {
                    model = args.get(index).cloned();
                }
            } else if model_flags.contains(&flag) {
                model = inline.map(str::to_owned);
            }
        } else {
            positional.push(args[index].as_str());
        }
        index += 1;
    }
    Some((positional, model, dash))
}

/// The prompt among a command's positional words. Some options take several
/// values, so the prompt is not always the first word: it is the longest.
fn longest(words: &[&str]) -> Option<String> {
    words
        .iter()
        .max_by_key(|word| word.len())
        .and_then(|word| prompt_text(word))
}

const CODEX_VALUED: &[&str] = &[
    "-c",
    "--config",
    "-m",
    "--model",
    "-p",
    "--profile",
    "-s",
    "--sandbox",
    "-C",
    "--cd",
    "-a",
    "--ask-for-approval",
    "--add-dir",
    "-i",
    "--image",
    "--enable",
    "--disable",
    "--local-provider",
    "-o",
    "--output-last-message",
    "--output-schema",
    "--color",
];

/// `codex exec`, Codex's headless run. Its interactive and admin commands are
/// not delegation.
fn codex(args: &[String], stdin: Option<&str>) -> Option<Delegation> {
    let (positional, model, dash) = scan(args, CODEX_VALUED, &["-m", "--model"])?;
    let (command, rest) = positional.split_first()?;
    if !matches!(*command, "exec" | "e") {
        return None;
    }
    // `codex exec [PROMPT]`, or `codex exec resume (SESSION_ID | --last) [PROMPT]`.
    let rest = match rest.split_first() {
        Some((&"resume", rest)) if args.iter().any(|arg| arg == "--last") => rest,
        Some((&"resume", rest)) => rest.get(1..).unwrap_or_default(),
        _ => rest,
    };
    let prompt = rest
        .first()
        .and_then(|prompt| prompt_text(prompt))
        .or_else(|| {
            (dash || rest.is_empty())
                .then(|| stdin.and_then(prompt_text))
                .flatten()
        });
    Some(Delegation {
        provider: "Codex",
        agent: "Codex agent".into(),
        model,
        prompt,
    })
}

const CLAUDE_VALUED: &[&str] = &[
    "--model",
    "--fallback-model",
    "--output-format",
    "--input-format",
    "--permission-mode",
    "--permission-prompt-tool",
    "--system-prompt",
    "--system-prompt-file",
    "--append-system-prompt",
    "--append-system-prompt-file",
    "--max-turns",
    "--max-budget-usd",
    "--mcp-config",
    "--settings",
    "--setting-sources",
    "--add-dir",
    "--plugin-dir",
    "--session-id",
    "-r",
    "--resume",
    "--agent",
    "--agents",
    "--allowedTools",
    "--allowed-tools",
    "--disallowedTools",
    "--disallowed-tools",
    "--tools",
    "--betas",
    "--effort",
    "--json-schema",
];

/// `claude -p`, Claude's headless run.
fn claude(args: &[String], stdin: Option<&str>) -> Option<Delegation> {
    // Claude's own options end at `--`; what follows belongs to someone else,
    // such as the server command `claude mcp add` stores.
    let own = args
        .iter()
        .position(|arg| arg == "--")
        .map_or(args, |end| &args[..end]);
    if !own
        .iter()
        .any(|arg| matches!(arg.as_str(), "-p" | "--print"))
    {
        return None;
    }
    let (positional, model, _) = scan(args, CLAUDE_VALUED, &["--model"])?;
    // Its admin commands are not a run, whatever flags they pass on. A word
    // after the print flag is the prompt, even when it reads like one.
    let print_at = own
        .iter()
        .position(|arg| matches!(arg.as_str(), "-p" | "--print"))
        .unwrap_or(0);
    let (before, _, _) = scan(&args[..print_at], CLAUDE_VALUED, &[])?;
    if before.first().is_some_and(|first| {
        matches!(
            *first,
            "mcp"
                | "config"
                | "plugin"
                | "plugins"
                | "auth"
                | "doctor"
                | "update"
                | "install"
                | "setup-token"
                | "migrate-installer"
                | "agents"
        )
    }) {
        return None;
    }
    let agent = args
        .iter()
        .position(|arg| arg == "--agent")
        .and_then(|index| args.get(index + 1))
        .cloned()
        .or_else(|| {
            args.iter()
                .find_map(|arg| arg.strip_prefix("--agent="))
                .map(str::to_owned)
        });
    Some(Delegation {
        provider: "Claude",
        agent: agent.unwrap_or_else(|| "Claude agent".into()),
        model,
        prompt: longest(&positional).or_else(|| stdin.and_then(prompt_text)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codex_opussy_wrapper_is_an_agent_with_its_heredoc_brief() {
        let found = detect(
            "~/.claude/skills/codex-opussy/bin/codex-agent -n auth-audit -s read-only -C /repo - <<'EOF'\n\
             Find every place that checks a token.\n\
             Report file:line for each.\n\
             EOF",
        )
        .unwrap();
        assert_eq!(found.provider, "Codex");
        assert_eq!(found.agent, "auth-audit");
        assert_eq!(
            found.prompt.as_deref(),
            Some("Find every place that checks a token.\nReport file:line for each.")
        );

        let patch = found.started("toolu_1", "Claude", None);
        assert_eq!(patch.launcher.as_deref(), Some("Claude"));
        assert_eq!(
            patch.description.as_deref(),
            Some("Find every place that checks a token.")
        );
        assert_eq!(patch.status, Some(SubagentStatus::Running));
    }

    #[test]
    fn headless_assistants_are_found_through_wrappers_and_shells() {
        let codex = detect(
            "cd /tmp/empty && timeout 600 codex exec --sandbox read-only -m gpt-6.1-sol --skip-git-repo-check - < prompt.txt",
        )
        .unwrap();
        assert_eq!(codex.provider, "Codex");
        assert_eq!(codex.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(codex.prompt, None);

        let claude =
            detect(r#"/bin/bash -lc 'claude -p --model opus "Review the diff in /tmp/d"'"#)
                .unwrap();
        assert_eq!(claude.provider, "Claude");
        assert_eq!(claude.model.as_deref(), Some("opus"));
        assert_eq!(claude.prompt.as_deref(), Some("Review the diff in /tmp/d"));

        let piped = detect("cat brief.md | env FOO=1 claude --print --output-format text").unwrap();
        assert_eq!(piped.provider, "Claude");

        let here = detect(r#"codex exec --model=o5 <<< "list the crates""#).unwrap();
        assert_eq!(here.model.as_deref(), Some("o5"));
        assert_eq!(here.prompt.as_deref(), Some("list the crates"));

        // Calling a function is not followed into its body, so it stays a
        // command; running the assistant outside the definition still counts.
        let called = detect("review() { codex exec \"$1\"; }; review 'Check it'");
        assert_eq!(called, None);
        assert!(detect("f() { ls; }\ncodex exec 'after the function'").is_some());
        assert!(detect("f() ( ls )\n(cd /repo && codex exec 'in a subshell')").is_some());

        let quoted = detect("answer=\"$(claude -p 'Review the diff')\"; echo \"$answer\"").unwrap();
        assert_eq!(quoted.prompt.as_deref(), Some("Review the diff"));
        assert!(detect("echo \"`codex exec 'old style'`\"").is_some());
        // An unquoted heredoc runs its substitutions; a quoted one is text.
        let heredoc = detect("cat <<EOF\nSummary: $(claude -p 'Summarize the diff')\nEOF").unwrap();
        assert_eq!(heredoc.prompt.as_deref(), Some("Summarize the diff"));
        assert_eq!(detect("cat <<'EOF'\n$(claude -p 'Summarize')\nEOF"), None);
        assert_eq!(detect("cat <<EOF\n\\$(claude -p 'Summarize')\nEOF"), None);

        // A shell reading its script from a heredoc or here-string runs it.
        let script = detect("bash <<'EOF'\nset -e\nclaude -p 'Review the diff'\nEOF").unwrap();
        assert_eq!(script.prompt.as_deref(), Some("Review the diff"));
        assert!(detect("bash -s -- arg <<< \"codex exec 'go'\"").is_some());
        assert_eq!(detect("bash -n <<'EOF'\nclaude -p 'Review'\nEOF"), None);
        assert_eq!(detect("bash run.sh <<'EOF'\nclaude -p 'Review'\nEOF"), None);

        let nested = detect(r#"answer="$(claude -p "Review the diff")"; echo "$answer""#).unwrap();
        assert_eq!(nested.prompt.as_deref(), Some("Review the diff"));
        let deeper =
            detect(r#"x="$(cd "$(git rev-parse --show-toplevel)" && codex exec "go")""#).unwrap();
        assert_eq!(deeper.prompt.as_deref(), Some("go"));
        assert!(detect("bash -e -o pipefail -c 'codex exec \"go\"'").is_some());

        let resumed =
            detect("codex exec resume 019aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 'Continue'").unwrap();
        assert_eq!(resumed.prompt.as_deref(), Some("Continue"));
        let last = detect("codex exec resume --last 'Keep going'").unwrap();
        assert_eq!(last.prompt.as_deref(), Some("Keep going"));

        let waited =
            detect("timeout --signal TERM --kill-after 5 600 codex exec 'Review the diff'")
                .unwrap();
        assert_eq!(waited.prompt.as_deref(), Some("Review the diff"));
        assert!(detect("claude -p -- 'Review the diff'").is_some());
        assert_eq!(
            detect("claude -p 'doctor'").unwrap().prompt.as_deref(),
            Some("doctor")
        );
        let unquoted =
            detect("summary=$(claude -p 'Summarize the diff') && echo $summary").unwrap();
        assert_eq!(unquoted.prompt.as_deref(), Some("Summarize the diff"));
        assert!(detect("x=`codex exec 'go'`").is_some());
        assert!(detect("(cd /repo && codex exec 'in a subshell')").is_some());

        let after =
            detect("echo start; CODEX_HOME=/x codex e 'fix the build' 2>&1 | tail -n 5").unwrap();
        assert_eq!(after.prompt.as_deref(), Some("fix the build"));
    }

    #[test]
    fn commands_that_only_mention_an_assistant_are_not_agents() {
        for command in [
            "codex --version",
            "codex login status",
            "codex exec --help",
            "claude --version",
            "claude mcp list",
            "claude mcp add my-server -- npx -y -p some-package server",
            "claude config set -p theme dark",
            "echo $(date +%F) codex exec 'example command'",
            "echo `date` claude -p 'example'",
            "claude 'interactive prompt'",
            "~/.claude/skills/codex-opussy/bin/codex-agent --print-config",
            "cat ~/.claude/skills/codex-opussy/bin/codex-agent",
            "grep -rn 'codex exec' host/src",
            "echo \"claude -p hi\"",
            "env -u",
            "timeout -k",
            "nice -n",
            "git commit -m 'run codex exec from claude -p'",
            "cat <<'EOF' > notes.md\ncodex exec do things\nEOF",
            "review() { codex exec 'Review the diff'; }",
            "function review {\n  claude -p 'Review the diff'\n}\necho defined",
            "bash -n -c 'codex exec \"Review\"'",
            "bash --noexec -c 'codex exec \"Review\"'",
            "bash -o noexec -c 'codex exec \"Review\"'",
            "review() ( codex exec 'Review the diff' )",
            "bash helper.sh -c 'codex exec \"Review\"'",
            "echo '$(claude -p hi)'",
            "echo \"\\$(claude -p 'Review the diff')\"",
            "git commit -m \"$(cat <<'EOF'\nfix: don't run claude -p twice\n\nUse codex exec instead.\nEOF\n)\"",
            "echo \"$(printf '%s' \"hello; codex exec hi\")\"",
            "review() {\n  printf '%s\\n' \"}\"\n  codex exec 'Review the diff'\n}",
            "echo \"\\`codex exec 'Review'\\`\"",
            "echo \"price: $(date)\"",
            "review () (\n  cd /repo && claude -p 'Review'\n)",
        ] {
            assert_eq!(detect(command), None, "{command}");
        }
    }

    #[test]
    fn the_wrapper_footer_is_read_and_dropped_from_the_report() {
        let parsed = report(
            "All three call sites check expiry.\n\n- a.rs:10\n\n---\n\
             [codex-opussy] ok · model=gpt-6.1-sol effort=medium sandbox=read-only · 42s · full log: /x.log\n",
        );
        assert_eq!(
            parsed.text,
            "All three call sites check expiry.\n\n- a.rs:10"
        );
        assert!(!parsed.failed);
        assert_eq!(parsed.model.as_deref(), Some("gpt-6.1-sol"));

        let failed = report(
            "(codex produced no final message)\n\n---\n[codex-opussy] FAILED (exit 1) · model=m · 3s · full log: /y",
        );
        assert!(failed.failed);
        assert_eq!(failed.text, "(codex produced no final message)");

        let plain = report("  just the answer\n");
        assert_eq!(plain.text, "just the answer");
        assert!(!plain.failed);
    }

    #[test]
    fn a_long_output_file_is_read_from_its_end() {
        let path =
            std::env::temp_dir().join(format!("xd-delegation-{}.output", uuid::Uuid::new_v4()));
        let mut text = "noise line\n".repeat(10_000);
        text.push_str("The report.\n");
        std::fs::write(&path, &text).unwrap();
        let read = read_output(path.to_str().unwrap()).unwrap();
        assert!(read.len() <= 64 * 1024);
        assert!(read.starts_with("noise line\n"));
        assert!(read.ends_with("The report.\n"));
        let _ = std::fs::remove_file(&path);
        assert_eq!(read_output("/nonexistent/xd/output"), None);
        assert_eq!(read_output(std::env::temp_dir().to_str().unwrap()), None);

        // One long line, as a JSON answer can be, is kept rather than dropped.
        let single =
            std::env::temp_dir().join(format!("xd-delegation-{}.output", uuid::Uuid::new_v4()));
        std::fs::write(&single, format!("head\n{}", "x".repeat(70_000))).unwrap();
        let read = read_output(single.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&single);
        assert_eq!(read.len(), 64 * 1024);
    }
}
