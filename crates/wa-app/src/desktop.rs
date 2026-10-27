//! Thin adapter over `wa-desktop` (tray, notifications, autostart).
//!
//! All `wa_desktop` calls live here so the rest of the UI is independent of
//! whether the integration is compiled in. With the `desktop` feature off
//! (until Task 4 lands its implementation of the contract) every call is a
//! logged no-op and autostart reports "unavailable".

use std::fmt;
use std::sync::{Arc, OnceLock};

use cosmic::iced::Subscription;
use cosmic::iced::futures::SinkExt;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub enum DesktopEvent {
    /// The tray is registered; keep the handle to update the badge.
    TrayReady(TrayHandle),
    /// No tray (no StatusNotifierItem host, or built without `desktop`):
    /// the window must stay reachable, so closing it quits instead of hiding.
    TrayUnavailable,
    /// Tray activate / "Show WhatsApp".
    Show,
    /// Tray "Quit".
    Quit,
    /// A notification for this chat id was clicked.
    NotificationClicked(String),
}

#[derive(Clone)]
pub struct TrayHandle {
    #[cfg(feature = "desktop")]
    inner: Arc<wa_desktop::tray::Tray>,
    #[cfg(not(feature = "desktop"))]
    _inner: Arc<()>,
}

impl fmt::Debug for TrayHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TrayHandle")
    }
}

impl TrayHandle {
    pub async fn set_unread(self, count: u32) {
        #[cfg(feature = "desktop")]
        self.inner.set_unread(count).await;
        #[cfg(not(feature = "desktop"))]
        tracing::trace!(count, "tray unavailable in this build");
    }
}

/// Sender that notification click handlers report to; set once by [`subscription`].
static CLICKS: OnceLock<mpsc::UnboundedSender<String>> = OnceLock::new();

/// Registers the tray and relays tray / notification-click events.
pub fn subscription() -> Subscription<DesktopEvent> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(16, async |mut output| {
            let (click_tx, mut click_rx) = mpsc::unbounded_channel::<String>();
            let _ = CLICKS.set(click_tx);

            #[cfg(feature = "desktop")]
            let mut tray_rx = {
                let (tray_tx, tray_rx) = mpsc::unbounded_channel();
                match wa_desktop::tray::Tray::spawn(tray_tx).await {
                    Ok(tray) => {
                        let handle = TrayHandle {
                            inner: Arc::new(tray),
                        };
                        let _ = output.send(DesktopEvent::TrayReady(handle)).await;
                    }
                    Err(err) => {
                        tracing::warn!(?err, "tray unavailable");
                        let _ = output.send(DesktopEvent::TrayUnavailable).await;
                    }
                }
                tray_rx
            };
            #[cfg(not(feature = "desktop"))]
            let mut tray_rx = {
                let _ = TrayHandle {
                    _inner: Arc::new(()),
                };
                tracing::info!("built without the `desktop` feature: no tray or notifications");
                let _ = output.send(DesktopEvent::TrayUnavailable).await;
                let (_tx, rx) = mpsc::unbounded_channel::<()>();
                rx
            };

            loop {
                let event = tokio::select! {
                    Some(chat) = click_rx.recv() => DesktopEvent::NotificationClicked(chat),
                    Some(tray) = tray_rx.recv() => map_tray(tray),
                    else => break,
                };
                if output.send(event).await.is_err() {
                    break;
                }
            }
            std::future::pending::<()>().await;
        })
    })
}

#[cfg(feature = "desktop")]
fn map_tray(event: wa_desktop::tray::TrayEvent) -> DesktopEvent {
    match event {
        wa_desktop::tray::TrayEvent::Show => DesktopEvent::Show,
        wa_desktop::tray::TrayEvent::Quit => DesktopEvent::Quit,
    }
}

#[cfg(not(feature = "desktop"))]
fn map_tray(_: ()) -> DesktopEvent {
    DesktopEvent::Show
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
