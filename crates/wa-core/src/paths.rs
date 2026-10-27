//! XDG locations. Each function creates its directory on first use.

use std::path::PathBuf;

const APP_DIR: &str = "whatsapp-cosmic";

fn ensure(base: Option<PathBuf>, fallback: &str) -> std::io::Result<PathBuf> {
    let base = base
        .or_else(|| dirs::home_dir().map(|h| h.join(fallback)))
        .ok_or_else(|| std::io::Error::other("no home directory"))?;
    let dir = base.join(APP_DIR);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// `~/.local/share/whatsapp-cosmic`
pub fn data_dir() -> std::io::Result<PathBuf> {
    ensure(dirs::data_dir(), ".local/share")
}

/// `~/.cache/whatsapp-cosmic/media`
pub fn media_dir() -> std::io::Result<PathBuf> {
    let dir = ensure(dirs::cache_dir(), ".cache")?.join("media");
    std::fs::create_dir_all(&dir)?;
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
