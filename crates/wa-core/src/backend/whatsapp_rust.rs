//! The real backend over the `whatsapp-rust` crate.
//!
//! CONTRACT (implemented in Task 3): `WhatsAppRustBackend::new` is fixed.

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::backend::WhatsAppBackend;
use crate::events::{BackendCommand, BackendEvent};

pub struct WhatsAppRustBackend {
    #[allow(dead_code)]
    session_db: PathBuf,
    #[allow(dead_code)]
    media_dir: PathBuf,
}

impl WhatsAppRustBackend {
    pub fn new(session_db: PathBuf, media_dir: PathBuf) -> Self {
        Self { session_db, media_dir }
    }
}

#[async_trait::async_trait]
impl WhatsAppBackend for WhatsAppRustBackend {
    async fn run(
        self: Box<Self>,
        commands: mpsc::Receiver<BackendCommand>,
        events: mpsc::Sender<BackendEvent>,
    ) -> anyhow::Result<()> {
        let _ = (commands, events);
        todo!("Task 3")
    }
}
