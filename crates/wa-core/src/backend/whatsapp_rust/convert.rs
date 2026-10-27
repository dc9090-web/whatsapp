//! Pure protocol → model mapping for the whatsapp-rust backend.
//!
//! Nothing in here touches the network or the disk, so every mapping rule is
//! unit-tested against hand-built protobuf messages.

use std::collections::HashMap;

use whatsapp_rust::Jid;
use whatsapp_rust::proto_helpers::MessageExt;
use whatsapp_rust::types::message::{MessageInfo, MessageSource};
use whatsapp_rust::wacore::types::presence::ReceiptType;
use whatsapp_rust::wacore_binary::JidExt;
use whatsapp_rust::waproto::buffa::MessageField;
use whatsapp_rust::waproto::codec;
use whatsapp_rust::waproto::whatsapp as wa;

use crate::backend::fake::OWN_REACTION_SENDER;
use crate::events::BackendEvent;
use crate::model::{
    Chat, ChatId, ChatKind, MediaRef, Message, MessageContent, MessageId, MessageStatus,
    QuotedMessage, Reaction,
};

/// Envelope data of one protocol message, already mapped to model types.
#[derive(Debug, Clone, PartialEq)]
pub struct MsgMeta {
    pub id: String,
    pub chat: ChatId,
    /// Non-AD sender JID (our own JID for `from_me`, may be empty if unknown).
    pub sender: String,
    pub sender_name: Option<String>,
    pub from_me: bool,
    /// Unix milliseconds.
    pub timestamp: i64,
    pub status: MessageStatus,
}

