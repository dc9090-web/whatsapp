//! The backend's own small sidecar database (next to the session database).
//!
//! The UI only hands the backend chat and message ids, but quoting, reacting
//! and read receipts need the original sender, `from_me` flag and message
//! body. Chat names and mute state are kept here too so every
//! `ChatUpserted` the backend emits carries the best-known values instead of
//! clobbering what an earlier event established.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use super::convert::KeyRecord;
use crate::model::ChatId;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS msg_keys (
  chat TEXT NOT NULL, id TEXT NOT NULL, sender TEXT NOT NULL,
  from_me INTEGER NOT NULL, ts INTEGER NOT NULL, quote BLOB,
  PRIMARY KEY (chat, id)
);
CREATE INDEX IF NOT EXISTS msg_keys_ts ON msg_keys(ts);
CREATE TABLE IF NOT EXISTS chat_meta (
  chat TEXT PRIMARY KEY, name TEXT, name_rank INTEGER NOT NULL DEFAULT 0,
  muted INTEGER NOT NULL DEFAULT 0, known INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS lid_map (lid TEXT PRIMARY KEY, pn TEXT NOT NULL);
";

/// Keep at most this many message keys (newest win).
pub const MAX_KEYS: usize = 50_000;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatMeta {
    pub name: Option<String>,
    pub muted: bool,
    /// Whether the UI has been told about this chat.
    pub known: bool,
}

pub struct Index {
    conn: Connection,
}

impl Index {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn put_keys(&mut self, records: &[KeyRecord]) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO msg_keys (chat, id, sender, from_me, ts, quote)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(chat, id) DO UPDATE SET
                   sender = excluded.sender, from_me = excluded.from_me,
                   quote = COALESCE(excluded.quote, msg_keys.quote)",
            )?;
            for r in records {
                stmt.execute(params![
                    r.chat.0,
                    r.id,
                    r.sender,
                    r.from_me,
                    r.timestamp,
                    r.quote
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn key(&self, chat: &ChatId, id: &str) -> anyhow::Result<Option<KeyRecord>> {
        Ok(self
            .conn
            .query_row(
                "SELECT sender, from_me, ts, quote FROM msg_keys WHERE chat = ?1 AND id = ?2",
                params![chat.0, id],
                |row| {
                    Ok(KeyRecord {
                        chat: chat.clone(),
                        id: id.to_owned(),
                        sender: row.get(0)?,
                        from_me: row.get(1)?,
                        timestamp: row.get(2)?,
                        quote: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// Drops the oldest keys beyond `max`.
    pub fn prune(&self, max: usize) -> anyhow::Result<usize> {
        let max = i64::try_from(max).unwrap_or(i64::MAX);
        Ok(self.conn.execute(
            "DELETE FROM msg_keys WHERE rowid IN (
               SELECT rowid FROM msg_keys ORDER BY ts DESC LIMIT -1 OFFSET ?1)",
            params![max],
        )?)
    }

    pub fn chat_meta(&self, chat: &ChatId) -> anyhow::Result<ChatMeta> {
        Ok(self
            .conn
            .query_row(
                "SELECT name, muted, known FROM chat_meta WHERE chat = ?1",
                params![chat.0],
                |row| {
                    Ok(ChatMeta {
                        name: row.get(0)?,
                        muted: row.get(1)?,
                        known: row.get(2)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    /// Direct chats the UI knows about: the people a status is shared with.
    pub fn known_direct_chats(&self) -> anyhow::Result<Vec<ChatId>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT chat FROM chat_meta WHERE known = 1 AND chat LIKE '%@s.whatsapp.net'",
        )?;
        let rows = stmt.query_map([], |r| Ok(ChatId(r.get(0)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn ensure_row(&self, chat: &ChatId) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO chat_meta (chat) VALUES (?1)",
            params![chat.0],
        )?;
        Ok(())
    }

    /// Records `name` unless a better-ranked name is already known. Returns
    /// `true` if the stored name changed.
    pub fn set_name(&self, chat: &ChatId, name: &str, rank: i64) -> anyhow::Result<bool> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(false);
        }
        self.ensure_row(chat)?;
        let changed = self.conn.execute(
            "UPDATE chat_meta SET name = ?2, name_rank = ?3
             WHERE chat = ?1 AND name_rank <= ?3 AND (name IS NULL OR name <> ?2)",
            params![chat.0, name, rank],
        )?;
        Ok(changed > 0)
    }

    /// Returns `true` if the mute state changed.
    pub fn set_muted(&self, chat: &ChatId, muted: bool) -> anyhow::Result<bool> {
        self.ensure_row(chat)?;
        let changed = self.conn.execute(
            "UPDATE chat_meta SET muted = ?2 WHERE chat = ?1 AND muted <> ?2",
            params![chat.0, muted],
        )?;
        Ok(changed > 0)
    }

    /// Marks the chat as announced to the UI. Returns `true` the first time.
    pub fn mark_known(&self, chat: &ChatId) -> anyhow::Result<bool> {
        self.ensure_row(chat)?;
        let changed = self.conn.execute(
            "UPDATE chat_meta SET known = 1 WHERE chat = ?1 AND known = 0",
            params![chat.0],
        )?;
        Ok(changed > 0)
    }

    pub fn put_lid(&self, lid_user: &str, pn_user: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO lid_map (lid, pn) VALUES (?1, ?2)",
            params![lid_user, pn_user],
        )?;
        Ok(())
    }

    pub fn lid_map(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT lid, pn FROM lid_map")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, ts: i64, quote: Option<Vec<u8>>) -> KeyRecord {
        KeyRecord {
            chat: ChatId("c@g.us".into()),
            id: id.into(),
            sender: "1@s.whatsapp.net".into(),
            from_me: false,
            timestamp: ts,
            quote,
        }
    }

    #[test]
    fn keys_round_trip_and_keep_quote_on_update() {
        let mut idx = Index::open_in_memory().expect("open");
        idx.put_keys(&[rec("a", 1, Some(vec![1, 2]))]).expect("put");
        idx.put_keys(&[rec("a", 1, None)]).expect("put again");
        let got = idx
            .key(&ChatId("c@g.us".into()), "a")
            .expect("query")
            .expect("present");
        assert_eq!(got.quote, Some(vec![1, 2]));
        assert_eq!(got.sender, "1@s.whatsapp.net");
        assert!(
            idx.key(&ChatId("c@g.us".into()), "zzz")
                .expect("query")
                .is_none()
        );
    }

    #[test]
    fn prune_keeps_newest() {
        let mut idx = Index::open_in_memory().expect("open");
        let recs: Vec<_> = (0..10).map(|i| rec(&format!("m{i}"), i, None)).collect();
        idx.put_keys(&recs).expect("put");
        assert_eq!(idx.prune(3).expect("prune"), 7);
        let chat = ChatId("c@g.us".into());
        assert!(idx.key(&chat, "m9").expect("q").is_some());
        assert!(idx.key(&chat, "m7").expect("q").is_some());
        assert!(idx.key(&chat, "m6").expect("q").is_none());
    }

    #[test]
    fn names_respect_rank() {
        let idx = Index::open_in_memory().expect("open");
        let chat = ChatId("1@s.whatsapp.net".into());
        assert!(idx.set_name(&chat, "push", 1).expect("set"));
        assert!(!idx.set_name(&chat, "push", 1).expect("same name"));
        assert!(idx.set_name(&chat, "Contact", 3).expect("better"));
        assert!(!idx.set_name(&chat, "push2", 1).expect("worse"));
        assert!(!idx.set_name(&chat, "  ", 3).expect("blank"));
        assert_eq!(
            idx.chat_meta(&chat).expect("meta").name.as_deref(),
            Some("Contact")
        );
    }

    #[test]
    fn mute_and_known_flags() {
        let idx = Index::open_in_memory().expect("open");
        let chat = ChatId("1@s.whatsapp.net".into());
        assert_eq!(idx.chat_meta(&chat).expect("meta"), ChatMeta::default());
        assert!(idx.set_muted(&chat, true).expect("mute"));
        assert!(!idx.set_muted(&chat, true).expect("again"));
        assert!(idx.mark_known(&chat).expect("known"));
        assert!(!idx.mark_known(&chat).expect("known again"));
        let meta = idx.chat_meta(&chat).expect("meta");
        assert!(meta.muted && meta.known);
    }

    #[test]
    fn lid_map_persists() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("idx.db");
        Index::open(&path)
            .expect("open")
            .put_lid("100", "1555")
            .expect("put");
        let idx = Index::open(&path).expect("reopen");
        assert_eq!(
            idx.lid_map().expect("map"),
            vec![("100".to_owned(), "1555".to_owned())]
        );
    }
}
