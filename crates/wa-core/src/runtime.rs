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

use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, oneshot};

use crate::backend::WhatsAppBackend;
use crate::events::{BackendCommand, BackendEvent};
use crate::model::{Message, MessageContent, MessageStatus, QuotedMessage};
use crate::store::Store;

/// Capacity of every channel between UI, runtime and backend.
const CHANNEL_CAPACITY: usize = 256;

pub struct Handle {
    pub commands: mpsc::Sender<BackendCommand>,
    pub events: mpsc::Receiver<BackendEvent>,
}

/// Store work requested by the command pump. The event pump (the single owner
/// of the write connection) performs it and acknowledges, so a command is only
/// forwarded to the backend once its optimistic effects are persisted and
/// announced; the backend's answer can therefore never overtake them.
struct StoreWork {
    command: BackendCommand,
    done: oneshot::Sender<()>,
}

/// Spawns the backend and the persistence task on the current tokio runtime.
pub fn start(backend: Box<dyn WhatsAppBackend>, store: Store) -> Handle {
    let (ui_cmd_tx, ui_cmd_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (ui_ev_tx, ui_ev_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (be_cmd_tx, be_cmd_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (be_ev_tx, be_ev_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (work_tx, work_rx) = mpsc::channel(CHANNEL_CAPACITY);

    let error_tx = be_ev_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = backend.run(be_cmd_rx, be_ev_tx).await {
            tracing::error!("backend stopped with an error: {e:#}");
            let _ = error_tx
                .send(BackendEvent::Error(format!("Backend stopped: {e:#}")))
                .await;
        }
    });
    tokio::spawn(command_pump(ui_cmd_rx, be_cmd_tx, work_tx));
    tokio::spawn(event_pump(store, be_ev_rx, work_rx, ui_ev_tx));

    Handle {
        commands: ui_cmd_tx,
        events: ui_ev_rx,
    }
}

async fn command_pump(
    mut ui: mpsc::Receiver<BackendCommand>,
    backend: mpsc::Sender<BackendCommand>,
    work: mpsc::Sender<StoreWork>,
) {
    while let Some(command) = ui.recv().await {
        let shutdown = command == BackendCommand::Shutdown;
        if matches!(
            command,
            BackendCommand::SendText { .. }
                | BackendCommand::MarkRead { .. }
                | BackendCommand::Logout
        ) {
            let (done, ack) = oneshot::channel();
            if work
                .send(StoreWork {
                    command: command.clone(),
                    done,
                })
                .await
                .is_ok()
            {
                // An error here means the event pump is gone; forward anyway.
                let _ = ack.await;
            }
        }
        if backend.send(command).await.is_err() {
            tracing::warn!("backend is not running; dropping command");
        }
        if shutdown {
            break;
        }
    }
    // Dropping `backend` closes the backend's command stream, which stops it.
}

async fn event_pump(
    store: Store,
    mut backend: mpsc::Receiver<BackendEvent>,
    mut work: mpsc::Receiver<StoreWork>,
    ui: mpsc::Sender<BackendEvent>,
) {
    let mut logout_requested = false;
    let mut work_open = true;
    loop {
        tokio::select! {
            biased;
            job = work.recv(), if work_open => match job {
                Some(StoreWork { command, done }) => {
                    for event in apply_command(&store, &command, &mut logout_requested) {
                        let _ = ui.send(event).await;
                    }
                    let _ = done.send(());
                }
                None => work_open = false,
            },
            event = backend.recv() => match event {
                Some(event) => {
                    if let Err(e) = apply_event(&store, &event, &mut logout_requested) {
                        tracing::error!("failed to persist {}: {e:#}", event_name(&event));
                        let _ = ui.send(BackendEvent::Error(format!("Storage error: {e:#}"))).await;
                    }
                    if ui.send(event).await.is_err() {
                        tracing::debug!("UI event receiver dropped");
                    }
                }
                // Every backend sender is gone: the backend has stopped.
                None => break,
            },
        }
    }
}

/// Optimistic effects of a UI command. Returns the events to emit before the
/// command is forwarded to the backend.
fn apply_command(
    store: &Store,
    command: &BackendCommand,
    logout_requested: &mut bool,
) -> Vec<BackendEvent> {
    match command {
        BackendCommand::SendText {
            chat,
            text,
            reply_to,
            local_id,
        } => {
            let reply_to = reply_to.as_ref().map(|id| {
                let quoted = store.message(chat, id).unwrap_or_else(|e| {
                    tracing::error!("loading quoted message {}: {e:#}", id.0);
                    None
                });
                match quoted {
                    Some(q) => QuotedMessage {
                        id: id.clone(),
                        sender: q.sender,
                        snippet: q.content.preview(),
                    },
                    None => QuotedMessage {
                        id: id.clone(),
                        sender: String::new(),
                        snippet: String::new(),
                    },
                }
            });
            let message = Message {
                id: local_id.clone(),
                chat: chat.clone(),
                sender: String::new(),
                sender_name: None,
                from_me: true,
                timestamp: now_ms(),
                content: MessageContent::Text(text.clone()),
                reply_to,
                status: MessageStatus::Pending,
            };
            let mut events = Vec::with_capacity(2);
            if let Err(e) = store.upsert_message(&message) {
                tracing::error!("storing outgoing message: {e:#}");
                events.push(BackendEvent::Error(format!("Storage error: {e:#}")));
            }
            events.push(BackendEvent::Message(message));
            events
        }
        BackendCommand::MarkRead { chat, .. } => match store.mark_read(chat) {
            Ok(()) => Vec::new(),
            Err(e) => {
                tracing::error!("marking {} read: {e:#}", chat.0);
                vec![BackendEvent::Error(format!("Storage error: {e:#}"))]
            }
        },
        BackendCommand::Logout => {
            *logout_requested = true;
            Vec::new()
        }
        BackendCommand::React { .. }
        | BackendCommand::FetchMedia { .. }
        | BackendCommand::Shutdown => Vec::new(),
    }
}

/// Persists a backend event. The caller forwards the event either way.
fn apply_event(
    store: &Store,
    event: &BackendEvent,
    logout_requested: &mut bool,
) -> anyhow::Result<()> {
    match event {
        BackendEvent::ChatUpserted(chat) => store.upsert_chat(chat),
        BackendEvent::Message(message) => store.upsert_message(message).map(drop),
        BackendEvent::MessageStatus {
            chat,
            message,
            status,
        } => store.set_status(chat, message, *status),
        BackendEvent::MessageSent { chat, local_id, id } => {
            store.replace_message_id(chat, local_id, id)?;
            store.set_status(chat, id, MessageStatus::Sent)
        }
        BackendEvent::SendFailed {
            chat,
            local_id,
            error,
        } => {
            tracing::warn!("sending to {} failed: {error}", chat.0);
            store.set_status(chat, local_id, MessageStatus::Failed)
        }
        BackendEvent::Reaction(reaction) => store.apply_reaction(reaction),
        BackendEvent::MediaReady {
            chat,
            message,
            path,
        } => store.set_media_path(chat, message, path),
        BackendEvent::LoggedOut => {
            if std::mem::take(logout_requested) {
                store.wipe()
            } else {
                Ok(())
            }
        }
        BackendEvent::Qr { .. }
        | BackendEvent::Paired
        | BackendEvent::Connection(_)
        | BackendEvent::SyncProgress(_)
        | BackendEvent::SyncDone
        | BackendEvent::MediaFailed { .. }
        | BackendEvent::Error(_) => Ok(()),
    }
}

fn event_name(event: &BackendEvent) -> &'static str {
    match event {
        BackendEvent::Qr { .. } => "Qr",
        BackendEvent::Paired => "Paired",
        BackendEvent::Connection(_) => "Connection",
        BackendEvent::LoggedOut => "LoggedOut",
        BackendEvent::SyncProgress(_) => "SyncProgress",
        BackendEvent::SyncDone => "SyncDone",
        BackendEvent::ChatUpserted(_) => "ChatUpserted",
        BackendEvent::Message(_) => "Message",
        BackendEvent::MessageStatus { .. } => "MessageStatus",
        BackendEvent::MessageSent { .. } => "MessageSent",
        BackendEvent::SendFailed { .. } => "SendFailed",
        BackendEvent::Reaction(_) => "Reaction",
        BackendEvent::MediaReady { .. } => "MediaReady",
        BackendEvent::MediaFailed { .. } => "MediaFailed",
        BackendEvent::Error(_) => "Error",
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChatId, MessageId};

    #[test]
    fn send_failed_marks_failed_and_retry_pending() {
        let store = Store::open_in_memory().unwrap();
        let chat = ChatId("1@s.whatsapp.net".into());
        let local_id = MessageId::new_local();
        let mut logout = false;
        let send = BackendCommand::SendText {
            chat: chat.clone(),
            text: "x".into(),
            reply_to: None,
            local_id: local_id.clone(),
        };
        apply_command(&store, &send, &mut logout);
        let failed = BackendEvent::SendFailed {
            chat: chat.clone(),
            local_id: local_id.clone(),
            error: "offline".into(),
        };
        apply_event(&store, &failed, &mut logout).unwrap();
        let status = store.message(&chat, &local_id).unwrap().unwrap().status;
        assert_eq!(status, MessageStatus::Failed);
    }

    #[test]
    fn logged_out_only_wipes_when_requested() {
        let store = Store::open_in_memory().unwrap();
        let chat = ChatId("1@s.whatsapp.net".into());
        let mut logout = false;
        let send = BackendCommand::SendText {
            chat: chat.clone(),
            text: "x".into(),
            reply_to: None,
            local_id: MessageId::new_local(),
        };
        apply_command(&store, &send, &mut logout);
        apply_event(&store, &BackendEvent::LoggedOut, &mut logout).unwrap();
        assert_eq!(store.chats().unwrap().len(), 1);
        apply_command(&store, &BackendCommand::Logout, &mut logout);
        assert!(logout);
        apply_event(&store, &BackendEvent::LoggedOut, &mut logout).unwrap();
        assert!(!logout);
        assert!(store.chats().unwrap().is_empty());
    }
}
