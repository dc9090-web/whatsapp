//! Messages crossing the UI ↔ backend boundary.

use std::path::PathBuf;

use crate::model::{
    CallState, Chat, ChatId, GroupInfo, MediaKind, MediaRef, Message, MessageContent, MessageId,
    MessageStatus, Reaction,
};

/// Something to do to a group we are in.
#[derive(Debug, Clone, PartialEq)]
pub enum GroupAction {
    SetSubject(String),
    /// Empty clears the description.
    SetDescription(String),
    Add(Vec<String>),
    Remove(Vec<String>),
    Promote(Vec<String>),
    Demote(Vec<String>),
    Leave,
    InviteLink,
}

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
    /// Send a file as a media message. `path` is already in its final form
    /// (images re-encoded as JPEG by the UI); `thumbnail` is a small JPEG.
    SendMedia {
        chat: ChatId,
        local_id: MessageId,
        path: PathBuf,
        kind: MediaKind,
        mime: String,
        file_name: String,
        size: u64,
        caption: Option<String>,
        thumbnail: Option<Vec<u8>>,
        reply_to: Option<MessageId>,
    },
    /// Fetch a group's metadata and members (answered by `GroupInfo`).
    GroupInfo {
        chat: ChatId,
    },
    /// Create a group with these members (phone-number JIDs).
    CreateGroup {
        name: String,
        members: Vec<String>,
    },
    GroupAction {
        chat: ChatId,
        action: GroupAction,
    },
    /// Place a voice call to a direct chat.
    StartCall {
        chat: ChatId,
    },
    AcceptCall {
        call_id: String,
    },
    RejectCall {
        call_id: String,
    },
    EndCall {
        call_id: String,
    },
    SetCallMuted {
        call_id: String,
        muted: bool,
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
    GroupInfo(GroupInfo),
    /// A group we created now exists; the runtime adds it to the chat list.
    GroupCreated {
        chat: ChatId,
        name: String,
    },
    GroupInviteLink {
        chat: ChatId,
        link: String,
    },
    GroupLeft {
        chat: ChatId,
    },
    /// Someone is calling us (voice, or video which we answer audio-only).
    CallIncoming {
        call_id: String,
        chat: ChatId,
        caller_name: Option<String>,
        is_video: bool,
    },
    /// A call we placed or answered changed state.
    CallState {
        call_id: String,
        chat: ChatId,
        state: CallState,
    },
    Error(String),
}
