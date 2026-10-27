//! The protocol boundary. A backend only speaks WhatsApp; persistence and
//! optimistic updates live in [`crate::runtime`].

use tokio::sync::mpsc;

use crate::events::{BackendCommand, BackendEvent};

pub mod fake;
pub mod whatsapp_rust;

#[async_trait::async_trait]
pub trait WhatsAppBackend: Send + 'static {
    /// Runs until `BackendCommand::Shutdown` is received or `commands` closes.
    ///
    /// Must map every protocol event to a [`BackendEvent`], handle reconnects
    /// itself (reporting them via `BackendEvent::Connection`), and never panic
    /// on malformed or unsupported protocol input.
    async fn run(
        self: Box<Self>,
        commands: mpsc::Receiver<BackendCommand>,
        events: mpsc::Sender<BackendEvent>,
    ) -> anyhow::Result<()>;
}
