//! Scripted backend for tests and `whatsapp-cosmic --fake`.
//!
//! CONTRACT (implemented in Task 2).

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;

use crate::backend::WhatsAppBackend;
use crate::events::{BackendCommand, BackendEvent, ConnectionState};
use crate::model::{
    Chat, ChatId, ChatKind, MediaRef, Message, MessageContent, MessageId, MessageStatus,
    QuotedMessage, Reaction,
};

/// A 320×240 sunset JPEG, also used as the inline thumbnail of demo images.
pub const SAMPLE_JPEG: &[u8] = include_bytes!("../../assets/sample.jpg");
/// A 160×160 transparent WebP smiley, served for demo stickers.
pub const SAMPLE_STICKER_WEBP: &[u8] = include_bytes!("../../assets/sticker.webp");
/// A 3 second, quiet 440 Hz OGG/Opus clip (mono, 48 kHz), served for demo voice notes.
pub const SAMPLE_OGG_OPUS: &[u8] = include_bytes!("../../assets/sample.ogg");
/// A one-page PDF, served for demo documents.
pub const SAMPLE_PDF: &[u8] = include_bytes!("../../assets/sample.pdf");
/// Two 160×160 JPEG profile pictures, served for some demo chats.
pub const SAMPLE_AVATARS: [&[u8]; 2] = [
    include_bytes!("../../assets/avatar-1.jpg"),
    include_bytes!("../../assets/avatar-2.jpg"),
];

/// `download_token` prefix understood by [`FakeBackend`]'s `FetchMedia`.
const TOKEN_PREFIX: &str = "fake-asset:";

/// Sender used for reactions we make ourselves (echoed back for `React`).
pub const OWN_REACTION_SENDER: &str = "me";

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
        Self {
            script: demo_script(now_ms()),
            received: None,
            auto_reply: true,
        }
    }
}

#[async_trait::async_trait]
impl WhatsAppBackend for FakeBackend {
    async fn run(
        self: Box<Self>,
        mut commands: mpsc::Receiver<BackendCommand>,
        events: mpsc::Sender<BackendEvent>,
    ) -> anyhow::Result<()> {
        let Self {
            script,
            received,
            auto_reply,
        } = *self;
        for event in script {
            if events.send(event).await.is_err() {
                return Ok(());
            }
            tokio::time::sleep(Duration::ZERO).await;
        }

        let mut sent = 0u64;
        while let Some(command) = commands.recv().await {
            if let Some(received) = &received {
                let _ = received.send(command.clone());
            }
            match command {
                BackendCommand::Shutdown => return Ok(()),
                BackendCommand::Logout => {
                    if events.send(BackendEvent::LoggedOut).await.is_err() {
                        return Ok(());
                    }
                }
                _ if !auto_reply => {}
                BackendCommand::SendText { chat, local_id, .. } => {
                    sent += 1;
                    let id = MessageId(format!("fake-{sent}"));
                    let ack = BackendEvent::MessageSent {
                        chat: chat.clone(),
                        local_id,
                        id: id.clone(),
                    };
                    if events.send(ack).await.is_err() {
                        return Ok(());
                    }
                    let events = events.clone();
                    tokio::spawn(async move {
                        for (delay, status) in
                            [(300, MessageStatus::Delivered), (700, MessageStatus::Read)]
                        {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            let event = BackendEvent::MessageStatus {
                                chat: chat.clone(),
                                message: id.clone(),
                                status,
                            };
                            if events.send(event).await.is_err() {
                                return;
                            }
                        }
                    });
                }
                BackendCommand::React {
                    chat,
                    target,
                    emoji,
                } => {
                    let echo = BackendEvent::Reaction(Reaction {
                        chat,
                        target,
                        sender: OWN_REACTION_SENDER.to_owned(),
                        emoji,
                        timestamp: now_ms(),
                    });
                    if events.send(echo).await.is_err() {
                        return Ok(());
                    }
                }
                BackendCommand::FetchMedia {
                    chat,
                    message,
                    media,
                } => {
                    let events = events.clone();
                    tokio::spawn(async move {
                        let event = match fetch_sample(&message, &media).await {
                            Ok(path) => BackendEvent::MediaReady {
                                chat,
                                message,
                                path,
                            },
                            Err(e) => BackendEvent::MediaFailed {
                                chat,
                                message,
                                error: e.to_string(),
                            },
                        };
                        let _ = events.send(event).await;
                    });
                }
                BackendCommand::FetchAvatar { chat } => {
                    let events = events.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        let event = match fetch_avatar(&chat).await {
                            Ok(Some(path)) => BackendEvent::AvatarReady { chat, path },
                            Ok(None) => BackendEvent::AvatarMissing { chat },
                            Err(e) => {
                                tracing::warn!("demo avatar failed: {e:#}");
                                BackendEvent::AvatarMissing { chat }
                            }
                        };
                        let _ = events.send(event).await;
                    });
                }
                BackendCommand::MarkRead { .. } => {}
            }
        }
        Ok(())
    }
}

