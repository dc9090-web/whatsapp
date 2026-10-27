//! The `cosmic::Application`: top-level state, messages and update logic.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use cosmic::app::{Core, Task, context_drawer};
use cosmic::iced::widget::scrollable::RelativeOffset;
use cosmic::iced::{Alignment, Background, Length, Size, Subscription, window};
use cosmic::widget::{self, button, container, icon, popover};
use cosmic::{Application, ApplicationExt, Element, cosmic_config, theme};
use wa_core::{
    BackendCommand, BackendEvent, Chat, ChatId, ConnectionState, MessageContent, MessageId, Store,
};

use crate::audio::{self, AudioEvent, AudioHandle};
use crate::backend_sub::{self, BackendConfig, BackendMsg, CommandSender};
use crate::config::{Appearance, Config};
use crate::desktop::{self, DesktopEvent, TrayHandle};
use crate::pages::conversation::{self, Conversation, PAGE_SIZE};
use crate::pages::{self, Page};
use crate::signals;
use crate::widgets::media::{self, MAX_INLINE, Thumb};
use crate::widgets::reactions::ALL_EMOJIS;

pub const APP_ID: &str = "com.github.whatsapp-cosmic";
const TITLE: &str = "WhatsApp";
/// Incoming messages older than this (history sync, catch-up) never notify.
const NOTIFY_MAX_AGE_MS: i64 = 2 * 60 * 1000;
/// Store reloads triggered by backend events are coalesced to at most one
/// per this interval (a history burst can deliver thousands of events).
const RELOAD_COALESCE: Duration = Duration::from_millis(100);
/// Upper bound on waiting for the backend's graceful shutdown on Quit
/// (its own disconnect timeout is 5 s, plus draining queued events).
const QUIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Startup parameters from `main`.
pub struct Flags {
    pub backend: BackendConfig,
    pub config: Config,
    pub config_handler: Option<cosmic_config::Config>,
    /// Keeps the `--fake` temp dir alive for the life of the app.
    pub temp_dir: Option<tempfile::TempDir>,
}

#[derive(Debug, Clone)]
pub enum Dialog {
    EmojiPicker(MessageId),
    ImageViewer { path: PathBuf },
    ConfirmLogout,
}

// Transient UI messages; boxing the event would only add allocations.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Message {
    BackendReady(CommandSender),
    Backend(BackendEvent),
    /// The backend finished shutting down (its event stream ended).
    BackendClosed,
    /// Flush coalesced store reloads.
    Reload,
    /// A background thumbnail decode finished.
    ThumbDecoded(ChatId, MessageId, Option<Thumb>),
    WindowFocused(window::Id),
    Desktop(DesktopEvent),
    Audio(AudioEvent),

    SelectChat(ChatId),
    CloseChat,
    ComposerChanged(String),
    SendComposer,
    StartReply(MessageId),
    CancelReply,
    React(MessageId, String),
    OpenMessageMenu(MessageId),
    CloseMessageMenu,
    OpenEmojiPicker(MessageId),
    CopyText(String),
    Hover(Option<MessageId>),
    /// The pointer left this message's bubble/actions area.
    HoverEnd(MessageId),
    Retry(MessageId),
    ConversationScrolled(f32),

    OpenMedia(MessageId),
    FetchMedia(MessageId),
    /// Copy a message's media into the Downloads folder (fetching it first).
    SaveMedia(MessageId),
    MediaSaved(MessageId, Result<PathBuf, String>),
    ToggleVoice(MessageId),
    OpenFile(PathBuf),
    CloseDialog,

    ToggleHeaderMenu,
    ShowSettings,
    CloseSettings,
    SetAppearance(Appearance),
    SetPreviews(bool),
    SetAutostart(bool),
    RequestLogout,
    ConfirmLogout,
    DismissError,

    CloseRequested(window::Id),
    WindowClosed(window::Id),
    HideWindow,
    Quit,
    Exit,
    None,
}

pub struct App {
    pub core: Core,
    pub backend_config: BackendConfig,
    pub page: Page,
    pub qr: Option<pages::login::QrImage>,
    pub sync_progress: f32,
    pub connection: Option<ConnectionState>,
    pub commands: Option<CommandSender>,
    pub store: Option<Store>,
    pub chats: Vec<Chat>,
    pub total_unread: u32,
    pub selected: Option<ChatId>,
    pub conversation: Option<Conversation>,
    pub composer: String,
    pub replying_to: Option<wa_core::Message>,
    pub hovered: Option<MessageId>,
    pub menu_for: Option<MessageId>,
    pub dialog: Option<Dialog>,
    pub header_menu_open: bool,
    pub show_settings: bool,
    pub config: Config,
    pub config_handler: Option<cosmic_config::Config>,
    pub autostart: Option<bool>,
    /// Our own JID, learned from `from_me` messages (for "mine" reactions).
    pub own_jid: Option<String>,
    pub fetching: HashSet<MessageId>,
    pub media_errors: HashMap<MessageId, String>,
    /// Media to show/open as soon as its download finishes.
    pub open_after_fetch: HashSet<MessageId>,
    /// Media to copy into Downloads once its download finishes.
    pub save_after_fetch: HashSet<MessageId>,
    /// Where media was saved this session (shown under the message).
    pub saved_media: HashMap<MessageId, PathBuf>,
    /// Downloaded profile pictures by chat.
    pub avatars: HashMap<ChatId, PathBuf>,
    /// Chats whose picture was already requested this session.
    pub avatars_requested: HashSet<ChatId>,
    pub play_after_fetch: Option<MessageId>,
    pub audio: Option<AudioHandle>,
    pub playing: Option<MessageId>,
    pub audio_paused: bool,
    pub tray: Option<TrayHandle>,
    pub error: Option<String>,
    pub logout_requested: bool,
    /// Quit was requested; exit once the backend has shut down.
    pub quitting: bool,
    /// Coalesced reload state (see `RELOAD_COALESCE`).
    pub chats_dirty: bool,
    pub conversation_dirty: bool,
    pub reload_scheduled: bool,
    /// Thumbnails being decoded in the background.
    pub thumbs_pending: HashSet<MessageId>,
    /// Decodes already tried for the open conversation, and whether the
    /// downloaded file was available for that try (failed decodes of a
    /// file are not retried on every reload).
    pub thumb_attempts: HashMap<MessageId, bool>,
    _temp_dir: Option<tempfile::TempDir>,
}

