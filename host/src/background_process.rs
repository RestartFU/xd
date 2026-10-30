use std::{ffi::OsStr, process::Command};

pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}
