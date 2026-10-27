//! Messages crossing the UI ↔ backend boundary.

use std::path::PathBuf;

use crate::model::{Chat, ChatId, MediaRef, Message, MessageId, MessageStatus, Reaction};

/// UI → backend.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendCommand {
    /// `local_id` is the optimistic id the runtime already stored as `Pending`.
    SendText { chat: ChatId, text: String, reply_to: Option<MessageId>, local_id: MessageId },
    /// Empty `emoji` removes our reaction.
    React { chat: ChatId, target: MessageId, emoji: String },
    MarkRead { chat: ChatId, messages: Vec<MessageId> },
    /// Download and decrypt the full media file of a stored message.
    FetchMedia { chat: ChatId, message: MessageId, media: MediaRef },
    /// Unlink this device from the account.
    Logout,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting { attempt: u32, retry_in_secs: u64 },
    Disconnected,
}

/// Backend → runtime → UI.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    /// A pairing QR payload to render; replaced by the next `Qr` when it expires.
    Qr { code: String, valid_for_secs: u64 },
    Paired,
    Connection(ConnectionState),
    /// The session is no longer valid (unlinked from the phone or via `Logout`).
    LoggedOut,
    /// Initial history sync progress in `0.0..=1.0`.
    SyncProgress(f32),
    SyncDone,
    ChatUpserted(Chat),
    Message(Message),
    MessageStatus { chat: ChatId, message: MessageId, status: MessageStatus },
    /// The optimistic `local_id` was accepted by the server as `id`.
    MessageSent { chat: ChatId, local_id: MessageId, id: MessageId },
    SendFailed { chat: ChatId, local_id: MessageId, error: String },
    Reaction(Reaction),
    MediaReady { chat: ChatId, message: MessageId, path: PathBuf },
    MediaFailed { chat: ChatId, message: MessageId, error: String },
    Error(String),
}
