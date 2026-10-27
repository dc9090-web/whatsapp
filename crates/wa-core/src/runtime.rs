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
use crate::events::{BackendCommand, BackendEvent, ConnectionState};
use crate::model::{
    ChatId, MediaKind, MediaRef, Message, MessageContent, MessageId, MessageStatus, QuotedMessage,
};
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
enum Work {
    Command {
        command: BackendCommand,
        done: oneshot::Sender<()>,
    },
    /// An event synthesized by the runtime itself (a command the backend could
    /// not take), persisted and forwarded like a backend event.
    Event(BackendEvent),
}

/// Spawns the backend and the persistence task on the current tokio runtime.
pub fn start(backend: Box<dyn WhatsAppBackend>, store: Store) -> Handle {
    // Whatever was still in flight when the previous run ended will never be
    // confirmed; make it retryable.
    match store.fail_stale_pending() {
        Ok(0) => {}
        Ok(n) => tracing::info!("marked {n} unsent message(s) from the last run as failed"),
        Err(e) => tracing::error!("could not fail stale pending messages: {e:#}"),
    }

    let (ui_cmd_tx, ui_cmd_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (ui_ev_tx, ui_ev_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (be_cmd_tx, be_cmd_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (be_ev_tx, be_ev_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (work_tx, work_rx) = mpsc::channel(CHANNEL_CAPACITY);

    tokio::spawn(supervise(backend, be_cmd_rx, be_ev_tx));
    tokio::spawn(command_pump(ui_cmd_rx, be_cmd_tx, work_tx));
    tokio::spawn(event_pump(store, be_ev_rx, work_rx, ui_ev_tx));

    Handle {
        commands: ui_cmd_tx,
        events: ui_ev_rx,
    }
}

/// Runs the backend in its own task and reports when it dies (error or
/// panic) instead of leaving the UI showing the last connection state.
async fn supervise(
    backend: Box<dyn WhatsAppBackend>,
    commands: mpsc::Receiver<BackendCommand>,
    events: mpsc::Sender<BackendEvent>,
) {
    let report = events.clone();
    let task = tokio::spawn(backend.run(commands, events));
    let failure = match task.await {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(format!("Backend stopped: {e:#}")),
        Err(e) if e.is_panic() => Some("Backend crashed; restart the app to reconnect".to_owned()),
        Err(e) => Some(format!("Backend stopped: {e}")),
    };
    if let Some(msg) = failure {
        tracing::error!("{msg}");
        let _ = report.send(BackendEvent::Error(msg)).await;
        let _ = report
            .send(BackendEvent::Connection(ConnectionState::Disconnected))
            .await;
    }
}

async fn command_pump(
    mut ui: mpsc::Receiver<BackendCommand>,
    backend: mpsc::Sender<BackendCommand>,
    work: mpsc::Sender<Work>,
) {
    while let Some(command) = ui.recv().await {
        let shutdown = command == BackendCommand::Shutdown;
        if matches!(
            command,
            BackendCommand::SendText { .. }
                | BackendCommand::SendMedia { .. }
                | BackendCommand::MarkRead { .. }
                | BackendCommand::Logout
                | BackendCommand::DeleteMessage {
                    for_everyone: false,
                    ..
                }
                | BackendCommand::DeleteChat { .. }
        ) {
            let (done, ack) = oneshot::channel();
            if work
                .send(Work::Command {
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
        if let Err(mpsc::error::SendError(command)) = backend.send(command).await {
            tracing::warn!("backend is not running; failing command");
            if let Some(event) = rejected(command) {
                let _ = work.send(Work::Event(event)).await;
            }
        }
        if shutdown {
            break;
        }
    }
    // Dropping `backend` closes the backend's command stream, which stops it.
}

/// The failure event for a command the backend never received.
fn rejected(command: BackendCommand) -> Option<BackendEvent> {
    const WHY: &str = "the WhatsApp backend is not running";
    match command {
        BackendCommand::SendText { chat, local_id, .. }
        | BackendCommand::SendMedia { chat, local_id, .. } => Some(BackendEvent::SendFailed {
            chat,
            local_id,
            error: WHY.to_owned(),
        }),
        BackendCommand::FetchMedia { chat, message, .. } => Some(BackendEvent::MediaFailed {
            chat,
            message,
            error: WHY.to_owned(),
        }),
        BackendCommand::FetchAvatar { chat } => Some(BackendEvent::AvatarMissing { chat }),
        BackendCommand::React { .. } | BackendCommand::Logout => {
            Some(BackendEvent::Error(format!("Not done: {WHY}")))
        }
        BackendCommand::DeleteMessage {
            for_everyone: true, ..
        } => Some(BackendEvent::Error(format!(
            "Not deleted for everyone: {WHY}"
        ))),
        BackendCommand::MarkRead { .. }
        | BackendCommand::DeleteMessage { .. }
        | BackendCommand::DeleteChat { .. }
        | BackendCommand::Shutdown => None,
    }
}

async fn event_pump(
    store: Store,
    mut backend: mpsc::Receiver<BackendEvent>,
    mut work: mpsc::Receiver<Work>,
    ui: mpsc::Sender<BackendEvent>,
) {
    let mut logout_requested = false;
    let mut work_open = true;
    let mut backend_open = true;
    // Ends once the backend has stopped *and* the command pump is gone, so
    // commands sent to a dead backend still get their failure events. The UI
    // sees the event stream end only after a requested shutdown completed.
    while work_open || backend_open {
        tokio::select! {
            biased;
            job = work.recv(), if work_open => match job {
                Some(Work::Command { command, done }) => {
                    let out = apply_command(&store, &command, &mut logout_requested);
                    forward(&ui, out).await;
                    let _ = done.send(());
                }
                Some(Work::Event(event)) => {
                    let out = persist(&store, event, &mut logout_requested);
                    forward(&ui, out).await;
                }
                None => work_open = false,
            },
            event = backend.recv(), if backend_open => match event {
                Some(event) => {
                    let out = persist(&store, event, &mut logout_requested);
                    forward(&ui, out).await;
                }
                // Every backend sender is gone: the backend has stopped.
                None => backend_open = false,
            },
        }
    }
}

/// Persists an event; returns the events to forward (a storage error, if
/// any, followed by the event itself). Synchronous so no `&Store` is held
/// across an await (the store is not `Sync`).
fn persist(store: &Store, event: BackendEvent, logout_requested: &mut bool) -> Vec<BackendEvent> {
    let mut out = Vec::with_capacity(2);
    if let Err(e) = apply_event(store, &event, logout_requested) {
        tracing::error!("failed to persist {}: {e:#}", event_name(&event));
        out.push(BackendEvent::Error(format!("Storage error: {e:#}")));
    }
    out.push(event);
    out
}

async fn forward(ui: &mpsc::Sender<BackendEvent>, events: Vec<BackendEvent>) {
    for event in events {
        if ui.send(event).await.is_err() {
            tracing::debug!("UI event receiver dropped");
        }
    }
}

/// The quote block for a reply, from the stored message being replied to.
fn quoted(store: &Store, chat: &ChatId, id: &MessageId) -> QuotedMessage {
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
}

/// Optimistic effects of a UI command. Returns the events to emit before the
/// command is forwarded to the backend.
fn apply_command(
    store: &Store,
    command: &BackendCommand,
    logout_requested: &mut bool,
) -> Vec<BackendEvent> {
    match command {
        BackendCommand::SendMedia {
            chat,
            local_id,
            path,
            kind,
            mime,
            file_name,
            size,
            caption,
            thumbnail,
            reply_to,
        } => {
            let media = MediaRef {
                mime: mime.clone(),
                size: *size,
                thumbnail: thumbnail.clone(),
                local_path: Some(path.clone()),
                download_token: Vec::new(),
            };
            let content = match kind {
                MediaKind::Image => MessageContent::Image {
                    caption: caption.clone(),
                    media,
                },
                MediaKind::Video => MessageContent::Video {
                    caption: caption.clone(),
                    media,
                },
                MediaKind::Audio => MessageContent::Audio {
                    voice_note: false,
                    duration_secs: 0,
                    media,
                },
                MediaKind::Document => MessageContent::Document {
                    file_name: file_name.clone(),
                    caption: caption.clone(),
                    media,
                },
            };
            let message = Message {
                id: local_id.clone(),
                chat: chat.clone(),
                sender: String::new(),
                sender_name: None,
                from_me: true,
                timestamp: now_ms(),
                content,
                reply_to: reply_to.as_ref().map(|id| quoted(store, chat, id)),
                status: MessageStatus::Pending,
            };
            let mut events = Vec::with_capacity(2);
            if let Err(e) = store.upsert_message(&message) {
                tracing::error!("storing outgoing media message: {e:#}");
                events.push(BackendEvent::Error(format!("Storage error: {e:#}")));
            }
            events.push(BackendEvent::Message(message));
            events
        }
        BackendCommand::DeleteMessage {
            chat,
            message,
            for_everyone: false,
        } => match store.delete_message(chat, message) {
            Ok(_) => vec![BackendEvent::MessageDeleted {
                chat: chat.clone(),
                message: message.clone(),
            }],
            Err(e) => vec![BackendEvent::Error(format!(
                "Could not delete the message: {e:#}"
            ))],
        },
        BackendCommand::DeleteChat { chat } => match store.delete_chat(chat) {
            Ok(()) => vec![BackendEvent::ChatDeleted { chat: chat.clone() }],
            Err(e) => vec![BackendEvent::Error(format!(
                "Could not delete the chat: {e:#}"
            ))],
        },
        BackendCommand::SendText {
            chat,
            text,
            reply_to,
            local_id,
        } => {
            let reply_to = reply_to.as_ref().map(|id| quoted(store, chat, id));
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
        | BackendCommand::FetchAvatar { .. }
        | BackendCommand::DeleteMessage {
            for_everyone: true, ..
        }
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
        BackendEvent::AvatarReady { chat, path } => store.set_chat_avatar(chat, path),
        BackendEvent::MessageDeleted { chat, message } => {
            store.delete_message(chat, message).map(drop)
        }
        BackendEvent::MessageRevoked {
            chat,
            message,
            content,
        } => store.replace_content(chat, message, content),
        BackendEvent::ChatDeleted { chat } => store.delete_chat(chat),
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
        | BackendEvent::AvatarMissing { .. }
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
        BackendEvent::AvatarReady { .. } => "AvatarReady",
        BackendEvent::AvatarMissing { .. } => "AvatarMissing",
        BackendEvent::MessageDeleted { .. } => "MessageDeleted",
        BackendEvent::MessageRevoked { .. } => "MessageRevoked",
        BackendEvent::ChatDeleted { .. } => "ChatDeleted",
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
        // Retry reuses the failed message's id: the same row goes back to
        // Pending instead of a duplicate bubble appearing.
        apply_command(&store, &send, &mut logout);
        let status = store.message(&chat, &local_id).unwrap().unwrap().status;
        assert_eq!(status, MessageStatus::Pending);
        assert_eq!(store.messages(&chat, None, 50).unwrap().len(), 1);
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
