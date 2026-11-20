//! The durable draft store (docs/05-threads.md#drafts--durable): unsent composer text, its
//! attachments, and the receipts of operations still in flight. User data, not cache — its own
//! file with its own migrations; "clear cache" and a cache schema change never touch it.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use yaac_protocol::{OpId, TargetId, ThreadId};

use crate::migrate::{self, MigrateError};
use crate::protocol::{Draft, Operation, OperationState};

/// Migrations, in order. Append only; never edit one that has shipped.
const MIGRATIONS: &[&str] = &[
    // 1: drafts and in-flight operations.
    r"
    CREATE TABLE drafts (
        key TEXT PRIMARY KEY,
        target_id TEXT,
        thread_id TEXT,
        text TEXT NOT NULL,
        attachments TEXT NOT NULL DEFAULT '[]',
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE operations (
        op_id TEXT PRIMARY KEY,
        target_id TEXT NOT NULL,
        method TEXT NOT NULL,
        thread_id TEXT,
        state TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    ",
];

/// A draft-store error.
#[derive(Debug, thiserror::Error)]
pub enum DraftsError {
    /// Opening or migrating.
    #[error(transparent)]
    Migrate(#[from] MigrateError),
    /// SQLite.
    #[error("drafts: {0}")]
    Sql(#[from] rusqlite::Error),
    /// JSON.
    #[error("drafts: {0}")]
    Json(#[from] serde_json::Error),
}

/// The draft store.
#[derive(Debug)]
pub struct DraftStore {
    conn: Connection,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl DraftStore {
    /// Opens the store, running any pending migrations.
    pub fn open(path: &Path) -> Result<Self, DraftsError> {
        Ok(Self { conn: migrate::open(path, "drafts", MIGRATIONS)? })
    }

    /// The schema this build migrates to.
    pub fn schema() -> usize {
        MIGRATIONS.len()
    }

    /// Stores a draft (replacing one with the same key). Returns it as stored.
    pub fn put(&mut self, draft: &Draft) -> Result<Draft, DraftsError> {
        let mut draft = draft.clone();
        draft.updated_at_ms = now_ms();
        self.conn.execute(
            "INSERT OR REPLACE INTO drafts (key, target_id, thread_id, text, attachments, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                draft.key,
                draft.target.as_ref().map(TargetId::as_str),
                draft.thread_id.as_ref().map(ThreadId::as_str),
                draft.text,
                serde_json::to_string(&draft.attachments)?,
                draft.updated_at_ms
            ],
        )?;
        Ok(draft)
    }

    /// Deletes a draft (sent, or emptied by the user).
    pub fn delete(&mut self, key: &str) -> Result<(), DraftsError> {
        self.conn.execute("DELETE FROM drafts WHERE key = ?1", params![key])?;
        Ok(())
    }

    /// One draft.
    pub fn get(&self, key: &str) -> Result<Option<Draft>, DraftsError> {
        let row = self
            .conn
            .query_row(
                "SELECT key, target_id, thread_id, text, attachments, updated_at FROM drafts WHERE key = ?1",
                params![key],
                draft_row,
            )
            .optional()?;
        row.map(finish_draft).transpose()
    }

    /// Every draft, most recent first.
    pub fn list(&self) -> Result<Vec<Draft>, DraftsError> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, target_id, thread_id, text, attachments, updated_at FROM drafts ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], draft_row)?;
        rows.map(|r| finish_draft(r?)).collect()
    }

    /// Records a routed state-changing operation as sent. Only identifiers are stored — never
    /// the params, which may carry a secret answer.
    pub fn op_sent(&mut self, op: &OpId, target: &TargetId, method: &str, thread: Option<&ThreadId>) -> Result<(), DraftsError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO operations (op_id, target_id, method, thread_id, state, created_at) VALUES (?1, ?2, ?3, ?4, 'sent', ?5)",
            params![op.as_str(), target.as_str(), method, thread.map(ThreadId::as_str), now_ms()],
        )?;
        Ok(())
    }

    /// The operation's outcome is known (the answer reached the window): forget it.
    pub fn op_done(&mut self, op: &OpId) -> Result<(), DraftsError> {
        self.conn.execute("DELETE FROM operations WHERE op_id = ?1", params![op.as_str()])?;
        Ok(())
    }

    /// The connection dropped before the answer: the outcome is uncertain.
    pub fn op_uncertain(&mut self, op: &OpId) -> Result<(), DraftsError> {
        self.conn.execute("UPDATE operations SET state = 'uncertain' WHERE op_id = ?1", params![op.as_str()])?;
        Ok(())
    }

    /// Every operation still in the store. Operations left `sent` by a hub that stopped are
    /// reported as uncertain.
    pub fn operations(&self) -> Result<Vec<Operation>, DraftsError> {
        let mut stmt =
            self.conn.prepare("SELECT op_id, target_id, method, thread_id, state, created_at FROM operations ORDER BY created_at")?;
        let rows = stmt.query_map([], |r| {
            Ok(Operation {
                op_id: OpId::new(r.get::<_, String>(0)?),
                target: TargetId::new(r.get::<_, String>(1)?),
                method: r.get(2)?,
                thread_id: r.get::<_, Option<String>>(3)?.map(ThreadId::new),
                state: if r.get::<_, String>(4)? == "sent" { OperationState::Sent } else { OperationState::Uncertain },
                created_at_ms: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// At start-up: anything still `sent` was in flight when the previous hub stopped.
    pub fn mark_all_uncertain(&mut self) -> Result<(), DraftsError> {
        self.conn.execute("UPDATE operations SET state = 'uncertain' WHERE state = 'sent'", [])?;
        Ok(())
    }

    /// Flushes the write-ahead log.
    pub fn checkpoint(&mut self) -> Result<(), DraftsError> {
        self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }
}

type DraftRow = (String, Option<String>, Option<String>, String, String, i64);

fn draft_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DraftRow> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
}

fn finish_draft((key, target, thread, text, attachments, updated): DraftRow) -> Result<Draft, DraftsError> {
    Ok(Draft {
        key,
        target: target.map(TargetId::new),
        thread_id: thread.map(ThreadId::new),
        text,
        attachments: serde_json::from_str(&attachments)?,
        updated_at_ms: updated,
    })
}
