//! End-to-end tests of `wa_core::start` against the scripted `FakeBackend`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::timeout;
use wa_core::backend::fake::FakeBackend;
use wa_core::{
    BackendCommand, BackendEvent, Chat, ChatId, ChatKind, Handle, Message, MessageContent,
    MessageId, MessageStatus, Reaction, Store, start,
};

const WAIT: Duration = Duration::from_secs(5);

fn alice() -> ChatId {
    ChatId("15550101001@s.whatsapp.net".into())
}

fn incoming(id: &str, ts: i64, text: &str) -> Message {
    Message {
        id: MessageId(id.into()),
        chat: alice(),
        sender: alice().0,
        sender_name: Some("Alice".into()),
        from_me: false,
        timestamp: ts,
        content: MessageContent::Text(text.into()),
        reply_to: None,
        status: MessageStatus::Received,
    }
}

struct Setup {
    _dir: tempfile::TempDir,
    db: PathBuf,
    handle: Handle,
    received: mpsc::UnboundedReceiver<BackendCommand>,
}

fn setup(script: Vec<BackendEvent>) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let store = Store::open(&db).unwrap();
    let (tx, received) = mpsc::unbounded_channel();
    let backend = FakeBackend {
        script,
        received: Some(tx),
        auto_reply: true,
    };
    let handle = start(Box::new(backend), store);
    Setup {
        _dir: dir,
        db,
        handle,
        received,
    }
}

async fn next(handle: &mut Handle) -> BackendEvent {
    timeout(WAIT, handle.events.recv())
        .await
        .expect("timed out")
        .expect("events closed")
}

fn reopen(db: &Path) -> Store {
    Store::open(db).unwrap()
}

#[tokio::test]
async fn send_text_is_optimistic_then_sent() {
    let mut s = setup(vec![]);
    let local_id = MessageId::new_local();
    s.handle
        .commands
        .send(BackendCommand::SendText {
            chat: alice(),
            text: "hello there".into(),
            reply_to: None,
            local_id: local_id.clone(),
        })
        .await
        .unwrap();

    match next(&mut s.handle).await {
        BackendEvent::Message(m) => {
            assert_eq!(m.id, local_id);
            assert_eq!(m.status, MessageStatus::Pending);
            assert!(m.from_me);
            assert_eq!(m.content, MessageContent::Text("hello there".into()));
        }
        other => panic!("expected optimistic Message, got {other:?}"),
    }
    let sent_id = match next(&mut s.handle).await {
        BackendEvent::MessageSent {
            chat,
            local_id: l,
            id,
        } => {
            assert_eq!(chat, alice());
            assert_eq!(l, local_id);
            id
        }
        other => panic!("expected MessageSent, got {other:?}"),
    };
    assert_eq!(sent_id, MessageId("fake-1".into()));

    // The backend received the command after it was stored.
    let cmd = timeout(WAIT, s.received.recv()).await.unwrap().unwrap();
    assert!(matches!(cmd, BackendCommand::SendText { .. }));

    let store = reopen(&s.db);
    let m = store
        .message(&alice(), &sent_id)
        .unwrap()
        .expect("stored under server id");
    assert!(m.status >= MessageStatus::Sent);
    assert!(store.message(&alice(), &local_id).unwrap().is_none());

    // Delivered then Read follow and are persisted.
    let mut statuses = Vec::new();
    while statuses.len() < 2 {
        if let BackendEvent::MessageStatus {
            message, status, ..
        } = next(&mut s.handle).await
        {
            assert_eq!(message, sent_id);
            statuses.push(status);
        }
    }
    assert_eq!(statuses, [MessageStatus::Delivered, MessageStatus::Read]);
    let m = reopen(&s.db).message(&alice(), &sent_id).unwrap().unwrap();
    assert_eq!(m.status, MessageStatus::Read);
}

#[tokio::test]
async fn reply_builds_quoted_snippet() {
    let mut s = setup(vec![BackendEvent::Message(incoming(
        "IN1",
        1_000,
        "Are you coming?",
    ))]);
    assert!(matches!(
        next(&mut s.handle).await,
        BackendEvent::Message(_)
    ));
    let local_id = MessageId::new_local();
    s.handle
        .commands
        .send(BackendCommand::SendText {
            chat: alice(),
            text: "Yes!".into(),
            reply_to: Some(MessageId("IN1".into())),
            local_id: local_id.clone(),
        })
        .await
        .unwrap();
    match next(&mut s.handle).await {
        BackendEvent::Message(m) => {
            let q = m.reply_to.expect("quoted");
            assert_eq!(q.id, MessageId("IN1".into()));
            assert_eq!(q.sender, alice().0);
            assert_eq!(q.snippet, "Are you coming?");
        }
        other => panic!("expected Message, got {other:?}"),
    }
}