/// Result of mapping one protocol message.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // short-lived; mirrors `BackendEvent`.
pub enum Converted {
    Message(Message),
    Reaction(Reaction),
    /// Protocol plumbing with nothing to show (key shares, sync notifications,
    /// sender-key distribution, ...). The string says why, for debug logs.
    Ignored(&'static str),
}

/// What the backend remembers about a message so it can later quote it, react
/// to it or send a read receipt for it (the UI only hands us ids).
#[derive(Debug, Clone, PartialEq)]
pub struct KeyRecord {
    pub chat: ChatId,
    pub id: String,
    /// Wire sender JID (non-AD), usable as `participant` in keys.
    pub sender: String,
    pub from_me: bool,
    pub timestamp: i64,
    /// Encoded `wa::Message` prepared for quoting, if small enough.
    pub quote: Option<Vec<u8>>,
}

/// Upper bound for a stored quote payload; larger messages are quoted without
/// their body.
const MAX_QUOTE_BYTES: usize = 64 * 1024;

pub fn secs_to_ms(secs: i64) -> i64 {
    secs.saturating_mul(1000)
}

/// Maps a lid user part to its phone-number user part.
pub type LidMap = HashMap<String, String>;

/// The chat id the model uses for a protocol chat JID: devices stripped, and
/// direct chats addressed by LID rewritten to their phone-number JID when it
/// is known (from `alt` or `lids`), so live traffic and history agree.
pub fn chat_id_for(chat: &Jid, alt: Option<&Jid>, lids: &LidMap) -> ChatId {
    if chat.is_lid() {
        if let Some(alt) = alt.filter(|a| a.is_pn()) {
            return ChatId(alt.to_non_ad_string());
        }
        if let Some(pn) = lids.get(chat.user_base()) {
            return ChatId(format!("{pn}@s.whatsapp.net"));
        }
    }
    ChatId(chat.to_non_ad_string())
}

/// Like [`chat_id_for`] but for a JID string (history sync, contact actions).
pub fn chat_id_from_str(jid: &str, lids: &LidMap) -> Option<ChatId> {
    let jid: Jid = jid.parse().ok()?;
    Some(chat_id_for(&jid, None, lids))
}

/// The model chat id of a live message or receipt source.
pub fn chat_id_from_source(src: &MessageSource, lids: &LidMap) -> ChatId {
    if src.is_group || src.chat.is_group() {
        return ChatId(src.chat.to_non_ad_string());
    }
    // For our own messages the peer's alternate address is the recipient's.
    let alt = if src.is_from_me {
        src.recipient_alt.as_ref()
    } else {
        src.sender_alt.as_ref()
    };
    chat_id_for(&src.chat, alt, lids)
}

/// Whether a live chat should be shown at all (status updates and channels
/// are out of scope for v1).
pub fn is_supported_chat(chat: &Jid) -> bool {
    !chat.is_status_broadcast() && !chat.is_newsletter() && !chat.is_broadcast_list()
}

pub fn meta_from_info(info: &MessageInfo, own_jid: &str, lids: &LidMap) -> MsgMeta {
    let src = &info.source;
    let from_me = src.is_from_me;
    let sender = if from_me {
        own_jid.to_owned()
    } else {
        src.sender.to_non_ad_string()
    };
    let push = info.push_name.trim();
    MsgMeta {
        id: info.id.clone(),
        chat: chat_id_from_source(src, lids),
        sender,
        sender_name: (!from_me && !push.is_empty()).then(|| push.to_owned()),
        from_me,
        timestamp: info.timestamp.timestamp_millis(),
        status: if from_me {
            MessageStatus::Sent
        } else {
            MessageStatus::Received
        },
    }
}

/// Maps a protocol message to a model event.
pub fn convert_message(meta: MsgMeta, msg: &wa::Message) -> Converted {
    let base = unwrap_message(msg);

    if let Some(pm) = base.protocol_message.as_option() {
        return convert_protocol(meta, pm);
    }
    if let Some(r) = base.reaction_message.as_option() {
        let Some(target) = r.key.as_option().and_then(|k| k.id.clone()) else {
            return Converted::Ignored("reaction without target");
        };
        return Converted::Reaction(Reaction {
            chat: meta.chat,
            target: MessageId(target),
            // Our own reactions use the shared "me" convention (see FakeBackend)
            // so the UI can highlight them without knowing our JID.
            sender: if meta.from_me {
                OWN_REACTION_SENDER.to_owned()
            } else {
                meta.sender
            },
            emoji: r.text.clone().unwrap_or_default(),
            timestamp: r.sender_timestamp_ms.unwrap_or(meta.timestamp),
        });
    }
    if base.enc_reaction_message.is_set() {
        return Converted::Ignored("encrypted reaction");
    }
    if base.poll_update_message.is_set() {
        return Converted::Ignored("poll vote");
    }
    if base.keep_in_chat_message.is_set() || base.pin_in_chat_message.is_set() {
        return Converted::Ignored("keep/pin action");
    }
    if base.album_message.is_set() {
        return Converted::Ignored("album header (children arrive separately)");
    }
    if base.secret_encrypted_message.is_set()
        || base.enc_event_response_message.is_set()
        || base.enc_comment_message.is_set()
        || base.placeholder_message.is_set()
        || base.message_history_bundle.is_set()
        || base.message_history_notice.is_set()
    {
        return Converted::Ignored("encrypted add-on or placeholder");
    }

    match content_of(base) {
        Some((content, reply_to)) => Converted::Message(build_message(meta, content, reply_to)),
        None => match unsupported_kind(base) {
            Some(kind) => Converted::Message(build_message(
                meta,
                MessageContent::Unsupported {
                    kind: kind.to_owned(),
                },
                None,
            )),
            None => Converted::Ignored("no user-visible content"),
        },
    }
}

fn build_message(
    meta: MsgMeta,
    content: MessageContent,
    reply_to: Option<QuotedMessage>,
) -> Message {
    Message {
        id: MessageId(meta.id),
        chat: meta.chat,
        sender: meta.sender,
        sender_name: meta.sender_name,
        from_me: meta.from_me,
        timestamp: meta.timestamp,
        content,
        reply_to,
        status: meta.status,
    }
}

fn convert_protocol(meta: MsgMeta, pm: &wa::message::ProtocolMessage) -> Converted {
    use wa::message::protocol_message::Type;
    let target = pm.key.as_option().and_then(|k| k.id.clone());
    match (pm.r#type, target) {
        (Some(Type::REVOKE), Some(target)) => {
            let meta = MsgMeta { id: target, ..meta };
            Converted::Message(build_message(
                meta,
                MessageContent::Text("🚫 This message was deleted".to_owned()),
                None,
            ))
        }
        (Some(Type::MESSAGE_EDIT), Some(target)) => {
            let Some((content, reply_to)) = pm
                .edited_message
                .as_option()
                .and_then(|m| content_of(m.get_base_message()))
            else {
                return Converted::Ignored("edit without content");
            };
            let meta = MsgMeta { id: target, ..meta };
            Converted::Message(build_message(meta, content, reply_to))
        }
        _ => Converted::Ignored("protocol message"),
    }
}

/// Strips the transport wrappers (device-sent, ephemeral, view-once, edited,
/// document-with-caption) plus the future-proof wrappers whatsapp-rust's
/// helper does not know about (lottie stickers, group mentions, album
/// children, forwarded bot messages).
fn unwrap_message(msg: &wa::Message) -> &wa::Message {
    let mut current = msg.get_base_message();
    for _ in 0..4 {
        let inner = [
            &current.lottie_sticker_message,
            &current.group_mentioned_message,
            &current.associated_child_message,
            &current.bot_forwarded_message,
        ]
        .into_iter()
        .find_map(|w| w.as_option().and_then(|f| f.message.as_option()));
        match inner {
            Some(m) => current = m.get_base_message(),
            None => break,
        }
    }
    current
}

fn non_empty(s: &Option<String>) -> Option<String> {
    s.as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The renderable content and quoted reply of a (base) message, or `None` if
/// it has no content type we render.
pub fn content_of(m: &wa::Message) -> Option<(MessageContent, Option<QuotedMessage>)> {
    if let Some(text) = m.conversation.as_ref().filter(|t| !t.is_empty()) {
        return Some((MessageContent::Text(text.clone()), None));
    }
    if let Some(ext) = m.extended_text_message.as_option() {
        let text = ext.text.clone().unwrap_or_default();
        return Some((MessageContent::Text(text), quoted(&ext.context_info)));
    }
    if let Some(img) = m.image_message.as_option() {
        let token = media_token(wa::Message {
            image_message: MessageField::some(wa::message::ImageMessage {
                jpeg_thumbnail: None,
                context_info: MessageField::none(),
                ..img.clone()
            }),
            ..Default::default()
        });
        let media = MediaRef {
            mime: img
                .mimetype
                .clone()
                .unwrap_or_else(|| "image/jpeg".to_owned()),
            size: img.file_length.unwrap_or(0),
            thumbnail: img.jpeg_thumbnail.clone().filter(|t| !t.is_empty()),
            local_path: None,
            download_token: token,
        };
        let content = MessageContent::Image {
            caption: non_empty(&img.caption),
            media,
        };
        return Some((content, quoted(&img.context_info)));
    }
    if let Some(st) = m.sticker_message.as_option() {
        let token = media_token(wa::Message {
            sticker_message: MessageField::some(wa::message::StickerMessage {
                png_thumbnail: None,
                context_info: MessageField::none(),
                ..st.clone()
            }),
            ..Default::default()
        });
        let media = MediaRef {
            mime: st
                .mimetype
                .clone()
                .unwrap_or_else(|| "image/webp".to_owned()),
            size: st.file_length.unwrap_or(0),
            thumbnail: st.png_thumbnail.clone().filter(|t| !t.is_empty()),
            local_path: None,
            download_token: token,
        };
        return Some((MessageContent::Sticker { media }, quoted(&st.context_info)));
    }
    if let Some(au) = m.audio_message.as_option() {
        let token = media_token(wa::Message {
            audio_message: MessageField::some(wa::message::AudioMessage {
                context_info: MessageField::none(),
                waveform: None,
                streaming_sidecar: None,
                ..au.clone()
            }),
            ..Default::default()
        });
        let media = MediaRef {
            mime: au
                .mimetype
                .clone()
                .unwrap_or_else(|| "audio/ogg; codecs=opus".to_owned()),
            size: au.file_length.unwrap_or(0),
            thumbnail: None,
            local_path: None,
            download_token: token,
        };
        let content = MessageContent::Audio {
            voice_note: au.ptt.unwrap_or(false),
            duration_secs: au.seconds.unwrap_or(0),
            media,
        };
        return Some((content, quoted(&au.context_info)));
    }
    // Round "video notes" (ptv) are ordinary videos for our purposes.
    if let Some(v) = m.video_message.as_option().or(m.ptv_message.as_option()) {
        let token = media_token(wa::Message {
            video_message: MessageField::some(wa::message::VideoMessage {
                jpeg_thumbnail: None,
                context_info: MessageField::none(),
                streaming_sidecar: None,
                ..v.clone()
            }),
            ..Default::default()
        });
        let media = MediaRef {
            mime: v.mimetype.clone().unwrap_or_else(|| "video/mp4".to_owned()),
            size: v.file_length.unwrap_or(0),
            thumbnail: v.jpeg_thumbnail.clone().filter(|t| !t.is_empty()),
            local_path: None,
            download_token: token,
        };
        let content = MessageContent::Video {
            caption: non_empty(&v.caption),
            media,
        };
        return Some((content, quoted(&v.context_info)));
    }
    if let Some(d) = m.document_message.as_option() {
        let token = media_token(wa::Message {
            document_message: MessageField::some(wa::message::DocumentMessage {
                jpeg_thumbnail: None,
                context_info: MessageField::none(),
                ..d.clone()
            }),
            ..Default::default()
        });
        let media = MediaRef {
            mime: d
                .mimetype
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_owned()),
            size: d.file_length.unwrap_or(0),
            thumbnail: d.jpeg_thumbnail.clone().filter(|t| !t.is_empty()),
            local_path: None,
            download_token: token,
        };
        let file_name = non_empty(&d.file_name)
            .or_else(|| non_empty(&d.title))
            .unwrap_or_else(|| "document".to_owned());
        let content = MessageContent::Document {
            file_name,
            caption: non_empty(&d.caption),
            media,
        };
        return Some((content, quoted(&d.context_info)));
    }
    None
}

/// Names a content type we do not render, so it becomes a visible
/// placeholder instead of being dropped.
fn unsupported_kind(m: &wa::Message) -> Option<&'static str> {
    let kinds: [(bool, &'static str); 26] = [
        (m.poll_creation_message.is_set(), "poll"),
        (m.poll_creation_message_v2.is_set(), "poll"),
        (m.poll_creation_message_v3.is_set(), "poll"),
        (m.poll_creation_message_v4.is_set(), "poll"),
        (m.call_log_messsage.is_set(), "call"),
        (m.scheduled_call_creation_message.is_set(), "scheduled call"),
        (m.bcall_message.is_set(), "call"),
        (m.location_message.is_set(), "location"),
        (m.live_location_message.is_set(), "live location"),
        (m.contact_message.is_set(), "contact"),
        (m.contacts_array_message.is_set(), "contacts"),
        (m.call.is_set(), "call"),
        (m.group_invite_message.is_set(), "group invite"),
        (m.template_message.is_set(), "template"),
        (m.buttons_message.is_set(), "buttons"),
        (m.buttons_response_message.is_set(), "buttons response"),
        (m.list_message.is_set(), "list"),
        (m.list_response_message.is_set(), "list response"),
        (m.interactive_message.is_set(), "interactive"),
        (
            m.interactive_response_message.is_set(),
            "interactive response",
        ),
        (m.product_message.is_set(), "product"),
        (m.order_message.is_set(), "order"),
        (m.send_payment_message.is_set(), "payment"),
        (m.request_payment_message.is_set(), "payment request"),
        (m.event_message.is_set(), "event"),
        (m.sticker_pack_message.is_set(), "sticker pack"),
    ];
    if let Some((_, kind)) = kinds.iter().find(|(set, _)| *set) {
        return Some(kind);
    }
    // Anything left beyond pure plumbing (sender-key distribution, context
    // info) is a newer content type: show it as "unknown" rather than drop it.
    has_unknown_payload(m).then_some("unknown")
}

fn has_unknown_payload(m: &wa::Message) -> bool {
    let stripped = wa::Message {
        sender_key_distribution_message: MessageField::none(),
        fast_ratchet_key_sender_key_distribution_message: MessageField::none(),
        message_context_info: MessageField::none(),
        device_sent_message: MessageField::none(),
        ..m.clone()
    };
    codec::message_encoded_len(&stripped) > 0
}

fn quoted(ctx: &MessageField<wa::ContextInfo>) -> Option<QuotedMessage> {
    let ctx = ctx.as_option()?;
    let id = ctx.stanza_id.clone().filter(|s| !s.is_empty())?;
    let snippet = ctx
        .quoted_message
        .as_option()
        .map(|q| {
            let base = unwrap_message(q);
            match content_of(base) {
                Some((content, _)) => content.preview(),
                None => match unsupported_kind(base) {
                    Some(kind) => format!("[{kind}]"),
                    None => String::new(),
                },
            }
        })
        .unwrap_or_default();
    let sender = ctx
        .participant
        .as_deref()
        .and_then(|p| p.parse::<Jid>().ok())
        .map(|j| j.to_non_ad_string())
        .unwrap_or_default();
    Some(QuotedMessage {
        id: MessageId(id),
        sender,
        snippet,
    })
}

fn media_token(m: wa::Message) -> Vec<u8> {
    codec::message_to_vec(&m)
}

/// A decoded `download_token`, ready for `Client::download`.
pub enum MediaPayload {
    Image(wa::message::ImageMessage),
    Sticker(wa::message::StickerMessage),
    Audio(wa::message::AudioMessage),
    Video(wa::message::VideoMessage),
    Document(wa::message::DocumentMessage),
}

impl MediaPayload {
    pub fn file_sha256(&self) -> Option<&[u8]> {
        match self {
            Self::Image(m) => m.file_sha256.as_deref(),
            Self::Sticker(m) => m.file_sha256.as_deref(),
            Self::Audio(m) => m.file_sha256.as_deref(),
            Self::Video(m) => m.file_sha256.as_deref(),
            Self::Document(m) => m.file_sha256.as_deref(),
        }
    }

