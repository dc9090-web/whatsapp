//! The real backend over the `whatsapp-rust` crate.
//!
//! CONTRACT (implemented in Task 3): `WhatsAppRustBackend::new` is fixed.
//!
//! Layout:
//! - [`convert`]: pure protocol → model mapping (unit-tested).
//! - [`index`]: a small sidecar SQLite file (`<session_db>.index`) remembering
//!   message keys (for quotes, reactions, read receipts), chat names, mute
//!   state and LID→phone-number mappings across restarts.
//! - this file: the session lifecycle. One *session* is one `Bot` over the
//!   session database. A logout (local or remote) wipes the session files and
//!   starts a fresh session, which shows a new pairing QR. whatsapp-rust
//!   reconnects by itself (Fibonacci backoff); we only report its state.

mod convert;
mod index;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::{Semaphore, mpsc};
use whatsapp_rust::bot::{Bot, BotHandle};
use whatsapp_rust::download::Downloadable;
use whatsapp_rust::http::HttpRequest;
use whatsapp_rust::prelude::{MessageBuilderExt, SqliteStore};
use whatsapp_rust::types::events::{Event, EventHandler};
use whatsapp_rust::wacore::store::DevicePropsOverride;
use whatsapp_rust::waproto::codec;
use whatsapp_rust::waproto::whatsapp as wa;
use whatsapp_rust::{Client, proto_helpers};
use whatsapp_rust::{ErrorChainExt as _, Jid, PictureType};

use self::convert::{Converted, KeyRecord, LidMap, MediaPayload};
use self::index::Index;
use crate::backend::WhatsAppBackend;
use crate::events::{BackendCommand, BackendEvent, ConnectionState};
use crate::model::{Chat, ChatId, MediaRef, MessageId, Reaction};
use whatsapp_rust::wacore::types::presence::ReceiptType;
use whatsapp_rust::wacore_binary::JidExt as _;

pub struct WhatsAppRustBackend {
    session_db: PathBuf,
    media_dir: PathBuf,
}

impl WhatsAppRustBackend {
    pub fn new(session_db: PathBuf, media_dir: PathBuf) -> Self {
        Self {
            session_db,
            media_dir,
        }
    }
}

/// If no history chunk arrives for this long after pairing, the initial sync
/// is considered done (the phone does not always send a final 100% chunk).
const SYNC_QUIET: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a finished session's event pump may keep persisting events that
/// whatsapp-rust already dispatched (they are never re-sent).
const DRAIN_TIMEOUT: Duration = Duration::from_secs(4);
/// After the stream was replaced (or the client rejected), try again this
/// late rather than staying disconnected until a restart.
const STOPPED_RETRY: Duration = Duration::from_secs(300);
/// Profile-picture lookups in flight at once, so a long chat list does not
/// fire hundreds of IQs at the server in one go.
const MAX_AVATAR_DOWNLOADS: usize = 3;

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEnd {
    /// `Shutdown` command or the command channel closed.
    Shutdown,
    /// The device was unlinked (locally or from the phone).
    LoggedOut,
    /// Start a fresh bot (pairing QRs exhausted, run loop ended).
    Restart,
    /// Another client took over the session; wait for the user.
    Stopped,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// State shared by the command side and the event pump of every session.
struct Shared {
    events: mpsc::Sender<BackendEvent>,
    index: Mutex<Index>,
    index_path: PathBuf,
    lids: Mutex<LidMap>,
    media_dir: PathBuf,
    /// Dedupes `LoggedOut` (whatsapp-rust dispatches one on `logout()` too).
    logged_out_sent: AtomicBool,
    /// Caps concurrent `FetchAvatar` lookups at [`MAX_AVATAR_DOWNLOADS`].
    avatar_slots: Semaphore,
}

impl Shared {
    async fn emit(&self, ev: BackendEvent) {
        if self.events.send(ev).await.is_err() {
            tracing::debug!("event receiver closed; dropping backend event");
        }
    }

    async fn emit_logged_out(&self) {
        if !self.logged_out_sent.swap(true, Ordering::SeqCst) {
            self.emit(BackendEvent::LoggedOut).await;
        }
    }

    fn lids(&self) -> LidMap {
        lock(&self.lids).clone()
    }

    /// A `ChatUpserted` payload carrying the best-known name and mute state.
    /// Activity/unread/preview are left empty so the store keeps its own.
    fn chat_snapshot(&self, chat: &ChatId) -> Chat {
        let meta = match lock(&self.index).chat_meta(chat) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("index read failed: {e:#}");
                Default::default()
            }
        };
        convert::new_chat(chat.clone(), meta.name, meta.muted)
    }

    /// Records a name and, if it changed for a chat the UI knows, announces it.
    async fn update_name(&self, chat: &ChatId, name: &str, rank: i64) {
        let (changed, known) = {
            let idx = lock(&self.index);
            let changed = idx.set_name(chat, name, rank).unwrap_or_else(|e| {
                tracing::warn!("index write failed: {e:#}");
                false
            });
            (
                changed,
                idx.chat_meta(chat).map(|m| m.known).unwrap_or(false),
            )
        };
        if changed && known {
            self.emit(BackendEvent::ChatUpserted(self.chat_snapshot(chat)))
                .await;
        }
    }

    /// Announces a chat the first time a message for it shows up.
    async fn ensure_known(&self, chat: &ChatId) {
        let newly = lock(&self.index).mark_known(chat).unwrap_or_else(|e| {
            tracing::warn!("index write failed: {e:#}");
            false
        });
        if newly {
            self.emit(BackendEvent::ChatUpserted(self.chat_snapshot(chat)))
                .await;
        }
    }

    fn put_keys(&self, records: &[KeyRecord]) {
        if records.is_empty() {
            return;
        }
        if let Err(e) = lock(&self.index).put_keys(records) {
            tracing::warn!("index write failed: {e:#}");
        }
    }

    fn key(&self, chat: &ChatId, id: &str) -> Option<KeyRecord> {
        lock(&self.index).key(chat, id).unwrap_or_else(|e| {
            tracing::warn!("index read failed: {e:#}");
            None
        })
    }

    fn remember_lid(&self, lid: &str, pn: &str) {
        lock(&self.lids).insert(lid.to_owned(), pn.to_owned());
        if let Err(e) = lock(&self.index).put_lid(lid, pn) {
            tracing::warn!("index write failed: {e:#}");
        }
    }
}

fn index_path_for(session_db: &Path) -> PathBuf {
    let mut name = session_db
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "session.db".into());
    name.push(".index");
    session_db.with_file_name(name)
}

/// Opens the sidecar index, falling back to an in-memory one (with a warning
/// for the user) if the file cannot be opened.
fn open_index(path: &Path) -> anyhow::Result<(Index, Option<String>)> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match Index::open(path) {
        Ok(idx) => Ok((idx, None)),
        Err(e) => {
            let msg = format!(
                "Could not open {}: {e:#}; replies and reactions to older messages may fail",
                path.display()
            );
            tracing::error!("{msg}");
            Ok((Index::open_in_memory()?, Some(msg)))
        }
    }
}

