//! The server's mailbox: one SQLite table on the instance's disk. It is lost with the instance, which the demo accepts;
//! the owner keeps their own copy of everything they have fetched.
use std::{
    path::Path,
    sync::{Mutex, MutexGuard, PoisonError},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Past this, new messages are refused rather than old ones dropped: one chatty sender must not be able to erase the rest.
pub const MAX_MESSAGES: i64 = 500;
pub const MAX_TOTAL_BYTES: i64 = 4 * 1024 * 1024;
/// Messages per `list` page; the owner pages through the rest.
const PAGE_SIZE: u32 = 50;

/// One message as the owner receives it. Which fields are verified is part of the contract: see the field comments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredMessage {
    /// The server's id; ids restart at 1 when the instance is replaced.
    pub id: i64,
    pub received_at: u64,
    /// Verified: the Iroh endpoint id the message came from, from the connection's handshake.
    pub sender_endpoint_id: String,
    /// Verified: the label an owner signed into the invite that was used.
    pub invite_label: String,
    pub invite_id: String,
    /// Not verified: who the sender says it is.
    pub from: Option<String>,
    /// Not verified: an invite to the sender's own answering machine.
    pub reply_invite: Option<String>,
    /// Not verified: the A2A message, as normalized by the server.
    pub message: Value,
}

/// What is waiting on the server. A named struct, not a tuple, so callers read `stats.waiting`, not `.0` (Canonical's
/// "Pattern matching discipline": https://canonical.github.io/rust-best-practices/pattern-matching-discipline.html).
#[derive(Debug, Clone, PartialEq)]
pub struct MailboxStats {
    pub waiting: i64,
    pub bytes: i64,
    pub oldest_received_at: Option<u64>,
}

#[derive(Debug)]
pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    /// The connection. A panic elsewhere while it was held (one failed request) leaves SQLite consistent, so the lock is
    /// taken back rather than every later request failing too.
    fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(connection: Connection) -> Result<Self> {
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS messages (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               received_at INTEGER NOT NULL,
               sender_endpoint_id TEXT NOT NULL,
               invite_label TEXT NOT NULL,
               invite_id TEXT NOT NULL,
               from_text TEXT,
               reply_invite TEXT,
               message TEXT NOT NULL,
               size INTEGER NOT NULL
             );",
        )?;
        Ok(Self { connection: Mutex::new(connection) })
    }

    /// Store a message (its `id` is ignored) and return the id it was given.
    pub fn insert(&self, message: &StoredMessage) -> Result<i64> {
        let body = serde_json::to_string(&message.message)?;
        let size = stored_size(&body, message);
        let connection = self.connection();
        let (count, bytes) = totals(&connection)?;
        if count >= MAX_MESSAGES || bytes + size > MAX_TOTAL_BYTES {
            bail!("the mailbox is full; try again after its owner has read their messages");
        }
        connection.execute(
            "INSERT INTO messages (received_at, sender_endpoint_id, invite_label, invite_id, from_text, reply_invite, message, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                message.received_at as i64,
                message.sender_endpoint_id,
                message.invite_label,
                message.invite_id,
                message.from,
                message.reply_invite,
                body,
                size
            ],
        )?;
        Ok(connection.last_insert_rowid())
    }

    /// Messages with an id above `since`, oldest first, one page at most.
    pub fn list(&self, since: i64) -> Result<Vec<StoredMessage>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT id, received_at, sender_endpoint_id, invite_label, invite_id, from_text, reply_invite, message
             FROM messages WHERE id > ?1 ORDER BY id LIMIT ?2",
        )?;
        let rows = statement.query_map(params![since, PAGE_SIZE], |row| {
            Ok((
                row.get::<_, String>(7)?,
                StoredMessage {
                    id: row.get(0)?,
                    received_at: row.get::<_, i64>(1)? as u64,
                    sender_endpoint_id: row.get(2)?,
                    invite_label: row.get(3)?,
                    invite_id: row.get(4)?,
                    from: row.get(5)?,
                    reply_invite: row.get(6)?,
                    message: Value::Null,
                },
            ))
        })?;
        let mut messages = Vec::new();
        for row in rows {
            let (body, mut message) = row?;
            // Every row was written from valid JSON; one that no longer parses is an error, not a blank message.
            message.message =
                serde_json::from_str(&body).with_context(|| format!("message {} is corrupt", message.id))?;
            messages.push(message);
        }
        Ok(messages)
    }

    /// Delete these messages; returns how many there were.
    pub fn ack(&self, ids: &[i64]) -> Result<usize> {
        let connection = self.connection();
        let mut deleted = 0;
        for id in ids {
            deleted += connection.execute("DELETE FROM messages WHERE id = ?1", [id])?;
        }
        Ok(deleted)
    }

    pub fn stats(&self) -> Result<MailboxStats> {
        let connection = self.connection();
        let (waiting, bytes) = totals(&connection)?;
        let oldest: Option<i64> =
            connection.query_row("SELECT min(received_at) FROM messages", [], |r| r.get(0)).optional()?.flatten();
        Ok(MailboxStats { waiting, bytes, oldest_received_at: oldest.map(|t| t as u64) })
    }
}