#[tokio::test]
async fn incoming_message_persisted_before_forwarded() {
    let msg = incoming("IN1", 1_000, "hi from alice");
    let mut s = setup(vec![
        BackendEvent::ChatUpserted(Chat {
            id: alice(),
            kind: ChatKind::Direct,
            name: "Alice".into(),
            last_activity: 0,
            unread: 0,
            muted: false,
            last_preview: None,
        }),
        BackendEvent::Message(msg.clone()),
    ]);
    assert!(matches!(
        next(&mut s.handle).await,
        BackendEvent::ChatUpserted(_)
    ));
    match next(&mut s.handle).await {
        BackendEvent::Message(m) => assert_eq!(m, msg),
        other => panic!("expected Message, got {other:?}"),
    }
    let store = reopen(&s.db);
    assert_eq!(store.message(&alice(), &msg.id).unwrap(), Some(msg));
    let chat = store.chat(&alice()).unwrap().unwrap();
    assert_eq!(chat.name, "Alice");
    assert_eq!(chat.unread, 1);
}

#[tokio::test]
async fn react_and_mark_read_are_persisted() {
    let mut s = setup(vec![BackendEvent::Message(incoming("IN1", 1_000, "hi"))]);
    assert!(matches!(
        next(&mut s.handle).await,
        BackendEvent::Message(_)
    ));
    s.handle
        .commands
        .send(BackendCommand::MarkRead {
            chat: alice(),
            messages: vec![MessageId("IN1".into())],
        })
        .await
        .unwrap();
    s.handle
        .commands
        .send(BackendCommand::React {
            chat: alice(),
            target: MessageId("IN1".into()),
            emoji: "❤️".into(),
        })
        .await
        .unwrap();
    match next(&mut s.handle).await {
        BackendEvent::Reaction(Reaction { emoji, sender, .. }) => {
            assert_eq!(emoji, "❤️");
            assert_eq!(sender, "me");
        }
        other => panic!("expected Reaction, got {other:?}"),
    }
    let store = reopen(&s.db);
    assert_eq!(store.chat(&alice()).unwrap().unwrap().unread, 0);
    assert_eq!(
        store
            .reactions(&alice(), &MessageId("IN1".into()))
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn fetch_media_writes_sample_and_records_path() {
    let demo = FakeBackend::demo();
    let image = demo
        .script
        .iter()
        .find_map(|e| match e {
            BackendEvent::Message(m) if matches!(m.content, MessageContent::Image { .. }) => {
                Some(m.clone())
            }
            _ => None,
        })
        .expect("demo has an image");
    let mut s = setup(vec![BackendEvent::Message(image.clone())]);
    assert!(matches!(
        next(&mut s.handle).await,
        BackendEvent::Message(_)
    ));
    let media = image.content.media().unwrap().clone();
    s.handle
        .commands
        .send(BackendCommand::FetchMedia {
            chat: image.chat.clone(),
            message: image.id.clone(),
            media: media.clone(),
        })
        .await
        .unwrap();
    let path = match next(&mut s.handle).await {
        BackendEvent::MediaReady { message, path, .. } => {
            assert_eq!(message, image.id);
            path
        }
        other => panic!("expected MediaReady, got {other:?}"),
    };
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len() as u64, media.size);
    assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "JPEG magic");
    let stored = reopen(&s.db)
        .message(&image.chat, &image.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.content.media().unwrap().local_path.as_deref(),
        Some(path.as_path())
    );
}

#[tokio::test]
async fn logout_wipes_store() {
    let mut s = setup(vec![BackendEvent::Message(incoming("IN1", 1_000, "hi"))]);
    assert!(matches!(
        next(&mut s.handle).await,
        BackendEvent::Message(_)
    ));
    s.handle
        .commands
        .send(BackendCommand::Logout)
        .await
        .unwrap();
    assert_eq!(next(&mut s.handle).await, BackendEvent::LoggedOut);
    let store = reopen(&s.db);
    assert!(store.chats().unwrap().is_empty());
    assert!(store.messages(&alice(), None, 50).unwrap().is_empty());
}

#[tokio::test]
async fn logged_out_without_request_keeps_store() {
    let mut s = setup(vec![
        BackendEvent::Message(incoming("IN1", 1_000, "hi")),
        BackendEvent::LoggedOut,
    ]);
    assert!(matches!(
        next(&mut s.handle).await,
        BackendEvent::Message(_)
    ));
    assert_eq!(next(&mut s.handle).await, BackendEvent::LoggedOut);
    let store = reopen(&s.db);
    assert_eq!(store.messages(&alice(), None, 50).unwrap().len(), 1);
}

