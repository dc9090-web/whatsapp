//! SIGTERM/SIGINT → the same graceful quit as closing the window, so logout
//! and `kill` let the backend disconnect and flush its session state.

use cosmic::iced::Subscription;
use cosmic::iced::futures::SinkExt;
use tokio::signal::unix::{SignalKind, signal};

/// Emits once, on the first SIGTERM or SIGINT. Never emits if the handlers
/// cannot be installed (logged).
pub fn subscription() -> Subscription<()> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(1, async move |mut output| {
            let (mut term, mut int) = match (
                signal(SignalKind::terminate()),
                signal(SignalKind::interrupt()),
            ) {
                (Ok(t), Ok(i)) => (t, i),
                (t, i) => {
                    let err = t.err().or(i.err());
                    tracing::warn!(
                        ?err,
                        "cannot install signal handlers; SIGTERM will not quit cleanly"
                    );
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            };
            tokio::select! {
                _ = term.recv() => tracing::info!("SIGTERM received; quitting"),
                _ = int.recv() => tracing::info!("SIGINT received; quitting"),
            }
            let _ = output.send(()).await;
            std::future::pending::<()>().await;
        })
    })
}
