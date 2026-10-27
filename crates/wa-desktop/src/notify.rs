//! Freedesktop notifications (org.freedesktop.Notifications via zbus).

use anyhow::Context as _;
use notify_rust::{Hint, Notification, NotificationResponse};
use tokio::sync::mpsc::UnboundedSender;

use crate::APP_ID;

/// Maximum body length in characters (including the trailing ellipsis).
const MAX_BODY_CHARS: usize = 200;

/// Shows a freedesktop notification. `body` is None when previews are off.
/// Clicking the notification sends `chat_id` on `on_click`.
///
/// Must be called from within a tokio runtime: the D-Bus round trip and the
/// wait for the click happen on a spawned task, so this returns immediately.
/// Delivery failures after spawning are logged with `tracing`.
pub fn show(
    summary: &str,
    body: Option<&str>,
    chat_id: String,
    on_click: UnboundedSender<String>,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Handle::try_current()
        .context("notify::show must be called from within a tokio runtime")?;

    let mut notification = Notification::new();
    notification
        .appname("WhatsApp")
        .summary(summary)
        .icon(APP_ID)
        .hint(Hint::DesktopEntry(APP_ID.to_owned()))
        .hint(Hint::Category("im.received".to_owned()))
        // "default" is the action invoked by clicking the notification body.
        .action("default", "Open chat");
    if let Some(body) = body {
        notification.body(&truncate_body(body));
    }

    runtime.spawn(async move {
        let handle = match notification.show_async().await {
            Ok(handle) => handle,
            Err(e) => {
                tracing::warn!(error = %e, "failed to show notification");
                return;
            }
        };
        handle
            .wait_for_action_async(move |response| {
                if is_click(response) && on_click.send(chat_id).is_err() {
                    tracing::debug!("notification clicked but receiver is gone");
                }
            })
            .await;
    });
    Ok(())
}

fn is_click(response: &NotificationResponse) -> bool {
    match response {
        NotificationResponse::Default => true,
        NotificationResponse::Action(action) => action == "default",
        _ => false,
    }
}

/// Truncates `body` to at most [`MAX_BODY_CHARS`] characters on a char
/// boundary, ending with `…` when something was cut.
pub(crate) fn truncate_body(body: &str) -> String {
    match body.char_indices().nth(MAX_BODY_CHARS) {
        None => body.to_owned(),
        Some(_) => {
            // Keep MAX_BODY_CHARS - 1 chars and spend the last one on the ellipsis.
            let cut = body
                .char_indices()
                .nth(MAX_BODY_CHARS - 1)
                .map_or(body.len(), |(i, _)| i);
            let mut out = String::with_capacity(cut + '…'.len_utf8());
            out.push_str(&body[..cut]);
            out.push('…');
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_body_is_unchanged() {
        assert_eq!(truncate_body("hello"), "hello");
        let exact: String = "a".repeat(MAX_BODY_CHARS);
        assert_eq!(truncate_body(&exact), exact);
    }

    #[test]
    fn long_body_is_cut_to_limit_with_ellipsis() {
        let long = "x".repeat(500);
        let out = truncate_body(&long);
        assert_eq!(out.chars().count(), MAX_BODY_CHARS);
        assert!(out.ends_with('…'));
        assert!(out.starts_with("xxx"));
    }

    #[test]
    fn truncation_respects_multibyte_chars() {
        // 4-byte emoji and 2-byte umlauts: byte slicing would panic or split.
        let long: String = "😀ü".repeat(300);
        let out = truncate_body(&long);
        assert_eq!(out.chars().count(), MAX_BODY_CHARS);
        assert!(out.ends_with('…'));
        let prefix: String = long.chars().take(MAX_BODY_CHARS - 1).collect();
        assert!(out.starts_with(&prefix));
    }

    #[test]
    fn click_detection() {
        assert!(is_click(&NotificationResponse::Default));
        assert!(is_click(&NotificationResponse::Action("default".into())));
        assert!(!is_click(&NotificationResponse::Action("other".into())));
    }
}
