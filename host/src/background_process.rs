use std::{ffi::OsStr, process::Command};

pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    crate::user_environment::apply(&mut command);
    command
}
