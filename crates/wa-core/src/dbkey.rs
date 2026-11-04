//! At-rest encryption for the SQLite databases (SQLCipher).
//!
//! Every database the app writes (`app.db`, `session.db` and its sidecar
//! index) is keyed with one random 256-bit key, so a copied file is useless
//! without it. The key is applied as a SQLCipher *raw* key, which skips the
//! passphrase KDF: opening stays instant and the strength is the key itself.
//!
//! Databases written by earlier versions are plaintext; [`migrate_plaintext`]
//! re-encrypts them in place the first time they are opened.

use std::fmt;
use std::path::Path;

use anyhow::Context as _;
use rusqlite::Connection;

const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// A 256-bit SQLCipher raw key.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DbKey([u8; 32]);

impl fmt::Debug for DbKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DbKey(..)")
    }
}

impl DbKey {
    /// A fresh key from the OS random source.
    pub fn random() -> Self {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("OS random source unavailable");
        Self(bytes)
    }

    /// Exactly 32 bytes, or `None`.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        <[u8; 32]>::try_from(bytes).ok().map(Self)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The key as SQLCipher's raw-key literal, `x'<64 hex digits>'`.
    fn raw_literal(&self) -> String {
        format!("\"x'{}'\"", hex::encode(self.0))
    }

    /// The `PRAGMA key` statement for this key, for connections not opened
    /// through [`open`] (the protocol library pools its own).
    pub(crate) fn key_pragma(&self) -> String {
        format!("PRAGMA key = {};", self.raw_literal())
    }

    /// Keys a freshly opened connection and verifies the key fits the file.
    /// Must be the first statement on the connection.
    pub fn apply(&self, conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(&self.key_pragma())
            .context("setting the database key")?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })
        .context("the database key does not fit this file")?;
        Ok(())
    }
}

/// How the key is filed in the desktop keyring (Secret Service).
const KEYRING_ATTRS: [(&str, &str); 2] = [("app", "whatsapp-cosmic"), ("purpose", "database-key")];
const KEYRING_LABEL: &str = "WhatsApp for COSMIC database key";

/// The app's database key from the desktop keyring, created on first run.
pub async fn load_or_create() -> anyhow::Result<DbKey> {
    load_or_create_with(&KEYRING_ATTRS).await
}

async fn load_or_create_with(attrs: &[(&str, &str); 2]) -> anyhow::Result<DbKey> {
    let keyring = oo7::Keyring::new()
        .await
        .context("connecting to the desktop keyring (Secret Service)")?;
    if keyring.is_locked().await.unwrap_or(false) {
        keyring.unlock().await.context("unlocking the keyring")?;
    }
    for item in keyring
        .search_items(attrs)
        .await
        .context("searching the keyring")?
    {
        let secret = item.secret().await.context("reading the key")?;
        match DbKey::from_bytes(secret.as_bytes()) {
            Some(key) => return Ok(key),
            None => tracing::warn!("ignoring a malformed database key in the keyring"),
        }
    }
    let key = DbKey::random();
    keyring
        .create_item(KEYRING_LABEL, attrs, key.as_bytes().to_vec(), true)
        .await
        .context("storing the new key in the keyring")?;
    tracing::info!("created a new database key in the keyring");
    Ok(key)
}

/// Whether `path` is an unencrypted SQLite file. A missing file is not.
pub fn is_plaintext(path: &Path) -> std::io::Result<bool> {
    use std::io::Read as _;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut header = [0u8; 16];
    let n = file.read(&mut header)?;
    Ok(n == header.len() && &header == SQLITE_HEADER)
}