/// Demo chats whose JID ends in an odd digit have a picture; the rest do not.
async fn fetch_avatar(chat: &ChatId) -> anyhow::Result<Option<PathBuf>> {
    let last = chat.user().bytes().last().unwrap_or(b'0');
    if !last.is_ascii_digit() || last.is_multiple_of(2) {
        return Ok(None);
    }
    let bytes = SAMPLE_AVATARS[usize::from((last - b'0') / 2 % 2)];
    let dir = std::env::temp_dir().join("whatsapp-cosmic-fake-media");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("avatar-{}.jpg", chat.user()));
    tokio::fs::write(&path, bytes).await?;
    Ok(Some(path))
}

/// Picks the bundled asset for `media` (by its `fake-asset:` token, else by
/// mime type) and writes it into a temporary media directory.
async fn fetch_sample(message: &MessageId, media: &MediaRef) -> anyhow::Result<PathBuf> {
    // Simulated network latency, so the UI's loading state is visible.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let (bytes, ext) = sample_for(media)
        .ok_or_else(|| anyhow::anyhow!("This media is not available in demo mode"))?;
    let dir = std::env::temp_dir().join("whatsapp-cosmic-fake-media");
    tokio::fs::create_dir_all(&dir).await?;
    let stem: String = message
        .0
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = dir.join(format!("{stem}.{ext}"));
    tokio::fs::write(&path, bytes).await?;
    Ok(path)
}

