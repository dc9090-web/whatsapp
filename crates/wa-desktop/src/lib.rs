//! Desktop integration with no UI-toolkit dependency (Task 4).
//!
//! CONTRACT — signatures other crates rely on:
//!
//! ```text
//! pub mod notify {
//!     /// Shows a freedesktop notification. `body` is None when previews are off.
//!     /// Clicking the notification sends `chat_id` on `on_click`.
//!     pub fn show(summary: &str, body: Option<&str>, chat_id: String,
//!                 on_click: tokio::sync::mpsc::UnboundedSender<String>) -> anyhow::Result<()>;
//! }
//! pub mod tray {
//!     pub enum TrayEvent { Show, Quit }
//!     pub struct Tray { /* ksni handle */ }
//!     impl Tray {
//!         /// Registers a StatusNotifierItem. Activate (left click) / "Show" → TrayEvent::Show.
//!         pub async fn spawn(events: tokio::sync::mpsc::UnboundedSender<TrayEvent>) -> anyhow::Result<Tray>;
//!         /// Updates tooltip + badge icon; 0 hides the badge.
//!         pub async fn set_unread(&self, count: u32);
//!     }
//! }
//! pub mod autostart {
//!     pub fn is_enabled() -> bool;
//!     /// Writes/removes ~/.config/autostart/com.github.whatsapp-cosmic.desktop
//!     /// with `Exec=whatsapp-cosmic --background`.
//!     pub fn set_enabled(enabled: bool) -> anyhow::Result<()>;
//! }
//! ```

/// Application id shared by the notification icon, tray id and autostart entry.
pub const APP_ID: &str = "com.github.whatsapp-cosmic";

pub mod autostart;
pub mod notify;
pub mod tray;