    pub fn file_name(&self) -> Option<&str> {
        match self {
            Self::Document(m) => m.file_name.as_deref(),
            _ => None,
        }
    }
}

pub fn decode_media_token(token: &[u8]) -> anyhow::Result<MediaPayload> {
    if token.is_empty() {
        anyhow::bail!("this message has no downloadable media");
    }
    let mut m = codec::message_decode(token)
        .map_err(|e| anyhow::anyhow!("invalid media reference: {e}"))?;
    if let Some(x) = m.image_message.take() {
        return Ok(MediaPayload::Image(x));
    }
    if let Some(x) = m.sticker_message.take() {
        return Ok(MediaPayload::Sticker(x));
    }
    if let Some(x) = m.audio_message.take() {
        return Ok(MediaPayload::Audio(x));
    }
    if let Some(x) = m.video_message.take() {
        return Ok(MediaPayload::Video(x));
    }
    if let Some(x) = m.document_message.take() {
        return Ok(MediaPayload::Document(x));
    }
    anyhow::bail!("media reference holds no media message")
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// File-system safe characters only.
fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if out.is_empty() {
        "media".to_owned()
    } else {
        out
    }
}

pub fn extension_for(mime: &str, file_name: Option<&str>) -> String {
    if let Some(ext) = file_name
        .and_then(|n| std::path::Path::new(n).extension())
        .and_then(|e| e.to_str())
        .map(sanitize)
        .filter(|e| !e.is_empty() && e.len() <= 10)
    {
        return ext.to_ascii_lowercase();
    }
    let essence = mime.split(';').next().unwrap_or_default().trim();
    match essence {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/mp4" | "audio/aac" | "audio/m4a" => "m4a",
        "video/mp4" => "mp4",
        "video/3gpp" => "3gp",
        "application/pdf" => "pdf",
        _ => "bin",
    }
    .to_owned()
}

/// `<sha256-hex-or-message-id>.<ext>` inside the media cache.
pub fn media_file_name(payload: &MediaPayload, mime: &str, message_id: &str) -> String {
    let stem = match payload.file_sha256() {
        Some(sha) if !sha.is_empty() => hex(sha),
        _ => sanitize(message_id),
    };
    format!("{stem}.{}", extension_for(mime, payload.file_name()))
}

/// Receipt type → outgoing message status (`None` = not a status change we track).
pub fn receipt_status(kind: &ReceiptType) -> Option<MessageStatus> {
    match kind {
        ReceiptType::Delivered => Some(MessageStatus::Delivered),
        ReceiptType::Sent => Some(MessageStatus::Sent),
        ReceiptType::Read | ReceiptType::Played => Some(MessageStatus::Read),
        _ => None,
    }
}

/// The status a receipt sets on our message. In groups a receipt comes from
/// one participant, and WhatsApp shows Delivered/Read only once *every*
/// recipient got there; we do not track participants, so group messages
/// stay at Sent rather than showing blue ticks after the first reader.
pub fn receipt_status_in(kind: &ReceiptType, group: bool) -> Option<MessageStatus> {
    match receipt_status(kind)? {
        MessageStatus::Delivered | MessageStatus::Read if group => None,
        status => Some(status),
    }
}

/// App-state timestamps arrive in seconds or milliseconds; returns ms.
pub fn sync_ts_to_ms(t: i64) -> i64 {
    if t.abs() >= 100_000_000_000 {
        t
    } else {
        secs_to_ms(t)
    }
}

/// The `ChatUpserted` payload for "read on another device up to `read_up_to`
/// (ms)": unread 0 at that activity. The store applies it only when no newer
/// message exists, and never moves `last_activity` back.
pub fn read_marker(mut snapshot: Chat, read_up_to: i64) -> Chat {
    snapshot.unread = 0;
    snapshot.last_activity = read_up_to;
    snapshot.last_preview = None;
    snapshot
}

/// History events in the order they must be applied: every message first,
/// then the chats. The store counts each new incoming message as unread, so
/// announcing the chat last lets the phone's own unread count win.
pub fn history_events(items: Vec<Converted>, chats: Vec<Chat>) -> Vec<BackendEvent> {
    let mut out = Vec::with_capacity(items.len() + chats.len());
    for item in items {
        match item {
            Converted::Message(m) => out.push(BackendEvent::Message(m)),
            Converted::Reaction(r) => out.push(BackendEvent::Reaction(r)),
            Converted::Ignored(_) => {}
        }
    }
    out.extend(chats.into_iter().map(BackendEvent::ChatUpserted));
    out
}

/// Our own estimate of whatsapp-rust's reconnect delay (Fibonacci from 1s,
/// capped at 900s) for the "Reconnecting in N s" banner. `attempt` is 1-based.
pub fn reconnect_delay_secs(attempt: u32) -> u64 {
    let (mut a, mut b) = (1u64, 1u64);
    for _ in 1..attempt.max(1) {
        let next = (a + b).min(900);
        a = b;
        b = next;
    }
    a.min(900)
}

/// Mute end timestamps arrive in seconds or milliseconds depending on the
/// source; negative means "forever".
pub fn muted_until(end: i64, now_ms: i64) -> bool {
    if end < 0 {
        return true;
    }
    let end_ms = if end < 100_000_000_000 {
        end.saturating_mul(1000)
    } else {
        end
    };
    end_ms > now_ms
}

pub fn new_chat(id: ChatId, name: Option<String>, muted: bool) -> Chat {
    let kind = if id.is_group() {
        ChatKind::Group
    } else {
        ChatKind::Direct
    };
    let name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| id.user().to_owned());
    Chat {
        id,
        kind,
        name,
        last_activity: 0,
        unread: 0,
        muted,
        last_preview: None,
    }
}

