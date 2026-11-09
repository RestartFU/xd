use std::{fs, io, path::Path};

pub fn secure_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

pub fn secure_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

pub fn create_private_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
    options.open(path)
}

/// Opens a file for reading without waiting for a FIFO's writer. Checking the
/// path first would not do: it can be replaced before the open, so a caller must
/// check the file this returns.
pub fn open_without_waiting(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

/// Like `open_without_waiting`, but refuses symlinks in every component
/// except a system alias at the top (see [`resolve_system_alias`]).
/// Walking with directory descriptors also prevents a path replacement from
/// redirecting the read to a different tree between checking and opening it.
pub fn open_without_waiting_no_symlinks(path: &Path) -> io::Result<fs::File> {
    use std::{
        ffi::CString,
        os::fd::{AsRawFd, FromRawFd},
        path::Component,
    };
    if !path.is_absolute() {
        return Err(io::Error::other("Expected an absolute path"));
    }
    let resolved = resolve_system_alias(path)?;
    let path = resolved.as_deref().unwrap_or(path);
    let mut directory = fs::File::open("/")?;
    let mut parts = path
        .components()
        .filter(|part| *part != Component::RootDir)
        .peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(io::Error::other("Expected a normal path component"));
        };
        let name = CString::new(name.as_encoded_bytes())?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if parts.peek().is_some() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        directory = unsafe { fs::File::from_raw_fd(fd) };
    }
    Ok(directory)
}

/// The path with its first component resolved, if that component is a
/// symlink root owns, such as macOS's `/tmp` and `/var`, which lead to
/// `/private/tmp` and `/private/var`. Only root can make an entry directly
/// under `/`, so such a link is the system's, not something an agent placed;
/// every component after it must still be free of symlinks.
fn resolve_system_alias(path: &Path) -> io::Result<Option<std::path::PathBuf>> {
    use std::{os::unix::fs::MetadataExt, path::Component};
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Ok(None);
    }
    let Some(Component::Normal(first)) = components.next() else {
        return Ok(None);
    };
    let top = Path::new("/").join(first);
    let Ok(metadata) = fs::symlink_metadata(&top) else {
        return Ok(None);
    };
    if !metadata.file_type().is_symlink() || metadata.uid() != 0 {
        return Ok(None);
    }
    Ok(Some(fs::canonicalize(&top)?.join(components.as_path())))
}

pub fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A top-level symlink root owns, as macOS's `/tmp` is, and a regular
    /// file reached through it. Merged-`/usr` Linux has `/bin` and `/lib`.
    fn system_alias_file() -> Option<std::path::PathBuf> {
        use std::os::unix::fs::MetadataExt;
        for top in ["tmp", "var", "bin", "lib", "sbin", "etc"] {
            let alias = Path::new("/").join(top);
            let Ok(metadata) = fs::symlink_metadata(&alias) else {
                continue;
            };
            if !metadata.file_type().is_symlink() || metadata.uid() != 0 {
                continue;
            }
            let Ok(entries) = fs::read_dir(&alias) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    return Some(alias.join(entry.file_name()));
                }
            }
        }
        None
    }

    #[test]
    fn a_system_alias_at_the_top_is_followed_but_no_other_symlink_is() {
        if let Some(file) = system_alias_file() {
            open_without_waiting_no_symlinks(&file).expect("opens through the system alias");
        } else {
            eprintln!("skipped: no root-owned top-level symlink to read through");
        }

        let directory = std::env::temp_dir().join(format!("xd-alias-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("real")).unwrap();
        fs::write(directory.join("real/output"), "line\n").unwrap();
        std::os::unix::fs::symlink(directory.join("real"), directory.join("link")).unwrap();
        let directory = fs::canonicalize(&directory).unwrap();
        assert!(open_without_waiting_no_symlinks(&directory.join("real/output")).is_ok());
        assert!(open_without_waiting_no_symlinks(&directory.join("link/output")).is_err());
        fs::remove_dir_all(&directory).unwrap();
    }
}