/// Removes a SQLite database and its WAL side files.
fn remove_db_files(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_os_string();
        p.push(suffix);
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("could not remove {}: {e}", PathBuf::from(p).display()),
        }
    }
}

#[async_trait::async_trait]
impl WhatsAppBackend for WhatsAppRustBackend {
    async fn run(
        self: Box<Self>,
        mut commands: mpsc::Receiver<BackendCommand>,
        events: mpsc::Sender<BackendEvent>,
    ) -> anyhow::Result<()> {
        let index_path = index_path_for(&self.session_db);
        let (idx, warning) = open_index(&index_path)?;
        if let Err(e) = idx.prune(index::MAX_KEYS) {
            tracing::warn!("index prune failed: {e:#}");
        }
        let lids: LidMap = idx.lid_map().unwrap_or_default().into_iter().collect();
        let shared = Arc::new(Shared {
            events,
            index: Mutex::new(idx),
            index_path,
            lids: Mutex::new(lids),
            media_dir: self.media_dir.clone(),
            logged_out_sent: AtomicBool::new(false),
            avatar_slots: Semaphore::new(MAX_AVATAR_DOWNLOADS),
        });
        if let Some(w) = warning {
            shared.emit(BackendEvent::Error(w)).await;
        }

        let mut failures: u32 = 0;
        loop {
            let end = match run_session(&self.session_db, &shared, &mut commands).await {
                Ok(end) => {
                    failures = 0;
                    end
                }
                Err(e) => {
                    failures = failures.saturating_add(1);
                    let msg = format!("WhatsApp session failed to start: {e:#}");
                    tracing::error!("{msg}");
                    shared.emit(BackendEvent::Error(msg)).await;
                    let delay = convert::reconnect_delay_secs(failures + 2).min(60);
                    shared
                        .emit(BackendEvent::Connection(ConnectionState::Reconnecting {
                            attempt: failures,
                            retry_in_secs: delay,
                        }))
                        .await;
                    match idle(&shared, &mut commands, Some(Duration::from_secs(delay))).await {
                        Idle::Shutdown => return Ok(()),
                        Idle::LoggedOut => {
                            local_logout_offline(&shared).await;
                            SessionEnd::LoggedOut
                        }
                        Idle::Elapsed => continue,
                    }
                }
            };
            match end {
                SessionEnd::Shutdown => return Ok(()),
                SessionEnd::LoggedOut => {
                    wipe_session(&self.session_db, &shared);
                    shared.emit_logged_out().await;
                    // A fresh session produces new pairing QR codes.
                    shared.logged_out_sent.store(false, Ordering::SeqCst);
                }
                SessionEnd::Restart => {
                    match idle(&shared, &mut commands, Some(Duration::from_secs(2))).await {
                        Idle::Shutdown => return Ok(()),
                        Idle::LoggedOut => {
                            local_logout_offline(&shared).await;
                            wipe_session(&self.session_db, &shared);
                            shared.emit_logged_out().await;
                            shared.logged_out_sent.store(false, Ordering::SeqCst);
                        }
                        Idle::Elapsed => {}
                    }
                }
                SessionEnd::Stopped => {
                    shared
                        .emit(BackendEvent::Connection(ConnectionState::Disconnected))
                        .await;
                    match idle(&shared, &mut commands, Some(STOPPED_RETRY)).await {
                        Idle::Shutdown => return Ok(()),
                        Idle::Elapsed => {}
                        Idle::LoggedOut => {
                            local_logout_offline(&shared).await;
                            wipe_session(&self.session_db, &shared);
                            shared.emit_logged_out().await;
                            shared.logged_out_sent.store(false, Ordering::SeqCst);
                        }
                    }
                }
            }
        }
    }
}