/// Should an incoming message raise a desktop notification?
/// Not for our own or muted messages, not for old (synced) ones, and not when
/// the user is already looking at that chat in a focused window.
pub fn should_notify(
    msg: &wa_core::Message,
    now_ms: i64,
    focused: bool,
    open_chat: Option<&ChatId>,
    muted: bool,
) -> bool {
    if msg.from_me || muted || now_ms - msg.timestamp > NOTIFY_MAX_AGE_MS {
        return false;
    }
    !(focused && open_chat == Some(&msg.chat))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl App {
    fn new(core: Core, flags: Flags) -> Self {
        let store = match Store::open(&flags.backend.app_db) {
            Ok(store) => Some(store),
            Err(err) => {
                tracing::error!(?err, "UI cannot open app database");
                None
            }
        };
        let mut app = Self::blank(core, flags.backend, flags.config, flags.config_handler);
        app._temp_dir = flags.temp_dir;
        app.autostart = desktop::autostart_enabled();
        if store.is_none() {
            app.error = Some("Cannot open the local message database; see the log.".into());
        }
        app.store = store;
        let _ = app.reload_chats();
        // Returning users go straight to their chats; first runs wait for a QR.
        app.page = if app.chats.is_empty() {
            Page::Login
        } else {
            Page::Main
        };
        app
    }

    /// State without any I/O (also used by tests).
    fn blank(
        core: Core,
        backend_config: BackendConfig,
        config: Config,
        config_handler: Option<cosmic_config::Config>,
    ) -> Self {
        Self {
            core,
            backend_config,
            page: Page::Login,
            qr: None,
            sync_progress: 0.0,
            connection: None,
            commands: None,
            store: None,
            chats: Vec::new(),
            total_unread: 0,
            selected: None,
            conversation: None,
            composer: String::new(),
            replying_to: None,
            hovered: None,
            menu_for: None,
            dialog: None,
            header_menu_open: false,
            show_settings: false,
            config,
            config_handler,
            autostart: None,
            own_jid: None,
            fetching: HashSet::new(),
            media_errors: HashMap::new(),
            open_after_fetch: HashSet::new(),
            save_after_fetch: HashSet::new(),
            saved_media: HashMap::new(),
            avatars: HashMap::new(),
            avatars_requested: HashSet::new(),
            play_after_fetch: None,
            audio: None,
            playing: None,
            audio_paused: false,
            tray: None,
            error: None,
            logout_requested: false,
            quitting: false,
            chats_dirty: false,
            conversation_dirty: false,
            reload_scheduled: false,
            thumbs_pending: HashSet::new(),
            thumb_attempts: HashMap::new(),
            _temp_dir: None,
        }
    }

    fn window_focused(&self) -> bool {
        self.core
            .main_window_id()
            .is_some_and(|id| self.core.focused_window() == Some(id))
    }

    fn send(&self, cmd: BackendCommand) -> bool {
        match &self.commands {
            Some(commands) => commands.send(cmd),
            None => {
                tracing::warn!(?cmd, "backend not ready; command dropped");
                false
            }
        }
    }

    fn store_error(&mut self, what: &str, err: anyhow::Error) {
        tracing::error!(?err, "{what}");
        self.error = Some(format!("{what}: {err:#}"));
    }

    /// Re-reads the chat list and returns a task updating the tray badge.
    fn reload_chats(&mut self) -> Task<Message> {
        let Some(store) = &self.store else {
            return Task::none();
        };
        let chats = store.chats();
        let total = store.total_unread();
        let avatars = store.chat_avatars();
        match chats {
            Ok(chats) => self.chats = chats,
            Err(err) => self.store_error("Failed to load chats", err),
        }
        match avatars {
            Ok(avatars) => self.avatars = avatars,
            Err(err) => tracing::error!(?err, "failed to load profile pictures"),
        }
        self.request_avatars();
        let total = match total {
            Ok(total) => total,
            Err(err) => {
                tracing::error!(?err, "failed to count unread");
                self.total_unread
            }
        };
        if let Some(conv) = &mut self.conversation
            && let Some(chat) = self.chats.iter().find(|c| c.id == conv.chat.id)
        {
            conv.chat = chat.clone();
        }
        self.set_total_unread(total)
    }

    /// Asks the backend for pictures of chats that have none yet, most
    /// recent first and a bounded number per reload.
    fn request_avatars(&mut self) {
        const PER_RELOAD: usize = 40;
        if self.commands.is_none() || self.page != Page::Main {
            return;
        }
        let wanted: Vec<ChatId> = self
            .chats
            .iter()
            .map(|c| c.id.clone())
            .filter(|id| !self.avatars.contains_key(id) && !self.avatars_requested.contains(id))
            .take(PER_RELOAD)
            .collect();
        for chat in wanted {
            if self.send(BackendCommand::FetchAvatar { chat: chat.clone() }) {
                self.avatars_requested.insert(chat);
            }
        }
    }

    /// Copies a message's local media into Downloads on a blocking thread.
    fn save_media(&mut self, id: &MessageId) -> Task<Message> {
        let Some(msg) = self.find_message(id) else {
            return Task::none();
        };
        let Some(local) = msg.content.media().and_then(media::local_file) else {
            return Task::none();
        };
        let name = media::save_name(&msg.content, local, msg.timestamp);
        let source = local.to_path_buf();
        let id = id.clone();
        cosmic::task::future(async move {
            let result =
                tokio::task::spawn_blocking(move || media::save_to_downloads(&source, &name))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
            Message::MediaSaved(id, result)
        })
    }

    /// Updates the cached unread total and the tray badge when it changed.
    fn set_total_unread(&mut self, total: u32) -> Task<Message> {
        let changed = total != self.total_unread;
        self.total_unread = total;
        match (&self.tray, changed) {
            (Some(tray), true) => {
                let tray = tray.clone();
                cosmic::task::future(async move {
                    tray.set_unread(total).await;
                    Message::None
                })
            }
            _ => Task::none(),
        }
    }

    /// Re-reads the open conversation, keeping at least as many messages as
    /// loaded. Returns the background thumbnail decodes it needs.
    fn reload_conversation(&mut self) -> Task<Message> {
        let (Some(store), Some(conv)) = (&self.store, &mut self.conversation) else {
            return Task::none();
        };
        let limit = conv.messages.len().max(PAGE_SIZE) + 1;
        match store.messages(&conv.chat.id, None, limit) {
            Ok(messages) => {
                if conv.messages.is_empty() && messages.len() < PAGE_SIZE {
                    conv.exhausted = true;
                }
                conv.messages = messages;
            }
            Err(err) => {
                tracing::error!(?err, "failed to load messages");
                self.error = Some(format!("Failed to load messages: {err:#}"));
                return Task::none();
            }
        }
        // One query for the whole page instead of one per message.
        conv.reactions = match conv.oldest_ts() {
            Some(oldest) => match store.reactions_since(&conv.chat.id, oldest) {
                Ok(list) => group_reactions(list, &conv.messages),
                Err(err) => {
                    tracing::error!(?err, "failed to load reactions");
                    HashMap::new()
                }
            },
            None => HashMap::new(),
        };
        if self.own_jid.is_none() {
            self.own_jid = conv
                .messages
                .iter()
                .find(|m| m.from_me && !m.sender.is_empty())
                .map(|m| m.sender.clone());
        }
        // Inline previews are tiny (stickers often have none), so download
        // stickers and images right away instead of upscaling the preview.
        let auto: Vec<MessageId> = conv
            .messages
            .iter()
            .filter(|m| media::auto_download(&m.content))
            .filter(|m| conv.thumbs.get(&m.id).is_none_or(|t| !t.full))
            .filter(|m| m.content.media().and_then(media::local_file).is_none())
            .map(|m| m.id.clone())
            .collect();
        for id in auto {
            if !self.fetching.contains(&id) && !self.media_errors.contains_key(&id) {
                self.request_media(&id);
            }
        }
        self.refresh_thumbs()
    }

    /// Starts background decodes for image/sticker thumbnails that are new
    /// or whose full file got downloaded. Decoding a large photo takes
    /// 100 ms+, so it never runs on the UI thread.
    fn refresh_thumbs(&mut self) -> Task<Message> {
        let Some(conv) = &mut self.conversation else {
            return Task::none();
        };
        let ids: HashSet<&MessageId> = conv.messages.iter().map(|m| &m.id).collect();
        conv.thumbs.retain(|id, _| ids.contains(id));
        let mut tasks = Vec::new();
        for msg in &conv.messages {
            let media = match &msg.content {
                MessageContent::Image { media, .. } | MessageContent::Sticker { media } => media,
                _ => continue,
            };
            let has_local = media::local_file(media).is_some();
            if !thumb_needed(
                conv.thumbs.get(&msg.id).map(|t| t.full),
                self.thumb_attempts.get(&msg.id).copied(),
                self.thumbs_pending.contains(&msg.id),
                has_local,
            ) {
                continue;
            }
            self.thumbs_pending.insert(msg.id.clone());
            self.thumb_attempts.insert(msg.id.clone(), has_local);
            let (chat, id, media) = (msg.chat.clone(), msg.id.clone(), media.clone());
            tasks.push(cosmic::task::future(async move {
                let thumb = tokio::task::spawn_blocking(move || {
                    media::decode_thumb(&media, (MAX_INLINE * 2.0) as u32)
                })
                .await
                .ok()
                .flatten();
                Message::ThumbDecoded(chat, id, thumb)
            }));
        }
        Task::batch(tasks)
    }

    /// Marks store-backed state stale; one `Message::Reload` refreshes it.
    fn schedule_reload(&mut self, conversation: bool) -> Task<Message> {
        self.chats_dirty = true;
        self.conversation_dirty |= conversation;
        if std::mem::replace(&mut self.reload_scheduled, true) {
            return Task::none();
        }
        cosmic::task::future(async {
            tokio::time::sleep(RELOAD_COALESCE).await;
            Message::Reload
        })
    }

    fn flush_reload(&mut self) -> Task<Message> {
        self.reload_scheduled = false;
        let mut tasks = Vec::with_capacity(2);
        if std::mem::take(&mut self.chats_dirty) {
            tasks.push(self.reload_chats());
        }
        if std::mem::take(&mut self.conversation_dirty) {
            tasks.push(self.reload_conversation());
        }
        Task::batch(tasks)
    }

    fn load_older(&mut self) -> Task<Message> {
        let (Some(store), Some(conv)) = (&self.store, &mut self.conversation) else {
            return Task::none();
        };
        let Some(oldest) = conv.messages.first() else {
            conv.exhausted = true;
            return Task::none();
        };
        conv.loading_older = true;
        // A (timestamp, row) cursor: messages sharing the oldest loaded
        // second are not skipped.
        match store.messages_before(&conv.chat.id, oldest.timestamp, &oldest.id, PAGE_SIZE) {
            Ok(mut older) => {
                if older.len() < PAGE_SIZE {
                    conv.exhausted = true;
                }
                if let Some(first) = older.first() {
                    match store.reactions_since(&conv.chat.id, first.timestamp) {
                        Ok(list) => conv.reactions.extend(group_reactions(list, &older)),
                        Err(err) => tracing::error!(?err, "failed to load reactions"),
                    }
                }
                older.append(&mut conv.messages);
                conv.messages = older;
            }
            Err(err) => {
                tracing::error!(?err, "failed to load older messages");
                conv.exhausted = true;
            }
        }
        conv.loading_older = false;
        self.refresh_thumbs()
    }

    fn open_chat(&mut self, id: ChatId) -> Task<Message> {
        let chat = self
            .chats
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .or_else(|| self.store.as_ref().and_then(|s| s.chat(&id).ok().flatten()));
        let Some(chat) = chat else {
            tracing::warn!(chat = %id.0, "cannot open unknown chat");
            return Task::none();
        };
        if self.selected.as_ref() != Some(&id) {
            self.replying_to = None;
            self.composer.clear();
            self.hovered = None;
            self.menu_for = None;
        }
        self.page = Page::Main;
        self.selected = Some(id);
        self.conversation = Some(Conversation::new(chat));
        // Thumbnails are per conversation; decode afresh.
        self.thumb_attempts.clear();
        self.thumbs_pending.clear();
        let load = self.reload_conversation();
        let mark = self.mark_open_chat_read();
        Task::batch([
            load,
            mark,
            cosmic::iced::widget::scrollable::snap_to(
                conversation::scroll_id(),
                RelativeOffset {
                    x: None,
                    y: Some(0.0),
                },
            ),
            widget::text_input::focus(conversation::composer_id()),
        ])
    }

    /// Sends `MarkRead` for the open chat's unread incoming messages.
    fn mark_open_chat_read(&mut self) -> Task<Message> {
        let Some(conv) = &self.conversation else {
            return Task::none();
        };
        if conv.chat.unread == 0 {
            return Task::none();
        }
        // May be empty (the unread messages were not stored, e.g. filtered
        // stubs): the command still clears the count, it just sends no
        // receipts.
        let ids = conversation::unread_incoming_ids(&conv.messages, conv.chat.unread);
        let chat = conv.chat.id.clone();
        if self.send(BackendCommand::MarkRead {
            chat: chat.clone(),
            messages: ids,
        }) {
            return self.mark_read_locally(&chat);
        }
        Task::none()
    }

    /// Mirrors a sent `MarkRead` in the UI state right away. The runtime
    /// updates the store asynchronously and emits no event for it, so a
    /// reload right now could still see the old count.
    fn mark_read_locally(&mut self, chat: &ChatId) -> Task<Message> {
        if let Some(conv) = &mut self.conversation
            && &conv.chat.id == chat
        {
            conv.chat.unread = 0;
        }
        if let Some(c) = self.chats.iter_mut().find(|c| &c.id == chat) {
            c.unread = 0;
        }
        let total = self
            .chats
            .iter()
            .filter(|c| !c.muted)
            .map(|c| c.unread)
            .fold(0u32, u32::saturating_add);
        self.set_total_unread(total)
    }

    fn find_message(&self, id: &MessageId) -> Option<&wa_core::Message> {
        self.conversation.as_ref().and_then(|c| c.find(id))
    }

    fn request_media(&mut self, id: &MessageId) -> bool {
        let Some(msg) = self.find_message(id) else {
            return false;
        };
        let Some(media) = msg.content.media() else {
            return false;
        };
        let cmd = BackendCommand::FetchMedia {
            chat: msg.chat.clone(),
            message: msg.id.clone(),
            media: media.clone(),
        };
        if self.send(cmd) {
            self.fetching.insert(id.clone());
            self.media_errors.remove(id);
            true
        } else {
            false
        }
    }

    /// Shows the image viewer or opens a file once it is local.
    fn open_local_media(&mut self, id: &MessageId) -> Task<Message> {
        let Some(msg) = self.find_message(id) else {
            return Task::none();
        };
        let Some(path) = msg
            .content
            .media()
            .and_then(media::local_file)
            .map(|p| p.to_path_buf())
        else {
            return Task::none();
        };
        match msg.content {
            MessageContent::Image { .. } | MessageContent::Sticker { .. } => {
                self.dialog = Some(Dialog::ImageViewer { path });
                Task::none()
            }
            _ => open_path(path),
        }
    }

    fn play_voice(&mut self, id: &MessageId) -> Task<Message> {
        // Only OGG/Opus (voice notes) plays inline; other audio files go to
        // the system player, like documents.
        if self
            .find_message(id)
            .is_some_and(|m| !plays_inline(&m.content))
        {
            return self.update(Message::OpenMedia(id.clone()));
        }
        let path = self
            .find_message(id)
            .and_then(|m| m.content.media())
            .and_then(media::local_file)
            .map(|p| p.to_path_buf());
        match (path, &self.audio) {
            (Some(path), Some(audio)) => {
                audio.play(id.clone(), path);
                self.playing = Some(id.clone());
                self.audio_paused = false;
            }
            (Some(_), None) => {
                self.media_errors
                    .insert(id.clone(), "Audio playback unavailable".into());
            }
            (None, _) => {
                if self.request_media(id) {
                    self.play_after_fetch = Some(id.clone());
                }
            }
        }
        Task::none()
    }

    fn show_window(&mut self) -> Task<Message> {
        // The user is coming back to the open conversation.
        let mark = self.mark_open_chat_read();
        if let Some(id) = self.core.main_window_id() {
            return Task::batch([mark, window::gain_focus(id)]);
        }
        let (id, open) = window::open(main_window_settings());
        self.core.set_main_window_id(Some(id));
        let title = self.set_window_title(TITLE.to_owned(), id);
        Task::batch([
            mark,
            open.map(|_| cosmic::Action::App(Message::None)),
            title,
        ])
    }

    fn handle_backend(&mut self, event: BackendEvent) -> Task<Message> {
        match event {
            BackendEvent::Qr { code, .. } => {
                self.page = Page::Login;
                self.qr = pages::login::QrImage::new(&code);
                if self.qr.is_none() {
                    self.error = Some("Could not render the pairing code".into());
                }
                Task::none()
            }
            BackendEvent::Paired => {
                self.qr = None;
                self.page = Page::Sync;
                self.sync_progress = 0.0;
                Task::none()
            }
            BackendEvent::Connection(state) => {
                if state == ConnectionState::Connected
                    && self.page == Page::Login
                    && self.qr.is_none()
                {
                    self.page = Page::Main;
                }
                self.connection = Some(state);
                Task::none()
            }
            BackendEvent::LoggedOut => {
                self.page = Page::Login;
                self.qr = None;
                self.connection = None;
                if self.logout_requested {
                    self.logout_requested = false;
                    self.selected = None;
                    self.conversation = None;
                    self.replying_to = None;
                    self.chats.clear();
                }
                self.reload_chats()
            }
            BackendEvent::SyncProgress(progress) => {
                if self.page != Page::Main {
                    self.page = Page::Sync;
                }
                self.sync_progress = progress;
                Task::none()
            }
            BackendEvent::SyncDone => {
                self.page = Page::Main;
                self.sync_progress = 1.0;
                self.schedule_reload(true)
            }
            // During history sync thousands of these arrive; SyncDone reloads once.
            BackendEvent::ChatUpserted(_) if self.page == Page::Sync => Task::none(),
            BackendEvent::Message(_) if self.page == Page::Sync => Task::none(),
            // `reload_chats` also refreshes the open conversation's chat.
            BackendEvent::ChatUpserted(_) => self.schedule_reload(false),
            BackendEvent::Message(msg) => {
                if msg.from_me && self.own_jid.is_none() && !msg.sender.is_empty() {
                    self.own_jid = Some(msg.sender.clone());
                }
                let is_open = self.selected.as_ref() == Some(&msg.chat);
                let mut tasks = vec![self.schedule_reload(is_open)];
                if is_open
                    && self.window_focused()
                    && !msg.from_me
                    && self.send(BackendCommand::MarkRead {
                        chat: msg.chat.clone(),
                        messages: vec![msg.id.clone()],
                    })
                {
                    tasks.push(self.mark_read_locally(&msg.chat));
                }
                tasks.push(self.maybe_notify(&msg));
                Task::batch(tasks)
            }
            BackendEvent::MessageStatus { chat, .. }
            | BackendEvent::MessageSent { chat, .. }
            | BackendEvent::SendFailed { chat, .. } => {
                let is_open = self.selected.as_ref() == Some(&chat);
                self.schedule_reload(is_open)
            }
            BackendEvent::Reaction(reaction) => {
                if self.selected.as_ref() == Some(&reaction.chat) {
                    self.conversation_dirty = true;
                    self.schedule_reload(true)
                } else {
                    Task::none()
                }
            }
            BackendEvent::MediaReady { chat, message, .. } => {
                self.fetching.remove(&message);
                self.media_errors.remove(&message);
                // Reloaded right away: opening/playing needs the local path.
                let mut tasks = Vec::new();
                if self.selected.as_ref() == Some(&chat) {
                    tasks.push(self.reload_conversation());
                }
                if self.open_after_fetch.remove(&message) {
                    tasks.push(self.open_local_media(&message));
                }
                if self.save_after_fetch.remove(&message) {
                    tasks.push(self.save_media(&message));
                }
                if self.play_after_fetch.as_ref() == Some(&message) {
                    self.play_after_fetch = None;
                    tasks.push(self.play_voice(&message));
                }
                Task::batch(tasks)
            }
            BackendEvent::MediaFailed { message, error, .. } => {
                tracing::warn!(id = %message.0, %error, "media download failed");
                self.fetching.remove(&message);
                self.open_after_fetch.remove(&message);
                self.save_after_fetch.remove(&message);
                if self.play_after_fetch.as_ref() == Some(&message) {
                    self.play_after_fetch = None;
                }
                self.media_errors.insert(message, "Download failed".into());
                Task::none()
            }
            BackendEvent::AvatarReady { chat, path } => {
                self.avatars.insert(chat, path);
                Task::none()
            }
            BackendEvent::AvatarMissing { .. } => Task::none(),
            BackendEvent::Error(error) => {
                tracing::error!(%error, "backend error");
                self.error = Some(error);
                Task::none()
            }
        }
    }

    fn maybe_notify(&self, msg: &wa_core::Message) -> Task<Message> {
        if self.page != Page::Main {
            return Task::none();
        }
        let chat = self
            .chats
            .iter()
            .find(|c| c.id == msg.chat)
            .cloned()
            .or_else(|| {
                self.store
                    .as_ref()
                    .and_then(|s| s.chat(&msg.chat).ok().flatten())
            });
        let muted = chat.as_ref().is_some_and(|c| c.muted);
        if !should_notify(
            msg,
            now_ms(),
            self.window_focused(),
            self.selected.as_ref(),
            muted,
        ) {
            return Task::none();
        }
        let chat_name = chat
            .map(|c| c.name)
            .unwrap_or_else(|| msg.chat.user().to_owned());
        let preview = msg.content.preview();
        let body = self.config.notification_previews.then(|| {
            match (&msg.sender_name, msg.chat.is_group()) {
                (Some(sender), true) => format!("{sender}: {preview}"),
                _ => preview,
            }
        });
        let chat_id = msg.chat.0.clone();
        cosmic::task::future(async move {
            desktop::notify(&chat_name, body.as_deref(), chat_id);
            Message::None
        })
    }

    fn apply_appearance(&mut self, appearance: Appearance) -> Task<Message> {
        self.config.appearance = appearance;
        self.config.save(self.config_handler.as_ref());
        cosmic::command::set_theme(appearance.theme())
    }
}

/// Whether a thumbnail decode should start: nothing shown or tried yet, or
/// the full file became available since the last try (which only the inline
/// thumbnail could be decoded for).
fn thumb_needed(
    shown_full: Option<bool>,
    attempted_with_local: Option<bool>,
    pending: bool,
    has_local: bool,
) -> bool {
    if pending {
        return false;
    }
    match (shown_full, attempted_with_local) {
        (Some(true), _) | (_, Some(true)) => false,
        (None, None) => true,
        _ => has_local,
    }
}

/// Groups a chat's reactions by target, keeping only `messages`' targets.
fn group_reactions(
    list: Vec<wa_core::Reaction>,
    messages: &[wa_core::Message],
) -> HashMap<MessageId, Vec<wa_core::Reaction>> {
    let ids: HashSet<&MessageId> = messages.iter().map(|m| &m.id).collect();
    let mut out: HashMap<MessageId, Vec<wa_core::Reaction>> = HashMap::new();
    for r in list {
        if ids.contains(&r.target) {
            out.entry(r.target.clone()).or_default().push(r);
        }
    }
    out
}

/// Whether audio content plays in the built-in (OGG/Opus) player.
fn plays_inline(content: &MessageContent) -> bool {
    match content {
        MessageContent::Audio {
            voice_note, media, ..
        } => {
            let mime = media.mime.to_ascii_lowercase();
            *voice_note || mime.contains("ogg") || mime.contains("opus")
        }
        _ => false,
    }
}

/// Re-sends a failed text under its own optimistic id, so the runtime moves
/// the same row Failed → Pending instead of adding a duplicate bubble.
fn retry_command(msg: wa_core::Message) -> Option<BackendCommand> {
    let MessageContent::Text(text) = msg.content else {
        return None;
    };
    let local_id = if msg.id.is_local() {
        msg.id
    } else {
        MessageId::new_local()
    };
    Some(BackendCommand::SendText {
        chat: msg.chat,
        text,
        reply_to: msg.reply_to.map(|q| q.id),
        local_id,
    })
}

fn open_path(path: PathBuf) -> Task<Message> {
    if let Err(err) = open::that_detached(&path) {
        tracing::error!(?err, path = %path.display(), "failed to open file");
    }
    Task::none()
}

/// Settings for (re)opening the main window, mirroring libcosmic's defaults.
fn main_window_settings() -> window::Settings {
    let mut settings = window::Settings {
        size: Size::new(1100.0, 760.0),
        min_size: Some(Size::new(420.0, 480.0)),
        decorations: false,
        transparent: true,
        resizable: true,
        resize_border: 8,
        exit_on_close_request: false,
        ..window::Settings::default()
    };
    settings.platform_specific.application_id = APP_ID.to_owned();
    settings
}

impl Application for App {
    type Executor = cosmic::executor::multi::Executor;
    type Flags = Flags;
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, flags: Flags) -> (Self, Task<Message>) {
        let mut app = App::new(core, flags);
        app.set_header_title(TITLE.to_owned());
        let task = match app.core.main_window_id() {
            Some(id) => app.set_window_title(TITLE.to_owned(), id),
            None => Task::none(),
        };
        (app, task)
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        let space = theme::spacing();
        let item = |label: &'static str, icon_name: &'static str, message: Message| {
            button::custom(
                widget::row::with_capacity(2)
                    .spacing(space.space_xs)
                    .align_y(Alignment::Center)
                    .push(icon::from_name(icon_name).size(16))
                    .push(widget::text::body(label)),
            )
            .width(Length::Fill)
            .padding([space.space_xxs, space.space_s])
            .class(theme::Button::MenuItem)
            .on_press(message)
        };
        let toggle =
            button::icon(icon::from_name("open-menu-symbolic")).on_press(Message::ToggleHeaderMenu);
        let mut menu = popover(toggle).position(widget::popover::Position::Bottom);
        if self.header_menu_open {
            menu = menu
                .popup(
                    container(
                        widget::column::with_capacity(2)
                            .push(item(
                                "Settings",
                                "preferences-system-symbolic",
                                Message::ShowSettings,
                            ))
                            .push(item("Quit", "application-exit-symbolic", Message::Quit)),
                    )
                    .padding(space.space_xxs)
                    .width(Length::Fixed(200.0))
                    .class(theme::Container::Dropdown),
                )
                .on_close(Message::ToggleHeaderMenu);
        }
        vec![menu.into()]
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        if !self.show_settings || !self.core.window.show_context {
            return None;
        }
        Some(
            context_drawer::context_drawer(pages::settings::view(self), Message::CloseSettings)
                .title("Settings"),
        )
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        let space = theme::spacing();
        match self.dialog.as_ref()? {
            Dialog::EmojiPicker(target) => {
                const PER_ROW: usize = 9;
                let mut grid = widget::column::with_capacity(ALL_EMOJIS.len() / PER_ROW + 1)
                    .spacing(space.space_xxxs)
                    .width(Length::Fill);
                for chunk in ALL_EMOJIS.chunks(PER_ROW) {
                    let mut row = widget::row::with_capacity(PER_ROW).spacing(space.space_xxxs);
                    for emoji in chunk {
                        row = row.push(
                            button::custom(widget::text(*emoji).size(24))
                                .padding([space.space_xxs, space.space_xs])
                                .class(theme::Button::Text)
                                .on_press(Message::React(target.clone(), (*emoji).to_owned())),
                        );
                    }
                    // Centred so a short last row does not hang on the left.
                    grid = grid.push(container(row).width(Length::Fill).align_x(Alignment::Center));
                }
                Some(
                    widget::dialog()
                        .title("Choose a reaction")
                        .control(widget::scrollable(grid).height(Length::Fixed(320.0)))
                        .secondary_action(button::standard("Cancel").on_press(Message::CloseDialog))
                        .into(),
                )
            }
            Dialog::ImageViewer { path } => Some(
                widget::dialog()
                    .control(
                        container(
                            widget::image(widget::image::Handle::from_path(path))
                                .content_fit(cosmic::iced::ContentFit::Contain)
                                .width(Length::Fill)
                                .height(Length::Fixed(520.0)),
                        )
                        .width(Length::Fixed(720.0)),
                    )
                    .primary_action(button::suggested("Close").on_press(Message::CloseDialog))
                    .secondary_action(button::standard("Open in viewer").on_press(Message::OpenFile(path.clone())))
                    .into(),
            ),
            Dialog::ConfirmLogout => Some(
                widget::dialog()
                    .title("Log out of WhatsApp?")
                    .body("This computer will be unlinked from your account and all messages stored here will be deleted.")
                    .primary_action(button::destructive("Log out").on_press(Message::ConfirmLogout))
                    .secondary_action(button::standard("Cancel").on_press(Message::CloseDialog))
                    .into(),
            ),
        }
    }

    fn on_escape(&mut self) -> Task<Message> {
        if self.dialog.take().is_none()
            && self.menu_for.take().is_none()
            && !std::mem::take(&mut self.header_menu_open)
        {
            self.replying_to = None;
        }
        Task::none()
    }

    fn on_app_exit(&mut self) -> Option<Message> {
        // Header close button: hide to tray instead of quitting.
        Some(Message::HideWindow)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        // libcosmic calls this when a surface has closed.
        Some(Message::WindowClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            backend_sub::subscription(self.backend_config.clone()).map(|msg| match msg {
                BackendMsg::Ready(sender) => Message::BackendReady(sender),
                BackendMsg::Event(event) => Message::Backend(event),
                BackendMsg::Closed => Message::BackendClosed,
            }),
            desktop::subscription().map(Message::Desktop),
            audio::subscription().map(Message::Audio),
            window::close_requests().map(Message::CloseRequested),
            signals::subscription().map(|()| Message::Quit),
            cosmic::iced::event::listen_with(|event, _status, id| match event {
                cosmic::iced::Event::Window(window::Event::Focused) => {
                    Some(Message::WindowFocused(id))
                }
                _ => None,
            }),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::BackendReady(sender) => {
                self.commands = Some(sender);
                Task::none()
            }
            Message::Backend(event) => self.handle_backend(event),
            Message::BackendClosed => {
                if self.quitting {
                    cosmic::iced::exit()
                } else {
                    self.connection = Some(ConnectionState::Disconnected);
                    Task::none()
                }
            }
            Message::Reload => self.flush_reload(),
            Message::ThumbDecoded(chat, id, thumb) => {
                self.thumbs_pending.remove(&id);
                if let (Some(conv), Some(thumb)) = (&mut self.conversation, thumb)
                    && conv.chat.id == chat
                    && conv.find(&id).is_some()
                {
                    conv.thumbs.insert(id, thumb);
                }
                Task::none()
            }
            Message::WindowFocused(id) => {
                // Messages that arrived in the open chat while the window was
                // unfocused are read now that the user is looking at them.
                if self.core.main_window_id() == Some(id) {
                    self.mark_open_chat_read()
                } else {
                    Task::none()
                }
            }
            Message::Desktop(event) => match event {
                DesktopEvent::TrayReady(tray) => {
                    let total = self.total_unread;
                    self.tray = Some(tray.clone());
                    cosmic::task::future(async move {
                        tray.set_unread(total).await;
                        Message::None
                    })
                }
                DesktopEvent::TrayUnavailable => {
                    self.tray = None;
                    // `--background` without a tray would leave nothing on screen.
                    if self.core.main_window_id().is_none() {
                        tracing::warn!("no tray available; showing the window instead");
                        self.show_window()
                    } else {
                        Task::none()
                    }
                }
                DesktopEvent::Show => self.show_window(),
                DesktopEvent::Quit => self.update(Message::Quit),
                DesktopEvent::NotificationClicked(chat) => {
                    let show = self.show_window();
                    let open = self.open_chat(ChatId(chat));
                    Task::batch([show, open])
                }
            },
            Message::Audio(event) => {
                match event {
                    AudioEvent::Ready(handle) => self.audio = Some(handle),
                    AudioEvent::Started(id) => {
                        self.playing = Some(id);
                        self.audio_paused = false;
                    }
                    AudioEvent::Finished(id) => {
                        if self.playing.as_ref() == Some(&id) {
                            self.playing = None;
                            self.audio_paused = false;
                        }
                    }
                    AudioEvent::Failed(id, error) => {
                        if self.playing.as_ref() == Some(&id) {
                            self.playing = None;
                        }
                        self.media_errors.insert(id, error);
                    }
                }
                Task::none()
            }

            Message::SelectChat(id) => self.open_chat(id),
            Message::CloseChat => {
                self.selected = None;
                self.conversation = None;
                self.replying_to = None;
                Task::none()
            }
            Message::ComposerChanged(text) => {
                self.composer = text;
                Task::none()
            }
            Message::SendComposer => {
                let text = self.composer.trim().to_owned();
                let Some(chat) = self.selected.clone() else {
                    return Task::none();
                };
                if text.is_empty() {
                    return Task::none();
                }
                let reply_to = self.replying_to.as_ref().map(|m| m.id.clone());
                if self.send(BackendCommand::SendText {
                    chat,
                    text,
                    reply_to,
                    local_id: MessageId::new_local(),
                }) {
                    self.composer.clear();
                    self.replying_to = None;
                    cosmic::iced::widget::scrollable::snap_to(
                        conversation::scroll_id(),
                        RelativeOffset {
                            x: None,
                            y: Some(0.0),
                        },
                    )
                } else {
                    self.error = Some("Not connected — message not sent".into());
                    Task::none()
                }
            }
            Message::StartReply(id) => {
                self.menu_for = None;
                self.replying_to = self.find_message(&id).cloned();
                widget::text_input::focus(conversation::composer_id())
            }
            Message::CancelReply => {
                self.replying_to = None;
                Task::none()
            }
            Message::React(target, emoji) => {
                self.menu_for = None;
                if matches!(self.dialog, Some(Dialog::EmojiPicker(_))) {
                    self.dialog = None;
                }
                if let Some(chat) = self.selected.clone() {
                    self.send(BackendCommand::React {
                        chat,
                        target,
                        emoji,
                    });
                }
                Task::none()
            }
            Message::OpenMessageMenu(id) => {
                self.menu_for = Some(id);
                Task::none()
            }
            Message::CloseMessageMenu => {
                self.menu_for = None;
                Task::none()
            }
            Message::OpenEmojiPicker(id) => {
                self.menu_for = None;
                self.dialog = Some(Dialog::EmojiPicker(id));
                Task::none()
            }
            Message::CopyText(text) => {
                self.menu_for = None;
                cosmic::iced::clipboard::write(text)
            }
            Message::Hover(id) => {
                // Keep the hover target while its menu is open.
                if self.menu_for.is_none() {
                    self.hovered = id;
                }
                Task::none()
            }
            Message::HoverEnd(id) => {
                // Only clear our own hover: entering the next message may be
                // delivered before leaving this one.
                if self.menu_for.is_none() && self.hovered.as_ref() == Some(&id) {
                    self.hovered = None;
                }
                Task::none()
            }
            Message::Retry(id) => {
                let Some(msg) = self.find_message(&id).cloned() else {
                    return Task::none();
                };
                if let Some(cmd) = retry_command(msg) {
                    self.send(cmd);
                }
                Task::none()
            }
            Message::ConversationScrolled(distance_from_top) => {
                let load = self.conversation.as_ref().is_some_and(|c| {
                    !c.messages.is_empty()
                        && conversation::should_load_older(
                            distance_from_top,
                            c.loading_older,
                            c.exhausted,
                        )
                });
                if load {
                    self.load_older()
                } else {
                    Task::none()
                }
            }

            Message::OpenMedia(id) => {
                let local = self
                    .find_message(&id)
                    .and_then(|m| m.content.media())
                    .and_then(media::local_file)
                    .is_some();
                if local {
                    self.open_local_media(&id)
                } else {
                    if self.request_media(&id) {
                        self.open_after_fetch.insert(id);
                    }
                    Task::none()
                }
            }
            Message::FetchMedia(id) => {
                self.request_media(&id);
                Task::none()
            }
            Message::SaveMedia(id) => {
                let local = self
                    .find_message(&id)
                    .and_then(|m| m.content.media())
                    .and_then(media::local_file)
                    .is_some();
                if local {
                    self.save_media(&id)
                } else {
                    if self.request_media(&id) {
                        self.save_after_fetch.insert(id);
                    }
                    Task::none()
                }
            }
            Message::MediaSaved(id, result) => {
                match result {
                    Ok(path) => {
                        tracing::info!(id = %id.0, path = %path.display(), "media saved");
                        self.media_errors.remove(&id);
                        self.saved_media.insert(id, path);
                    }
                    Err(error) => {
                        tracing::warn!(id = %id.0, %error, "saving media failed");
                        self.media_errors
                            .insert(id, format!("Save failed: {error}"));
                    }
                }
                Task::none()
            }
            Message::ToggleVoice(id) => {
                if self.playing.as_ref() == Some(&id) {
                    if let Some(audio) = &self.audio {
                        if self.audio_paused {
                            audio.resume();
                        } else {
                            audio.pause();
                        }
                        self.audio_paused = !self.audio_paused;
                    }
                } else if !self.fetching.contains(&id) {
                    return self.play_voice(&id);
                }
                Task::none()
            }
            Message::OpenFile(path) => open_path(path),
            Message::CloseDialog => {
                self.dialog = None;
                Task::none()
            }

            Message::ToggleHeaderMenu => {
                self.header_menu_open = !self.header_menu_open;
                Task::none()
            }
            Message::ShowSettings => {
                self.header_menu_open = false;
                self.autostart = desktop::autostart_enabled();
                self.show_settings = true;
                self.set_show_context(true);
                Task::none()
            }
            Message::CloseSettings => {
                self.show_settings = false;
                self.set_show_context(false);
                Task::none()
            }
            Message::SetAppearance(appearance) => self.apply_appearance(appearance),
            Message::SetPreviews(enabled) => {
                self.config.notification_previews = enabled;
                self.config.save(self.config_handler.as_ref());
                Task::none()
            }
            Message::SetAutostart(enabled) => {
                match desktop::set_autostart(enabled) {
                    Ok(()) => self.autostart = Some(enabled),
                    Err(err) => {
                        tracing::error!(?err, "failed to change autostart");
                        self.error = Some(format!("Could not change start at login: {err:#}"));
                    }
                }
                Task::none()
            }
            Message::RequestLogout => {
                self.dialog = Some(Dialog::ConfirmLogout);
                Task::none()
            }
            Message::ConfirmLogout => {
                self.dialog = None;
                self.show_settings = false;
                self.set_show_context(false);
                if self.send(BackendCommand::Logout) {
                    self.logout_requested = true;
                } else {
                    self.error = Some("Not connected — cannot log out right now".into());
                }
                Task::none()
            }
            Message::DismissError => {
                self.error = None;
                Task::none()
            }

            Message::CloseRequested(id) => {
                if self.core.main_window_id() == Some(id) {
                    self.update(Message::HideWindow)
                } else {
                    window::close(id)
                }
            }
            Message::WindowClosed(id) => {
                if self.core.main_window_id() == Some(id) {
                    self.core.set_main_window_id(None);
                    self.hovered = None;
                    self.menu_for = None;
                    self.header_menu_open = false;
                }
                Task::none()
            }
            // Without a tray there is no way back to a hidden window: quit instead.
            Message::HideWindow if self.tray.is_none() => self.update(Message::Quit),
            Message::HideWindow => match self.core.main_window_id() {
                Some(id) => window::close(id),
                None => Task::none(),
            },
            Message::Quit => {
                if let Some(audio) = &self.audio {
                    audio.stop();
                }
                if std::mem::replace(&mut self.quitting, true) {
                    return Task::none();
                }
                // Exit once the backend has disconnected and flushed its
                // session state (`BackendClosed`), or after a timeout.
                let wait = if self.send(BackendCommand::Shutdown) {
                    QUIT_TIMEOUT
                } else {
                    Duration::from_millis(100)
                };
                cosmic::task::future(async move {
                    tokio::time::sleep(wait).await;
                    Message::Exit
                })
            }
            Message::Exit => cosmic::iced::exit(),
            Message::None => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let space = theme::spacing();
        let page: Element<'_, Message> = match self.page {
            Page::Login => pages::login::view(self),
            Page::Sync => pages::sync::view(self),
            Page::Main => self.main_view(),
        };
        let mut column = widget::column::with_capacity(3);
        if let Some(error) = &self.error {
            column = column.push(banner(error.clone(), true));
        }
        if self.page == Page::Main
            && let Some(text) = connection_text(self.connection.as_ref())
        {
            column = column.push(banner(text, false));
        }
        column
            .push(page)
            .spacing(space.space_none)
            .height(Length::Fill)
            .into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        // Only the main window exists; never panic on a stray surface.
        widget::Space::new().into()
    }
}

