//! Thin adapter over `wa-desktop` (notifications, autostart).
//!
//! All `wa_desktop` calls live here so the rest of the UI is independent of
//! whether the integration is compiled in. With the `desktop` feature off
//! every call is a logged no-op and autostart reports "unavailable".

use std::sync::OnceLock;

use cosmic::iced::Subscription;
use cosmic::iced::futures::SinkExt;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum DesktopEvent {
    /// A notification for this chat id was clicked.
    NotificationClicked(String),
}

/// Sender that notification click handlers report to; set once by [`subscription`].
static CLICKS: OnceLock<mpsc::UnboundedSender<String>> = OnceLock::new();

/// Relays notification-click events.
pub fn subscription() -> Subscription<DesktopEvent> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(16, async |mut output| {
            let (click_tx, mut click_rx) = mpsc::unbounded_channel::<String>();
            let _ = CLICKS.set(click_tx);
            while let Some(chat) = click_rx.recv().await {
                if output
                    .send(DesktopEvent::NotificationClicked(chat))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            std::future::pending::<()>().await;
        })
    })
}

/// Shows a notification. Must run inside the tokio runtime (call from a Task).
pub fn notify(summary: &str, body: Option<&str>, chat_id: String) {
    #[cfg(feature = "desktop")]
    {
        let Some(on_click) = CLICKS.get().cloned() else {
            tracing::warn!("notification before desktop subscription started");
            return;
        };
        if let Err(err) = wa_desktop::notify::show(summary, body, chat_id, on_click) {
            tracing::warn!(?err, "failed to show notification");
        }
    }
    #[cfg(not(feature = "desktop"))]
    tracing::debug!(
        summary,
        has_body = body.is_some(),
        chat_id,
        "notification (no desktop feature)"
    );
}

/// Whether start-at-login is enabled; `None` when unsupported in this build.
pub fn autostart_enabled() -> Option<bool> {
    #[cfg(feature = "desktop")]
    return Some(wa_desktop::autostart::is_enabled());
    #[cfg(not(feature = "desktop"))]
    None
}

pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    #[cfg(feature = "desktop")]
    return wa_desktop::autostart::set_enabled(enabled);
    #[cfg(not(feature = "desktop"))]
    {
        let _ = enabled;
        anyhow::bail!("start at login is not available in this build")
    }
}
