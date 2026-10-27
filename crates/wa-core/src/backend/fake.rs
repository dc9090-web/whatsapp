//! Scripted backend for tests and `whatsapp-cosmic --fake`.
//!
//! CONTRACT (implemented in Task 2).

use tokio::sync::mpsc;

use crate::backend::WhatsAppBackend;
use crate::events::{BackendCommand, BackendEvent};

pub struct FakeBackend {
    /// Emitted in order at startup.
    pub script: Vec<BackendEvent>,
    /// Every command received is forwarded here (for test assertions).
    pub received: Option<mpsc::UnboundedSender<BackendCommand>>,
    /// Acknowledge SendText with MessageSent/Delivered/Read, React with a
    /// Reaction echo, FetchMedia with MediaReady pointing at a bundled sample.
    pub auto_reply: bool,
}

impl FakeBackend {
    /// A believable demo: Connected, ~6 chats (direct + group) with text,
    /// replies, an image, a voice note, a document, reactions and one
    /// Unsupported message, then SyncDone. Used by `--fake`.
    pub fn demo() -> Self {
        todo!("Task 2")
    }
}

#[async_trait::async_trait]
impl WhatsAppBackend for FakeBackend {
    async fn run(
        self: Box<Self>,
        commands: mpsc::Receiver<BackendCommand>,
        events: mpsc::Sender<BackendEvent>,
    ) -> anyhow::Result<()> {
        let _ = (commands, events);
        todo!("Task 2")
    }
}