impl App {
    fn main_view(&self) -> Element<'_, Message> {
        let condensed = self.core.is_condensed();
        if condensed {
            return if self.conversation.is_some() {
                pages::conversation::view(self)
            } else {
                pages::chats::view(self)
            };
        }
        // The chat list sits on its own (primary) surface; the window
        // background shows through the conversation pane.
        let sidebar = container(pages::chats::view(self))
            .width(Length::Fixed(pages::chats::LIST_WIDTH))
            .height(Length::Fill)
            .class(theme::Container::custom(|theme| container::Style {
                background: Some(Background::Color(
                    theme.cosmic().primary_container_color().into(),
                )),
                ..container::Style::default()
            }));
        widget::row::with_capacity(3)
            .push(sidebar)
            .push(widget::divider::vertical::default())
            .push(pages::conversation::view(self))
            .height(Length::Fill)
            .into()
    }
}

/// Banner text for non-connected states; `None` hides the banner.
pub fn connection_text(state: Option<&ConnectionState>) -> Option<String> {
    match state? {
        ConnectionState::Connected => None,
        ConnectionState::Connecting => Some("Connecting…".to_owned()),
        ConnectionState::Reconnecting { retry_in_secs, .. } => Some(format!(
            "Connection lost — reconnecting in {retry_in_secs}s…"
        )),
        ConnectionState::Disconnected => {
            Some("Disconnected — messages will arrive when you reconnect".to_owned())
        }
    }
}

