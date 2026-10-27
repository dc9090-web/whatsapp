//! XDG locations. Each function creates its directory on first use.
//!
//! The directories hold the Signal session keys, every message and decrypted
//! media, so they are created (and kept) owner-only (`0700`).

use std::path::{Path, PathBuf};

const APP_DIR: &str = "whatsapp-cosmic";

fn ensure(base: Option<PathBuf>, fallback: &str) -> std::io::Result<PathBuf> {
    let base = base
        .or_else(|| dirs::home_dir().map(|h| h.join(fallback)))
        .ok_or_else(|| std::io::Error::other("no home directory"))?;
    let dir = base.join(APP_DIR);
    create_private_dir(&dir)?;
    Ok(dir)
}

/// Creates `dir` (and missing parents) and restricts `dir` itself to its
/// owner, also tightening a directory left behind with wider permissions.
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let perms = std::fs::metadata(dir)?.permissions();
        if perms.mode() & 0o777 != 0o700 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// `~/.local/share/whatsapp-cosmic`
pub fn data_dir() -> std::io::Result<PathBuf> {
    ensure(dirs::data_dir(), ".local/share")
}

/// `~/.cache/whatsapp-cosmic/media`
pub fn media_dir() -> std::io::Result<PathBuf> {
    let dir = ensure(dirs::cache_dir(), ".cache")?.join("media");
    create_private_dir(&dir)?;
    Ok(dir)
}

/// `~/.local/state/whatsapp-cosmic`
pub fn state_dir() -> std::io::Result<PathBuf> {
    ensure(dirs::state_dir(), ".local/state")
}

pub fn session_db() -> std::io::Result<PathBuf> {
    Ok(data_dir()?.join("session.db"))
}

pub fn app_db() -> std::io::Result<PathBuf> {
    Ok(data_dir()?.join("app.db"))
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn app_dirs_are_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = ensure(Some(tmp.path().to_path_buf()), "unused").unwrap();
        assert_eq!(dir, tmp.path().join(APP_DIR));
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        // An existing, world-readable directory is tightened.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure(Some(tmp.path().to_path_buf()), "unused").unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}
