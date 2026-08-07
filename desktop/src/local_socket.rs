#[cfg(unix)]
pub use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(windows)]
pub use uds_windows::{UnixListener, UnixStream};

#[cfg(unix)]
pub fn path_is_socket(path: &std::path::Path) -> bool {
    use std::os::unix::fs::FileTypeExt;

    std::fs::metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket())
}

#[cfg(windows)]
pub fn path_is_socket(path: &std::path::Path) -> bool {
    UnixStream::connect(path).is_ok()
}
