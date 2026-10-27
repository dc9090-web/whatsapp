//! The app's own SQLite database (`app.db`): chats, messages, reactions.
//!
//! Separate from the protocol library's session database so its schema never
//! constrains UI queries. Each `Store` owns one connection; open one per thread
//! (the runtime and the UI each open their own; WAL mode makes that safe).
//!
//! CONTRACT (implemented in Task 2): signatures below are fixed — other crates
//! are written against them in parallel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rusqlite::{OptionalExtension as _, Row, params};

use crate::model::{
    Chat, ChatId, ChatKind, Message, MessageContent, MessageId, MessageStatus, QuotedMessage,
    Reaction,
};

const SCHEMA: &str = include_str!("store/schema.sql");

const MESSAGE_COLUMNS: &str =
    "id, chat, sender, sender_name, from_me, ts, content, reply_to, status";
const CHAT_COLUMNS: &str = "id, kind, name, last_activity, unread, muted, last_preview";

pub struct Store {
    conn: rusqlite::Connection,
}

impl Store {
    /// Opens (creating and migrating if needed) the database at `path`, in WAL mode.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = rusqlite::Connection::open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(rusqlite::Connection::open_in_memory()?)
    }

    fn init(conn: rusqlite::Connection) -> anyhow::Result<Self> {
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        // In-memory databases report "memory" here; that is fine.
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA).context("applying schema")?;
        migrate(&conn).context("migrating schema")?;
        Ok(Self { conn })
    }

    /// Records where the chat's downloaded profile picture lives.
    pub fn set_chat_avatar(&self, chat: &ChatId, path: &Path) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE chats SET avatar_path = ?2 WHERE id = ?1",
            params![chat.0, path.to_string_lossy()],
        )?;
        Ok(())
    }

    /// Every chat that has a downloaded profile picture.
    pub fn chat_avatars(&self) -> anyhow::Result<HashMap<ChatId, PathBuf>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, avatar_path FROM chats WHERE avatar_path IS NOT NULL")?;
        let rows = stmt.query_map([], |r| {
            Ok((ChatId(r.get(0)?), PathBuf::from(r.get::<_, String>(1)?)))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Inserts or replaces chat metadata. Preserves the existing `unread`,
    /// `last_activity` and `last_preview` if the incoming values are older/empty.
    pub fn upsert_chat(&self, chat: &Chat) -> anyhow::Result<()> {
        // Incoming activity at least as new as ours wins for unread/preview;
        // an empty preview never overwrites a known one.
        self.conn.execute(
            "INSERT INTO chats (id, kind, name, last_activity, unread, muted, last_preview)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
               kind = excluded.kind,
               name = excluded.name,
               muted = excluded.muted,
               unread = CASE
                 WHEN excluded.last_activity > 0
                  AND excluded.last_activity >= chats.last_activity
                 THEN excluded.unread ELSE chats.unread END,
               last_preview = CASE
                 WHEN excluded.last_activity >= chats.last_activity
                  AND excluded.last_preview IS NOT NULL AND excluded.last_preview <> ''
                 THEN excluded.last_preview ELSE chats.last_preview END,
               last_activity = MAX(chats.last_activity, excluded.last_activity)",
            params![
                chat.id.0,
                kind_to_str(chat.kind),
                chat.name,
                chat.last_activity,
                chat.unread,
                chat.muted,
                chat.last_preview.as_deref().filter(|p| !p.is_empty()),
            ],
        )?;
        Ok(())
    }

    pub fn chat(&self, id: &ChatId) -> anyhow::Result<Option<Chat>> {
        let sql = format!("SELECT {CHAT_COLUMNS} FROM chats WHERE id = ?1");
        Ok(self
            .conn
            .query_row(&sql, params![id.0], row_to_chat)
            .optional()?)
    }

    /// All chats, most recent `last_activity` first.
    pub fn chats(&self) -> anyhow::Result<Vec<Chat>> {
        let sql = format!(
            "SELECT {CHAT_COLUMNS} FROM chats ORDER BY last_activity DESC, name COLLATE NOCASE"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([], row_to_chat)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Idempotent on `(chat, id)`. Returns `true` if the message was new.
    /// When new: creates a placeholder chat if missing (name = `chat.user()`),
    /// bumps the chat's `last_activity`/`last_preview` if this message is newer,
    /// and increments `unread` when `!from_me`.
    /// When it already exists: updates content/status but never downgrades status.
    pub fn upsert_message(&self, msg: &Message) -> anyhow::Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let existing = load_message(&tx, &msg.chat, &msg.id)?;
        let reply_to = msg
            .reply_to
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        let new = match existing {
            Some(old) => {
                let mut content = msg.content.clone();
                // Keep an already downloaded file when the message is re-delivered
                // (history sync) without one.
                if let (Some(old_media), Some(new_media)) =
                    (old.content.media(), content.media_mut())
                    && new_media.local_path.is_none()
                {
                    new_media.local_path.clone_from(&old_media.local_path);
                }
                let status = merge_status(old.status, msg.status);
                tx.execute(
                    "UPDATE messages SET sender = ?3, sender_name = COALESCE(?4, sender_name),
                       content = ?5, reply_to = COALESCE(?6, reply_to), status = ?7
                     WHERE chat = ?1 AND id = ?2",
                    params![
                        msg.chat.0,
                        msg.id.0,
                        msg.sender,
                        msg.sender_name,
                        serde_json::to_string(&content)?,
                        reply_to,
                        status_to_str(status)?,
                    ],
                )?;
                false
            }
            None => {
                tx.execute(
                    "INSERT INTO messages
                       (chat, id, sender, sender_name, from_me, ts, content, reply_to, status)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        msg.chat.0,
                        msg.id.0,
                        msg.sender,
                        msg.sender_name,
                        msg.from_me,
                        msg.timestamp,
                        serde_json::to_string(&msg.content)?,
                        reply_to,
                        status_to_str(msg.status)?,
                    ],
                )?;
                let kind = if msg.chat.is_group() {
                    ChatKind::Group
                } else {
                    ChatKind::Direct
                };
                tx.execute(
                    "INSERT OR IGNORE INTO chats (id, kind, name) VALUES (?1, ?2, ?3)",
                    params![msg.chat.0, kind_to_str(kind), msg.chat.user()],
                )?;
                tx.execute(
                    "UPDATE chats SET last_activity = ?2, last_preview = ?3
                     WHERE id = ?1 AND last_activity <= ?2",
                    params![msg.chat.0, msg.timestamp, msg.content.preview()],
                )?;
                if !msg.from_me {
                    tx.execute(
                        "UPDATE chats SET unread = unread + 1 WHERE id = ?1",
                        params![msg.chat.0],
                    )?;
                }
                true
            }
        };
        tx.commit()?;
        Ok(new)
    }

    pub fn message(&self, chat: &ChatId, id: &MessageId) -> anyhow::Result<Option<Message>> {
        load_message(&self.conn, chat, id)
    }

    /// Up to `limit` messages strictly older than `before` (unix ms), or the newest
    /// `limit` if `before` is `None`. Returned in ascending timestamp order.
    ///
    /// For paging back from a loaded message prefer [`Store::messages_before`]:
    /// timestamps have whole-second resolution, so several messages can share
    /// the `before` timestamp and this would skip the ones not yet loaded.
    pub fn messages(
        &self,
        chat: &ChatId,
        before: Option<i64>,
        limit: usize,
    ) -> anyhow::Result<Vec<Message>> {
        let sql = format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages
             WHERE chat = ?1 AND ts < ?2
             ORDER BY ts DESC, rowid DESC LIMIT ?3"
        );
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(
            params![chat.0, before.unwrap_or(i64::MAX), limit],
            row_to_raw_message,
        )?;
        collect_messages(rows)
    }

    /// Messages whose text, caption or file name contains `query`
    /// (case-insensitively for ASCII), newest first, at most `limit`.
    pub fn search_messages(&self, query: &str, limit: usize) -> anyhow::Result<Vec<Message>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%{}%", like_escape(query));
        let sql = format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages
             WHERE json_extract(content, '$.Text') LIKE ?1 ESCAPE '\\'
                OR json_extract(content, '$.Image.caption') LIKE ?1 ESCAPE '\\'
                OR json_extract(content, '$.Video.caption') LIKE ?1 ESCAPE '\\'
                OR json_extract(content, '$.Document.caption') LIKE ?1 ESCAPE '\\'
                OR json_extract(content, '$.Document.file_name') LIKE ?1 ESCAPE '\\'
             ORDER BY ts DESC, rowid DESC LIMIT ?2"
        );
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![pattern, limit], row_to_raw_message)?;
        let mut found = collect_messages(rows)?;
        // `collect_messages` returns ascending order; search wants newest first.
        found.reverse();
        Ok(found)
    }

    /// Up to `limit` messages that sort before the stored message `oldest`
    /// (whose timestamp is `ts`) in the order `messages` uses — `(ts, rowid)` —
    /// so messages sharing `oldest`'s second are not skipped. Ascending order.
    /// If `oldest` is not stored (any more), falls back to `ts < ts`.
    pub fn messages_before(
        &self,
        chat: &ChatId,
        ts: i64,
        oldest: &MessageId,
        limit: usize,
    ) -> anyhow::Result<Vec<Message>> {
        let sql = format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages
             WHERE chat = ?1 AND (ts < ?2 OR (ts = ?2 AND rowid <
               COALESCE((SELECT rowid FROM messages WHERE chat = ?1 AND id = ?3), -1)))
             ORDER BY ts DESC, rowid DESC LIMIT ?4"
        );
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![chat.0, ts, oldest.0, limit], row_to_raw_message)?;
        collect_messages(rows)
    }

    /// Sets status, except that it never moves backwards (Read stays Read),
    /// with the one exception that `Failed` may replace `Pending`.
    pub fn set_status(
        &self,
        chat: &ChatId,
        id: &MessageId,
        status: MessageStatus,
    ) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let current: Option<String> = tx
            .query_row(
                "SELECT status FROM messages WHERE chat = ?1 AND id = ?2",
                params![chat.0, id.0],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(current) = current {
            let merged = merge_status(status_from_str(&current)?, status);
            tx.execute(
                "UPDATE messages SET status = ?3 WHERE chat = ?1 AND id = ?2",
                params![chat.0, id.0, status_to_str(merged)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Re-keys an optimistic message (and its reactions) once the server id is known.
    pub fn replace_message_id(
        &self,
        chat: &ChatId,
        old: &MessageId,
        new: &MessageId,
    ) -> anyhow::Result<()> {
        if old == new {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        let new_exists = tx
            .query_row(
                "SELECT 1 FROM messages WHERE chat = ?1 AND id = ?2",
                params![chat.0, new.0],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if new_exists {
            // The server copy already arrived (e.g. echoed back); drop the optimistic one.
            tx.execute(
                "DELETE FROM messages WHERE chat = ?1 AND id = ?2",
                params![chat.0, old.0],
            )?;
        } else {
            tx.execute(
                "UPDATE messages SET id = ?3 WHERE chat = ?1 AND id = ?2",
                params![chat.0, old.0, new.0],
            )?;
        }
        tx.execute(
            "UPDATE OR REPLACE reactions SET target = ?3 WHERE chat = ?1 AND target = ?2",
            params![chat.0, old.0, new.0],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Upsert keyed by `(chat, target, sender)`; an empty emoji deletes the row.
    pub fn apply_reaction(&self, reaction: &Reaction) -> anyhow::Result<()> {
        if reaction.emoji.is_empty() {
            self.conn.execute(
                "DELETE FROM reactions WHERE chat = ?1 AND target = ?2 AND sender = ?3",
                params![reaction.chat.0, reaction.target.0, reaction.sender],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO reactions (chat, target, sender, emoji, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(chat, target, sender) DO UPDATE SET
                   emoji = excluded.emoji, ts = excluded.ts",
                params![
                    reaction.chat.0,
                    reaction.target.0,
                    reaction.sender,
                    reaction.emoji,
                    reaction.timestamp
                ],
            )?;
        }
        Ok(())
    }

    pub fn reactions(&self, chat: &ChatId, target: &MessageId) -> anyhow::Result<Vec<Reaction>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT chat, target, sender, emoji, ts FROM reactions
             WHERE chat = ?1 AND target = ?2 ORDER BY ts, sender",
        )?;
        let rows = stmt.query_map(params![chat.0, target.0], |r| {
            Ok(Reaction {
                chat: ChatId(r.get(0)?),
                target: MessageId(r.get(1)?),
                sender: r.get(2)?,
                emoji: r.get(3)?,
                timestamp: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Every reaction in `chat` whose target message has a timestamp of at
    /// least `min_ts`, in one query (for a whole loaded conversation page).
    pub fn reactions_since(&self, chat: &ChatId, min_ts: i64) -> anyhow::Result<Vec<Reaction>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT r.chat, r.target, r.sender, r.emoji, r.ts FROM reactions r
             JOIN messages m ON m.chat = r.chat AND m.id = r.target
             WHERE r.chat = ?1 AND m.ts >= ?2 ORDER BY r.ts, r.sender",
        )?;
        let rows = stmt.query_map(params![chat.0, min_ts], |r| {
            Ok(Reaction {
                chat: ChatId(r.get(0)?),
                target: MessageId(r.get(1)?),
                sender: r.get(2)?,
                emoji: r.get(3)?,
                timestamp: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records where the decrypted media file lives (updates `MediaRef::local_path`).
    pub fn set_media_path(&self, chat: &ChatId, id: &MessageId, path: &Path) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let content: Option<String> = tx
            .query_row(
                "SELECT content FROM messages WHERE chat = ?1 AND id = ?2",
                params![chat.0, id.0],
                |r| r.get(0),
            )
            .optional()?;
        let Some(content) = content else {
            anyhow::bail!("set_media_path: no message {} in {}", id.0, chat.0);
        };
        let mut content: MessageContent = serde_json::from_str(&content)?;
        let Some(media) = content.media_mut() else {
            anyhow::bail!("set_media_path: message {} has no media", id.0);
        };
        media.local_path = Some(path.to_path_buf());
        tx.execute(
            "UPDATE messages SET content = ?3 WHERE chat = ?1 AND id = ?2",
            params![chat.0, id.0, serde_json::to_string(&content)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Sets the chat's unread count to 0.
    pub fn mark_read(&self, chat: &ChatId) -> anyhow::Result<()> {
        self.conn
            .execute("UPDATE chats SET unread = 0 WHERE id = ?1", params![chat.0])?;
        Ok(())
    }

    /// Sum of `unread` over non-muted chats.
    pub fn total_unread(&self) -> anyhow::Result<u32> {
        let total: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(unread), 0) FROM chats WHERE muted = 0",
            [],
            |r| r.get(0),
        )?;
        Ok(u32::try_from(total.max(0)).unwrap_or(u32::MAX))
    }

    /// Marks our own optimistic messages that are still `Pending` as `Failed`.
    /// Called once at startup: whatever was in flight when the previous run
    /// ended (quit, crash, dead backend) will never be confirmed, and only
    /// `Failed` messages offer Retry. Returns how many were changed.
    pub fn fail_stale_pending(&self) -> anyhow::Result<usize> {
        let changed = self.conn.execute(
            "UPDATE messages SET status = ?1
             WHERE from_me = 1 AND status = ?2 AND id LIKE 'local-%'",
            params![
                status_to_str(MessageStatus::Failed)?,
                status_to_str(MessageStatus::Pending)?
            ],
        )?;
        Ok(changed)
    }

    /// Removes one message and its reactions; returns `true` if it existed.
    /// The chat's preview is recomputed from the newest remaining message.
    pub fn delete_message(&self, chat: &ChatId, id: &MessageId) -> anyhow::Result<bool> {
        self.conn.execute(
            "DELETE FROM reactions WHERE chat = ?1 AND target = ?2",
            params![chat.0, id.0],
        )?;
        let removed = self.conn.execute(
            "DELETE FROM messages WHERE chat = ?1 AND id = ?2",
            params![chat.0, id.0],
        )?;
        if removed > 0 {
            self.refresh_summary(chat)?;
        }
        Ok(removed > 0)
    }

    /// Removes a chat with all its messages and reactions.
    pub fn delete_chat(&self, chat: &ChatId) -> anyhow::Result<()> {
        self.conn
            .execute("DELETE FROM reactions WHERE chat = ?1", params![chat.0])?;
        self.conn
            .execute("DELETE FROM messages WHERE chat = ?1", params![chat.0])?;
        self.conn
            .execute("DELETE FROM chats WHERE id = ?1", params![chat.0])?;
        Ok(())
    }

    /// Replaces a message's content (a revoked message becomes a placeholder),
    /// keeping its position, sender and status.
    pub fn replace_content(
        &self,
        chat: &ChatId,
        id: &MessageId,
        content: &MessageContent,
    ) -> anyhow::Result<()> {
        let changed = self.conn.execute(
            "UPDATE messages SET content = ?3 WHERE chat = ?1 AND id = ?2",
            params![chat.0, id.0, serde_json::to_string(content)?],
        )?;
        if changed > 0 {
            self.refresh_summary(chat)?;
        }
        Ok(())
    }

    /// Sets the chat's preview to its newest message (or clears it).
    fn refresh_summary(&self, chat: &ChatId) -> anyhow::Result<()> {
        let newest = self.messages(chat, None, 1)?.pop();
        self.conn.execute(
            "UPDATE chats SET last_preview = ?2 WHERE id = ?1",
            params![chat.0, newest.map(|m| m.content.preview())],
        )?;
        Ok(())
    }

    /// Deletes all rows (used by explicit Log out).
    pub fn wipe(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "BEGIN; DELETE FROM reactions; DELETE FROM messages; DELETE FROM chats; COMMIT;",
        )?;
        Ok(())
    }
}

/// Never moves a status backwards, except `Pending` ↔ `Failed` (failure / retry).
fn merge_status(current: MessageStatus, incoming: MessageStatus) -> MessageStatus {
    use MessageStatus::{Failed, Pending};
    match (current, incoming) {
        (Pending, Failed) | (Failed, Pending) => incoming,
        _ => current.max(incoming),
    }
}

fn kind_to_str(kind: ChatKind) -> &'static str {
    match kind {
        ChatKind::Direct => "direct",
        ChatKind::Group => "group",
    }
}

fn status_to_str(status: MessageStatus) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&status)?)
}

fn status_from_str(s: &str) -> anyhow::Result<MessageStatus> {
    serde_json::from_str(s).with_context(|| format!("bad status {s:?}"))
}

/// Escapes `%`, `_` and `\` for a `LIKE … ESCAPE '\'` pattern.
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Additive upgrades for databases created before a column existed.
fn migrate(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    let has_avatar = conn
        .prepare("PRAGMA table_info(chats)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .any(|c| c.is_ok_and(|c| c == "avatar_path"));
    if !has_avatar {
        conn.execute_batch("ALTER TABLE chats ADD COLUMN avatar_path TEXT;")?;
    }
    Ok(())
}

fn row_to_chat(r: &Row<'_>) -> rusqlite::Result<Chat> {
    let id: String = r.get(0)?;
    let kind: String = r.get(1)?;
    let unread: i64 = r.get(4)?;
    Ok(Chat {
        kind: match kind.as_str() {
            "group" => ChatKind::Group,
            "direct" => ChatKind::Direct,
            _ if id.ends_with("@g.us") => ChatKind::Group,
            _ => ChatKind::Direct,
        },
        id: ChatId(id),
        name: r.get(2)?,
        last_activity: r.get(3)?,
        unread: u32::try_from(unread.max(0)).unwrap_or(u32::MAX),
        muted: r.get(5)?,
        last_preview: r.get(6)?,
    })
}

/// A message row with its JSON columns still undecoded, so decode errors can
/// be reported as `anyhow` errors rather than squeezed into `rusqlite::Error`.
struct RawMessage {
    id: String,
    chat: String,
    sender: String,
    sender_name: Option<String>,
    from_me: bool,
    ts: i64,
    content: String,
    reply_to: Option<String>,
    status: String,
}

impl RawMessage {
    fn decode(self) -> anyhow::Result<Message> {
        let content: MessageContent = serde_json::from_str(&self.content)
            .with_context(|| format!("decoding content of message {}", self.id))?;
        let reply_to: Option<QuotedMessage> = self
            .reply_to
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        Ok(Message {
            id: MessageId(self.id),
            chat: ChatId(self.chat),
            sender: self.sender,
            sender_name: self.sender_name,
            from_me: self.from_me,
            timestamp: self.ts,
            content,
            reply_to,
            status: status_from_str(&self.status)?,
        })
    }
}

impl RawMessage {
    fn decode_lossy(self) -> Message {
        let fallback = (
            self.id.clone(),
            self.chat.clone(),
            self.sender.clone(),
            self.from_me,
            self.ts,
        );
        match self.decode() {
            Ok(m) => m,
            Err(e) => {
                let (id, chat, sender, from_me, ts) = fallback;
                tracing::warn!("undecodable message {id} in {chat}: {e:#}");
                Message {
                    id: MessageId(id),
                    chat: ChatId(chat),
                    sender,
                    sender_name: None,
                    from_me,
                    timestamp: ts,
                    content: MessageContent::Unsupported {
                        kind: "unreadable".to_owned(),
                    },
                    reply_to: None,
                    status: if from_me {
                        MessageStatus::Sent
                    } else {
                        MessageStatus::Received
                    },
                }
            }
        }
    }
}

fn row_to_raw_message(r: &Row<'_>) -> rusqlite::Result<RawMessage> {
    Ok(RawMessage {
        id: r.get(0)?,
        chat: r.get(1)?,
        sender: r.get(2)?,
        sender_name: r.get(3)?,
        from_me: r.get(4)?,
        ts: r.get(5)?,
        content: r.get(6)?,
        reply_to: r.get(7)?,
        status: r.get(8)?,
    })
}

/// Decodes a page of rows. A row whose JSON cannot be decoded (corrupt, or
/// written by another build) is shown as `Unsupported` instead of failing the
/// whole page, so one bad row never makes a chat unopenable.
fn collect_messages(
    rows: impl Iterator<Item = rusqlite::Result<RawMessage>>,
) -> anyhow::Result<Vec<Message>> {
    let mut out = Vec::new();
    for raw in rows {
        out.push(raw?.decode_lossy());
    }
    out.reverse();
    Ok(out)
}

fn load_message(
    conn: &rusqlite::Connection,
    chat: &ChatId,
    id: &MessageId,
) -> anyhow::Result<Option<Message>> {
    let sql = format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE chat = ?1 AND id = ?2");
    let raw = conn
        .query_row(&sql, params![chat.0, id.0], row_to_raw_message)
        .optional()?;
    raw.map(RawMessage::decode).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChatKind, MediaRef, MessageContent};

    fn direct() -> ChatId {
        ChatId("15551234567@s.whatsapp.net".into())
    }

    fn msg(chat: &ChatId, id: &str, ts: i64, from_me: bool, text: &str) -> Message {
        Message {
            id: MessageId(id.into()),
            chat: chat.clone(),
            sender: if from_me {
                String::new()
            } else {
                chat.0.clone()
            },
            sender_name: None,
            from_me,
            timestamp: ts,
            content: MessageContent::Text(text.into()),
            reply_to: None,
            status: if from_me {
                MessageStatus::Sent
            } else {
                MessageStatus::Received
            },
        }
    }

    fn chat(id: &str, name: &str, last_activity: i64, unread: u32, muted: bool) -> Chat {
        Chat {
            id: ChatId(id.into()),
            kind: if id.ends_with("@g.us") {
                ChatKind::Group
            } else {
                ChatKind::Direct
            },
            name: name.into(),
            last_activity,
            unread,
            muted,
            last_preview: None,
        }
    }

    #[test]
    fn upsert_message_is_idempotent() {
        let s = Store::open_in_memory().unwrap();
        let m = msg(&direct(), "A", 100, false, "hi");
        assert!(s.upsert_message(&m).unwrap());
        assert!(!s.upsert_message(&m).unwrap());
        assert_eq!(s.messages(&direct(), None, 50).unwrap().len(), 1);
        assert_eq!(s.chat(&direct()).unwrap().unwrap().unread, 1);
    }

    #[test]
    fn upsert_creates_placeholder_chat() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_message(&msg(&direct(), "A", 100, false, "hi"))
            .unwrap();
        let c = s.chat(&direct()).unwrap().unwrap();
        assert_eq!(c.name, "15551234567");
        assert_eq!(c.kind, ChatKind::Direct);
        assert_eq!(c.last_preview.as_deref(), Some("hi"));

        let g = ChatId("120363000000000001@g.us".into());
        s.upsert_message(&msg(&g, "B", 100, false, "yo")).unwrap();
        assert_eq!(s.chat(&g).unwrap().unwrap().kind, ChatKind::Group);
    }

    #[test]
    fn from_me_does_not_increment_unread() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_message(&msg(&direct(), "A", 100, true, "hi"))
            .unwrap();
        assert_eq!(s.chat(&direct()).unwrap().unwrap().unread, 0);
    }

    #[test]
    fn older_message_does_not_bump_last_activity() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_message(&msg(&direct(), "new", 200, false, "newer"))
            .unwrap();
        s.upsert_message(&msg(&direct(), "old", 100, false, "older"))
            .unwrap();
        let c = s.chat(&direct()).unwrap().unwrap();
        assert_eq!(c.last_activity, 200);
        assert_eq!(c.last_preview.as_deref(), Some("newer"));
        assert_eq!(c.unread, 2);
    }

    #[test]
    fn upsert_chat_preserves_newer_activity_and_unread() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_message(&msg(&direct(), "A", 500, false, "latest"))
            .unwrap();
        s.upsert_chat(&chat(&direct().0, "Alice", 0, 0, true))
            .unwrap();
        let c = s.chat(&direct()).unwrap().unwrap();
        assert_eq!(c.name, "Alice");
        assert!(c.muted);
        assert_eq!(c.last_activity, 500);
        assert_eq!(c.unread, 1);
        assert_eq!(c.last_preview.as_deref(), Some("latest"));

        let mut newer = chat(&direct().0, "Alice B", 900, 4, false);
        newer.last_preview = Some("from sync".into());
        s.upsert_chat(&newer).unwrap();
        let c = s.chat(&direct()).unwrap().unwrap();
        assert_eq!(c.last_activity, 900);
        assert_eq!(c.unread, 4);
        assert_eq!(c.last_preview.as_deref(), Some("from sync"));
    }

    #[test]
    fn chats_sorted_by_recent_activity() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_chat(&chat("a@s.whatsapp.net", "A", 10, 0, false))
            .unwrap();
        s.upsert_chat(&chat("b@s.whatsapp.net", "B", 30, 0, false))
            .unwrap();
        s.upsert_chat(&chat("c@g.us", "C", 20, 0, false)).unwrap();
        let names: Vec<_> = s.chats().unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["B", "C", "A"]);
    }

    #[test]
    fn messages_pagination() {
        let s = Store::open_in_memory().unwrap();
        for ts in 1..=120 {
            s.upsert_message(&msg(&direct(), &format!("m{ts}"), ts, ts % 2 == 0, "x"))
                .unwrap();
        }
        let ts = |v: Vec<Message>| v.into_iter().map(|m| m.timestamp).collect::<Vec<_>>();
        assert_eq!(
            ts(s.messages(&direct(), None, 50).unwrap()),
            (71..=120).collect::<Vec<_>>()
        );
        assert_eq!(
            ts(s.messages(&direct(), Some(71), 50).unwrap()),
            (21..=70).collect::<Vec<_>>()
        );
        assert_eq!(
            ts(s.messages(&direct(), Some(21), 50).unwrap()),
            (1..=20).collect::<Vec<_>>()
        );
        assert!(s.messages(&direct(), Some(1), 50).unwrap().is_empty());
    }

    #[test]
    fn status_never_downgrades() {
        let s = Store::open_in_memory().unwrap();
        let c = direct();
        let id = MessageId("A".into());
        let status = |s: &Store| s.message(&c, &id).unwrap().unwrap().status;

        let mut m = msg(&c, "A", 1, true, "hi");
        m.status = MessageStatus::Read;
        s.upsert_message(&m).unwrap();
        s.set_status(&c, &id, MessageStatus::Delivered).unwrap();
        assert_eq!(status(&s), MessageStatus::Read);
        // Re-upsert with an older status keeps Read too.
        m.status = MessageStatus::Sent;
        s.upsert_message(&m).unwrap();
        assert_eq!(status(&s), MessageStatus::Read);

        let id = MessageId("B".into());
        let status = |s: &Store| s.message(&c, &id).unwrap().unwrap().status;
        let mut m = msg(&c, "B", 2, true, "hi");
        m.status = MessageStatus::Pending;
        s.upsert_message(&m).unwrap();
        s.set_status(&c, &id, MessageStatus::Failed).unwrap();
        assert_eq!(status(&s), MessageStatus::Failed);
        s.set_status(&c, &id, MessageStatus::Pending).unwrap();
        assert_eq!(status(&s), MessageStatus::Pending);
        s.set_status(&c, &id, MessageStatus::Delivered).unwrap();
        assert_eq!(status(&s), MessageStatus::Delivered);
        s.set_status(&c, &id, MessageStatus::Failed).unwrap();
        assert_eq!(status(&s), MessageStatus::Delivered);
    }

    #[test]
    fn set_status_on_unknown_message_is_ok() {
        let s = Store::open_in_memory().unwrap();
        s.set_status(&direct(), &MessageId("nope".into()), MessageStatus::Read)
            .unwrap();
    }

    #[test]
    fn replace_message_id_rekeys_message_and_reactions() {
        let s = Store::open_in_memory().unwrap();
        let c = direct();
        let old = MessageId::new_local();
        let new = MessageId("SERVER1".into());
        let mut m = msg(&c, &old.0, 5, true, "hello");
        m.status = MessageStatus::Pending;
        s.upsert_message(&m).unwrap();
        s.apply_reaction(&Reaction {
            chat: c.clone(),
            target: old.clone(),
            sender: "me".into(),
            emoji: "👍".into(),
            timestamp: 6,
        })
        .unwrap();
        s.replace_message_id(&c, &old, &new).unwrap();
        assert!(s.message(&c, &old).unwrap().is_none());
        let got = s.message(&c, &new).unwrap().unwrap();
        assert_eq!(got.id, new);
        assert_eq!(got.content, MessageContent::Text("hello".into()));
        assert!(s.reactions(&c, &old).unwrap().is_empty());
        assert_eq!(s.reactions(&c, &new).unwrap().len(), 1);
    }

    #[test]
    fn replace_message_id_when_new_already_exists_drops_old() {
        let s = Store::open_in_memory().unwrap();
        let c = direct();
        s.upsert_message(&msg(&c, "local-1", 5, true, "hello"))
            .unwrap();
        s.upsert_message(&msg(&c, "SERVER1", 5, true, "hello"))
            .unwrap();
        s.replace_message_id(
            &c,
            &MessageId("local-1".into()),
            &MessageId("SERVER1".into()),
        )
        .unwrap();
        assert_eq!(s.messages(&c, None, 50).unwrap().len(), 1);
    }

    #[test]
    fn reaction_upsert_and_removal() {
        let s = Store::open_in_memory().unwrap();
        let c = direct();
        let t = MessageId("A".into());
        let r = |emoji: &str, ts| Reaction {
            chat: c.clone(),
            target: t.clone(),
            sender: "bob".into(),
            emoji: emoji.into(),
            timestamp: ts,
        };
        s.apply_reaction(&r("👍", 1)).unwrap();
        s.apply_reaction(&r("❤️", 2)).unwrap();
        let got = s.reactions(&c, &t).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], r("❤️", 2));
        s.apply_reaction(&r("", 3)).unwrap();
        assert!(s.reactions(&c, &t).unwrap().is_empty());
    }

    #[test]
    fn set_media_path_updates_local_path() {
        let s = Store::open_in_memory().unwrap();
        let c = direct();
        let mut m = msg(&c, "IMG", 1, false, "");
        m.content = MessageContent::Image {
            caption: Some("look".into()),
            media: MediaRef {
                mime: "image/jpeg".into(),
                size: 1234,
                thumbnail: Some(vec![1, 2, 3]),
                local_path: None,
                download_token: vec![9, 9],
            },
        };
        s.upsert_message(&m).unwrap();
        let path = Path::new("/tmp/media/IMG.jpg");
        s.set_media_path(&c, &m.id, path).unwrap();
        let got = s.message(&c, &m.id).unwrap().unwrap();
        assert_eq!(
            got.content.media().unwrap().local_path.as_deref(),
            Some(path)
        );
        // A later re-delivery without local_path (e.g. history sync) keeps it.
        s.upsert_message(&m).unwrap();
        let got = s.message(&c, &m.id).unwrap().unwrap();
        assert_eq!(
            got.content.media().unwrap().local_path.as_deref(),
            Some(path)
        );
    }

    #[test]
    fn mark_read_and_total_unread_ignores_muted() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_chat(&chat("a@s.whatsapp.net", "A", 1, 3, false))
            .unwrap();
        s.upsert_chat(&chat("b@g.us", "B", 1, 5, true)).unwrap();
        s.upsert_chat(&chat("c@s.whatsapp.net", "C", 1, 2, false))
            .unwrap();
        assert_eq!(s.total_unread().unwrap(), 5);
        s.mark_read(&ChatId("a@s.whatsapp.net".into())).unwrap();
        assert_eq!(
            s.chat(&ChatId("a@s.whatsapp.net".into()))
                .unwrap()
                .unwrap()
                .unread,
            0
        );
        assert_eq!(s.total_unread().unwrap(), 2);
    }

    #[test]
    fn delete_message_removes_row_and_refreshes_preview() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_message(&msg(&direct(), "A", 100, false, "first"))
            .unwrap();
        s.upsert_message(&msg(&direct(), "B", 200, false, "second"))
            .unwrap();
        s.apply_reaction(&Reaction {
            chat: direct(),
            target: MessageId("B".into()),
            sender: "x".into(),
            emoji: "👍".into(),
            timestamp: 1,
        })
        .unwrap();
        assert!(s.delete_message(&direct(), &MessageId("B".into())).unwrap());
        assert!(!s.delete_message(&direct(), &MessageId("B".into())).unwrap());
        assert_eq!(s.messages(&direct(), None, 10).unwrap().len(), 1);
        assert!(
            s.reactions(&direct(), &MessageId("B".into()))
                .unwrap()
                .is_empty()
        );
        let c = s.chat(&direct()).unwrap().unwrap();
        assert_eq!(c.last_preview.as_deref(), Some("first"));
        assert!(s.delete_message(&direct(), &MessageId("A".into())).unwrap());
        assert_eq!(s.chat(&direct()).unwrap().unwrap().last_preview, None);
    }

    #[test]
    fn delete_chat_removes_everything_for_that_chat_only() {
        let s = Store::open_in_memory().unwrap();
        let other = ChatId("2@s.whatsapp.net".into());
        s.upsert_message(&msg(&direct(), "A", 100, false, "a"))
            .unwrap();
        s.upsert_message(&msg(&other, "B", 100, false, "b"))
            .unwrap();
        s.delete_chat(&direct()).unwrap();
        assert!(s.chat(&direct()).unwrap().is_none());
        assert!(s.messages(&direct(), None, 10).unwrap().is_empty());
        assert_eq!(s.chats().unwrap().len(), 1);
        assert_eq!(s.messages(&other, None, 10).unwrap().len(), 1);
    }

    #[test]
    fn replace_content_keeps_row_and_updates_preview() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_message(&msg(&direct(), "A", 100, true, "oops"))
            .unwrap();
        let placeholder = MessageContent::Text(crate::model::REVOKED_BY_ME.to_owned());
        s.replace_content(&direct(), &MessageId("A".into()), &placeholder)
            .unwrap();
        let m = s
            .message(&direct(), &MessageId("A".into()))
            .unwrap()
            .unwrap();
        assert_eq!(m.content, placeholder);
        assert_eq!(m.timestamp, 100);
        assert!(m.from_me);
        assert_eq!(
            s.chat(&direct()).unwrap().unwrap().last_preview.as_deref(),
            Some(crate::model::REVOKED_BY_ME)
        );
    }

    #[test]
    fn search_matches_text_captions_and_file_names_only() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_message(&msg(
            &direct(),
            "A",
            100,
            false,
            "Dinner at Luigi's tonight",
        ))
        .unwrap();
        s.upsert_message(&msg(&direct(), "B", 200, false, "see you"))
            .unwrap();
        let media = MediaRef {
            mime: "application/pdf".into(),
            size: 1,
            thumbnail: Some(vec![255, 216, 100]),
            local_path: None,
            download_token: vec![1, 2, 3],
        };
        let mut doc = msg(&direct(), "C", 300, false, "");
        doc.content = MessageContent::Document {
            file_name: "Dinner-menu.pdf".into(),
            caption: Some("menu".into()),
            media: media.clone(),
        };
        s.upsert_message(&doc).unwrap();
        let mut img = msg(&direct(), "D", 400, true, "");
        img.content = MessageContent::Image {
            caption: Some("Luigi's terrace".into()),
            media,
        };
        s.upsert_message(&img).unwrap();

        let ids = |q: &str| -> Vec<String> {
            s.search_messages(q, 50)
                .unwrap()
                .into_iter()
                .map(|m| m.id.0)
                .collect()
        };
        assert_eq!(ids("dinner"), ["C", "A"], "newest first, case-insensitive");
        assert_eq!(ids("luigi"), ["D", "A"]);
        assert_eq!(ids("menu"), ["C"]);
        assert!(ids("Text").is_empty(), "JSON keys must not match");
        assert!(ids("255").is_empty(), "thumbnail bytes must not match");
        assert!(ids("mime").is_empty());
        assert!(ids("  ").is_empty());
        assert_eq!(ids("100%"), Vec::<String>::new(), "wildcards are literal");
        assert_eq!(s.search_messages("e", 1).unwrap().len(), 1);
    }

    #[test]
    fn like_escape_quotes_wildcards() {
        assert_eq!(like_escape("a%b_c\\d"), "a\\%b\\_c\\\\d");
    }

    #[test]
    fn wipe_clears_everything() {
        let s = Store::open_in_memory().unwrap();
        let c = direct();
        s.upsert_message(&msg(&c, "A", 1, false, "x")).unwrap();
        s.apply_reaction(&Reaction {
            chat: c.clone(),
            target: MessageId("A".into()),
            sender: "x".into(),
            emoji: "😂".into(),
            timestamp: 1,
        })
        .unwrap();
        s.wipe().unwrap();
        assert!(s.chats().unwrap().is_empty());
        assert!(s.messages(&c, None, 50).unwrap().is_empty());
        assert!(s.reactions(&c, &MessageId("A".into())).unwrap().is_empty());
    }

    #[test]
    fn chat_avatars_round_trip_and_survive_upsert() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.chat_avatars().unwrap().is_empty());
        s.upsert_chat(&chat(&direct().0, "Alice", 10, 0, false))
            .unwrap();
        s.set_chat_avatar(&direct(), Path::new("/tmp/a.jpg"))
            .unwrap();
        // A later chat upsert must not clear the picture.
        s.upsert_chat(&chat(&direct().0, "Alice B", 20, 1, false))
            .unwrap();
        let avatars = s.chat_avatars().unwrap();
        assert_eq!(avatars.get(&direct()), Some(&PathBuf::from("/tmp/a.jpg")));
        // Unknown chat: no row, no error.
        s.set_chat_avatar(&ChatId("nobody@s.whatsapp.net".into()), Path::new("/x"))
            .unwrap();
        assert_eq!(s.chat_avatars().unwrap().len(), 1);
    }

    #[test]
    fn old_database_gains_avatar_column() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE chats (id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL,
             last_activity INTEGER NOT NULL DEFAULT 0, unread INTEGER NOT NULL DEFAULT 0,
             muted INTEGER NOT NULL DEFAULT 0, last_preview TEXT);",
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        s.upsert_chat(&chat(&direct().0, "Alice", 10, 0, false))
            .unwrap();
        s.set_chat_avatar(&direct(), Path::new("/tmp/a.jpg"))
            .unwrap();
        assert_eq!(s.chat_avatars().unwrap().len(), 1);
    }

    #[test]
    fn open_file_db_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        {
            let s = Store::open(&path).unwrap();
            s.upsert_message(&msg(&direct(), "A", 1, false, "persist me"))
                .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let got = s
            .message(&direct(), &MessageId("A".into()))
            .unwrap()
            .unwrap();
        assert_eq!(got.content, MessageContent::Text("persist me".into()));
        // A second concurrent handle works too (WAL).
        let s2 = Store::open(&path).unwrap();
        assert_eq!(s2.chats().unwrap().len(), 1);
    }

    #[test]
    fn messages_before_does_not_skip_same_second_messages() {
        let s = Store::open_in_memory().unwrap();
        // Two oldest messages share a timestamp (an album / burst).
        s.upsert_message(&msg(&direct(), "a", 1000, false, "a"))
            .unwrap();
        s.upsert_message(&msg(&direct(), "b", 1000, false, "b"))
            .unwrap();
        for i in 0..50 {
            s.upsert_message(&msg(&direct(), &format!("n{i}"), 2000 + i, false, "x"))
                .unwrap();
        }
        let page = s.messages(&direct(), None, 51).unwrap();
        assert_eq!(page.len(), 51);
        let oldest = &page[0];
        assert_eq!(oldest.id.0, "b");
        // The old `ts < before` cursor loses "a" for good.
        assert!(
            s.messages(&direct(), Some(oldest.timestamp), 50)
                .unwrap()
                .is_empty()
        );
        let older = s
            .messages_before(&direct(), oldest.timestamp, &oldest.id, 50)
            .unwrap();
        let ids: Vec<_> = older.iter().map(|m| m.id.0.as_str()).collect();
        assert_eq!(ids, ["a"]);
        let none = s
            .messages_before(&direct(), older[0].timestamp, &older[0].id, 50)
            .unwrap();
        assert!(none.is_empty());
        // Unknown cursor id: plain `ts < before`.
        let fallback = s
            .messages_before(&direct(), 2000, &MessageId("gone".into()), 50)
            .unwrap();
        assert_eq!(fallback.len(), 2);
    }

    #[test]
    fn fail_stale_pending_only_touches_local_pending() {
        let s = Store::open_in_memory().unwrap();
        let c = direct();
        let mut pending = msg(&c, "local-1", 1, true, "x");
        pending.status = MessageStatus::Pending;
        s.upsert_message(&pending).unwrap();
        let mut sent = msg(&c, "local-2", 2, true, "y");
        sent.status = MessageStatus::Sent;
        s.upsert_message(&sent).unwrap();
        let mut server = msg(&c, "SERVER", 3, true, "z");
        server.status = MessageStatus::Pending;
        s.upsert_message(&server).unwrap();
        assert_eq!(s.fail_stale_pending().unwrap(), 1);
        let status = |id: &str| {
            s.message(&c, &MessageId(id.into()))
                .unwrap()
                .unwrap()
                .status
        };
        assert_eq!(status("local-1"), MessageStatus::Failed);
        assert_eq!(status("local-2"), MessageStatus::Sent);
        assert_eq!(status("SERVER"), MessageStatus::Pending);
    }

    #[test]
    fn undecodable_row_does_not_break_the_page() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_message(&msg(&direct(), "good", 1, false, "fine"))
            .unwrap();
        s.upsert_message(&msg(&direct(), "bad", 2, false, "x"))
            .unwrap();
        s.conn
            .execute(
                "UPDATE messages SET content = '{\"FutureVariant\":1}' WHERE id = 'bad'",
                [],
            )
            .unwrap();
        let page = s.messages(&direct(), None, 50).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].content, MessageContent::Text("fine".into()));
        assert!(matches!(
            page[1].content,
            MessageContent::Unsupported { .. }
        ));
    }

    #[test]
    fn reactions_since_loads_a_page_in_one_query() {
        let s = Store::open_in_memory().unwrap();
        let c = direct();
        s.upsert_message(&msg(&c, "old", 1, false, "x")).unwrap();
        s.upsert_message(&msg(&c, "new", 10, false, "y")).unwrap();
        for target in ["old", "new"] {
            s.apply_reaction(&Reaction {
                chat: c.clone(),
                target: MessageId(target.into()),
                sender: "a".into(),
                emoji: "👍".into(),
                timestamp: 5,
            })
            .unwrap();
        }
        let got = s.reactions_since(&c, 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].target.0, "new");
        assert_eq!(s.reactions_since(&c, 0).unwrap().len(), 2);
    }

    #[test]
    fn upsert_chat_at_latest_activity_clears_unread_without_reordering() {
        // How the backend reports "read on another device".
        let s = Store::open_in_memory().unwrap();
        let c = direct();
        s.upsert_message(&msg(&c, "A", 1000, false, "x")).unwrap();
        s.upsert_message(&msg(&c, "B", 2000, false, "y")).unwrap();
        assert_eq!(s.chat(&c).unwrap().unwrap().unread, 2);
        // Read up to an older message: newer unread stays.
        s.upsert_chat(&chat(&c.0, "Ana", 1000, 0, false)).unwrap();
        assert_eq!(s.chat(&c).unwrap().unwrap().unread, 2);
        s.upsert_chat(&chat(&c.0, "Ana", 2000, 0, false)).unwrap();
        let got = s.chat(&c).unwrap().unwrap();
        assert_eq!(got.unread, 0);
        assert_eq!(got.last_activity, 2000);
        assert_eq!(got.last_preview.as_deref(), Some("y"));
    }
}