fn banner<'a>(text: String, error: bool) -> Element<'a, Message> {
    let space = theme::spacing();
    let mut row = widget::row::with_capacity(3)
        .spacing(space.space_s)
        .align_y(Alignment::Center)
        .push(
            icon::from_name(if error {
                "dialog-warning-symbolic"
            } else {
                "network-wireless-acquiring-symbolic"
            })
            .size(16),
        )
        .push(widget::text::body(text).width(Length::Fill));
    if error {
        row = row.push(
            button::icon(icon::from_name("window-close-symbolic"))
                .extra_small()
                .on_press(Message::DismissError),
        );
    }
    container(row)
        .padding([space.space_xxs, space.space_m])
        .width(Length::Fill)
        .class(theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();
            let component = if error {
                &cosmic.destructive
            } else {
                &cosmic.warning
            };
            cosmic::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(component.base.into())),
                text_color: Some(component.on.into()),
                icon_color: Some(component.on.into()),
                ..Default::default()
            }
        }))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wa_core::{MessageStatus, Reaction};

    fn test_app() -> App {
        App::blank(
            Core::default(),
            BackendConfig {
                fake: true,
                app_db: PathBuf::from("/nonexistent/app.db"),
                session_db: PathBuf::from("/nonexistent/session.db"),
                media_dir: PathBuf::from("/nonexistent/media"),
            },
            Config::default(),
            None,
        )
    }

    fn incoming(chat: &str, ts: i64) -> wa_core::Message {
        wa_core::Message {
            id: MessageId("m1".into()),
            chat: ChatId(chat.into()),
            sender: chat.into(),
            sender_name: Some("Ana".into()),
            from_me: false,
            timestamp: ts,
            content: MessageContent::Text("hi".into()),
            reply_to: None,
            status: MessageStatus::Received,
        }
    }

    #[test]
    fn qr_event_switches_to_login_with_image() {
        let mut app = test_app();
        app.page = Page::Main;
        let _ = app.update(Message::Backend(BackendEvent::Qr {
            code: "2@abc,def".into(),
            valid_for_secs: 20,
        }));
        assert_eq!(app.page, Page::Login);
        assert!(app.qr.is_some());
    }

    #[test]
    fn sync_done_switches_to_main() {
        let mut app = test_app();
        let _ = app.update(Message::Backend(BackendEvent::Paired));
        assert_eq!(app.page, Page::Sync);
        let _ = app.update(Message::Backend(BackendEvent::SyncProgress(0.5)));
        assert_eq!(app.page, Page::Sync);
        assert!((app.sync_progress - 0.5).abs() < f32::EPSILON);
        let _ = app.update(Message::Backend(BackendEvent::SyncDone));
        assert_eq!(app.page, Page::Main);
    }

    #[test]
    fn connected_without_qr_goes_to_main_but_not_during_pairing() {
        let mut app = test_app();
        let _ = app.update(Message::Backend(BackendEvent::Connection(
            ConnectionState::Connected,
        )));
        assert_eq!(app.page, Page::Main);

        let mut app = test_app();
        let _ = app.update(Message::Backend(BackendEvent::Qr {
            code: "x".into(),
            valid_for_secs: 20,
        }));
        let _ = app.update(Message::Backend(BackendEvent::Connection(
            ConnectionState::Connected,
        )));
        assert_eq!(app.page, Page::Login);
    }

    #[test]
    fn logged_out_returns_to_login_and_clears_only_when_requested() {
        let mut app = test_app();
        app.page = Page::Main;
        app.chats.push(Chat {
            id: ChatId("1@s.whatsapp.net".into()),
            kind: wa_core::ChatKind::Direct,
            name: "Ana".into(),
            last_activity: 0,
            unread: 0,
            muted: false,
            last_preview: None,
        });
        let _ = app.update(Message::Backend(BackendEvent::LoggedOut));
        assert_eq!(app.page, Page::Login);
        assert_eq!(app.chats.len(), 1, "remote logout keeps local data");

        app.logout_requested = true;
        let _ = app.update(Message::Backend(BackendEvent::LoggedOut));
        assert!(app.chats.is_empty());
        assert!(!app.logout_requested);
    }

    #[test]
    fn media_events_update_fetch_state() {
        let mut app = test_app();
        let id = MessageId("m".into());
        let chat = ChatId("c@g.us".into());
        app.fetching.insert(id.clone());
        let _ = app.update(Message::Backend(BackendEvent::MediaFailed {
            chat: chat.clone(),
            message: id.clone(),
            error: "boom".into(),
        }));
        assert!(!app.fetching.contains(&id));
        assert!(app.media_errors.contains_key(&id));
        let _ = app.update(Message::Backend(BackendEvent::MediaReady {
            chat,
            message: id.clone(),
            path: "/x".into(),
        }));
        assert!(!app.media_errors.contains_key(&id));
    }

    #[test]
    fn backend_error_is_shown_and_dismissable() {
        let mut app = test_app();
        let _ = app.update(Message::Backend(BackendEvent::Error("bad".into())));
        assert_eq!(app.error.as_deref(), Some("bad"));
        let _ = app.update(Message::DismissError);
        assert!(app.error.is_none());
    }

    #[test]
    fn reaction_for_other_chat_is_ignored_without_store() {
        let mut app = test_app();
        let _ = app.update(Message::Backend(BackendEvent::Reaction(Reaction {
            chat: ChatId("x@g.us".into()),
            target: MessageId("m".into()),
            sender: "a".into(),
            emoji: "👍".into(),
            timestamp: 0,
        })));
    }

    #[test]
    fn notify_truth_table() {
        let now = 1_000_000_000;
        let chat = ChatId("1@s.whatsapp.net".into());
        let other = ChatId("2@s.whatsapp.net".into());
        let msg = incoming(&chat.0, now - 1000);

        // Unfocused window: notify even if the chat is open.
        assert!(should_notify(&msg, now, false, Some(&chat), false));
        // Focused but a different chat open: notify.
        assert!(should_notify(&msg, now, true, Some(&other), false));
        assert!(should_notify(&msg, now, true, None, false));
        // Focused and looking at it: no.
        assert!(!should_notify(&msg, now, true, Some(&chat), false));
        // Muted: never.
        assert!(!should_notify(&msg, now, false, None, true));
        // Our own messages: never.
        let mut mine = msg.clone();
        mine.from_me = true;
        assert!(!should_notify(&mine, now, false, None, false));
        // Old (history sync) messages: never.
        let old = incoming(&chat.0, now - NOTIFY_MAX_AGE_MS - 1);
        assert!(!should_notify(&old, now, false, None, false));
    }

    #[test]
    fn connection_banner_text() {
        assert_eq!(connection_text(None), None);
        assert_eq!(connection_text(Some(&ConnectionState::Connected)), None);
        assert_eq!(
            connection_text(Some(&ConnectionState::Connecting)).as_deref(),
            Some("Connecting…")
        );
        assert!(
            connection_text(Some(&ConnectionState::Reconnecting {
                attempt: 2,
                retry_in_secs: 4
            }))
            .is_some_and(|t| t.contains("4s"))
        );
    }

    fn with_commands(app: &mut App) -> tokio::sync::mpsc::Receiver<BackendCommand> {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        app.commands = Some(CommandSender(tx));
        rx
    }

    fn open_conversation(app: &mut App, unread: u32, messages: Vec<wa_core::Message>) {
        let chat = Chat {
            id: ChatId("1@s.whatsapp.net".into()),
            kind: wa_core::ChatKind::Direct,
            name: "Ana".into(),
            last_activity: 0,
            unread,
            muted: false,
            last_preview: None,
        };
        app.page = Page::Main;
        app.selected = Some(chat.id.clone());
        app.chats = vec![chat.clone()];
        let mut conv = Conversation::new(chat);
        conv.messages = messages;
        app.conversation = Some(conv);
    }

    #[test]
    fn retry_reuses_the_failed_local_id() {
        let mut app = test_app();
        let mut rx = with_commands(&mut app);
        let mut failed = incoming("1@s.whatsapp.net", 5);
        failed.id = MessageId::new_local();
        failed.from_me = true;
        failed.status = MessageStatus::Failed;
        let id = failed.id.clone();
        open_conversation(&mut app, 0, vec![failed]);
        let _ = app.update(Message::Retry(id.clone()));
        match rx.try_recv() {
            Ok(BackendCommand::SendText { local_id, text, .. }) => {
                assert_eq!(local_id, id);
                assert_eq!(text, "hi");
            }
            other => panic!("expected SendText, got {other:?}"),
        }
    }

    #[test]
    fn focusing_the_window_marks_the_open_chat_read() {
        let mut app = test_app();
        let mut rx = with_commands(&mut app);
        let win = window::Id::unique();
        app.core.set_main_window_id(Some(win));
        open_conversation(&mut app, 1, vec![incoming("1@s.whatsapp.net", 5)]);
        let _ = app.update(Message::WindowFocused(win));
        assert!(matches!(
            rx.try_recv(),
            Ok(BackendCommand::MarkRead { messages, .. }) if messages == vec![MessageId("m1".into())]
        ));
        assert_eq!(app.chats[0].unread, 0);
        // Nothing left to mark.
        let _ = app.update(Message::WindowFocused(win));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unread_without_stored_incoming_messages_still_clears() {
        let mut app = test_app();
        let mut rx = with_commands(&mut app);
        let mut mine = incoming("1@s.whatsapp.net", 5);
        mine.from_me = true;
        open_conversation(&mut app, 2, vec![mine]);
        let _ = app.mark_open_chat_read();
        assert!(matches!(
            rx.try_recv(),
            Ok(BackendCommand::MarkRead { messages, .. }) if messages.is_empty()
        ));
        assert_eq!(app.conversation.as_ref().map(|c| c.chat.unread), Some(0));
    }

    #[test]
    fn quit_waits_for_backend_shutdown() {
        let mut app = test_app();
        let mut rx = with_commands(&mut app);
        let _ = app.update(Message::Quit);
        assert!(app.quitting);
        assert_eq!(rx.try_recv().ok(), Some(BackendCommand::Shutdown));
        // A second Quit does not send Shutdown again.
        let _ = app.update(Message::Quit);
        assert!(rx.try_recv().is_err());

        // Without a pending quit, a closed backend is shown as disconnected.
        let mut app = test_app();
        let _ = app.update(Message::BackendClosed);
        assert_eq!(app.connection, Some(ConnectionState::Disconnected));
    }

    #[test]
    fn backend_events_coalesce_reloads() {
        let mut app = test_app();
        app.page = Page::Main;
        let _ = app.update(Message::Backend(BackendEvent::Message(incoming(
            "2@s.whatsapp.net",
            1,
        ))));
        assert!(app.reload_scheduled && app.chats_dirty);
        let _ = app.update(Message::Backend(BackendEvent::Message(incoming(
            "2@s.whatsapp.net",
            2,
        ))));
        let _ = app.update(Message::Reload);
        assert!(!app.reload_scheduled && !app.chats_dirty && !app.conversation_dirty);
    }

    #[test]
    fn thumb_decode_decisions() {
        // Nothing shown or tried: decode.
        assert!(thumb_needed(None, None, false, false));
        // Already running: wait.
        assert!(!thumb_needed(None, None, true, true));
        // Inline thumb shown, file now downloaded: decode the file once.
        assert!(thumb_needed(Some(false), Some(false), false, true));
        assert!(!thumb_needed(Some(false), Some(false), false, false));
        // The file was already tried (even if it failed): never again.
        assert!(!thumb_needed(Some(false), Some(true), false, true));
        assert!(!thumb_needed(None, Some(true), false, true));
        assert!(!thumb_needed(Some(true), None, false, true));
        // An inline-only try that failed is retried only once a file exists.
        assert!(!thumb_needed(None, Some(false), false, false));
        assert!(thumb_needed(None, Some(false), false, true));
    }

    #[test]
    fn only_opus_audio_plays_inline() {
        let media = |mime: &str| wa_core::MediaRef {
            mime: mime.into(),
            size: 1,
            thumbnail: None,
            local_path: None,
            download_token: Vec::new(),
        };
        let audio = |voice_note, mime: &str| MessageContent::Audio {
            voice_note,
            duration_secs: 1,
            media: media(mime),
        };
        assert!(plays_inline(&audio(true, "audio/ogg; codecs=opus")));
        assert!(plays_inline(&audio(false, "audio/ogg")));
        assert!(!plays_inline(&audio(false, "audio/mpeg")));
        assert!(!plays_inline(&audio(false, "audio/mp4")));
        assert!(!plays_inline(&MessageContent::Text("x".into())));
    }

    #[test]
    fn reactions_are_grouped_for_loaded_messages_only() {
        let msgs = vec![incoming("1@s.whatsapp.net", 1)];
        let r = |target: &str| Reaction {
            chat: ChatId("1@s.whatsapp.net".into()),
            target: MessageId(target.into()),
            sender: "a".into(),
            emoji: "👍".into(),
            timestamp: 0,
        };
        let grouped = group_reactions(vec![r("m1"), r("other"), r("m1")], &msgs);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[&MessageId("m1".into())].len(), 2);
    }

    #[test]
    fn escape_closes_layers_in_order() {
        let mut app = test_app();
        app.dialog = Some(Dialog::ConfirmLogout);
        app.menu_for = Some(MessageId("m".into()));
        let _ = app.on_escape();
        assert!(app.dialog.is_none());
        assert!(app.menu_for.is_some());
        let _ = app.on_escape();
        assert!(app.menu_for.is_none());
    }
}
