//! Fallback for headless assistants hidden behind interpreter scripts. Shell
//! parsing remains the fast path; this observes real child processes, never
//! evaluates a script or guesses from its contents.
use crate::delegation::{self, Delegation};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
struct Process {
    pid: u32,
    parent: u32,
    start: u64,
    args: Vec<String>,
}

impl Process {
    fn key(&self) -> String {
        format!("process:{}:{}", self.pid, self.start)
    }
    fn agent(&self) -> Option<Delegation> {
        delegation::detect_argv(&self.args)
    }
    fn interpreter(&self) -> bool {
        let name = self
            .args
            .first()
            .map(|s| s.rsplit('/').next().unwrap_or(s))
            .unwrap_or("");
        name == "python"
            || name == "python3"
            || name.starts_with("python3.")
            || matches!(name, "node" | "nodejs" | "ruby" | "perl")
    }
}

/// Read identity before and after argv so PID reuse cannot splice two processes.
fn read_process(pid: u32) -> Option<Process> {
    let path = format!("/proc/{pid}");
    let stat = std::fs::read_to_string(format!("{path}/stat")).ok()?;
    let (parent, start) = identity(&stat)?;
    let bytes = std::fs::read(format!("{path}/cmdline")).ok()?;
    let after = std::fs::read_to_string(format!("{path}/stat")).ok()?;
    if identity(&after)? != (parent, start) {
        return None;
    }
    let args = bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    Some(Process {
        pid,
        parent,
        start,
        args,
    })
}

fn identity(stat: &str) -> Option<(u32, u64)> {
    let fields: Vec<_> = stat.rsplit_once(')')?.1.split_ascii_whitespace().collect();
    if matches!(*fields.first()?, "Z" | "X") {
        return None;
    }
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

fn wrapped(process: &Process, root: u32, processes: &HashMap<u32, Process>) -> bool {
    let mut parent = process.parent;
    let mut seen = HashSet::new();
    let mut interpreter = false;
    while parent != root {
        if !seen.insert(parent) {
            return false;
        }
        let Some(ancestor) = processes.get(&parent) else {
            return false;
        };
        // A nested assistant belongs to the assistant that launched it. A
        // shell with an explicit agent command already has a parser-owned card.
        if ancestor.agent().is_some() {
            return false;
        }
        interpreter |= ancestor.interpreter();
        parent = ancestor.parent;
    }
    interpreter
}

#[derive(Default)]
pub(crate) struct Tracker {
    active: HashMap<String, Process>,
}

impl Tracker {
    /// None means the snapshot failed; do not end cards on a transient error.
    pub(crate) fn poll(&mut self, root: u32) -> Option<(Vec<(String, Delegation)>, Vec<String>)> {
        let mut identities = HashMap::new();
        for entry in std::fs::read_dir("/proc").ok()? {
            let entry = entry.ok()?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            match std::fs::read_to_string(entry.path().join("stat")) {
                Ok(stat) => {
                    if let Some(id) = identity(&stat) {
                        identities.insert(pid, id);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                // Other users may hide their process metadata. Only an unreadable
                // process we track makes this snapshot inconclusive.
                Err(_) if self.active.values().any(|p| p.pid == pid) => return None,
                Err(_) => {}
            }
        }
        let mut relevant = HashSet::from([root]);
        loop {
            let before = relevant.len();
            for (&pid, &(parent, _)) in &identities {
                if relevant.contains(&parent) {
                    relevant.insert(pid);
                }
            }
            if relevant.len() == before {
                break;
            }
        }
        relevant.extend(self.active.values().map(|p| p.pid));
        let mut processes = HashMap::new();
        for pid in relevant {
            if let Some(process) = read_process(pid) {
                processes.insert(pid, process);
            } else if let Some(old) = self.active.values().find(|p| p.pid == pid)
                && identities
                    .get(&pid)
                    .is_some_and(|(_, start)| *start == old.start)
            {
                // Do not interpret a transient argv read error as completion.
                processes.insert(pid, old.clone());
            }
        }
        Some(self.update(root, &processes))
    }

    fn update(
        &mut self,
        root: u32,
        processes: &HashMap<u32, Process>,
    ) -> (Vec<(String, Delegation)>, Vec<String>) {
        let mut ended = Vec::new();
        self.active.retain(|key, old| {
            let alive = processes
                .get(&old.pid)
                .is_some_and(|p| p.start == old.start);
            if !alive {
                ended.push(key.clone());
            }
            alive
        });
        let mut started = Vec::new();
        for process in processes.values() {
            if process.pid == root || self.active.contains_key(&process.key()) {
                continue;
            }
            if let Some(agent) = process.agent()
                && wrapped(process, root, processes)
            {
                let key = process.key();
                self.active.insert(key.clone(), process.clone());
                started.push((key, agent));
            }
        }
        (started, ended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(pid: u32, parent: u32, args: &[&str]) -> Process {
        Process {
            pid,
            parent,
            start: 10,
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }
    #[test]
    fn observes_a_real_python_subprocess_and_its_exit() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};
        // Bash supplies a harmless process with Claude's argv; no model or
        // credentials are needed. Python really launches and reaps it.
        let mut wrapper = Command::new("python3").args(["-c", r#"
import subprocess, sys
p = subprocess.Popen(['claude', '-p', '-c', 'read -r line'], executable='/bin/bash', stdin=subprocess.PIPE)
print(p.pid, flush=True)
sys.stdin.readline()
p.communicate(b'done\n')
"#]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(wrapper.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pid: u32 = line.trim().parse().unwrap();
        let mut tracker = Tracker::default();
        // Treat our test process as the owning agent, with Python between it
        // and Claude, just as in the reported failure.
        let (started, _) = tracker.poll(std::process::id()).unwrap();
        wrapper
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"done\n")
            .unwrap();
        assert!(wrapper.wait().unwrap().success());
        let (_, ended) = tracker.poll(std::process::id()).unwrap();
        let key = started
            .iter()
            .find(|(key, _)| key.starts_with(&format!("process:{pid}:")));
        assert!(key.is_some(), "actual child not found: {started:?}");
        assert!(ended.contains(&key.unwrap().0));
    }

    #[test]
    fn tracks_wrapped_agents_and_pid_reuse_without_other_chats_or_duplicates() {
        let mut processes: HashMap<_, _> = [
            p(1, 0, &["codex", "app-server"]),
            p(2, 1, &["python3", "review.py"]),
            p(3, 2, &["claude", "-p", "--model", "opus", "Review"]),
            p(4, 1, &["bash", "-lc", "claude -p Review"]),
            p(5, 4, &["claude", "-p", "Review"]),
            p(6, 3, &["python3", "nested.py"]),
            p(7, 6, &["claude", "-p", "Nested"]),
            p(8, 999, &["python3", "other-chat.py"]),
            p(9, 8, &["claude", "-p", "Other"]),
            p(10, 2, &["echo", "claude -p example"]),
        ]
        .into_iter()
        .map(|p| (p.pid, p))
        .collect();
        let mut tracker = Tracker::default();
        let (started, ended) = tracker.update(1, &processes);
        assert!(ended.is_empty());
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].0, "process:3:10");
        assert_eq!(started[0].1.model.as_deref(), Some("opus"));
        assert!(tracker.update(1, &processes).0.is_empty());
        // Reparenting does not end an agent that was already observed.
        processes.get_mut(&3).unwrap().parent = 999;
        assert!(tracker.update(1, &processes).1.is_empty());
        processes.get_mut(&3).unwrap().start = 11;
        assert_eq!(tracker.update(1, &processes).1, vec!["process:3:10"]);
    }
}
