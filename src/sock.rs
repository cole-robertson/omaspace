//! Private unix sockets: listeners only this user can connect to, for the
//! live view and agent browsers' DevTools. A loopback TCP port would be open
//! to every user on the machine.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

/// The user's runtime directory (`/run/user/<uid>`, mode 0700).
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { getuid() })))
}

/// Bind `path` so only this user can connect: created under a 0077 umask (no
/// window where it is open to others), then chmod 0600 to be sure. A stale
/// socket from an earlier run is replaced; anything else at the path is not.
pub fn bind_private(path: &Path) -> anyhow::Result<UnixListener> {
    if let Ok(m) = std::fs::symlink_metadata(path) {
        use std::os::unix::fs::FileTypeExt;
        anyhow::ensure!(
            m.file_type().is_socket(),
            "{} exists and is not a socket",
            path.display()
        );
        std::fs::remove_file(path)?;
    }
    let old = unsafe { umask(0o077) };
    let bound = UnixListener::bind(path);
    unsafe { umask(old) };
    let listener = bound?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

unsafe extern "C" {
    fn getuid() -> u32;
    fn umask(mask: u32) -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_is_private_and_replaces_only_a_stale_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("view.sock");
        let first = bind_private(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(first);
        bind_private(&path).expect("a stale socket from an earlier run is replaced");
        let file = dir.path().join("not-a-socket");
        std::fs::write(&file, "keep").unwrap();
        assert!(bind_private(&file).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep");
    }
}
