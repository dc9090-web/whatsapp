//! Messages crossing the UI ↔ backend boundary.

use std::path::PathBuf;

use crate::model::{
    Chat, ChatId, MediaRef, Message, MessageContent, MessageId, MessageStatus, Reaction,
};

/// UI → backend.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendCommand {
    /// `local_id` is the optimistic id the runtime already stored as `Pending`.
    SendText {
        chat: ChatId,
        text: String,
        reply_to: Option<MessageId>,
        local_id: MessageId,
    },
    /// Empty `emoji` removes our reaction.
    React {
        chat: ChatId,
        target: MessageId,
        emoji: String,
    },
    MarkRead {
        chat: ChatId,
        messages: Vec<MessageId>,
    },
    /// Download and decrypt the full media file of a stored message.
    FetchMedia {
        chat: ChatId,
        message: MessageId,
        media: MediaRef,
    },
    /// Fetch the chat's (contact or group) profile picture.
    FetchAvatar {
        chat: ChatId,
    },
    /// Delete a message: just for us (`for_everyone: false`; applied locally
    /// by the runtime and synced to the phone), or revoked for every
    /// participant (`true`; only our own messages).
    DeleteMessage {
        chat: ChatId,
        message: MessageId,
        for_everyone: bool,
    },
    /// Delete a chat with all its messages, locally and on the phone.
    DeleteChat {
        chat: ChatId,
    },
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
// `Message` dominates the size; boxing it would change the shared contract.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    /// A pairing QR payload to render; replaced by the next `Qr` when it expires.
    Qr {
        code: String,
        valid_for_secs: u64,
    },
    Paired,
    Connection(ConnectionState),
    /// The session is no longer valid (unlinked from the phone or via `Logout`).
    LoggedOut,
    /// Initial history sync progress in `0.0..=1.0`.
    SyncProgress(f32),
    SyncDone,
    ChatUpserted(Chat),
    Message(Message),
    MessageStatus {
        chat: ChatId,
        message: MessageId,
        status: MessageStatus,
    },
    /// The optimistic `local_id` was accepted by the server as `id`.
    MessageSent {
        chat: ChatId,
        local_id: MessageId,
        id: MessageId,
    },
    SendFailed {
        chat: ChatId,
        local_id: MessageId,
        error: String,
    },
    Reaction(Reaction),
    MediaReady {
        chat: ChatId,
        message: MessageId,
        path: PathBuf,
    },
    MediaFailed {
        chat: ChatId,
        message: MessageId,
        error: String,
    },
    /// The chat's profile picture was downloaded to `path` (a JPEG).
    AvatarReady {
        chat: ChatId,
        path: PathBuf,
    },
    /// The chat has no picture we may see (none set, hidden by privacy, or
    /// the lookup failed); the UI should not ask again this session.
    AvatarMissing {
        chat: ChatId,
    },
    /// A message was removed locally.
    MessageDeleted {
        chat: ChatId,
        message: MessageId,
    },
    /// A message was revoked for everyone; its content is now `content`.
    MessageRevoked {
        chat: ChatId,
        message: MessageId,
        content: MessageContent,
    },
    /// A chat and all its messages were removed locally.
    ChatDeleted {
        chat: ChatId,
    },
    Error(String),
}
