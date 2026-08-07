#[cfg(unix)]
pub use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(windows)]
pub use uds_windows::{UnixListener, UnixStream};

#[cfg(unix)]
pub fn path_is_socket(path: &std::path::Path) -> bool {
    use std::os::unix::fs::FileTypeExt;

    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket())
}

#[cfg(windows)]
pub fn path_is_socket(path: &std::path::Path) -> bool {
    UnixStream::connect(path).is_ok()
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