pub fn key_record(meta: &MsgMeta, msg: &wa::Message, raw_sender: &str) -> KeyRecord {
    let quote = msg.get_base_message().prepare_for_quote();
    let bytes = codec::message_to_vec(&quote);
    KeyRecord {
        chat: meta.chat.clone(),
        id: meta.id.clone(),
        sender: raw_sender.to_owned(),
        from_me: meta.from_me,
        timestamp: meta.timestamp,
        quote: (bytes.len() <= MAX_QUOTE_BYTES).then_some(bytes),
    }
}

/// Name rank: higher wins. Contact book and group subjects beat chat names
/// from history, which beat push names.
pub const RANK_PUSH: i64 = 1;
pub const RANK_CONVERSATION: i64 = 2;
pub const RANK_CONTACT: i64 = 3;

/// Everything extracted from one history sync blob.
#[derive(Debug, Default)]
pub struct HistoryBatch {
    /// Chats with their unread/last-activity/mute state from the phone.
    pub chats: Vec<(Chat, Option<String>)>,
    /// `(model event, key record)` in chronological order per chat.
    pub items: Vec<(Converted, Option<KeyRecord>)>,
    /// `(chat id, push name)` pairs.
    pub push_names: Vec<(ChatId, String)>,
    /// lid user → pn user mappings learnt from this blob.
    pub lids: Vec<(String, String)>,
}

fn web_status(s: Option<wa::web_message_info::Status>, from_me: bool) -> MessageStatus {
    use wa::web_message_info::Status;
    if !from_me {
        return MessageStatus::Received;
    }
    match s {
        Some(Status::ERROR) => MessageStatus::Failed,
        Some(Status::PENDING) => MessageStatus::Pending,
        Some(Status::DELIVERY_ACK) => MessageStatus::Delivered,
        Some(Status::READ) | Some(Status::PLAYED) => MessageStatus::Read,
        _ => MessageStatus::Sent,
    }
}

fn jid_user(jid: &str) -> Option<String> {
    jid.parse::<Jid>().ok().map(|j| j.user_base().to_owned())
}