/// Re-encrypts a plaintext database at `path` with `key`, in place. Returns
/// whether anything was done. The plaintext copy is gone afterwards.
pub fn migrate_plaintext(path: &Path, key: &DbKey) -> anyhow::Result<bool> {
    if !is_plaintext(path)? {
        return Ok(false);
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".enc-tmp");
    let tmp = std::path::PathBuf::from(tmp);
    remove_db_files(&tmp);
    let tmp_str = tmp.to_str().context("database path is not valid UTF-8")?;

    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    conn.execute(
        &format!("ATTACH DATABASE ?1 AS enc KEY {}", key.raw_literal()),
        [tmp_str],
    )
    .context("attaching the encrypted copy")?;
    let version: i64 = conn.query_row("PRAGMA main.user_version", [], |r| r.get(0))?;
    conn.execute_batch(&format!(
        "SELECT sqlcipher_export('enc'); PRAGMA enc.user_version = {version}; DETACH DATABASE enc;"
    ))
    .context("exporting to the encrypted copy")?;
    drop(conn);

    std::fs::rename(&tmp, path)
        .with_context(|| format!("replacing {} with its encrypted copy", path.display()))?;
    // A clean close already removed the plaintext WAL; make sure nothing of
    // the old file survives next to the new one.
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut side = path.as_os_str().to_os_string();
        side.push(suffix);
        let _ = std::fs::remove_file(side);
    }
    tracing::info!(path = %path.display(), "encrypted an existing database");
    Ok(true)
}

fn remove_db_files(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_os_string();
        p.push(suffix);
        let _ = std::fs::remove_file(p);
    }
}

/// Opens (creating if needed) the database at `path` keyed with `key`,
/// encrypting a plaintext file from an earlier version first.
pub fn open(path: &Path, key: &DbKey) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    migrate_plaintext(path, key).with_context(|| format!("encrypting {}", path.display()))?;
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    key.apply(&conn)
        .with_context(|| format!("unlocking {}", path.display()))?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn count_tables(conn: &Connection) -> rusqlite::Result<i64> {
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
    }

    #[test]
    fn random_keys_are_32_bytes_and_differ() {
        let a = DbKey::random();
        let b = DbKey::random();
        assert_eq!(a.as_bytes().len(), 32);
        assert_ne!(a.as_bytes(), b.as_bytes());
        assert_eq!(
            DbKey::from_bytes(a.as_bytes()).unwrap().as_bytes(),
            a.as_bytes()
        );
        assert!(DbKey::from_bytes(&[1, 2, 3]).is_none());
    }

    #[test]
    fn keyed_database_is_unreadable_without_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.db");
        let key = DbKey::random();
        {
            let conn = open(&path, &key).unwrap();
            conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
                .unwrap();
        }
        assert!(
            !is_plaintext(&path).unwrap(),
            "file still has a SQLite header"
        );

        let plain = Connection::open(&path).unwrap();
        assert!(count_tables(&plain).is_err(), "opened without a key");

        let wrong = Connection::open(&path).unwrap();
        assert!(DbKey::random().apply(&wrong).is_err(), "wrong key accepted");

        let right = open(&path, &key).unwrap();
        let x: i64 = right
            .query_row("SELECT x FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(x, 1);
    }

    #[test]
    fn plaintext_database_is_migrated_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (7);")
                .unwrap();
        }
        assert!(is_plaintext(&path).unwrap());
        let key = DbKey::random();

        assert!(
            migrate_plaintext(&path, &key).unwrap(),
            "first run migrates"
        );
        assert!(!is_plaintext(&path).unwrap());
        assert!(
            !migrate_plaintext(&path, &key).unwrap(),
            "second run is a no-op"
        );

        let conn = open(&path, &key).unwrap();
        let x: i64 = conn.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(x, 7);

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n != "p.db" && !n.starts_with("p.db-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "plaintext copy left behind: {leftovers:?}"
        );
    }

    /// Talks to the real Secret Service; run by hand with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn keyring_round_trip() {
        let attrs = [("app", "whatsapp-cosmic"), ("purpose", "dbkey-test")];
        let keyring = oo7::Keyring::new().await.unwrap();
        keyring.delete(&attrs).await.unwrap();

        let first = load_or_create_with(&attrs).await.unwrap();
        let second = load_or_create_with(&attrs).await.unwrap();
        assert_eq!(first, second, "a second launch must find the same key");

        keyring.delete(&attrs).await.unwrap();
        let third = load_or_create_with(&attrs).await.unwrap();
        assert_ne!(first, third, "a missing key is regenerated");
        keyring.delete(&attrs).await.unwrap();
    }

    #[test]
    fn missing_database_needs_no_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("none.db");
        assert!(!is_plaintext(&path).unwrap());
        assert!(!migrate_plaintext(&path, &DbKey::random()).unwrap());
    }
}
