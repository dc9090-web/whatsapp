CREATE TABLE IF NOT EXISTS chats (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL,
  last_activity INTEGER NOT NULL DEFAULT 0, unread INTEGER NOT NULL DEFAULT 0,
  muted INTEGER NOT NULL DEFAULT 0, last_preview TEXT
);
CREATE TABLE IF NOT EXISTS messages (
  chat TEXT NOT NULL, id TEXT NOT NULL, sender TEXT NOT NULL, sender_name TEXT,
  from_me INTEGER NOT NULL, ts INTEGER NOT NULL, content TEXT NOT NULL, -- serde_json MessageContent
  reply_to TEXT, -- serde_json QuotedMessage
  status TEXT NOT NULL, -- serde_json MessageStatus
  PRIMARY KEY (chat, id)
);
CREATE INDEX IF NOT EXISTS messages_by_ts ON messages(chat, ts);
CREATE TABLE IF NOT EXISTS reactions (
  chat TEXT NOT NULL, target TEXT NOT NULL, sender TEXT NOT NULL,
  emoji TEXT NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY (chat, target, sender)
);
PRAGMA user_version = 1;