/// Maps a decoded history sync blob. `lids` should already contain the
/// mappings of earlier blobs; this blob's own mappings are applied too.
pub fn convert_history(
    hs: &wa::HistorySync,
    own_jid: &str,
    lids: &LidMap,
    now_ms: i64,
) -> HistoryBatch {
    let mut out = HistoryBatch::default();
    let mut lids = lids.clone();
    for m in &hs.phone_number_to_lid_mappings {
        if let (Some(pn), Some(lid)) = (m.pn_jid.as_deref(), m.lid_jid.as_deref())
            && let (Some(pn), Some(lid)) = (jid_user(pn), jid_user(lid))
        {
            out.lids.push((lid.clone(), pn.clone()));
            lids.insert(lid, pn);
        }
    }

    for p in &hs.pushnames {
        if let (Some(id), Some(name)) = (p.id.as_deref(), non_empty(&p.pushname))
            && let Some(chat) = chat_id_from_str(id, &lids)
        {
            out.push_names.push((chat, name));
        }
    }

    for conv in &hs.conversations {
        let raw_id = conv
            .pn_jid
            .as_deref()
            .filter(|p| !p.is_empty() && !conv.id.ends_with("@g.us"))
            .unwrap_or(&conv.id);
        let Some(chat_jid) = raw_id.parse::<Jid>().ok() else {
            continue;
        };
        if !is_supported_chat(&chat_jid) {
            continue;
        }
        let chat_id = chat_id_for(&chat_jid, None, &lids);
        let name = non_empty(&conv.name).or_else(|| non_empty(&conv.display_name));
        let muted = conv
            .mute_end_time
            .is_some_and(|t| t != 0 && muted_until(t as i64, now_ms));
        let mut chat = new_chat(chat_id.clone(), name.clone(), muted);
        chat.unread = conv.unread_count.unwrap_or(0);
        chat.last_activity = conv
            .conversation_timestamp
            .or(conv.last_msg_timestamp)
            .map(|t| secs_to_ms(t as i64))
            .unwrap_or(0);

        let mut items = Vec::new();
        for hm in &conv.messages {
            let Some(wmi) = hm.message.as_option() else {
                continue;
            };
            let Some(key) = wmi.key.as_option() else {
                continue;
            };
            let Some(id) = key.id.clone() else { continue };
            let Some(msg) = wmi.message.as_option() else {
                continue; // system stub (group created, ...)
            };
            let from_me = key.from_me.unwrap_or(false);
            let raw_sender = if from_me {
                own_jid.to_owned()
            } else if chat_id.is_group() {
                key.participant
                    .as_deref()
                    .or(wmi.participant.as_deref())
                    .and_then(|p| p.parse::<Jid>().ok())
                    .map(|j| j.to_non_ad_string())
                    .unwrap_or_default()
            } else {
                key.remote_jid
                    .as_deref()
                    .and_then(|p| p.parse::<Jid>().ok())
                    .map(|j| j.to_non_ad_string())
                    .unwrap_or_else(|| chat_id.0.clone())
            };
            let meta = MsgMeta {
                id: id.clone(),
                chat: chat_id.clone(),
                sender: raw_sender.clone(),
                sender_name: if from_me {
                    None
                } else {
                    non_empty(&wmi.push_name)
                },
                from_me,
                timestamp: secs_to_ms(wmi.message_timestamp.unwrap_or(0) as i64),
                status: web_status(wmi.status, from_me),
            };
            let record = key_record(&meta, msg, &raw_sender);
            let ts = meta.timestamp;
            let converted = convert_message(meta, msg);
            let keep_record = matches!(converted, Converted::Message(_));
            items.push((converted, keep_record.then_some(record)));

            for r in &wmi.reactions {
                let Some(rk) = r.key.as_option() else {
                    continue;
                };
                let sender = if rk.from_me.unwrap_or(false) {
                    OWN_REACTION_SENDER.to_owned()
                } else {
                    rk.participant
                        .as_deref()
                        .or(rk.remote_jid.as_deref())
                        .and_then(|p| p.parse::<Jid>().ok())
                        .map(|j| j.to_non_ad_string())
                        .unwrap_or_default()
                };
                items.push((
                    Converted::Reaction(Reaction {
                        chat: chat_id.clone(),
                        target: MessageId(id.clone()),
                        sender,
                        emoji: r.text.clone().unwrap_or_default(),
                        timestamp: r.sender_timestamp_ms.unwrap_or(ts),
                    }),
                    None,
                ));
            }
        }
        // History lists newest first; deliver oldest first.
        items.sort_by_key(|(c, _)| match c {
            Converted::Message(m) => m.timestamp,
            Converted::Reaction(r) => r.timestamp,
            Converted::Ignored(_) => i64::MAX,
        });
        // The chat is announced *after* its messages (see `history_events`),
        // so its activity must cover them for the phone's unread count to win.
        let newest = items
            .iter()
            .filter_map(|(c, _)| match c {
                Converted::Message(m) => Some(m.timestamp),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        chat.last_activity = chat.last_activity.max(newest);
        out.chats.push((chat, name));
        out.items.extend(items);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatsapp_rust::chrono::{TimeZone, Utc};
    use whatsapp_rust::prelude::MessageBuilderExt;

    fn jid(s: &str) -> Jid {
        s.parse().expect("valid jid")
    }

    fn meta(chat: &str) -> MsgMeta {
        MsgMeta {
            id: "ABC".into(),
            chat: ChatId(chat.into()),
            sender: "15550001111@s.whatsapp.net".into(),
            sender_name: Some("Alice".into()),
            from_me: false,
            timestamp: 1_700_000_000_000,
            status: MessageStatus::Received,
        }
    }

    fn as_message(c: Converted) -> Message {
        match c {
            Converted::Message(m) => m,
            other => panic!("expected message, got {other:?}"),
        }
    }

    #[test]
    fn plain_conversation_is_text() {
        let m = as_message(convert_message(
            meta("1@s.whatsapp.net"),
            &wa::Message::text("hello"),
        ));
        assert_eq!(m.content, MessageContent::Text("hello".into()));
        assert_eq!(m.reply_to, None);
        assert_eq!(m.id, MessageId("ABC".into()));
        assert_eq!(m.sender_name.as_deref(), Some("Alice"));
        assert_eq!(m.status, MessageStatus::Received);
    }

    #[test]
    fn extended_text_with_context_has_reply() {
        let ctx = wa::ContextInfo {
            stanza_id: Some("QUOTED1".into()),
            participant: Some("15552223333:4@s.whatsapp.net".into()),
            quoted_message: MessageField::some(wa::Message::text("original line\nsecond")),
            ..Default::default()
        };
        let msg = wa::Message::text_with_context("my answer", ctx);
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &msg));
        assert_eq!(m.content, MessageContent::Text("my answer".into()));
        assert_eq!(
            m.reply_to,
            Some(QuotedMessage {
                id: MessageId("QUOTED1".into()),
                sender: "15552223333@s.whatsapp.net".into(),
                snippet: "original line".into(),
            })
        );
    }

    #[test]
    fn image_maps_caption_media_and_token() {
        let msg = wa::Message {
            image_message: MessageField::some(wa::message::ImageMessage {
                mimetype: Some("image/jpeg".into()),
                caption: Some("sunset".into()),
                file_length: Some(12345),
                jpeg_thumbnail: Some(vec![0xFF, 0xD8, 1, 2]),
                direct_path: Some("/v/t62/abc".into()),
                media_key: Some(vec![7; 32]),
                file_sha256: Some(vec![1; 32]),
                file_enc_sha256: Some(vec![2; 32]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &msg));
        let MessageContent::Image { caption, media } = m.content else {
            panic!("expected image");
        };
        assert_eq!(caption.as_deref(), Some("sunset"));
        assert_eq!(media.mime, "image/jpeg");
        assert_eq!(media.size, 12345);
        assert_eq!(media.thumbnail, Some(vec![0xFF, 0xD8, 1, 2]));
        assert!(!media.download_token.is_empty());
        // The token decodes back to the downloadable fields (minus the thumbnail).
        let MediaPayload::Image(img) = decode_media_token(&media.download_token).expect("decodes")
        else {
            panic!("expected image payload");
        };
        assert_eq!(img.direct_path.as_deref(), Some("/v/t62/abc"));
        assert_eq!(img.media_key, Some(vec![7; 32]));
        assert_eq!(img.jpeg_thumbnail, None);
    }

    #[test]
    fn sticker_maps() {
        let msg = wa::Message {
            sticker_message: MessageField::some(wa::message::StickerMessage {
                mimetype: Some("image/webp".into()),
                file_length: Some(999),
                direct_path: Some("/s".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &msg));
        let MessageContent::Sticker { media } = m.content else {
            panic!("expected sticker")
        };
        assert_eq!(media.mime, "image/webp");
        assert_eq!(media.size, 999);
        assert!(matches!(
            decode_media_token(&media.download_token),
            Ok(MediaPayload::Sticker(_))
        ));
    }

    #[test]
    fn voice_note_maps_ptt_and_duration() {
        let msg = wa::Message {
            audio_message: MessageField::some(wa::message::AudioMessage {
                mimetype: Some("audio/ogg; codecs=opus".into()),
                ptt: Some(true),
                seconds: Some(17),
                file_length: Some(4000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &msg));
        let MessageContent::Audio {
            voice_note,
            duration_secs,
            media,
        } = m.content
        else {
            panic!("expected audio");
        };
        assert!(voice_note);
        assert_eq!(duration_secs, 17);
        assert_eq!(media.size, 4000);
        assert!(!media.download_token.is_empty());
    }

    #[test]
    fn document_maps_file_name() {
        let msg = wa::Message {
            document_message: MessageField::some(wa::message::DocumentMessage {
                mimetype: Some("application/pdf".into()),
                file_name: Some("report.pdf".into()),
                caption: Some("Q3".into()),
                file_length: Some(1 << 20),
                ..Default::default()
            }),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &msg));
        let MessageContent::Document {
            file_name,
            caption,
            media,
        } = m.content
        else {
            panic!("expected document");
        };
        assert_eq!(file_name, "report.pdf");
        assert_eq!(caption.as_deref(), Some("Q3"));
        assert_eq!(media.mime, "application/pdf");
    }

    #[test]
    fn video_maps() {
        let msg = wa::Message {
            video_message: MessageField::some(wa::message::VideoMessage {
                mimetype: Some("video/mp4".into()),
                caption: Some("clip".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &msg));
        assert!(
            matches!(m.content, MessageContent::Video { caption: Some(ref c), .. } if c == "clip")
        );
    }

    #[test]
    fn reaction_message_is_reaction() {
        let msg = wa::Message {
            reaction_message: MessageField::some(wa::message::ReactionMessage {
                key: MessageField::some(wa::MessageKey {
                    remote_jid: Some("1@s.whatsapp.net".into()),
                    from_me: Some(true),
                    id: Some("TARGET".into()),
                    participant: None,
                }),
                text: Some("👍".into()),
                sender_timestamp_ms: Some(1_700_000_000_500),
                ..Default::default()
            }),
            ..Default::default()
        };
        let got = convert_message(meta("1@s.whatsapp.net"), &msg);
        assert_eq!(
            got,
            Converted::Reaction(Reaction {
                chat: ChatId("1@s.whatsapp.net".into()),
                target: MessageId("TARGET".into()),
                sender: "15550001111@s.whatsapp.net".into(),
                emoji: "👍".into(),
                timestamp: 1_700_000_000_500,
            })
        );
    }

    #[test]
    fn removed_reaction_has_empty_emoji() {
        let msg = wa::Message {
            reaction_message: MessageField::some(wa::message::ReactionMessage {
                key: MessageField::some(wa::MessageKey {
                    id: Some("T".into()),
                    ..Default::default()
                }),
                text: Some(String::new()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let Converted::Reaction(r) = convert_message(meta("1@s.whatsapp.net"), &msg) else {
            panic!("expected reaction");
        };
        assert_eq!(r.emoji, "");
        assert_eq!(r.timestamp, 1_700_000_000_000);
    }

    #[test]
    fn own_reaction_uses_me_sender() {
        let msg = wa::Message {
            reaction_message: MessageField::some(wa::message::ReactionMessage {
                key: MessageField::some(wa::MessageKey {
                    id: Some("T".into()),
                    ..Default::default()
                }),
                text: Some("👍".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let own = MsgMeta {
            from_me: true,
            ..meta("1@s.whatsapp.net")
        };
        let Converted::Reaction(r) = convert_message(own, &msg) else {
            panic!("expected reaction");
        };
        assert_eq!(r.sender, OWN_REACTION_SENDER);
        let Converted::Reaction(r) = convert_message(meta("1@s.whatsapp.net"), &msg) else {
            panic!("expected reaction");
        };
        assert_eq!(r.sender, "15550001111@s.whatsapp.net");
    }

    #[test]
    fn poll_is_unsupported() {
        let msg = wa::Message {
            poll_creation_message: MessageField::some(Default::default()),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &msg));
        assert_eq!(
            m.content,
            MessageContent::Unsupported {
                kind: "poll".into()
            }
        );
    }

    #[test]
    fn location_is_unsupported() {
        let msg = wa::Message {
            location_message: MessageField::some(Default::default()),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &msg));
        assert_eq!(
            m.content,
            MessageContent::Unsupported {
                kind: "location".into()
            }
        );
    }

    #[test]
    fn unknown_content_is_unsupported_not_dropped() {
        let msg = wa::Message {
            scheduled_call_edit_message: MessageField::some(Default::default()),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &msg));
        assert_eq!(
            m.content,
            MessageContent::Unsupported {
                kind: "unknown".into()
            }
        );
    }

    #[test]
    fn video_note_and_wrapped_sticker_map() {
        let ptv = wa::Message {
            ptv_message: MessageField::some(wa::message::VideoMessage {
                mimetype: Some("video/mp4".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &ptv));
        assert!(matches!(m.content, MessageContent::Video { .. }));

        let lottie = wa::Message {
            lottie_sticker_message: MessageField::some(wa::message::FutureProofMessage {
                message: MessageField::some(wa::Message {
                    sticker_message: MessageField::some(Default::default()),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &lottie));
        assert!(matches!(m.content, MessageContent::Sticker { .. }));
    }

    #[test]
    fn empty_and_plumbing_messages_are_ignored() {
        assert!(matches!(
            convert_message(meta("1@s.whatsapp.net"), &wa::Message::default()),
            Converted::Ignored(_)
        ));
        let skdm = wa::Message {
            sender_key_distribution_message: MessageField::some(Default::default()),
            ..Default::default()
        };
        assert!(matches!(
            convert_message(meta("1@s.whatsapp.net"), &skdm),
            Converted::Ignored(_)
        ));
        let proto = wa::Message {
            protocol_message: MessageField::some(wa::message::ProtocolMessage {
                r#type: Some(wa::message::protocol_message::Type::APP_STATE_SYNC_KEY_SHARE),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(matches!(
            convert_message(meta("1@s.whatsapp.net"), &proto),
            Converted::Ignored(_)
        ));
    }

    #[test]
    fn ephemeral_wrapper_is_unwrapped() {
        let msg = wa::Message {
            ephemeral_message: MessageField::some(wa::message::FutureProofMessage {
                message: MessageField::some(wa::Message::text("secret")),
            }),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &msg));
        assert_eq!(m.content, MessageContent::Text("secret".into()));
    }

    #[test]
    fn revoke_and_edit_target_original_id() {
        use wa::message::protocol_message::Type;
        let key = wa::MessageKey {
            id: Some("ORIG".into()),
            ..Default::default()
        };
        let revoke = wa::Message {
            protocol_message: MessageField::some(wa::message::ProtocolMessage {
                key: MessageField::some(key.clone()),
                r#type: Some(Type::REVOKE),
                ..Default::default()
            }),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &revoke));
        assert_eq!(m.id, MessageId("ORIG".into()));
        assert!(matches!(m.content, MessageContent::Text(ref t) if t.contains("deleted")));

        let edit = wa::Message {
            protocol_message: MessageField::some(wa::message::ProtocolMessage {
                key: MessageField::some(key),
                r#type: Some(Type::MESSAGE_EDIT),
                edited_message: MessageField::some(wa::Message::text("fixed")),
                ..Default::default()
            }),
            ..Default::default()
        };
        let m = as_message(convert_message(meta("1@s.whatsapp.net"), &edit));
        assert_eq!(m.id, MessageId("ORIG".into()));
        assert_eq!(m.content, MessageContent::Text("fixed".into()));
    }

    fn info(chat: &str, sender: &str, from_me: bool, is_group: bool) -> MessageInfo {
        MessageInfo {
            source: MessageSource {
                chat: jid(chat),
                sender: jid(sender),
                is_from_me: from_me,
                is_group,
                ..Default::default()
            },
            id: "MID".into(),
            push_name: "Bob".into(),
            timestamp: Utc.timestamp_opt(1_700_000_123, 0).single().expect("ts"),
            ..Default::default()
        }
    }

    #[test]
    fn timestamps_are_unix_ms() {
        let m = meta_from_info(
            &info(
                "15551234567@s.whatsapp.net",
                "15551234567@s.whatsapp.net",
                false,
                false,
            ),
            "",
            &LidMap::new(),
        );
        assert_eq!(m.timestamp, 1_700_000_123_000);
        assert_eq!(secs_to_ms(5), 5000);
    }

    #[test]
    fn group_chat_id_comes_from_chat_not_participant() {
        let m = meta_from_info(
            &info(
                "120363000000000001@g.us",
                "15550001111:3@s.whatsapp.net",
                false,
                true,
            ),
            "",
            &LidMap::new(),
        );
        assert_eq!(m.chat, ChatId("120363000000000001@g.us".into()));
        assert_eq!(m.sender, "15550001111@s.whatsapp.net");
        assert_eq!(m.sender_name.as_deref(), Some("Bob"));
        assert!(m.chat.is_group());
    }

    #[test]
    fn direct_chat_id_strips_device() {
        let m = meta_from_info(
            &info(
                "15551234567:7@s.whatsapp.net",
                "15551234567:7@s.whatsapp.net",
                false,
                false,
            ),
            "",
            &LidMap::new(),
        );
        assert_eq!(m.chat, ChatId("15551234567@s.whatsapp.net".into()));
        assert!(!m.chat.is_group());
    }

    #[test]
    fn from_me_uses_own_jid_and_sent_status() {
        let m = meta_from_info(
            &info(
                "15551234567@s.whatsapp.net",
                "15559999999:2@s.whatsapp.net",
                true,
                false,
            ),
            "15559999999@s.whatsapp.net",
            &LidMap::new(),
        );
        assert!(m.from_me);
        assert_eq!(m.sender, "15559999999@s.whatsapp.net");
        assert_eq!(m.sender_name, None);
        assert_eq!(m.status, MessageStatus::Sent);
    }

    #[test]
    fn lid_direct_chat_maps_to_phone_number() {
        let mut i = info("100000012345678@lid", "100000012345678@lid", false, false);
        i.source.sender_alt = Some(jid("15551234567@s.whatsapp.net"));
        let m = meta_from_info(&i, "", &LidMap::new());
        assert_eq!(m.chat, ChatId("15551234567@s.whatsapp.net".into()));

        let mut lids = LidMap::new();
        lids.insert("100000012345678".into(), "15557654321".into());
        let i = info("100000012345678@lid", "100000012345678@lid", false, false);
        assert_eq!(
            meta_from_info(&i, "", &lids).chat,
            ChatId("15557654321@s.whatsapp.net".into())
        );
        // Unknown LID stays as-is.
        let m = meta_from_info(&i, "", &LidMap::new());
        assert_eq!(m.chat, ChatId("100000012345678@lid".into()));
    }

    #[test]
    fn receipt_types_map_to_status() {
        assert_eq!(
            receipt_status(&ReceiptType::Delivered),
            Some(MessageStatus::Delivered)
        );
        assert_eq!(
            receipt_status(&ReceiptType::Read),
            Some(MessageStatus::Read)
        );
        assert_eq!(
            receipt_status(&ReceiptType::Played),
            Some(MessageStatus::Read)
        );
        assert_eq!(
            receipt_status(&ReceiptType::Sent),
            Some(MessageStatus::Sent)
        );
        assert_eq!(receipt_status(&ReceiptType::ReadSelf), None);
        assert_eq!(receipt_status(&ReceiptType::Retry), None);
    }

    #[test]
    fn reconnect_delays_follow_fibonacci_with_cap() {
        let got: Vec<u64> = (1..=7).map(reconnect_delay_secs).collect();
        assert_eq!(got, vec![1, 1, 2, 3, 5, 8, 13]);
        assert_eq!(reconnect_delay_secs(0), 1);
        assert_eq!(reconnect_delay_secs(100), 900);
    }

    #[test]
    fn mute_timestamps() {
        let now = 1_700_000_000_000;
        assert!(muted_until(-1, now));
        assert!(muted_until(1_800_000_000, now)); // seconds, future
        assert!(!muted_until(1_600_000_000, now)); // seconds, past
        assert!(muted_until(1_800_000_000_000, now)); // ms, future
        assert!(!muted_until(0, now));
    }

    #[test]
    fn media_file_names() {
        let payload = MediaPayload::Image(wa::message::ImageMessage {
            file_sha256: Some(vec![0xAB, 0x01]),
            ..Default::default()
        });
        assert_eq!(media_file_name(&payload, "image/jpeg", "X"), "ab01.jpg");
        let doc = MediaPayload::Document(wa::message::DocumentMessage {
            file_name: Some("Report.PDF".into()),
            ..Default::default()
        });
        assert_eq!(
            media_file_name(&doc, "application/pdf", "id/../1"),
            "id____1.pdf"
        );
        assert_eq!(extension_for("audio/ogg; codecs=opus", None), "ogg");
        assert_eq!(extension_for("weird/type", None), "bin");
    }

    #[test]
    fn bad_media_tokens_are_errors() {
        assert!(decode_media_token(&[]).is_err());
        assert!(decode_media_token(&[0xFF, 0xFF, 0xFF]).is_err());
        let text_only = codec::message_to_vec(&wa::Message::text("x"));
        assert!(decode_media_token(&text_only).is_err());
    }

    #[test]
    fn key_record_keeps_quote_payload() {
        let rec = key_record(
            &meta("1@s.whatsapp.net"),
            &wa::Message::text("hi"),
            "1@s.whatsapp.net",
        );
        assert_eq!(rec.id, "ABC");
        let quote = rec.quote.expect("small message keeps quote");
        let decoded = codec::message_decode(&quote).expect("decodes");
        assert_eq!(decoded.conversation.as_deref(), Some("hi"));
    }

    fn wmi(
        id: &str,
        from_me: bool,
        ts: u64,
        msg: wa::Message,
        participant: Option<&str>,
    ) -> wa::HistorySyncMsg {
        wa::HistorySyncMsg {
            message: MessageField::some(wa::WebMessageInfo {
                key: MessageField::some(wa::MessageKey {
                    remote_jid: Some("ignored@s.whatsapp.net".into()),
                    from_me: Some(from_me),
                    id: Some(id.into()),
                    participant: participant.map(str::to_owned),
                }),
                message: MessageField::some(msg),
                message_timestamp: Some(ts),
                status: Some(wa::web_message_info::Status::READ),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn history_sync_maps_chats_and_messages() {
        let hs = wa::HistorySync {
            conversations: vec![
                wa::Conversation {
                    id: "120363000000000001@g.us".into(),
                    name: Some("Climbing".into()),
                    unread_count: Some(2),
                    conversation_timestamp: Some(1_700_000_200),
                    mute_end_time: Some(u64::MAX >> 1),
                    messages: vec![
                        wmi(
                            "G2",
                            false,
                            1_700_000_200,
                            wa::Message::text("second"),
                            Some("15550001111@s.whatsapp.net"),
                        ),
                        wmi("G1", true, 1_700_000_100, wa::Message::text("first"), None),
                    ],
                    ..Default::default()
                },
                wa::Conversation {
                    id: "100000012345678@lid".into(),
                    pn_jid: Some("15551234567@s.whatsapp.net".into()),
                    ..Default::default()
                },
                wa::Conversation {
                    id: "status@broadcast".into(),
                    ..Default::default()
                },
            ],
            pushnames: vec![wa::Pushname {
                id: Some("15550001111@s.whatsapp.net".into()),
                pushname: Some("Alice".into()),
            }],
            ..Default::default()
        };
        let batch = convert_history(
            &hs,
            "15559999999@s.whatsapp.net",
            &LidMap::new(),
            1_700_000_000_000,
        );
        assert_eq!(batch.chats.len(), 2);
        let (group, name) = &batch.chats[0];
        assert_eq!(group.id, ChatId("120363000000000001@g.us".into()));
        assert_eq!(group.kind, ChatKind::Group);
        assert_eq!(group.name, "Climbing");
        assert_eq!(name.as_deref(), Some("Climbing"));
        assert_eq!(group.unread, 2);
        assert_eq!(group.last_activity, 1_700_000_200_000);
        assert!(group.muted);
        assert_eq!(
            batch.chats[1].0.id,
            ChatId("15551234567@s.whatsapp.net".into())
        );
        assert_eq!(batch.chats[1].0.name, "15551234567");

        let msgs: Vec<&Message> = batch
            .items
            .iter()
            .filter_map(|(c, _)| match c {
                Converted::Message(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].id, MessageId("G1".into()));
        assert!(msgs[0].from_me);
        assert_eq!(msgs[0].sender, "15559999999@s.whatsapp.net");
        assert_eq!(msgs[0].status, MessageStatus::Read);
        assert_eq!(msgs[1].sender, "15550001111@s.whatsapp.net");
        assert_eq!(msgs[1].status, MessageStatus::Received);
        assert_eq!(msgs[1].timestamp, 1_700_000_200_000);
        assert!(batch.items.iter().all(|(_, rec)| rec.is_some()));
        assert_eq!(
            batch.push_names,
            vec![(ChatId("15550001111@s.whatsapp.net".into()), "Alice".into())]
        );
    }

    #[test]
    fn group_receipts_never_upgrade_past_sent() {
        assert_eq!(receipt_status_in(&ReceiptType::Read, true), None);
        assert_eq!(receipt_status_in(&ReceiptType::Delivered, true), None);
        assert_eq!(
            receipt_status_in(&ReceiptType::Sent, true),
            Some(MessageStatus::Sent)
        );
        assert_eq!(
            receipt_status_in(&ReceiptType::Read, false),
            Some(MessageStatus::Read)
        );
        assert_eq!(receipt_status_in(&ReceiptType::ReadSelf, false), None);
    }

    #[test]
    fn sync_timestamps_normalise_to_ms() {
        assert_eq!(sync_ts_to_ms(1_700_000_000), 1_700_000_000_000);
        assert_eq!(sync_ts_to_ms(1_700_000_000_123), 1_700_000_000_123);
    }

    #[test]
    fn history_sync_does_not_double_count_unread() {
        let chat_id = ChatId("15550001111@s.whatsapp.net".into());
        let hs = wa::HistorySync {
            conversations: vec![wa::Conversation {
                id: chat_id.0.clone(),
                unread_count: Some(1),
                // Older than the newest message: activity must still cover it.
                conversation_timestamp: Some(1_700_000_000),
                messages: (0..40)
                    .map(|i| {
                        wmi(
                            &format!("M{i}"),
                            false,
                            1_700_000_000 + i,
                            wa::Message::text("hi"),
                            None,
                        )
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let batch = convert_history(&hs, "15559999999@s.whatsapp.net", &LidMap::new(), 0);
        assert_eq!(batch.chats[0].0.last_activity, 1_700_000_039_000);
        let items = batch.items.into_iter().map(|(c, _)| c).collect();
        let chats = batch.chats.into_iter().map(|(c, _)| c).collect();
        let store = crate::store::Store::open_in_memory().expect("store");
        for ev in history_events(items, chats) {
            match ev {
                BackendEvent::Message(m) => {
                    store.upsert_message(&m).expect("msg");
                }
                BackendEvent::ChatUpserted(c) => store.upsert_chat(&c).expect("chat"),
                _ => {}
            }
        }
        let chat = store.chat(&chat_id).expect("read").expect("chat");
        assert_eq!(chat.unread, 1, "the phone's unread count wins");
        assert_eq!(store.messages(&chat_id, None, 100).expect("msgs").len(), 40);

        // Then the user reads it on the phone.
        store
            .upsert_chat(&read_marker(chat.clone(), 1_700_000_039_000))
            .expect("read marker");
        let after = store.chat(&chat_id).expect("read").expect("chat");
        assert_eq!(after.unread, 0);
        assert_eq!(after.last_activity, chat.last_activity);
        assert_eq!(after.last_preview, chat.last_preview);
    }

    #[test]
    fn new_chat_falls_back_to_user_part() {
        let c = new_chat(
            ChatId("15551234567@s.whatsapp.net".into()),
            Some("  ".into()),
            false,
        );
        assert_eq!(c.name, "15551234567");
        assert_eq!(c.kind, ChatKind::Direct);
    }
}