/// (messages waiting, their bytes), as counted against the mailbox limits.
fn totals(connection: &Connection) -> Result<(i64, i64)> {
    Ok(connection
        .query_row("SELECT count(*), coalesce(sum(size), 0) FROM messages", [], |r| Ok((r.get(0)?, r.get(1)?)))?)
}

/// What a message costs against [`MAX_TOTAL_BYTES`]: everything the sender chose the size of.
fn stored_size(body: &str, message: &StoredMessage) -> i64 {
    let from = message.from.as_deref().map_or(0, str::len);
    let reply_invite = message.reply_invite.as_deref().map_or(0, str::len);
    (body.len() + from + reply_invite) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(text: &str) -> StoredMessage {
        StoredMessage {
            id: 0,
            received_at: 1,
            sender_endpoint_id: "sender".into(),
            invite_label: "Bob".into(),
            invite_id: "i1".into(),
            from: Some("Codex".into()),
            reply_invite: None,
            message: json!({"parts": [{"kind": "text", "text": text}]}),
        }
    }

    #[test]
    fn listing_resumes_after_since_and_ack_deletes_only_known_ids() {
        let store = Store::in_memory().unwrap();
        let a = store.insert(&message("one")).unwrap();
        let b = store.insert(&message("two")).unwrap();
        let listed = store.list(0).unwrap();
        assert_eq!(listed.iter().map(|m| m.id).collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(listed[1].message["parts"][0]["text"], "two");
        assert_eq!(store.list(a).unwrap().len(), 1);
        assert_eq!(store.ack(&[a, 999]).unwrap(), 1);
        assert_eq!(store.stats().unwrap().waiting, 1);
    }

    #[test]
    fn a_full_mailbox_refuses_new_messages_and_keeps_the_old_ones() {
        let store = Store::in_memory().unwrap();
        for _ in 0..MAX_MESSAGES {
            store.insert(&message("x")).unwrap();
        }
        assert!(store.insert(&message("one too many")).unwrap_err().to_string().contains("full"));
        assert_eq!(store.stats().unwrap().waiting, MAX_MESSAGES);
        assert_eq!(store.list(0).unwrap()[0].message["parts"][0]["text"], "x");
    }

    #[test]
    fn a_panic_while_holding_the_store_does_not_break_it() {
        let store = std::sync::Arc::new(Store::in_memory().unwrap());
        let poisoner = store.clone();
        let panicked = std::thread::spawn(move || {
            let _held = poisoner.connection.lock();
            panic!("a request handler hits a bug while holding the store");
        })
        .join();
        assert!(panicked.is_err() && store.connection.is_poisoned());
        store.insert(&message("still works")).unwrap();
        assert_eq!(store.stats().unwrap().waiting, 1);
    }
}
