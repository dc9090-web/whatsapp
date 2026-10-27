//! Library-agnostic domain model shared by the backend, the store and the UI.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Placeholder text of a message its sender deleted for everyone.
pub const REVOKED_BY_SENDER: &str = "🚫 This message was deleted";
/// Placeholder text of a message we deleted for everyone.
pub const REVOKED_BY_ME: &str = "🚫 You deleted this message";

/// A WhatsApp JID as a string, e.g. `15551234567@s.whatsapp.net` or `1203...@g.us`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChatId(pub String);

/// A WhatsApp message id. Optimistic outgoing messages use a `local-` prefixed id
/// until the backend reports the server id via `BackendEvent::MessageSent`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MessageId(pub String);

/// The pseudo-chat WhatsApp delivers status updates (stories) in.
pub const STATUS_CHAT: &str = "status@broadcast";

impl ChatId {
    pub fn is_group(&self) -> bool {
        self.0.ends_with("@g.us")
    }

    /// The status-updates pseudo-chat.
    pub fn is_status(&self) -> bool {
        self.0 == STATUS_CHAT
    }

    pub fn status() -> Self {
        Self(STATUS_CHAT.to_owned())
    }

    /// The user part of the JID, used as a fallback display name.
    pub fn user(&self) -> &str {
        self.0.split('@').next().unwrap_or(&self.0)
    }
}

impl MessageId {
    pub fn new_local() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        Self(format!(
            "local-{nanos}-{}",
            SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    pub fn is_local(&self) -> bool {
        self.0.starts_with("local-")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatKind {
    Direct,
    Group,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chat {
    pub id: ChatId,
    pub kind: ChatKind,
    pub name: String,
    /// Unix milliseconds of the most recent message.
    pub last_activity: i64,
    pub unread: u32,
    pub muted: bool,
    pub last_preview: Option<String>,
}

/// Delivery state. Incoming messages are always `Received`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MessageStatus {
    Failed,
    Pending,
    Sent,
    Delivered,
    Read,
    Received,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub chat: ChatId,
    /// Sender JID. For `from_me` messages this is our own JID (or empty if unknown).
    pub sender: String,
    pub sender_name: Option<String>,
    pub from_me: bool,
    /// Unix milliseconds.
    pub timestamp: i64,
    pub content: MessageContent,
    pub reply_to: Option<QuotedMessage>,
    pub status: MessageStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotedMessage {
    pub id: MessageId,
    pub sender: String,
    pub snippet: String,
}

/// A group member as the server reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupMember {
    /// Phone-number JID (`…@s.whatsapp.net`) when known, else the wire JID.
    pub jid: String,
    pub name: Option<String>,
    pub admin: bool,
    pub superadmin: bool,
}

/// A group's metadata and membership.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupInfo {
    pub chat: ChatId,
    pub subject: String,
    pub description: Option<String>,
    pub members: Vec<GroupMember>,
    /// Whether we may administer the group.
    pub me_admin: bool,
    pub owner: Option<String>,
}

/// What kind of media message a file is sent as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Document,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MessageContent {
    Text(String),
    Image {
        caption: Option<String>,
        media: MediaRef,
    },
    Sticker {
        media: MediaRef,
    },
    Audio {
        voice_note: bool,
        duration_secs: u32,
        media: MediaRef,
    },
    Video {
        caption: Option<String>,
        media: MediaRef,
    },
    Document {
        file_name: String,
        caption: Option<String>,
        media: MediaRef,
    },
    /// Anything we cannot render (polls, call logs, ...). Never dropped silently.
    Unsupported {
        kind: String,
    },
}

impl MessageContent {
    /// One-line preview for chat list, notifications and quoted replies.
    pub fn preview(&self) -> String {
        match self {
            Self::Text(t) => t.lines().next().unwrap_or_default().to_owned(),
            Self::Image { caption, .. } => with_caption("📷 Photo", caption),
            Self::Sticker { .. } => "Sticker".to_owned(),
            Self::Audio {
                voice_note: true, ..
            } => "🎤 Voice message".to_owned(),
            Self::Audio { .. } => "🎵 Audio".to_owned(),
            Self::Video { caption, .. } => with_caption("🎥 Video", caption),
            Self::Document { file_name, .. } => format!("📄 {file_name}"),
            Self::Unsupported { .. } => "Unsupported message".to_owned(),
        }
    }

    pub fn media(&self) -> Option<&MediaRef> {
        match self {
            Self::Image { media, .. }
            | Self::Sticker { media }
            | Self::Audio { media, .. }
            | Self::Video { media, .. }
            | Self::Document { media, .. } => Some(media),
            Self::Text(_) | Self::Unsupported { .. } => None,
        }
    }

    pub fn media_mut(&mut self) -> Option<&mut MediaRef> {
        match self {
            Self::Image { media, .. }
            | Self::Sticker { media }
            | Self::Audio { media, .. }
            | Self::Video { media, .. }
            | Self::Document { media, .. } => Some(media),
            Self::Text(_) | Self::Unsupported { .. } => None,
        }
    }
}

fn with_caption(label: &str, caption: &Option<String>) -> String {
    match caption {
        Some(c) if !c.is_empty() => format!("{label} {c}"),
        _ => label.to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    pub mime: String,
    pub size: u64,
    /// Small inline JPEG thumbnail shipped with the message, if any.
    pub thumbnail: Option<Vec<u8>>,
    /// Set once the full file is downloaded and decrypted into the media cache.
    pub local_path: Option<PathBuf>,
    /// Opaque, backend-specific bytes needed to download this media later
    /// (for the whatsapp-rust backend: the encoded protobuf media message).
    pub download_token: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub chat: ChatId,
    pub target: MessageId,
    pub sender: String,
    /// Empty string means the sender removed their reaction.
    pub emoji: String,
    pub timestamp: i64,
}
