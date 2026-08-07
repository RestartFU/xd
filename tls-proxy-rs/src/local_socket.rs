#[cfg(unix)]
pub use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(windows)]
pub use uds_windows::{UnixListener, UnixStream};

#[cfg(unix)]
pub fn existing_socket(_path: &std::path::Path, metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::FileTypeExt;

    metadata.file_type().is_socket()
}

#[cfg(windows)]
pub fn existing_socket(_path: &std::path::Path, metadata: &std::fs::Metadata) -> bool {
    !metadata.file_type().is_symlink() && metadata.is_file()
}

#[cfg(unix)]
pub fn make_private(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(windows)]
pub fn make_private(_: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}