/// Removes every downloaded (decrypted) media file. Explicit Log out
/// promises that nothing stored on this computer survives.
fn wipe_media(media_dir: &Path) {
    let entries = match std::fs::read_dir(media_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!("could not list {}: {e}", media_dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let removed = if entry.file_type().is_ok_and(|t| t.is_dir()) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = removed {
            tracing::warn!("could not remove {}: {e}", path.display());
        }
    }
}

/// A Log out requested while no session is connected: the server is not told,
/// so the user must remove the device on the phone.
async fn local_logout_offline(shared: &Shared) {
    wipe_media(&shared.media_dir);
    shared
        .emit(BackendEvent::Error(NOT_UNLINKED.to_owned()))
        .await;
}

const NOT_UNLINKED: &str = "Logged out locally, but WhatsApp could not be reached: remove \
     \"WhatsApp for COSMIC\" under Linked devices on your phone";

fn wipe_session(session_db: &Path, shared: &Shared) {
    remove_db_files(session_db);
    remove_db_files(&shared.index_path);
    match open_index(&shared.index_path) {
        Ok((idx, _)) => *lock(&shared.index) = idx,
        Err(e) => tracing::error!("could not recreate the backend index: {e:#}"),
    }
    lock(&shared.lids).clear();
}

enum Idle {
    Shutdown,
    LoggedOut,
    Elapsed,
}

/// Waits (optionally bounded) while no session is running, failing commands
/// that need a connection.
async fn idle(
    shared: &Shared,
    commands: &mut mpsc::Receiver<BackendCommand>,
    limit: Option<Duration>,
) -> Idle {
    let sleep = async {
        match limit {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            _ = &mut sleep => return Idle::Elapsed,
            cmd = commands.recv() => match cmd {
                None | Some(BackendCommand::Shutdown) => return Idle::Shutdown,
                Some(BackendCommand::Logout) => return Idle::LoggedOut,
                Some(cmd) => reject(shared, cmd, "not connected to WhatsApp").await,
            },
        }
    }
}

async fn reject(shared: &Shared, cmd: BackendCommand, why: &str) {
    match cmd {
        BackendCommand::SendText { chat, local_id, .. }
        | BackendCommand::SendMedia { chat, local_id, .. } => {
            shared
                .emit(BackendEvent::SendFailed {
                    chat,
                    local_id,
                    error: why.to_owned(),
                })
                .await;
        }
        BackendCommand::FetchMedia { chat, message, .. } => {
            shared
                .emit(BackendEvent::MediaFailed {
                    chat,
                    message,
                    error: why.to_owned(),
                })
                .await;
        }
        BackendCommand::FetchAvatar { chat } => {
            shared.emit(BackendEvent::AvatarMissing { chat }).await;
        }
        BackendCommand::React { .. } => {
            shared
                .emit(BackendEvent::Error(format!("Reaction not sent: {why}")))
                .await;
        }
        BackendCommand::MarkRead { .. } => {
            tracing::debug!("read receipts skipped: {why}");
        }
        BackendCommand::DeleteMessage {
            for_everyone: true, ..
        } => {
            shared
                .emit(BackendEvent::Error(format!(
                    "Not deleted for everyone: {why}"
                )))
                .await;
        }
        BackendCommand::DeleteMessage { .. } | BackendCommand::DeleteChat { .. } => {
            tracing::debug!("delete not synced to the phone: {why}");
        }
        BackendCommand::Logout | BackendCommand::Shutdown => {}
    }
}

/// Forwards bus events, in dispatch order and without dropping, to the
/// session's event pump.
/// `None` is the end-of-session marker the session loop sends after the bot
/// stopped, so the pump drains everything dispatched before it and then ends.
struct Forwarder(mpsc::UnboundedSender<Option<Arc<Event>>>);

impl EventHandler for Forwarder {
    fn handle_event(&self, event: Arc<Event>) {
        let _ = self.0.send(Some(event));
    }
}

async fn shutdown_bot(handle: BotHandle) {
    if tokio::time::timeout(SHUTDOWN_TIMEOUT, handle.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("whatsapp-rust did not shut down in time; aborted");
    }
}

async fn run_session(
    session_db: &Path,
    shared: &Arc<Shared>,
    commands: &mut mpsc::Receiver<BackendCommand>,
) -> anyhow::Result<SessionEnd> {
    if let Some(parent) = session_db.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let url = session_db
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("session path is not valid UTF-8"))?;
    let store = SqliteStore::new(url).await?;

    let (bus_tx, bus_rx) = mpsc::unbounded_channel();
    let bot = Bot::builder()
        .with_backend(store)
        .with_event_handler(Forwarder(bus_tx.clone()))
        .with_device_props(DevicePropsOverride::new().with_os("WhatsApp for COSMIC"))
        .build()
        .await?;

    shared
        .emit(BackendEvent::Connection(ConnectionState::Connecting))
        .await;
    let mut handle = bot.spawn();
    let client = handle.client();

    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel::<SessionEnd>();
    let pump = EventPump {
        shared: Arc::clone(shared),
        client: Arc::clone(&client),
        ctl: ctl_tx,
        attempt: 0,
        syncing: false,
        sync_deadline: None,
    };
    let mut pump_task = tokio::spawn(pump.run(bus_rx));
    let mut pump_alive = true;

    let end = loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                None | Some(BackendCommand::Shutdown) => {
                    shutdown_bot(handle).await;
                    break SessionEnd::Shutdown;
                }
                Some(BackendCommand::Logout) => {
                    // whatsapp-rust skips the unlink request when offline.
                    let online = client.is_connected();
                    client.logout().await;
                    shutdown_bot(handle).await;
                    wipe_media(&shared.media_dir);
                    if !online {
                        shared.emit(BackendEvent::Error(NOT_UNLINKED.to_owned())).await;
                    }
                    break SessionEnd::LoggedOut;
                }
                Some(cmd) => handle_command(shared, &client, cmd).await,
            },
            end = ctl_rx.recv() => {
                let end = end.unwrap_or(SessionEnd::Restart);
                shutdown_bot(handle).await;
                break end;
            }
            res = &mut pump_task, if pump_alive => {
                // The pump only ends on its own by panicking.
                pump_alive = false;
                let msg = match res {
                    Err(e) if e.is_panic() => "WhatsApp event handling crashed; reconnecting".to_owned(),
                    _ => "WhatsApp event handling stopped; reconnecting".to_owned(),
                };
                tracing::error!("{msg}");
                shared.emit(BackendEvent::Error(msg)).await;
                shutdown_bot(handle).await;
                break SessionEnd::Restart;
            }
            () = &mut handle => {
                // The run loop ended on its own; see whether the pump knows why.
                let end = tokio::time::timeout(Duration::from_millis(500), ctl_rx.recv())
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(SessionEnd::Restart);
                break end;
            }
        }
    };
    if pump_alive {
        // Let the pump persist what was already dispatched instead of
        // dropping it: whatsapp-rust will not deliver those events again.
        let _ = bus_tx.send(None);
        if tokio::time::timeout(DRAIN_TIMEOUT, &mut pump_task)
            .await
            .is_err()
        {
            tracing::warn!("event pump did not drain in time; dropping the rest");
            pump_task.abort();
        }
    }
    Ok(end)
}

fn own_jid(client: &Client) -> String {
    client
        .pn()
        .map(|j| j.to_non_ad_string())
        .unwrap_or_default()
}

