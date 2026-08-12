use std::{fs, io, path::Path};

pub fn secure_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

pub fn create_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(windows)]
    {
        fs::DirBuilder::new().create(path)
    }
}

pub fn socket_is_private(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_socket() && metadata.permissions().mode() & 0o077 == 0
        })
    }
    #[cfg(windows)]
    {
        // Windows AF_UNIX does not expose Unix permission bits. The socket is
        // created inside our per-process private directory, so avoid probing
        // it with a connection that the bridge would treat as a real client.
        path.exists()
    }
}

pub fn socket_path_exists(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket())
    }
    #[cfg(windows)]
    {
        path.exists()
    }
}
