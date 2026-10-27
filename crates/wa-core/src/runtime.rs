//! Sits between the UI and a [`WhatsAppBackend`]: persists every event to the
//! [`Store`] before forwarding it, and applies optimistic updates for commands.
//!
//! CONTRACT (implemented in Task 2): `start` and `Handle` are fixed.
//!
//! Behaviour:
//! - `SendText` from UI: store a `Pending` outgoing message with `local_id`,
//!   emit `BackendEvent::Message` for it, then forward the command.
//! - `MarkRead`: `store.mark_read(chat)`, then forward.
//! - `Logout`: remember it was requested, forward; on the following
//!   `LoggedOut` event, `store.wipe()`.
//! - Backend events: `ChatUpserted` → `upsert_chat`; `Message` → `upsert_message`;
//!   `MessageStatus` → `set_status`; `MessageSent` → `replace_message_id` +
//!   `set_status(Sent)`; `SendFailed` → `set_status(Failed)`; `Reaction` →
//!   `apply_reaction`; `MediaReady` → `set_media_path`. Then forward unchanged.
//! - Store errors are logged with `tracing::error!` and forwarded as
//!   `BackendEvent::Error`; they never stop the runtime.

use tokio::sync::mpsc;

use crate::backend::WhatsAppBackend;
use crate::events::{BackendCommand, BackendEvent};
use crate::store::Store;

pub struct Handle {
    pub commands: mpsc::Sender<BackendCommand>,
    pub events: mpsc::Receiver<BackendEvent>,
}

/// Spawns the backend and the persistence task on the current tokio runtime.
pub fn start(backend: Box<dyn WhatsAppBackend>, store: Store) -> Handle {
    let _ = (backend, store);
    todo!("Task 2")
}