/// "Delete for everyone": a sender revoke of our own message.
async fn revoke_for_everyone(
    client: &Arc<Client>,
    chat: &ChatId,
    message: &MessageId,
) -> anyhow::Result<()> {
    let jid = parse_jid(chat)?;
    client
        .revoke_message(jid, message.0.clone(), whatsapp_rust::RevokeType::Sender)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// "Delete for me": an app-state action so the phone drops its copy too.
/// The message key needs the direction and (for others' messages) the
/// sender, which the side index remembers.
async fn delete_for_me(
    shared: &Arc<Shared>,
    client: &Arc<Client>,
    chat: &ChatId,
    message: &MessageId,
) -> anyhow::Result<()> {
    let jid = parse_jid(chat)?;
    let (from_me, participant) = match lock(&shared.index).key(chat, &message.0) {
        Ok(Some(rec)) => (rec.from_me, (!rec.from_me).then_some(rec.sender)),
        _ => (true, None),
    };
    let participant = participant.as_deref().and_then(|p| p.parse::<Jid>().ok());
    client
        .chat_actions()
        .delete_message_for_me(&jid, participant.as_ref(), &message.0, from_me, true, None)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn parse_jid(chat: &ChatId) -> anyhow::Result<Jid> {
    chat.0
        .parse::<Jid>()
        .map_err(|e| anyhow::anyhow!("invalid chat id {}: {e}", chat.0))
}

async fn handle_command(shared: &Arc<Shared>, client: &Arc<Client>, cmd: BackendCommand) {
    match cmd {
        BackendCommand::SendText {
            chat,
            text,
            reply_to,
            local_id,
        } => match send_text(shared, client, &chat, text, reply_to).await {
            Ok(id) => {
                shared
                    .emit(BackendEvent::MessageSent {
                        chat,
                        local_id,
                        id: MessageId(id),
                    })
                    .await;
            }
            Err(e) => {
                tracing::warn!("send failed: {e:#}");
                shared
                    .emit(BackendEvent::SendFailed {
                        chat,
                        local_id,
                        error: format!("{e:#}"),
                    })
                    .await;
            }
        },
        BackendCommand::SendMedia {
            chat,
            local_id,
            path,
            kind,
            mime,
            file_name,
            caption,
            thumbnail,
            reply_to,
            ..
        } => {
            let shared = Arc::clone(shared);
            let client = Arc::clone(client);
            tokio::spawn(async move {
                let outgoing = OutgoingMedia {
                    path,
                    kind,
                    mime,
                    file_name,
                    caption,
                    thumbnail,
                    reply_to,
                };
                let ev = match send_media(&shared, &client, &chat, outgoing).await {
                    Ok(id) => BackendEvent::MessageSent {
                        chat,
                        local_id,
                        id: MessageId(id),
                    },
                    Err(e) => {
                        tracing::warn!("media send failed: {e:#}");
                        BackendEvent::SendFailed {
                            chat,
                            local_id,
                            error: format!("{e:#}"),
                        }
                    }
                };
                shared.emit(ev).await;
            });
        }
        BackendCommand::React {
            chat,
            target,
            emoji,
        } => match react(shared, client, &chat, &target, &emoji).await {
            Ok(()) => {
                shared
                    .emit(BackendEvent::Reaction(Reaction {
                        chat,
                        target,
                        sender: crate::backend::fake::OWN_REACTION_SENDER.to_owned(),
                        emoji,
                        timestamp: now_ms(),
                    }))
                    .await;
            }
            Err(e) => {
                tracing::warn!("reaction failed: {e:#}");
                shared
                    .emit(BackendEvent::Error(format!("Reaction not sent: {e:#}")))
                    .await;
            }
        },
        BackendCommand::DeleteMessage {
            chat,
            message,
            for_everyone,
        } => {
            let shared = Arc::clone(shared);
            let client = Arc::clone(client);
            tokio::spawn(async move {
                if for_everyone {
                    match revoke_for_everyone(&client, &chat, &message).await {
                        Ok(()) => {
                            shared
                                .emit(BackendEvent::MessageRevoked {
                                    chat,
                                    message,
                                    content: crate::model::MessageContent::Text(
                                        crate::model::REVOKED_BY_ME.to_owned(),
                                    ),
                                })
                                .await;
                        }
                        Err(e) => {
                            tracing::warn!("revoke failed: {e:#}");
                            shared
                                .emit(BackendEvent::Error(format!(
                                    "Not deleted for everyone: {e:#}"
                                )))
                                .await;
                        }
                    }
                } else if let Err(e) = delete_for_me(&shared, &client, &chat, &message).await {
                    // The local copy is already gone (runtime); the phone
                    // just keeps its own.
                    tracing::warn!("delete-for-me not synced to the phone: {e:#}");
                }
            });
        }
        BackendCommand::DeleteChat { chat } => {
            let client = Arc::clone(client);
            tokio::spawn(async move {
                let result = match parse_jid(&chat) {
                    Ok(jid) => client
                        .chat_actions()
                        .delete_chat(&jid, true, None)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}")),
                    Err(e) => Err(e),
                };
                if let Err(e) = result {
                    tracing::warn!("chat deletion not synced to the phone: {e:#}");
                }
            });
        }
        BackendCommand::MarkRead { chat, messages } => {
            if let Err(e) = mark_read(shared, client, &chat, &messages).await {
                tracing::warn!("read receipts failed: {e:#}");
            }
        }
        BackendCommand::FetchMedia {
            chat,
            message,
            media,
        } => {
            let shared = Arc::clone(shared);
            let client = Arc::clone(client);
            tokio::spawn(async move {
                let ev = match fetch_media(&shared.media_dir, &client, &message, &media).await {
                    Ok(path) => BackendEvent::MediaReady {
                        chat,
                        message,
                        path,
                    },
                    Err(e) => {
                        tracing::warn!("media download failed: {e:#}");
                        BackendEvent::MediaFailed {
                            chat,
                            message,
                            error: format!("{e:#}"),
                        }
                    }
                };
                shared.emit(ev).await;
            });
        }
        BackendCommand::FetchAvatar { chat } => {
            let shared = Arc::clone(shared);
            let client = Arc::clone(client);
            tokio::spawn(async move {
                let ev = {
                    // Held until the download finished; `Err` only if the
                    // semaphore was closed, which it never is.
                    let _slot = shared.avatar_slots.acquire().await;
                    match fetch_avatar(&shared.media_dir, &client, &chat).await {
                        Ok(Some(path)) => BackendEvent::AvatarReady { chat, path },
                        Ok(None) => {
                            tracing::debug!("no profile picture for {}", chat.0);
                            BackendEvent::AvatarMissing { chat }
                        }
                        Err(e) => {
                            log_avatar_failure(&chat, &e);
                            BackendEvent::AvatarMissing { chat }
                        }
                    }
                };
                shared.emit(ev).await;
            });
        }
        // Handled by the session loop.
        BackendCommand::Logout | BackendCommand::Shutdown => {}
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

async fn send_text(
    shared: &Shared,
    client: &Arc<Client>,
    chat: &ChatId,
    text: String,
    reply_to: Option<MessageId>,
) -> anyhow::Result<String> {
    let to = parse_jid(chat)?;
    let message = match quote_context(shared, client, chat, reply_to, &to) {
        Some(ctx) => wa::Message::text_with_context(text, ctx),
        None => wa::Message::text(text),
    };
    let result = client.send_message(to, message.clone()).await?;
    let own = own_jid(client);
    let meta = convert::MsgMeta {
        id: result.message_id.clone(),
        chat: chat.clone(),
        sender: own.clone(),
        sender_name: None,
        from_me: true,
        timestamp: now_ms(),
        status: crate::model::MessageStatus::Sent,
    };
    shared.put_keys(&[convert::key_record(&meta, &message, &own)]);
    Ok(result.message_id)
}

/// The quote context for a reply to `reply_to`, if we still know that message.
fn quote_context(
    shared: &Shared,
    client: &Arc<Client>,
    chat: &ChatId,
    reply_to: Option<MessageId>,
    to: &Jid,
) -> Option<wa::ContextInfo> {
    let (id, rec) = reply_to.and_then(|id| shared.key(chat, &id.0).map(|rec| (id, rec)))?;
    let quoted = rec
        .quote
        .as_deref()
        .and_then(|q| codec::message_decode(q).ok())
        .unwrap_or_default();
    let sender = rec
        .sender
        .parse::<Jid>()
        .ok()
        .or_else(|| if rec.from_me { client.pn() } else { None })
        .unwrap_or_else(|| to.clone());
    Some(proto_helpers::build_quote_context_with_info(
        &id.0, &sender, to, to, &quoted,
    ))
}

/// A file the UI prepared for sending.
struct OutgoingMedia {
    path: PathBuf,
    kind: crate::model::MediaKind,
    mime: String,
    file_name: String,
    caption: Option<String>,
    thumbnail: Option<Vec<u8>>,
    reply_to: Option<MessageId>,
}

/// Uploads the file (encrypted, to WhatsApp's CDN) and sends the matching
/// media message. Returns the server message id.
async fn send_media(
    shared: &Shared,
    client: &Arc<Client>,
    chat: &ChatId,
    media: OutgoingMedia,
) -> anyhow::Result<String> {
    use crate::model::MediaKind;
    use whatsapp_rust::media::{self, AudioOptions, DocumentOptions, ImageOptions, VideoOptions};
    use whatsapp_rust::wacore::download::MediaType;

    let to = parse_jid(chat)?;
    let data = tokio::fs::read(&media.path)
        .await
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", media.path.display()))?;
    let media_type = match media.kind {
        MediaKind::Image => MediaType::Image,
        MediaKind::Video => MediaType::Video,
        MediaKind::Audio => MediaType::Audio,
        MediaKind::Document => MediaType::Document,
    };
    let upload = client
        .upload(
            data,
            media_type,
            whatsapp_rust::upload::UploadOptions::new(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("upload failed: {e:#}"))?;
    let context_info = quote_context(shared, client, chat, media.reply_to, &to).map(Box::new);
    let message = match media.kind {
        MediaKind::Image => media::image_message(
            upload,
            ImageOptions {
                caption: media.caption,
                mimetype: Some(media.mime),
                jpeg_thumbnail: media.thumbnail,
                context_info,
            },
        ),
        MediaKind::Video => media::video_message(
            upload,
            VideoOptions {
                caption: media.caption,
                mimetype: Some(media.mime),
                jpeg_thumbnail: media.thumbnail,
                context_info,
                ..Default::default()
            },
        ),
        MediaKind::Audio => media::audio_message(
            upload,
            AudioOptions {
                mimetype: Some(media.mime),
                ptt: Some(false),
                context_info,
                ..Default::default()
            },
        ),
        MediaKind::Document => media::document_message(
            upload,
            DocumentOptions {
                mimetype: Some(media.mime),
                file_name: Some(media.file_name.clone()),
                title: Some(media.file_name),
                caption: media.caption,
                jpeg_thumbnail: media.thumbnail,
                context_info,
                ..Default::default()
            },
        ),
    };
    let result = client
        .send_message(to, message.clone())
        .await
        .map_err(|e| anyhow::anyhow!("send failed: {e:#}"))?;
    let own = own_jid(client);
    let meta = convert::MsgMeta {
        id: result.message_id.clone(),
        chat: chat.clone(),
        sender: own.clone(),
        sender_name: None,
        from_me: true,
        timestamp: now_ms(),
        status: crate::model::MessageStatus::Sent,
    };
    shared.put_keys(&[convert::key_record(&meta, &message, &own)]);
    Ok(result.message_id)
}

async fn react(
    shared: &Shared,
    client: &Arc<Client>,
    chat: &ChatId,
    target: &MessageId,
    emoji: &str,
) -> anyhow::Result<()> {
    let to = parse_jid(chat)?;
    let rec = shared.key(chat, &target.0);
    let (from_me, participant) = match (&rec, chat.is_group()) {
        (Some(r), true) => (r.from_me, Some(r.sender.clone()).filter(|s| !s.is_empty())),
        (Some(r), false) => (r.from_me, None),
        (None, false) => (false, None),
        (None, true) => anyhow::bail!("the original message is not known on this device"),
    };
    if chat.is_group() && participant.is_none() {
        anyhow::bail!("the original sender is unknown");
    }
    let key = wa::MessageKey {
        remote_jid: Some(to.to_string()),
        from_me: Some(from_me),
        id: Some(target.0.clone()),
        participant,
    };
    client.send_reaction(to, key, emoji).await?;
    Ok(())
}

async fn mark_read(
    shared: &Shared,
    client: &Arc<Client>,
    chat: &ChatId,
    messages: &[MessageId],
) -> anyhow::Result<()> {
    let to = parse_jid(chat)?;
    // Group read receipts are addressed per original sender.
    let mut by_sender: Vec<(Option<Jid>, Vec<String>)> = Vec::new();
    for id in messages.iter().filter(|m| !m.is_local()) {
        let rec = shared.key(chat, &id.0);
        if rec.as_ref().is_some_and(|r| r.from_me) {
            continue;
        }
        let sender = if chat.is_group() {
            match rec.and_then(|r| r.sender.parse::<Jid>().ok()) {
                Some(s) => Some(s),
                None => {
                    tracing::debug!("skipping read receipt for unknown group message {}", id.0);
                    continue;
                }
            }
        } else {
            None
        };
        match by_sender.iter_mut().find(|(s, _)| *s == sender) {
            Some((_, ids)) => ids.push(id.0.clone()),
            None => by_sender.push((sender, vec![id.0.clone()])),
        }
    }
    for (sender, ids) in by_sender {
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        client.mark_as_read(&to, sender.as_ref(), &refs).await?;
    }
    Ok(())
}

async fn fetch_media(
    media_dir: &Path,
    client: &Arc<Client>,
    message: &MessageId,
    media: &MediaRef,
) -> anyhow::Result<PathBuf> {
    let payload = convert::decode_media_token(&media.download_token)?;
    let target = media_dir.join(convert::media_file_name(&payload, &media.mime, &message.0));
    if tokio::fs::metadata(&target)
        .await
        .is_ok_and(|m| m.len() > 0)
    {
        return Ok(target);
    }
    let downloadable: &dyn Downloadable = match &payload {
        MediaPayload::Image(m) => m,
        MediaPayload::Sticker(m) => m,
        MediaPayload::Audio(m) => m,
        MediaPayload::Video(m) => m,
        MediaPayload::Document(m) => m,
    };
    tokio::fs::create_dir_all(media_dir).await?;
    // Stream to a temp name first (constant memory, whatever the size), so a
    // crash never leaves a truncated file under the final name (which the
    // cache check above would trust).
    let tmp = target.with_extension("part");
    let file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp)?;
    match client.download_to_writer(downloadable, file).await {
        Ok(file) => drop(file),
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
    }
    tokio::fs::rename(&tmp, &target).await?;
    Ok(target)
}

/// Looks a chat's preview profile picture up and caches it under
/// `media_dir`. `Ok(None)` means the chat has no picture (or we may not see
/// it); the picture is re-fetched on every call so a changed one shows up.
async fn fetch_avatar(
    media_dir: &Path,
    client: &Arc<Client>,
    chat: &ChatId,
) -> anyhow::Result<Option<PathBuf>> {
    let jid = parse_jid(chat)?;
    let Some(url) = lookup_avatar_url(client, &jid).await? else {
        return Ok(None);
    };
    // A plain CDN URL: no media key, nothing to decrypt. whatsapp-rust's own
    // HTTP client already does the blocking I/O on a blocking thread.
    let response = client
        .http_client
        .execute(HttpRequest::get(url))
        .await
        .map_err(|e| anyhow::anyhow!("avatar download failed: {e:#}"))?;
    if response.status_code != 200 {
        anyhow::bail!("avatar download returned HTTP {}", response.status_code);
    }
    if response.body.is_empty() {
        anyhow::bail!("avatar download returned an empty body");
    }
    tokio::fs::create_dir_all(media_dir).await?;
    let target = media_dir.join(convert::avatar_file_name(chat));
    // Write-then-rename so a reader never sees a half-written picture.
    let tmp = target.with_extension("part");
    if let Err(e) = tokio::fs::write(&tmp, &response.body).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    tokio::fs::rename(&tmp, &target).await?;
    Ok(Some(target))
}

/// The preview picture URL for `jid`, per [`convert::avatar_route`].
async fn lookup_avatar_url(client: &Arc<Client>, jid: &Jid) -> anyhow::Result<Option<String>> {
    let route = convert::avatar_route(jid);
    match client.contacts().get_profile_picture(jid, true).await {
        Ok(Some(pic)) if !pic.url.is_empty() => return Ok(Some(pic.url)),
        Ok(_) if route == convert::AvatarRoute::Contact => return Ok(None),
        Err(e) if route == convert::AvatarRoute::Contact => return Err(e.into()),
        Ok(_) => tracing::debug!("no picture for group {jid} via profile IQ; trying group query"),
        Err(e) => {
            tracing::debug!("profile IQ failed for group {jid}: {e:#}; trying group query")
        }
    }
    let pics = client
        .groups()
        .get_profile_pictures(vec![jid.clone()], PictureType::Preview)
        .await?;
    Ok(convert::group_picture_url(&pics, jid))
}

/// Avatars are decorative: a failure is only ever a log line. Rate limits
/// are called out so a flood of them is recognisable.
fn log_avatar_failure(chat: &ChatId, e: &anyhow::Error) {
    let chain: &(dyn std::error::Error + Send + Sync + 'static) = e.as_ref();
    match chain.server_rejection() {
        Some(r) if r.code == 429 => {
            tracing::warn!(
                "profile picture lookup for {} rate-limited by the server (backoff {:?}s)",
                chat.0,
                r.backoff
            );
        }
        Some(r) => tracing::debug!(
            "profile picture lookup for {} rejected: {} {}",
            chat.0,
            r.code,
            r.text
        ),
        None => tracing::warn!("profile picture for {} failed: {e:#}", chat.0),
    }
}

/// Translates bus events into `BackendEvent`s, in order.
struct EventPump {
    shared: Arc<Shared>,
    client: Arc<Client>,
    ctl: mpsc::UnboundedSender<SessionEnd>,
    attempt: u32,
    /// Between `Paired` and `SyncDone`.
    syncing: bool,
    sync_deadline: Option<tokio::time::Instant>,
}

impl EventPump {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Option<Arc<Event>>>) {
        loop {
            let deadline = self.sync_deadline;
            let next = tokio::select! {
                ev = rx.recv() => ev,
                () = async {
                    match deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.finish_sync().await;
                    continue;
                }
            };
            // A closed channel or the end-of-session marker.
            let Some(Some(ev)) = next else { break };
            self.handle(ev).await;
        }
    }

    async fn emit(&self, ev: BackendEvent) {
        self.shared.emit(ev).await;
    }

    async fn finish_sync(&mut self) {
        self.sync_deadline = None;
        if self.syncing {
            self.syncing = false;
            self.emit(BackendEvent::SyncProgress(1.0)).await;
            self.emit(BackendEvent::SyncDone).await;
        }
    }

    fn signal(&self, end: SessionEnd) {
        let _ = self.ctl.send(end);
    }

    async fn handle(&mut self, event: Arc<Event>) {
        match &*event {
            Event::PairingQrCode(qr) => {
                self.emit(BackendEvent::Qr {
                    code: qr.code.clone(),
                    valid_for_secs: qr.timeout.as_secs(),
                })
                .await;
            }
            Event::PairSuccess(_) => {
                self.syncing = true;
                self.sync_deadline = Some(tokio::time::Instant::now() + SYNC_QUIET * 2);
                self.emit(BackendEvent::Paired).await;
                self.emit(BackendEvent::SyncProgress(0.0)).await;
            }
            Event::PairError(e) => {
                self.emit(BackendEvent::Error(format!("Pairing failed: {}", e.error)))
                    .await;
            }
            Event::PairingQrCodesExhausted(_) => {
                self.emit(BackendEvent::Error(
                    "The QR code expired; generating a new one".to_owned(),
                ))
                .await;
                self.signal(SessionEnd::Restart);
            }
            Event::QrScannedWithoutMultidevice(_) => {
                self.emit(BackendEvent::Error(
                    "Update WhatsApp on your phone to link devices".to_owned(),
                ))
                .await;
            }
            Event::Connected(_) => {
                self.attempt = 0;
                self.emit(BackendEvent::Connection(ConnectionState::Connected))
                    .await;
                self.spawn_group_names();
            }
            Event::Disconnected(_) => {
                self.attempt = self.attempt.saturating_add(1);
                self.emit(BackendEvent::Connection(ConnectionState::Reconnecting {
                    attempt: self.attempt,
                    retry_in_secs: convert::reconnect_delay_secs(self.attempt),
                }))
                .await;
            }
            Event::LoggedOut(l) => {
                tracing::info!(
                    "logged out (reason {:?}, on_connect {})",
                    l.reason,
                    l.on_connect
                );
                self.shared.emit_logged_out().await;
                self.signal(SessionEnd::LoggedOut);
            }
            Event::StreamReplaced(_) => {
                self.emit(BackendEvent::Error(
                    "WhatsApp was opened on another computer with this session".to_owned(),
                ))
                .await;
                self.signal(SessionEnd::Stopped);
            }
            Event::TemporaryBan(b) => {
                self.emit(BackendEvent::Error(format!(
                    "WhatsApp temporarily banned this account: {}",
                    b.code
                )))
                .await;
            }
            Event::ClientOutdated(_) => {
                self.emit(BackendEvent::Error(
                    "WhatsApp rejected this client version; an update is needed".to_owned(),
                ))
                .await;
                self.signal(SessionEnd::Stopped);
            }
            Event::ConnectFailure(f) if !f.reason.is_logged_out() => {
                tracing::warn!("connect failure: {:?} {:?}", f.reason, f.message);
            }
            Event::Messages(batch) => {
                for inbound in batch.iter() {
                    self.on_message(&inbound.message, &inbound.info).await;
                }
            }
            Event::Receipt(r) => {
                let group = r.source.is_group || r.source.chat.is_group();
                if !group {
                    self.learn_lid(&r.source.chat).await;
                }
                let chat = convert::chat_id_from_source(&r.source, &self.shared.lids());
                if r.r#type == ReceiptType::ReadSelf {
                    // We read these on another device (the phone).
                    let read_up_to = r
                        .message_ids
                        .iter()
                        .filter_map(|id| self.shared.key(&chat, id))
                        .map(|rec| rec.timestamp)
                        .max();
                    if let Some(ts) = read_up_to {
                        self.read_elsewhere(&chat, ts).await;
                    }
                    return;
                }
                let Some(status) = convert::receipt_status_in(&r.r#type, group) else {
                    return;
                };
                for id in &r.message_ids {
                    self.emit(BackendEvent::MessageStatus {
                        chat: chat.clone(),
                        message: MessageId(id.clone()),
                        status,
                    })
                    .await;
                }
            }
            Event::HistorySync(_) => self.on_history(Arc::clone(&event)).await,
            Event::PushNameUpdate(p) => {
                let chat = convert::chat_id_for(&p.jid, None, &self.shared.lids());
                if !chat.is_group() {
                    self.shared
                        .update_name(&chat, &p.new_push_name, convert::RANK_PUSH)
                        .await;
                }
            }
            Event::ContactUpdate(c) => {
                let name = c
                    .action
                    .full_name
                    .clone()
                    .or_else(|| c.action.first_name.clone())
                    .unwrap_or_default();
                let lids = self.shared.lids();
                let mut chats = vec![convert::chat_id_for(&c.jid, None, &lids)];
                if let Some(pn) = c
                    .action
                    .pn_jid
                    .as_deref()
                    .and_then(|p| convert::chat_id_from_str(p, &lids))
                {
                    chats.push(pn);
                }
                chats.dedup();
                for chat in chats {
                    self.shared
                        .update_name(&chat, &name, convert::RANK_CONTACT)
                        .await;
                }
            }
            Event::MuteUpdate(m) => {
                let chat = convert::chat_id_for(&m.jid, None, &self.shared.lids());
                let muted = m.action.muted.unwrap_or(false)
                    && m.action
                        .mute_end_timestamp
                        .is_none_or(|end| end == 0 || convert::muted_until(end, now_ms()));
                let (changed, known) = {
                    let idx = lock(&self.shared.index);
                    let changed = idx.set_muted(&chat, muted).unwrap_or(false);
                    (
                        changed,
                        idx.chat_meta(&chat).map(|m| m.known).unwrap_or(false),
                    )
                };
                if changed && known {
                    self.emit(BackendEvent::ChatUpserted(self.shared.chat_snapshot(&chat)))
                        .await;
                }
            }
            Event::GroupUpdate(g) => {
                let jid = g.group_jid.clone();
                let shared = Arc::clone(&self.shared);
                let client = Arc::clone(&self.client);
                tokio::spawn(async move {
                    match client.groups().get_metadata(&jid).await {
                        Ok(meta) => {
                            let chat = ChatId(jid.to_non_ad_string());
                            shared
                                .update_name(&chat, &meta.subject, convert::RANK_CONTACT)
                                .await;
                        }
                        Err(e) => tracing::debug!("group metadata refresh failed: {e}"),
                    }
                });
            }
            Event::MarkChatAsReadUpdate(u) => {
                if u.action.read != Some(true) {
                    return; // "mark as unread" is not mirrored
                }
                let Some(ts) = u
                    .action
                    .message_range
                    .as_option()
                    .and_then(|r| r.last_message_timestamp)
                else {
                    return;
                };
                if !u.jid.is_group() {
                    self.learn_lid(&u.jid).await;
                }
                let chat = convert::chat_id_for(&u.jid, None, &self.shared.lids());
                self.read_elsewhere(&chat, convert::sync_ts_to_ms(ts)).await;
            }
            Event::OfflineSyncCompleted(c) => {
                tracing::debug!("offline sync completed ({} items)", c.count);
            }
            _ => {}
        }
    }

    /// Learns the phone number behind a LID-addressed peer from the client's
    /// own LID→PN cache, so LID-addressed traffic maps to the PN chat.
    async fn learn_lid(&self, jid: &Jid) {
        if !jid.is_lid() {
            return;
        }
        let lid_user = jid.user_base().to_owned();
        if self.shared.lids().contains_key(&lid_user) {
            return;
        }
        if let Ok(Some(entry)) = self.client.get_lid_pn_entry(jid).await {
            self.shared.remember_lid(&lid_user, &entry.phone_number);
        }
    }

    /// The chat was read on another device up to `read_up_to` (ms): clear
    /// its unread count unless newer messages arrived since.
    async fn read_elsewhere(&self, chat: &ChatId, read_up_to: i64) {
        let known = lock(&self.shared.index)
            .chat_meta(chat)
            .map(|m| m.known)
            .unwrap_or(false);
        if !known {
            return;
        }
        let marker = convert::read_marker(self.shared.chat_snapshot(chat), read_up_to);
        self.emit(BackendEvent::ChatUpserted(marker)).await;
    }

    /// Announces every group we participate in, with its subject.
    fn spawn_group_names(&self) {
        let shared = Arc::clone(&self.shared);
        let client = Arc::clone(&self.client);
        tokio::spawn(async move {
            match client.groups().get_participating().await {
                Ok(groups) => {
                    for (jid, meta) in groups {
                        let chat = ChatId(jid.to_non_ad_string());
                        shared
                            .update_name(&chat, &meta.subject, convert::RANK_CONTACT)
                            .await;
                        shared.ensure_known(&chat).await;
                    }
                }
                Err(e) => tracing::warn!("could not list groups: {e}"),
            }
        });
    }

    async fn on_message(
        &self,
        msg: &wa::Message,
        info: &whatsapp_rust::types::message::MessageInfo,
    ) {
        if !convert::is_supported_chat(&info.source.chat) {
            return;
        }
        let own = own_jid(&self.client);
        // Learn LID → phone number mappings so LID-addressed direct chats
        // merge with their phone-number chat.
        if !info.source.is_group {
            self.learn_lid(&info.source.chat).await;
        }
        let lids = self.shared.lids();
        let meta = convert::meta_from_info(info, &own, &lids);
        let raw_sender = meta.sender.clone();
        let record = convert::key_record(&meta, msg, &raw_sender);
        let chat = meta.chat.clone();
        let push = (!meta.from_me && !chat.is_group())
            .then(|| meta.sender_name.clone())
            .flatten();
        match convert::convert_message(meta, msg) {
            Converted::Message(m) => {
                self.shared.put_keys(&[record]);
                if let Some(push) = push {
                    self.shared
                        .update_name(&chat, &push, convert::RANK_PUSH)
                        .await;
                }
                self.shared.ensure_known(&chat).await;
                self.emit(BackendEvent::Message(m)).await;
            }
            Converted::Reaction(r) => {
                self.shared.ensure_known(&chat).await;
                self.emit(BackendEvent::Reaction(r)).await;
            }
            Converted::Ignored(why) => tracing::trace!("ignored message {}: {why}", info.id),
        }
    }

    async fn on_history(&mut self, event: Arc<Event>) {
        let Event::HistorySync(lazy) = &*event else {
            return;
        };
        let progress = lazy.progress();
        let own = own_jid(&self.client);
        let lids = self.shared.lids();
        let ev = Arc::clone(&event);
        let batch = tokio::task::spawn_blocking(move || {
            let Event::HistorySync(lazy) = &*ev else {
                return None;
            };
            lazy.get()
                .map(|hs| convert::convert_history(hs, &own, &lids, now_ms()))
        })
        .await;
        let batch = match batch {
            Ok(Some(b)) => b,
            Ok(None) => {
                tracing::warn!("could not decode a history sync chunk");
                return;
            }
            Err(e) => {
                tracing::error!("history sync conversion task failed: {e}");
                return;
            }
        };

        for (lid, pn) in &batch.lids {
            self.shared.remember_lid(lid, pn);
        }
        let mut chats = Vec::with_capacity(batch.chats.len());
        for (chat, name) in batch.chats {
            {
                let idx = lock(&self.shared.index);
                if let Some(name) = &name
                    && let Err(e) = idx.set_name(&chat.id, name, convert::RANK_CONVERSATION)
                {
                    tracing::warn!("index write failed: {e:#}");
                }
                let _ = idx.set_muted(&chat.id, chat.muted);
                let _ = idx.mark_known(&chat.id);
            }
            chats.push(chat);
        }
        for (chat, name) in &batch.push_names {
            if !chat.is_group() {
                self.shared
                    .update_name(chat, name, convert::RANK_PUSH)
                    .await;
            }
        }
        // Best-known names, now that this chunk's names are recorded.
        for chat in &mut chats {
            chat.name = self.shared.chat_snapshot(&chat.id).name;
        }
        let records: Vec<KeyRecord> = batch.items.iter().filter_map(|(_, r)| r.clone()).collect();
        self.shared.put_keys(&records);
        let items = batch.items.into_iter().map(|(item, _)| item).collect();
        for ev in convert::history_events(items, chats) {
            self.emit(ev).await;
        }

        if let Some(p) = progress {
            self.emit(BackendEvent::SyncProgress((p.min(100) as f32) / 100.0))
                .await;
        }
        if self.syncing {
            if progress.is_some_and(|p| p >= 100) {
                self.finish_sync().await;
            } else {
                self.sync_deadline = Some(tokio::time::Instant::now() + SYNC_QUIET);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_lives_next_to_session_db() {
        assert_eq!(
            index_path_for(Path::new("/data/wa/session.db")),
            PathBuf::from("/data/wa/session.db.index")
        );
    }

    #[test]
    fn remove_db_files_removes_side_files() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = dir.path().join("s.db");
        for suffix in ["", "-wal", "-shm"] {
            std::fs::write(format!("{}{suffix}", db.display()), b"x").expect("write");
        }
        remove_db_files(&db);
        assert_eq!(std::fs::read_dir(dir.path()).expect("ls").count(), 0);
        // Missing files are fine.
        remove_db_files(&db);
    }

    #[test]
    fn wipe_media_empties_the_cache_dir() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join("a.jpg"), b"x").expect("write");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        std::fs::write(dir.path().join("sub/b.ogg"), b"y").expect("write");
        wipe_media(dir.path());
        assert!(dir.path().exists());
        assert_eq!(std::fs::read_dir(dir.path()).expect("ls").count(), 0);
        // A missing directory is fine.
        wipe_media(&dir.path().join("missing"));
    }

    #[tokio::test]
    async fn idle_rejects_commands_and_stops_on_shutdown() {
        let (ev_tx, mut ev_rx) = mpsc::channel(8);
        let shared = Shared {
            events: ev_tx,
            index: Mutex::new(Index::open_in_memory().expect("index")),
            index_path: PathBuf::from("/nonexistent/index"),
            lids: Mutex::new(LidMap::new()),
            media_dir: PathBuf::from("/nonexistent"),
            logged_out_sent: AtomicBool::new(false),
            avatar_slots: Semaphore::new(MAX_AVATAR_DOWNLOADS),
        };
        let (cmd_tx, mut cmd_rx) = mpsc::channel(8);
        let chat = ChatId("1@s.whatsapp.net".into());
        let local_id = MessageId::new_local();
        cmd_tx
            .send(BackendCommand::SendText {
                chat: chat.clone(),
                text: "hi".into(),
                reply_to: None,
                local_id: local_id.clone(),
            })
            .await
            .expect("send");
        cmd_tx.send(BackendCommand::Shutdown).await.expect("send");
        assert!(matches!(
            idle(&shared, &mut cmd_rx, None).await,
            Idle::Shutdown
        ));
        match ev_rx.recv().await {
            Some(BackendEvent::SendFailed {
                chat: c,
                local_id: l,
                ..
            }) => {
                assert_eq!(c, chat);
                assert_eq!(l, local_id);
            }
            other => panic!("expected SendFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn logged_out_is_emitted_once() {
        let (ev_tx, mut ev_rx) = mpsc::channel(8);
        let shared = Shared {
            events: ev_tx,
            index: Mutex::new(Index::open_in_memory().expect("index")),
            index_path: PathBuf::from("/nonexistent/index"),
            lids: Mutex::new(LidMap::new()),
            media_dir: PathBuf::from("/nonexistent"),
            logged_out_sent: AtomicBool::new(false),
            avatar_slots: Semaphore::new(MAX_AVATAR_DOWNLOADS),
        };
        shared.emit_logged_out().await;
        shared.emit_logged_out().await;
        drop(shared);
        assert_eq!(ev_rx.recv().await, Some(BackendEvent::LoggedOut));
        assert_eq!(ev_rx.recv().await, None);
    }

    #[tokio::test]
    async fn names_are_announced_only_for_known_chats() {
        let (ev_tx, mut ev_rx) = mpsc::channel(8);
        let shared = Shared {
            events: ev_tx,
            index: Mutex::new(Index::open_in_memory().expect("index")),
            index_path: PathBuf::from("/nonexistent/index"),
            lids: Mutex::new(LidMap::new()),
            media_dir: PathBuf::from("/nonexistent"),
            logged_out_sent: AtomicBool::new(false),
            avatar_slots: Semaphore::new(MAX_AVATAR_DOWNLOADS),
        };
        let chat = ChatId("15551234567@s.whatsapp.net".into());
        shared.update_name(&chat, "Alice", convert::RANK_PUSH).await;
        shared.ensure_known(&chat).await;
        shared.ensure_known(&chat).await;
        shared
            .update_name(&chat, "Alice Smith", convert::RANK_CONTACT)
            .await;
        drop(shared);
        let mut names = Vec::new();
        while let Some(ev) = ev_rx.recv().await {
            match ev {
                BackendEvent::ChatUpserted(c) => names.push(c.name),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(names, vec!["Alice".to_owned(), "Alice Smith".to_owned()]);
    }
}
