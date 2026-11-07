//! Which processes on this machine would take others down with them.

use std::collections::HashSet;

/// One process, as the machine lists it.
#[derive(Debug, PartialEq, Eq)]
struct Listed {
    pid: u32,
    parent: u32,
    group: u32,
}

/// Every process with a living child or with another living process in the group
/// it leads, or `None` when the machine will not say.
///
/// A process that exits hands its children to init, so what it left running can
/// only be found by the group it stayed in. What is neither a child nor in the
/// group has moved away, and survives anything done to this process.
///
/// A system that hides other users' processes from `/proc` altogether, with
/// `hidepid=2`, looks the same as one where they are not running.
pub(crate) fn with_company() -> Option<HashSet<u32>> {
    let mut found = HashSet::new();
    for process in list_processes()? {
        found.insert(process.parent);
        if process.group != process.pid {
            found.insert(process.group);
        }
    }
    Some(found)
}

#[cfg(target_os = "linux")]
fn list_processes() -> Option<Vec<Listed>> {
    let mut processes = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()? {
        let path = entry.ok()?.path();
        let is_process = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.bytes().all(|byte| byte.is_ascii_digit()));
        if !is_process {
            continue;
        }
        match std::fs::read_to_string(path.join("stat")) {
            Ok(stat) => processes.extend(living_in_stat(&stat)),
            // It has exited since the directory was listed.
            Err(error) if process_has_gone(&error) => {}
            // It may be anyone's child, so nothing can be said about anyone.
            Err(_) => return None,
        }
    }
    Some(processes)
}

#[cfg(any(test, target_os = "linux"))]
fn process_has_gone(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH)
}

#[cfg(not(target_os = "linux"))]
fn list_processes() -> Option<Vec<Listed>> {
    let output = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,pgid=,stat="])
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(living_in_ps)
            .collect()
    })
}

/// The process a `/proc/<pid>/stat` describes, unless it has exited. The command
/// name is free text in parentheses, so fields count from the last one.
#[cfg(any(test, target_os = "linux"))]
fn living_in_stat(stat: &str) -> Option<Listed> {
    let (pid, fields) = stat.rsplit_once(')')?;
    let pid = pid.split_ascii_whitespace().next()?.parse().ok()?;
    let mut fields = fields.split_ascii_whitespace();
    let state = fields.next()?;
    let parent = fields.next()?.parse().ok()?;
    let group = fields.next()?.parse().ok()?;
    (!matches!(state, "Z" | "X")).then_some(Listed { pid, parent, group })
}

/// The same for a `pid ppid pgid stat` line from `ps`.
#[cfg(any(test, not(target_os = "linux")))]
fn living_in_ps(line: &str) -> Option<Listed> {
    let mut fields = line.split_ascii_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let parent = fields.next()?.parse().ok()?;
    let group = fields.next()?.parse().ok()?;
    let state = fields.next()?;
    (!state.starts_with('Z')).then_some(Listed { pid, parent, group })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTED: Listed = Listed {
        pid: 9,
        parent: 7,
        group: 8,
    };

    #[test]
    fn a_stat_line_describes_a_living_process() {
        assert_eq!(living_in_stat("9 (bash) S 7 8 9 0"), Some(LISTED));
        assert_eq!(
            living_in_stat("9 (odd) name) (x) R 7 8 9 0"),
            Some(LISTED),
            "a command name may contain anything"
        );
        assert_eq!(living_in_stat("9 (bash) Z 7 8 9 0"), None);
        assert_eq!(living_in_stat("damaged"), None);
    }

    #[test]
    fn only_a_process_that_has_gone_is_passed_over() {
        use std::io::{Error, ErrorKind};

        assert!(process_has_gone(&Error::from(ErrorKind::NotFound)));
        assert!(process_has_gone(&Error::from_raw_os_error(libc::ESRCH)));
        assert!(!process_has_gone(&Error::from(ErrorKind::PermissionDenied)));
    }

    #[test]
    fn a_ps_line_describes_a_living_process() {
        assert_eq!(living_in_ps("    9     7     8 Ss+"), Some(LISTED));
        assert_eq!(living_in_ps("    9     7     8 Z+"), None);
        assert_eq!(living_in_ps("damaged"), None);
    }

    #[cfg(unix)]
    #[test]
    fn this_process_has_company_while_a_child_it_started_runs() {
        let me = std::process::id();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        assert!(with_company().unwrap().contains(&me));
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
