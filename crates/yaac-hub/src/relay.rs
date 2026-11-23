//! The durable mesh relay queue (docs/10-agent-mesh.md#durability-hop-by-hop): its own SQLite
//! file with its own migrations, never part of the cache. Once the hub has acknowledged a
//! `mesh/relay`, this file is the only place the message exists.
//!
//! A message's life here:
//!
//! ```text
//! mesh/relay from the source ─► accept: stored, deduplicated by id, in one transaction ─► answered "queued"
//!     │
//!     ▼ destination connected (now, or when it reconnects)
//! mesh/deliver ─► destination answers ─► handed off (or failed) + receipt pending, in one transaction
//!     │
//!     ▼ source connected (now, or when it reconnects)
//! mesh/receipt ─► source answers ─► receipt sent
//! ```

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use yaac_protocol::{MeshMessage, MessageId, Receipt, ReceiptReason, ReceiptStatus, TargetId};

use crate::migrate::{self, MigrateError};

/// Migrations, in order. Append only.
const MIGRATIONS: &[&str] = &[
    // 1: the queue.
    r"
    CREATE TABLE messages (
        message_id TEXT PRIMARY KEY,
        source_target TEXT NOT NULL,
        dest_target TEXT NOT NULL,
        message TEXT NOT NULL,
        state TEXT NOT NULL,
        receipt TEXT,
        receipt_pending INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX messages_by_dest ON messages (dest_target, state);
    CREATE INDEX messages_by_source ON messages (source_target, receipt_pending);
    ",
    // 2: whether the sender was told the message is overdue.
    r"
    ALTER TABLE messages ADD COLUMN noticed INTEGER NOT NULL DEFAULT 0;
    ",
];

/// The source recorded for messages the hub itself originates (undelivered notices): no
/// target has this id, so no receipt is ever owed for them.
pub fn system_source() -> TargetId {
    TargetId::new("")
}

/// The passive notice a sender gets when its message has waited `waited` undelivered. It is
/// a `notice` (never wakes a thread), about the recipient, addressed to the sender, with an id
/// derived from the message's so a repeat is harmless.
pub fn undelivered_notice(message: &MeshMessage, waited: std::time::Duration) -> MeshMessage {
    let minutes = waited.as_secs() / 60;
    MeshMessage {
        id: MessageId::new(format!("undelivered:{}", message.id)),
        from: message.to.clone(),
        sender_skips_approvals: None,
        to: message.from.clone(),
        kind: yaac_protocol::MeshMessageKind::Notice,
        text: format!(
            "Your message to {} has not been delivered after {minutes} minutes. It is still queued and will be delivered when it can; it has not been cancelled.",
            message.to
        ),
        in_reply_to: Some(message.id.clone()),
        sent_at: yaac_protocol::Timestamp::new(now_rfc3339()),
        seq: 0,
        root: message.root.clone(),
        delivery: None,
    }
}

fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = secs / 86_400;
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// How long a finished message is kept, so a late duplicate of it is still recognised.
pub const RETAIN_FINISHED_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// A relay-queue error.
#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    /// Opening or migrating.
    #[error(transparent)]
    Migrate(#[from] MigrateError),
    /// SQLite.
    #[error("relay queue: {0}")]
    Sql(#[from] rusqlite::Error),
    /// JSON.
    #[error("relay queue: {0}")]
    Json(#[from] serde_json::Error),
}

/// The answer to a `mesh/relay`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted {
    /// The receipt to answer with.
    pub receipt: Receipt,
    /// Whether the message is new (a duplicate changes nothing and is not delivered again).
    pub fresh: bool,
}

/// The relay queue.
#[derive(Debug)]
pub struct RelayQueue {
    conn: Connection,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A receipt.
pub fn receipt(status: ReceiptStatus, reason: Option<ReceiptReason>, detail: Option<String>) -> Receipt {
    Receipt { status, reason, detail, handling: None }
}

impl RelayQueue {
    /// Opens the queue, running any pending migrations.
    pub fn open(path: &Path) -> Result<Self, RelayError> {
        Ok(Self { conn: migrate::open(path, "relay queue", MIGRATIONS)? })
    }

    /// Stores a message from `source`, deduplicated by its id, in one transaction — before the
    /// source is answered. `dest` is `None` when the address names no known target: the
    /// message is then recorded as failed (so a duplicate gets the same answer).
    pub fn accept(&mut self, source: &TargetId, dest: Option<&TargetId>, message: &MeshMessage, dest_online: bool) -> Result<Accepted, RelayError> {
        let tx = self.conn.transaction()?;
        let existing: Option<(String, Option<String>)> = tx
            .query_row("SELECT state, receipt FROM messages WHERE message_id = ?1", params![message.id.as_str()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        if let Some((state, stored)) = existing {
            tx.commit()?;
            let receipt = match (state.as_str(), stored) {
                (_, Some(stored)) => serde_json::from_str(&stored)?,
                _ => queued(dest_online),
            };
            return Ok(Accepted { receipt, fresh: false });
        }
        let now = now_ms();
        let (state, receipt) = match dest {
            Some(_) => ("queued", queued(dest_online)),
            None => (
                "failed",
                receipt(ReceiptStatus::Failed, Some(ReceiptReason::UnknownAddress), Some("no such target".into())),
            ),
        };
        let stored = (state == "failed").then(|| serde_json::to_string(&receipt)).transpose()?;
        tx.execute(
            "INSERT INTO messages (message_id, source_target, dest_target, message, state, receipt, receipt_pending, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?7)",
            params![
                message.id.as_str(),
                source.as_str(),
                dest.map_or("", TargetId::as_str),
                serde_json::to_string(message)?,
                state,
                stored,
                now
            ],
        )?;
        tx.commit()?;
        Ok(Accepted { receipt, fresh: state == "queued" })
    }

    /// Messages waiting for `dest`, oldest first.
    pub fn queued_for(&self, dest: &TargetId) -> Result<Vec<MeshMessage>, RelayError> {
        let mut stmt =
            self.conn.prepare("SELECT message FROM messages WHERE dest_target = ?1 AND state = 'queued' ORDER BY rowid")?;
        let rows = stmt.query_map(params![dest.as_str()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// The destination answered `mesh/deliver`: the hop is done (or failed for good). The
    /// receipt is now owed to the source. Returns the source, or `None` if the message was
    /// not queued (already handled).
    pub fn finish(&mut self, message: &MessageId, receipt: &Receipt) -> Result<Option<TargetId>, RelayError> {
        let tx = self.conn.transaction()?;
        let source: Option<String> = tx
            .query_row(
                "SELECT source_target FROM messages WHERE message_id = ?1 AND state = 'queued'",
                params![message.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(source) = source else {
            tx.commit()?;
            return Ok(None);
        };
        let state = if receipt.status == ReceiptStatus::Failed { "failed" } else { "handedOff" };
        // The hub's own notices owe nobody a receipt.
        let owed = i64::from(!source.is_empty());
        tx.execute(
            "UPDATE messages SET state = ?2, receipt = ?3, receipt_pending = ?5, updated_at = ?4 WHERE message_id = ?1",
            params![message.as_str(), state, serde_json::to_string(receipt)?, now_ms(), owed],
        )?;
        tx.commit()?;
        Ok((!source.is_empty()).then(|| TargetId::new(source)))
    }

    /// Messages from agents queued longer than `limit` whose sender has not been told: each
    /// with its source target.
    pub fn overdue(&self, limit: std::time::Duration) -> Result<Vec<(TargetId, MeshMessage)>, RelayError> {
        let cutoff = now_ms() - i64::try_from(limit.as_millis()).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT source_target, message FROM messages WHERE state = 'queued' AND noticed = 0 AND source_target != '' AND created_at <= ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![cutoff], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (source, message) = row?;
            out.push((TargetId::new(source), serde_json::from_str(&message)?));
        }
        Ok(out)
    }

    /// Commit the recipient's final outcome. Only the original destination can report it.
    /// A replay with the same receipt is harmless even after the source acknowledged it.
    pub fn record_outcome(&mut self, dest: &TargetId, message: &MessageId, receipt: &Receipt) -> Result<Option<TargetId>, RelayError> {
        let tx = self.conn.transaction()?;
        let row: Option<(String, Option<String>)> = tx.query_row(
            "SELECT source_target, receipt FROM messages WHERE message_id = ?1 AND dest_target = ?2 AND state != 'failed'",
            params![message.as_str(), dest.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?;
        let Some((source, previous)) = row else { return Ok(None) };
        if source.is_empty() { return Ok(None) }
        let encoded = serde_json::to_string(receipt)?;
        if previous.as_deref() == Some(&encoded) { return Ok(None) }
        let state = if receipt.status == ReceiptStatus::Failed { "failed" } else { "handedOff" };
        tx.execute(
            "UPDATE messages SET state = ?2, receipt = ?3, receipt_pending = 1, updated_at = ?4 WHERE message_id = ?1",
            params![message.as_str(), state, encoded, now_ms()],
        )?;
        tx.commit()?;
        Ok(Some(TargetId::new(source)))
    }

    /// The sender of `message` has been told it is overdue.
    pub fn mark_noticed(&mut self, message: &MessageId) -> Result<(), RelayError> {
        self.conn.execute("UPDATE messages SET noticed = 1 WHERE message_id = ?1", params![message.as_str()])?;
        Ok(())
    }

    /// Receipts owed to `source`.
    pub fn pending_receipts(&self, source: &TargetId) -> Result<Vec<(MessageId, Receipt)>, RelayError> {
        let mut stmt = self.conn.prepare(
            "SELECT message_id, receipt FROM messages WHERE source_target = ?1 AND receipt_pending = 1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![source.as_str()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, receipt) = row?;
            out.push((MessageId::new(id), serde_json::from_str(&receipt)?));
        }
        Ok(out)
    }

    /// The source acknowledged the receipt.
    pub fn receipt_sent(&mut self, message: &MessageId, receipt: &Receipt) -> Result<(), RelayError> {
        self.conn.execute(
            "UPDATE messages SET receipt_pending = 0, updated_at = ?2 WHERE message_id = ?1 AND receipt = ?3",
            params![message.as_str(), now_ms(), serde_json::to_string(receipt)?],
        )?;
        Ok(())
    }

    /// The destination target was removed: everything queued for it fails, and the receipts
    /// are owed to the sources.
    pub fn fail_for_removed(&mut self, dest: &TargetId) -> Result<Vec<(TargetId, MessageId, Receipt)>, RelayError> {
        let receipt = receipt(ReceiptStatus::Failed, Some(ReceiptReason::UnknownAddress), Some("the target was removed".into()));
        let json = serde_json::to_string(&receipt)?;
        let tx = self.conn.transaction()?;
        let rows: Vec<(String, String)> = {
            let mut stmt = tx.prepare("SELECT source_target, message_id FROM messages WHERE dest_target = ?1 AND state = 'queued'")?;
            stmt.query_map(params![dest.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
        };
        tx.execute(
            "UPDATE messages SET state = 'failed', receipt = ?2, receipt_pending = 1, updated_at = ?3 WHERE dest_target = ?1 AND state = 'queued'",
            params![dest.as_str(), json, now_ms()],
        )?;
        tx.commit()?;
        Ok(rows.into_iter().map(|(s, m)| (TargetId::new(s), MessageId::new(m), receipt.clone())).collect())
    }

    /// Messages queued plus receipts owed: what keeps the hub up.
    pub fn in_flight(&self) -> Result<u64, RelayError> {
        let n: i64 =
            self.conn.query_row("SELECT count(*) FROM messages WHERE state = 'queued' OR receipt_pending = 1", [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// The state of one message (tests, diagnostics): `queued`, `handedOff`, or `failed`, and
    /// whether its receipt is still owed.
    pub fn state(&self, message: &MessageId) -> Result<Option<(String, bool)>, RelayError> {
        Ok(self
            .conn
            .query_row("SELECT state, receipt_pending FROM messages WHERE message_id = ?1", params![message.as_str()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))
            })
            .optional()?)
    }

    /// Forgets finished messages older than the retention period.
    pub fn purge_finished(&mut self) -> Result<(), RelayError> {
        self.conn.execute(
            "DELETE FROM messages WHERE state != 'queued' AND receipt_pending = 0 AND updated_at < ?1",
            params![now_ms() - RETAIN_FINISHED_MS],
        )?;
        Ok(())
    }

    /// Flushes the write-ahead log into the main file ("checkpoint the relay queue").
    pub fn checkpoint(&mut self) -> Result<(), RelayError> {
        self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }
}

fn queued(dest_online: bool) -> Receipt {
    if dest_online {
        receipt(ReceiptStatus::Queued, None, None)
    } else {
        receipt(ReceiptStatus::Queued, Some(ReceiptReason::TargetOffline), None)
    }
}
