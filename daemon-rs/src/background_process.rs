use std::{ffi::OsStr, process::Command};

#[cfg(any(windows, test))]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    configure(&mut command);
    command
}

fn configure(_command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        _command.creation_flags(background_creation_flags(true));
    }
}

#[cfg(any(windows, test))]
fn background_creation_flags(windows: bool) -> u32 {
    if windows { CREATE_NO_WINDOW } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_background_processes_do_not_create_console_windows() {
        assert_eq!(background_creation_flags(true), CREATE_NO_WINDOW);
        assert_eq!(background_creation_flags(false), 0);
    }
}
