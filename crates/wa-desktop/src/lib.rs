//! Desktop integration with no UI-toolkit dependency (Task 4).
//!
//! CONTRACT — signatures other crates rely on:
//!
//! ```text
//! pub mod notify {
//!     /// Shows a freedesktop notification. `body` is None when previews are off.
//!     /// Clicking the notification sends `chat_id` on `on_click`.
//!     pub fn show(summary: &str, body: Option<&str>, chat_id: String,
//!                 picture: Option<&std::path::Path>,
//!                 on_click: tokio::sync::mpsc::UnboundedSender<String>) -> anyhow::Result<()>;
//! }
//! pub mod autostart {
//!     pub fn is_enabled() -> bool;
//!     /// Writes/removes ~/.config/autostart/com.github.whatsapp-cosmic.desktop
//!     /// with `Exec=whatsapp-cosmic`.
//!     pub fn set_enabled(enabled: bool) -> anyhow::Result<()>;
//! }
//! ```

/// Application id shared by the notification icon and the autostart entry.
pub const APP_ID: &str = "com.github.whatsapp-cosmic";

pub mod autostart;
pub mod notify;
