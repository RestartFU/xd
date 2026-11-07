//! Which processes on this machine have a living child.

use std::collections::HashSet;

/// Every process with a child that is still running, or `None` when the machine
/// will not say. A process that exits hands its children to init, so anything
/// still descended from a process keeps one of its direct children alive.
///
/// A system that hides other users' processes from `/proc` altogether, with
/// `hidepid=2`, looks the same as one where they are not running.
pub(crate) fn parents() -> Option<HashSet<u32>> {
    read_parents()
}

#[cfg(target_os = "linux")]
fn read_parents() -> Option<HashSet<u32>> {
    let mut parents = HashSet::new();
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
            Ok(stat) => parents.extend(living_parent_in_stat(&stat)),
            // It has exited since the directory was listed.
            Err(error) if process_has_gone(&error) => {}
            // It may be anyone's child, so nothing can be said about anyone.
            Err(_) => return None,
        }
    }
    Some(parents)
}

#[cfg(any(test, target_os = "linux"))]
fn process_has_gone(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH)
}

#[cfg(not(target_os = "linux"))]
fn read_parents() -> Option<HashSet<u32>> {
    let output = std::process::Command::new("ps")
        .args(["-A", "-o", "ppid=,stat="])
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(living_parent_in_ps)
            .collect()
    })
}

/// The parent named by a `/proc/<pid>/stat`, unless the process has exited. The
/// command name is free text in parentheses, so fields count from the last one.
#[cfg(any(test, target_os = "linux"))]
fn living_parent_in_stat(stat: &str) -> Option<u32> {
    let (_, fields) = stat.rsplit_once(')')?;
    let mut fields = fields.split_ascii_whitespace();
    let state = fields.next()?;
    let parent = fields.next()?.parse().ok()?;
    (!matches!(state, "Z" | "X")).then_some(parent)
}

/// The same for a `ppid stat` line from `ps`.
#[cfg(any(test, not(target_os = "linux")))]
fn living_parent_in_ps(line: &str) -> Option<u32> {
    let mut fields = line.split_ascii_whitespace();
    let parent = fields.next()?.parse().ok()?;
    let state = fields.next()?;
    (!state.starts_with('Z')).then_some(parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stat_line_names_the_parent_of_a_living_process() {
        assert_eq!(living_parent_in_stat("9 (bash) S 7 9 9 0"), Some(7));
        assert_eq!(
            living_parent_in_stat("9 (odd) name) (x) R 7 9 9 0"),
            Some(7),
            "a command name may contain anything"
        );
        assert_eq!(living_parent_in_stat("9 (bash) Z 7 9 9 0"), None);
        assert_eq!(living_parent_in_stat("damaged"), None);
    }

    #[test]
    fn only_a_process_that_has_gone_is_passed_over() {
        use std::io::{Error, ErrorKind};

        assert!(process_has_gone(&Error::from(ErrorKind::NotFound)));
        assert!(process_has_gone(&Error::from_raw_os_error(libc::ESRCH)));
        assert!(!process_has_gone(&Error::from(ErrorKind::PermissionDenied)));
    }

    #[test]
    fn a_ps_line_names_the_parent_of_a_living_process() {
        assert_eq!(living_parent_in_ps("    7 Ss+"), Some(7));
        assert_eq!(living_parent_in_ps("    7 Z+"), None);
        assert_eq!(living_parent_in_ps("damaged"), None);
    }

    #[cfg(unix)]
    #[test]
    fn this_process_is_the_parent_of_a_child_it_started_until_that_child_is_gone() {
        let me = std::process::id();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        assert!(parents().unwrap().contains(&me));
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