#[tokio::test]
async fn shutdown_closes_event_stream() {
    let mut s = setup(vec![]);
    s.handle
        .commands
        .send(BackendCommand::Shutdown)
        .await
        .unwrap();
    let end = timeout(WAIT, s.handle.events.recv())
        .await
        .expect("timed out");
    assert_eq!(end, None);
}

#[tokio::test]
async fn demo_script_is_rich_and_persists() {
    let demo = FakeBackend::demo();
    let msgs: Vec<&Message> = demo
        .script
        .iter()
        .filter_map(|e| match e {
            BackendEvent::Message(m) => Some(m),
            _ => None,
        })
        .collect();
    let chats: Vec<&Chat> = demo
        .script
        .iter()
        .filter_map(|e| match e {
            BackendEvent::ChatUpserted(c) => Some(c),
            _ => None,
        })
        .collect();
    assert!(matches!(
        demo.script.first(),
        Some(BackendEvent::Connection(_))
    ));
    assert_eq!(demo.script.last(), Some(&BackendEvent::SyncDone));
    assert!(chats.len() >= 6);
    assert!(chats.iter().any(|c| c.kind == ChatKind::Group));
    assert!(chats.iter().any(|c| c.muted));
    assert!(chats.iter().any(|c| c.unread > 0));
    assert!(msgs.iter().any(|m| m.reply_to.is_some()));
    assert!(
        msgs.iter()
            .any(|m| matches!(m.content, MessageContent::Image { .. }))
    );
    assert!(msgs.iter().any(|m| matches!(
        m.content,
        MessageContent::Audio {
            voice_note: true,
            ..
        }
    )));
    assert!(
        msgs.iter()
            .any(|m| matches!(m.content, MessageContent::Document { .. }))
    );
    assert!(
        msgs.iter()
            .any(|m| matches!(m.content, MessageContent::Unsupported { .. }))
    );
    assert!(
        demo.script
            .iter()
            .any(|e| matches!(e, BackendEvent::Reaction(_)))
    );
    assert!(demo.auto_reply);

    // Run it through the runtime: the store ends up with the scripted chat metadata.
    let expected: Vec<Chat> = chats.into_iter().cloned().collect();
    let n = demo.script.len();
    let mut s = setup(demo.script);
    for _ in 0..n {
        next(&mut s.handle).await;
    }
    let store = reopen(&s.db);
    let stored = store.chats().unwrap();
    assert_eq!(stored.len(), expected.len());
    for c in &expected {
        let got = store.chat(&c.id).unwrap().unwrap();
        assert_eq!(&got, c, "chat {} persisted as scripted", c.name);
    }
    // Most recent first.
    assert!(
        stored
            .windows(2)
            .all(|w| w[0].last_activity >= w[1].last_activity)
    );
}

/// A backend that dies immediately, like a panic inside whatsapp-rust.
struct PanickingBackend;

#[async_trait::async_trait]
impl wa_core::WhatsAppBackend for PanickingBackend {
    async fn run(
        self: Box<Self>,
        _commands: mpsc::Receiver<BackendCommand>,
        _events: mpsc::Sender<BackendEvent>,
    ) -> anyhow::Result<()> {
        panic!("simulated backend crash");
    }
}

#[tokio::test]
async fn dead_backend_is_reported_and_commands_fail() {
    let store = Store::open_in_memory().unwrap();
    let mut handle = start(Box::new(PanickingBackend), store);
    assert!(matches!(next(&mut handle).await, BackendEvent::Error(_)));
    assert_eq!(
        next(&mut handle).await,
        BackendEvent::Connection(wa_core::ConnectionState::Disconnected)
    );
    let local_id = MessageId::new_local();
    handle
        .commands
        .send(BackendCommand::SendText {
            chat: alice(),
            text: "anyone?".into(),
            reply_to: None,
            local_id: local_id.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(next(&mut handle).await, BackendEvent::Message(m) if m.id == local_id));
    match next(&mut handle).await {
        BackendEvent::SendFailed { local_id: l, .. } => assert_eq!(l, local_id),
        other => panic!("expected SendFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn pending_messages_from_last_run_become_failed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let local_id = MessageId::new_local();
    {
        let store = Store::open(&db).unwrap();
        let mut m = incoming(&local_id.0, 1, "in flight");
        m.from_me = true;
        m.status = MessageStatus::Pending;
        store.upsert_message(&m).unwrap();
    }
    let backend = FakeBackend {
        script: vec![],
        received: None,
        auto_reply: false,
    };
    let _handle = start(Box::new(backend), Store::open(&db).unwrap());
    let status = reopen(&db)
        .message(&alice(), &local_id)
        .unwrap()
        .unwrap()
        .status;
    assert_eq!(status, MessageStatus::Failed);
}
