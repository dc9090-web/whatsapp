//! Start-at-login via an XDG autostart `.desktop` entry.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

const FILE_NAME: &str = "com.github.whatsapp-cosmic.desktop";

/// Contents of the autostart entry.
const DESKTOP_ENTRY: &str = "[Desktop Entry]
Type=Application
Name=WhatsApp
Comment=Open WhatsApp at login
Exec=whatsapp-cosmic
Icon=com.github.whatsapp-cosmic
Terminal=false
X-GNOME-Autostart-enabled=true
";

fn autostart_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("autostart"))
}

/// True when the autostart entry exists in `~/.config/autostart`.
pub fn is_enabled() -> bool {
    autostart_dir().is_some_and(|d| is_enabled_in(&d))
}

/// Writes/removes `~/.config/autostart/com.github.whatsapp-cosmic.desktop`
/// with `Exec=whatsapp-cosmic`.
pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let dir = autostart_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine the XDG config directory"))?;
    set_enabled_in(&dir, enabled)
}

pub(crate) fn is_enabled_in(dir: &Path) -> bool {
    dir.join(FILE_NAME).is_file()
}

pub(crate) fn set_enabled_in(dir: &Path, enabled: bool) -> anyhow::Result<()> {
    let path = dir.join(FILE_NAME);
    if enabled {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating autostart directory {}", dir.display()))?;
        // Write to a temp file and rename so a crash never leaves a half-written entry.
        let tmp = dir.join(format!(".{FILE_NAME}.tmp"));
        std::fs::write(&tmp, DESKTOP_ENTRY)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
        tracing::info!(path = %path.display(), "autostart enabled");
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::info!(path = %path.display(), "autostart disabled"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| format!("removing {}", path.display()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_writes_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("autostart");
        set_enabled_in(&dir, true).unwrap();
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(text.starts_with("[Desktop Entry]"));
        assert!(text.contains("Exec=whatsapp-cosmic\n"));
        assert!(text.contains("Type=Application"));
        assert!(text.contains("X-GNOME-Autostart-enabled=true"));
        assert!(is_enabled_in(&dir));
    }

    #[test]
    fn disable_removes_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("autostart");
        set_enabled_in(&dir, true).unwrap();
        set_enabled_in(&dir, false).unwrap();
        assert!(!dir.join(FILE_NAME).exists());
        assert!(!is_enabled_in(&dir));
    }

    #[test]
    fn disable_when_missing_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("does-not-exist");
        set_enabled_in(&dir, false).unwrap();
        assert!(!is_enabled_in(&dir));
    }

    #[test]
    fn enable_twice_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        set_enabled_in(tmp.path(), true).unwrap();
        set_enabled_in(tmp.path(), true).unwrap();
        assert!(is_enabled_in(tmp.path()));
    }
}
