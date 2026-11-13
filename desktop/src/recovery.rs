//! Telling a run that ended badly from one that was closed, so the next launch
//! can avoid repeating it.
//!
//! xd reopens what was open: the last chat, the browser pane and its tabs. If
//! one of those is what brought the app down, reopening it brings the app down
//! again, on every launch, with nothing the user can do about it short of
//! deleting their settings. A marker file written at launch and removed on the
//! way out says which kind of run the last one was, and a launch that follows a
//! bad one leaves the browser panes closed.
//!
//! The platform glue lives in `windows.rs`; this part is plain files and text,
//! and is tested wherever the tests run.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

const MARKER: &str = "running";
const CRASH_LOG: &str = "crash.log";

/// A crash log is for the last few failures, not a history of them.
const CRASH_LOG_LIMIT: u64 = 256 * 1024;

static CURRENT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where the crash log of the app keeping its state in `directory` is written.
pub fn crash_log(directory: &Path) -> PathBuf {
    directory.join(CRASH_LOG)
}

/// Record that a run has started, and learn whether the one before it ended
/// without [`finish`]. Call once, when no other instance can be running.
pub fn begin(directory: &Path) -> bool {
    let marker = directory.join(MARKER);
    let failed = marker.exists();
    // Without the marker the next launch only loses this protection.
    let _ = fs::create_dir_all(directory);
    let _ = fs::write(&marker, std::process::id().to_string());
    *CURRENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(marker);
    failed
}

/// The run is ending the way it should: by being closed, or by the session
/// ending under it.
pub fn finish() {
    let marker = CURRENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(marker) = marker {
        let _ = fs::remove_file(marker);
    }
}

/// What is known about a panic, laid out for a person to read and to send on.
pub struct CrashReport<'a> {
    pub version: &'a str,
    pub thread: &'a str,
    pub message: &'a str,
    pub location: Option<&'a str>,
    pub backtrace: &'a str,
    /// Seconds since the Unix epoch.
    pub time: u64,
}

impl CrashReport<'_> {
    pub fn text(&self) -> String {
        format!(
            "xd {} crashed at {} (Unix time)\nthread: {}\nwhere: {}\nwhat: {}\n\n{}\n\n",
            self.version,
            self.time,
            self.thread,
            self.location.unwrap_or("unknown"),
            self.message,
            self.backtrace.trim_end(),
        )
    }

    /// Add the report to the crash log, newest last, and return the log.
    pub fn append(&self, directory: &Path) -> std::io::Result<PathBuf> {
        fs::create_dir_all(directory)?;
        let path = crash_log(directory);
        // Start over rather than grow without bound across many crashes.
        if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > CRASH_LOG_LIMIT) {
            fs::remove_file(&path)?;
        }
        let mut log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        log.write_all(self.text().as_bytes())?;
        log.sync_all()?;
        Ok(path)
    }
}

/// What the app says once, after a launch that follows a failed run.
pub fn notice(crash_log: Option<&Path>) -> String {
    let mut notice = String::from(
        "xd did not close properly last time, so the browser panes were left closed. Reopen one from the title bar.",
    );
    if let Some(path) = crash_log {
        notice.push_str(&format!(" A crash report is in {}.", path.display()));
    }
    notice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "xd-recovery-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    // One test, because the marker of the current run is process-wide state.
    #[test]
    fn a_run_that_never_finished_is_reported_to_the_next_launch() {
        let directory = directory("marker");

        assert!(!begin(&directory), "a first launch follows no failure");
        // The process dies here without finishing.
        assert!(begin(&directory), "the unfinished run is noticed");
        finish();
        assert!(!directory.join(MARKER).exists());
        assert!(!begin(&directory), "a finished run is not a failure");
        finish();
        // Finishing twice, as a session end followed by a close would, is harmless.
        finish();

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn crash_reports_accumulate_newest_last_and_name_the_cause() {
        let directory = directory("log");
        let report = |message: &'static str| CrashReport {
            version: "0.2.17 (abcdef0)",
            thread: "main",
            message,
            location: Some("desktop/src/main.rs:1:2"),
            backtrace: "0: frame\n",
            time: 1_700_000_000,
        };

        let path = report("first").append(&directory).unwrap();
        report("second").append(&directory).unwrap();
        let log = fs::read_to_string(&path).unwrap();

        assert_eq!(path, crash_log(&directory));
        assert!(log.starts_with("xd 0.2.17 (abcdef0) crashed at 1700000000"));
        assert!(log.contains("where: desktop/src/main.rs:1:2\nwhat: first\n\n0: frame\n\n"));
        assert!(log.find("what: first").unwrap() < log.find("what: second").unwrap());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_overgrown_crash_log_starts_over() {
        let directory = directory("limit");
        let path = crash_log(&directory);
        fs::write(&path, vec![b'x'; CRASH_LOG_LIMIT as usize + 1]).unwrap();

        CrashReport {
            version: "0.2.17",
            thread: "main",
            message: "fresh",
            location: None,
            backtrace: "",
            time: 0,
        }
        .append(&directory)
        .unwrap();

        let log = fs::read_to_string(&path).unwrap();
        assert!(log.starts_with("xd 0.2.17 crashed"));
        assert!(log.contains("where: unknown"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_notice_points_at_a_crash_report_only_when_there_is_one() {
        assert!(!notice(None).contains("crash report"));
        let path = Path::new("C:/Users/me/AppData/Local/xd/crash.log");
        assert!(
            notice(Some(path))
                .ends_with("A crash report is in C:/Users/me/AppData/Local/xd/crash.log.")
        );
    }
}