fn sample_for(media: &MediaRef) -> Option<(&'static [u8], &'static str)> {
    let token = std::str::from_utf8(&media.download_token).unwrap_or_default();
    match token.strip_prefix(TOKEN_PREFIX) {
        Some("sample.jpg") => return Some((SAMPLE_JPEG, "jpg")),
        Some("sticker.webp") => return Some((SAMPLE_STICKER_WEBP, "webp")),
        Some("sample.ogg") => return Some((SAMPLE_OGG_OPUS, "ogg")),
        Some("sample.pdf") => return Some((SAMPLE_PDF, "pdf")),
        Some(_) => return None,
        None => {}
    }
    let mime = media.mime.as_str();
    if mime == "image/webp" {
        Some((SAMPLE_STICKER_WEBP, "webp"))
    } else if mime.starts_with("image/") {
        Some((SAMPLE_JPEG, "jpg"))
    } else if mime.starts_with("audio/") {
        Some((SAMPLE_OGG_OPUS, "ogg"))
    } else if mime == "application/pdf" {
        Some((SAMPLE_PDF, "pdf"))
    } else {
        None
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Demo data
// ---------------------------------------------------------------------------

const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;

struct Person {
    jid: &'static str,
    name: &'static str,
}

const ALICE: Person = Person {
    jid: "15550101001@s.whatsapp.net",
    name: "Alice Johnson",
};
const MUM: Person = Person {
    jid: "15550101002@s.whatsapp.net",
    name: "Mum",
};
const BEN: Person = Person {
    jid: "15550101003@s.whatsapp.net",
    name: "Ben Carter",
};
const CHLOE: Person = Person {
    jid: "15550101004@s.whatsapp.net",
    name: "Chloe Park",
};
const PRIYA: Person = Person {
    jid: "15550101005@s.whatsapp.net",
    name: "Priya Shah",
};
const MARCO: Person = Person {
    jid: "15550101006@s.whatsapp.net",
    name: "Marco Rossi",
};
const CARLA: Person = Person {
    jid: "15550101007@s.whatsapp.net",
    name: "Carla Mendes",
};
const DAD: Person = Person {
    jid: "15550101008@s.whatsapp.net",
    name: "Dad",
};
const LENA: Person = Person {
    jid: "15550101009@s.whatsapp.net",
    name: "Lena",
};

/// Who wrote a demo message.
enum Who<'a> {
    Me(MessageStatus),
    Them(&'a Person),
}

/// Accumulates one chat's events.
struct DemoChat {
    id: ChatId,
    name: &'static str,
    muted: bool,
    unread: u32,
    seq: u32,
    tag: &'static str,
    messages: Vec<Message>,
    reactions: Vec<Reaction>,
}

impl DemoChat {
    fn new(jid: &str, name: &'static str, tag: &'static str) -> Self {
        Self {
            id: ChatId(jid.to_owned()),
            name,
            muted: false,
            unread: 0,
            seq: 0,
            tag,
            messages: Vec::new(),
            reactions: Vec::new(),
        }
    }

    fn muted(mut self) -> Self {
        self.muted = true;
        self
    }

    /// Adds a message and returns its id (for replies and reactions).
    fn add(&mut self, from: Who<'_>, ts: i64, content: MessageContent) -> MessageId {
        self.seq += 1;
        let id = MessageId(format!(
            "3EB0{}{:04X}{:08X}",
            self.tag,
            self.seq,
            ts.rem_euclid(1 << 32)
        ));
        let group = self.id.is_group();
        let (sender, sender_name, from_me, status) = match from {
            Who::Me(status) => (String::new(), None, true, status),
            Who::Them(p) => (
                p.jid.to_owned(),
                // Direct chats are named after the contact already.
                group.then(|| p.name.to_owned()),
                false,
                MessageStatus::Received,
            ),
        };
        self.messages.push(Message {
            id: id.clone(),
            chat: self.id.clone(),
            sender,
            sender_name,
            from_me,
            timestamp: ts,
            content,
            reply_to: None,
            status,
        });
        id
    }

    fn text(&mut self, from: Who<'_>, ts: i64, text: &str) -> MessageId {
        self.add(from, ts, MessageContent::Text(text.to_owned()))
    }

    /// Adds a text message quoting `quoted`.
    fn reply(&mut self, from: Who<'_>, ts: i64, quoted: &MessageId, text: &str) -> MessageId {
        let quote = self
            .messages
            .iter()
            .find(|m| &m.id == quoted)
            .map(|q| QuotedMessage {
                id: q.id.clone(),
                sender: q.sender.clone(),
                snippet: q.content.preview(),
            });
        let id = self.text(from, ts, text);
        if let Some(last) = self.messages.last_mut() {
            last.reply_to = quote;
        }
        id
    }

    fn react(&mut self, target: &MessageId, sender: &str, emoji: &str, ts: i64) {
        self.reactions.push(Reaction {
            chat: self.id.clone(),
            target: target.clone(),
            sender: sender.to_owned(),
            emoji: emoji.to_owned(),
            timestamp: ts,
        });
    }

    fn into_events(self, out: &mut Vec<BackendEvent>) {
        let last = self.messages.iter().max_by_key(|m| m.timestamp);
        let chat = Chat {
            id: self.id.clone(),
            kind: if self.id.is_group() {
                ChatKind::Group
            } else {
                ChatKind::Direct
            },
            name: self.name.to_owned(),
            last_activity: last.map(|m| m.timestamp).unwrap_or_default(),
            unread: self.unread,
            muted: self.muted,
            last_preview: last.map(|m| m.content.preview()),
        };
        // Messages first, then the chat: the chat's authoritative unread count
        // (as history sync reports it) overrides the per-message increments.
        out.extend(self.messages.into_iter().map(BackendEvent::Message));
        out.extend(self.reactions.into_iter().map(BackendEvent::Reaction));
        out.push(BackendEvent::ChatUpserted(chat));
    }
}

fn media(mime: &str, bytes: &[u8], asset: &str, thumbnail: Option<&[u8]>) -> MediaRef {
    MediaRef {
        mime: mime.to_owned(),
        size: bytes.len() as u64,
        thumbnail: thumbnail.map(<[u8]>::to_vec),
        local_path: None,
        download_token: format!("{TOKEN_PREFIX}{asset}").into_bytes(),
    }
}

fn image(caption: Option<&str>) -> MessageContent {
    MessageContent::Image {
        caption: caption.map(str::to_owned),
        media: media("image/jpeg", SAMPLE_JPEG, "sample.jpg", Some(SAMPLE_JPEG)),
    }
}

fn demo_script(now: i64) -> Vec<BackendEvent> {
    use MessageStatus::{Delivered, Failed, Read};
    let me_read = || Who::Me(Read);

    let mut chats = Vec::new();

    // 1:1, active a few minutes ago, two unread.
    let mut alice = DemoChat::new(ALICE.jid, ALICE.name, "A1");
    alice.text(
        me_read(),
        now - 3 * HOUR,
        "Hey! Are we still on for dinner tomorrow?",
    );
    let when = alice.text(
        Who::Them(&ALICE),
        now - 3 * HOUR + 4 * MIN,
        "Yes! 7pm at Luigi's?",
    );
    let booked = alice.reply(
        me_read(),
        now - 3 * HOUR + 6 * MIN,
        &when,
        "Perfect, I'll book a table 🍝",
    );
    alice.react(&booked, ALICE.jid, "❤️", now - 3 * HOUR + 7 * MIN);
    alice.text(
        Who::Them(&ALICE),
        now - 12 * MIN,
        "Running a bit late tomorrow, maybe 7:30?",
    );
    alice.text(
        Who::Them(&ALICE),
        now - 11 * MIN,
        "Also bringing Sam, hope that's ok 😊",
    );
    alice.unread = 2;
    chats.push(alice);

    // Group with an image, a sticker, replies and several reactions.
    let mut hiking = DemoChat::new("120363025550000001@g.us", "Weekend Hiking 🥾", "B2");
    hiking.text(
        Who::Them(&BEN),
        now - DAY - 2 * HOUR,
        "Trail report: the ridge path is open again!",
    );
    let photo = hiking.add(
        Who::Them(&CHLOE),
        now - DAY - 2 * HOUR + 10 * MIN,
        image(Some("View from the top last weekend 🌄")),
    );
    hiking.reply(me_read(), now - DAY - HOUR, &photo, "Wow, that's stunning");
    hiking.react(&photo, BEN.jid, "😮", now - DAY - 2 * HOUR + 12 * MIN);
    hiking.react(&photo, ALICE.jid, "❤️", now - DAY - 2 * HOUR + 15 * MIN);
    hiking.react(&photo, OWN_REACTION_SENDER, "❤️", now - DAY - HOUR);
    hiking.add(
        Who::Them(&ALICE),
        now - 40 * MIN,
        MessageContent::Sticker {
            media: media("image/webp", SAMPLE_STICKER_WEBP, "sticker.webp", None),
        },
    );
    let plan = hiking.text(
        Who::Them(&BEN),
        now - 35 * MIN,
        "Saturday 8am at the north trailhead?",
    );
    hiking.reply(Who::Them(&CHLOE), now - 30 * MIN, &plan, "I'm in! 🙋‍♀️");
    hiking.react(&plan, CHLOE.jid, "👍", now - 30 * MIN);
    hiking.unread = 3;
    chats.push(hiking);

    // 1:1 with a voice note.
    let mut mum = DemoChat::new(MUM.jid, MUM.name, "C3");
    mum.text(Who::Them(&MUM), now - 5 * HOUR, "Did you get home safely?");
    let thanks = mum.text(
        me_read(),
        now - 5 * HOUR + 10 * MIN,
        "Yes, all good! Thanks for lunch ❤️",
    );
    mum.react(&thanks, MUM.jid, "🥰", now - 5 * HOUR + 12 * MIN);
    mum.add(
        Who::Them(&MUM),
        now - HOUR,
        MessageContent::Audio {
            voice_note: true,
            duration_secs: 3,
            media: media(
                "audio/ogg; codecs=opus",
                SAMPLE_OGG_OPUS,
                "sample.ogg",
                None,
            ),
        },
    );
    mum.unread = 1;
    chats.push(mum);

    // Muted work group with a document and an unsupported poll.
    let mut atlas = DemoChat::new("120363025550000002@g.us", "Project Atlas", "D4").muted();
    atlas.text(
        Who::Them(&PRIYA),
        now - 2 * DAY,
        "Draft of the Q3 report is attached",
    );
    let doc = atlas.add(
        Who::Them(&PRIYA),
        now - 2 * DAY + MIN,
        MessageContent::Document {
            file_name: "Q3-report.pdf".to_owned(),
            caption: Some("Comments by Friday please".to_owned()),
            media: media("application/pdf", SAMPLE_PDF, "sample.pdf", None),
        },
    );
    atlas.react(&doc, OWN_REACTION_SENDER, "👍", now - DAY - 3 * HOUR);
    atlas.react(&doc, MARCO.jid, "👍", now - 2 * DAY + 20 * MIN);
    atlas.add(
        Who::Them(&MARCO),
        now - 2 * DAY + 10 * MIN,
        MessageContent::Unsupported {
            kind: "poll".to_owned(),
        },
    );
    atlas.text(
        Who::Me(Delivered),
        now - DAY - 3 * HOUR,
        "Looks great, left a few comments in the doc",
    );
    let standup = atlas.text(
        Who::Them(&MARCO),
        now - 90 * MIN,
        "Standup moved to 10:15 tomorrow",
    );
    atlas.reply(
        Who::Them(&PRIYA),
        now - 80 * MIN,
        &standup,
        "👍 works for me",
    );
    atlas.unread = 4;
    chats.push(atlas);

    // Yesterday; we wrote last and it was delivered but not read.
    let mut ben = DemoChat::new(BEN.jid, BEN.name, "E5");
    let map = ben.text(
        Who::Them(&BEN),
        now - DAY - 5 * HOUR,
        "Can you send me the hiking map later?",
    );
    ben.reply(
        me_read(),
        now - DAY - 4 * HOUR,
        &map,
        "Here you go: https://maps.example.com/ridge-loop",
    );
    ben.text(
        Who::Them(&BEN),
        now - DAY - 4 * HOUR + 5 * MIN,
        "Thanks!! 🙏",
    );
    ben.text(Who::Me(Delivered), now - DAY - 3 * HOUR, "See you Saturday");
    chats.push(ben);

    // Last week; ends with a failed send (Retry in the UI).
    let mut carla = DemoChat::new(CARLA.jid, CARLA.name, "F6");
    carla.add(Who::Them(&CARLA), now - 4 * DAY, image(None));
    carla.text(me_read(), now - 4 * DAY + 5 * MIN, "Haha where was this?");
    let lisbon = carla.text(
        Who::Them(&CARLA),
        now - 4 * DAY + 20 * MIN,
        "Lisbon! Miradouro da Graça at sunset",
    );
    carla.react(&lisbon, OWN_REACTION_SENDER, "😍", now - 4 * DAY + 25 * MIN);
    carla.text(
        Who::Me(Failed),
        now - 4 * DAY + 30 * MIN,
        "So jealous 😄 we should go in spring",
    );
    chats.push(carla);

    // Weeks ago; a video whose download fails in demo mode.
    let mut family = DemoChat::new("120363025550000003@g.us", "Family 👨‍👩‍👧", "G7");
    let bday = family.text(Who::Them(&DAD), now - 16 * DAY, "Happy birthday Lena! 🎂🎉");
    family.react(&bday, LENA.jid, "❤️", now - 16 * DAY + 5 * MIN);
    family.react(&bday, OWN_REACTION_SENDER, "🎉", now - 16 * DAY + 6 * MIN);
    family.add(
        Who::Them(&DAD),
        now - 16 * DAY + 2 * MIN,
        MessageContent::Video {
            caption: Some("Blowing out the candles".to_owned()),
            media: MediaRef {
                mime: "video/mp4".to_owned(),
                size: 1_843_200,
                thumbnail: Some(SAMPLE_JPEG.to_vec()),
                local_path: None,
                download_token: format!("{TOKEN_PREFIX}video.mp4").into_bytes(),
            },
        },
    );
    family.text(
        Who::Them(&LENA),
        now - 16 * DAY + 30 * MIN,
        "Thank you all!! Best day ❤️",
    );
    family.text(
        me_read(),
        now - 16 * DAY + 45 * MIN,
        "Happy birthday sis! 🥳",
    );
    chats.push(family);

    let total = chats.len();
    let mut out = vec![
        BackendEvent::Connection(ConnectionState::Connecting),
        BackendEvent::Connection(ConnectionState::Connected),
        BackendEvent::SyncProgress(0.0),
    ];
    for (i, chat) in chats.into_iter().enumerate() {
        chat.into_events(&mut out);
        out.push(BackendEvent::SyncProgress((i + 1) as f32 / total as f32));
    }
    out.push(BackendEvent::SyncDone);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_assets_are_tiny_and_valid() {
        for (bytes, magic) in [
            (SAMPLE_JPEG, &b"\xFF\xD8\xFF"[..]),
            (SAMPLE_STICKER_WEBP, b"RIFF"),
            (SAMPLE_OGG_OPUS, b"OggS"),
            (SAMPLE_PDF, b"%PDF"),
        ] {
            assert!(bytes.len() < 50 * 1024);
            assert!(bytes.starts_with(magic));
        }
        assert!(SAMPLE_OGG_OPUS.windows(8).any(|w| w == b"OpusHead"));
    }

    #[test]
    fn demo_ids_are_unique_and_replies_resolve() {
        let script = demo_script(1_800_000_000_000);
        let msgs: Vec<&Message> = script
            .iter()
            .filter_map(|e| {
                if let BackendEvent::Message(m) = e {
                    Some(m)
                } else {
                    None
                }
            })
            .collect();
        let mut ids: Vec<_> = msgs.iter().map(|m| (&m.chat, &m.id)).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), msgs.len());
        for m in &msgs {
            if let Some(q) = &m.reply_to {
                let target = msgs.iter().find(|t| t.id == q.id && t.chat == m.chat);
                assert!(target.is_some(), "reply in {} resolves", m.chat.0);
                assert!(!q.snippet.is_empty());
            }
            if m.chat.is_group() && !m.from_me {
                assert!(m.sender_name.is_some());
            }
        }
        for e in &script {
            if let BackendEvent::Reaction(r) = e {
                assert!(msgs.iter().any(|m| m.id == r.target && m.chat == r.chat));
            }
        }
    }

    #[test]
    fn every_demo_media_resolves_except_video() {
        let script = demo_script(1_800_000_000_000);
        for e in &script {
            if let BackendEvent::Message(m) = e
                && let Some(media) = m.content.media()
            {
                let expect = !matches!(m.content, MessageContent::Video { .. });
                assert_eq!(sample_for(media).is_some(), expect, "{:?}", m.content);
                if let Some((bytes, _)) = sample_for(media) {
                    assert_eq!(bytes.len() as u64, media.size);
                }
            }
        }
    }
}
