//! Bridges `wa_core::start` into an iced [`Subscription`].
//!
//! On first poll the subscription builds the backend, opens the runtime's own
//! [`Store`] connection, starts the runtime, hands the command sender to the UI
//! (`BackendMsg::Ready`) and then streams every [`BackendEvent`].

use std::fmt;
use std::path::PathBuf;

use cosmic::iced::Subscription;
use cosmic::iced::futures::SinkExt;
use tokio::sync::mpsc;
use wa_core::backend::fake::FakeBackend;
use wa_core::backend::whatsapp_rust::WhatsAppRustBackend;
use wa_core::{BackendCommand, BackendEvent, Store, WhatsAppBackend};

/// Which backend to run and where its data lives. Hashable so it identifies
/// the subscription: the runtime starts exactly once per configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendConfig {
    pub fake: bool,
    pub app_db: PathBuf,
    pub session_db: PathBuf,
    pub media_dir: PathBuf,
}

/// Command channel to the runtime, wrapped for `Debug`/`Clone` messages.
#[derive(Clone)]
pub struct CommandSender(pub mpsc::Sender<BackendCommand>);

impl fmt::Debug for CommandSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CommandSender")
    }
}

impl CommandSender {
    /// Non-blocking send; a full or closed channel is logged, never panics.
    pub fn send(&self, cmd: BackendCommand) -> bool {
        match self.0.try_send(cmd) {
            Ok(()) => true,
            Err(err) => {
                tracing::error!(%err, "failed to send backend command");
                false
            }
        }
    }
}

// Transient UI messages; boxing the event would only add allocations.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum BackendMsg {
    Ready(CommandSender),
    Event(BackendEvent),
    /// The runtime's event stream ended: after `Shutdown`, the backend has
    /// finished its graceful disconnect and everything is persisted.
    Closed,
}

pub fn subscription(config: BackendConfig) -> Subscription<BackendMsg> {
    Subscription::run_with(config, |config| {
        let config = config.clone();
        cosmic::iced::stream::channel(256, async move |mut output| {
            let backend: Box<dyn WhatsAppBackend> = if config.fake {
                Box::new(FakeBackend::demo())
            } else {
                Box::new(WhatsAppRustBackend::new(
                    config.session_db.clone(),
                    config.media_dir.clone(),
                ))
            };
            let store = match Store::open(&config.app_db) {
                Ok(store) => store,
                Err(err) => {
                    tracing::error!(?err, "runtime cannot open app database");
                    let _ = output
                        .send(BackendMsg::Event(BackendEvent::Error(format!(
                            "Cannot open the message database: {err:#}"
                        ))))
                        .await;
                    std::future::pending::<()>().await;
                    return;
                }
            };
            let mut handle = wa_core::start(backend, store);
            if output
                .send(BackendMsg::Ready(CommandSender(handle.commands.clone())))
                .await
                .is_err()
            {
                return;
            }
            while let Some(event) = handle.events.recv().await {
                if output.send(BackendMsg::Event(event)).await.is_err() {
                    break;
                }
            }
            tracing::info!("backend event stream ended");
            let _ = output.send(BackendMsg::Closed).await;
            std::future::pending::<()>().await;
        })
    })
}
