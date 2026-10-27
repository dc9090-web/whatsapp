//! The app's own SQLite database (`app.db`): chats, messages, reactions.
//!
//! Separate from the protocol library's session database so its schema never
//! constrains UI queries. Each `Store` owns one connection; open one per thread
//! (the runtime and the UI each open their own; WAL mode makes that safe).
//!
//! CONTRACT (implemented in Task 2): signatures below are fixed — other crates
//! are written against them in parallel.

use std::path::Path;

use crate::model::{Chat, ChatId, Message, MessageId, MessageStatus, Reaction};

pub struct Store {
    #[allow(dead_code)]
    conn: rusqlite::Connection,
}

impl Store {
    /// Opens (creating and migrating if needed) the database at `path`, in WAL mode.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let _ = path;
        todo!("Task 2")
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        todo!("Task 2")
    }

    /// Inserts or replaces chat metadata. Preserves the existing `unread`,
    /// `last_activity` and `last_preview` if the incoming values are older/empty.
    pub fn upsert_chat(&self, chat: &Chat) -> anyhow::Result<()> {
        let _ = chat;
        todo!("Task 2")
    }

    pub fn chat(&self, id: &ChatId) -> anyhow::Result<Option<Chat>> {
        let _ = id;
        todo!("Task 2")
    }

    /// All chats, most recent `last_activity` first.
    pub fn chats(&self) -> anyhow::Result<Vec<Chat>> {
        todo!("Task 2")
    }

    /// Idempotent on `(chat, id)`. Returns `true` if the message was new.
    /// When new: creates a placeholder chat if missing (name = `chat.user()`),
    /// bumps the chat's `last_activity`/`last_preview` if this message is newer,
    /// and increments `unread` when `!from_me`.
    /// When it already exists: updates content/status but never downgrades status.
    pub fn upsert_message(&self, msg: &Message) -> anyhow::Result<bool> {
        let _ = msg;
        todo!("Task 2")
    }

    pub fn message(&self, chat: &ChatId, id: &MessageId) -> anyhow::Result<Option<Message>> {
        let _ = (chat, id);
        todo!("Task 2")
    }

    /// Up to `limit` messages strictly older than `before` (unix ms), or the newest
    /// `limit` if `before` is `None`. Returned in ascending timestamp order.
    pub fn messages(
        &self,
        chat: &ChatId,
        before: Option<i64>,
        limit: usize,
    ) -> anyhow::Result<Vec<Message>> {
        let _ = (chat, before, limit);
        todo!("Task 2")
    }

    /// Sets status, except that it never moves backwards (Read stays Read),
    /// with the one exception that `Failed` may replace `Pending`.
    pub fn set_status(
        &self,
        chat: &ChatId,
        id: &MessageId,
        status: MessageStatus,
    ) -> anyhow::Result<()> {
        let _ = (chat, id, status);
        todo!("Task 2")
    }

    /// Re-keys an optimistic message (and its reactions) once the server id is known.
    pub fn replace_message_id(
        &self,
        chat: &ChatId,
        old: &MessageId,
        new: &MessageId,
    ) -> anyhow::Result<()> {
        let _ = (chat, old, new);
        todo!("Task 2")
    }

    /// Upsert keyed by `(chat, target, sender)`; an empty emoji deletes the row.
    pub fn apply_reaction(&self, reaction: &Reaction) -> anyhow::Result<()> {
        let _ = reaction;
        todo!("Task 2")
    }

    pub fn reactions(&self, chat: &ChatId, target: &MessageId) -> anyhow::Result<Vec<Reaction>> {
        let _ = (chat, target);
        todo!("Task 2")
    }

    /// Records where the decrypted media file lives (updates `MediaRef::local_path`).
    pub fn set_media_path(
        &self,
        chat: &ChatId,
        id: &MessageId,
        path: &Path,
    ) -> anyhow::Result<()> {
        let _ = (chat, id, path);
        todo!("Task 2")
    }

    /// Sets the chat's unread count to 0.
    pub fn mark_read(&self, chat: &ChatId) -> anyhow::Result<()> {
        let _ = chat;
        todo!("Task 2")
    }

    /// Sum of `unread` over non-muted chats (tray badge).
    pub fn total_unread(&self) -> anyhow::Result<u32> {
        todo!("Task 2")
    }

    /// Deletes all rows (used by explicit Log out).
    pub fn wipe(&self) -> anyhow::Result<()> {
        todo!("Task 2")
    }
}

