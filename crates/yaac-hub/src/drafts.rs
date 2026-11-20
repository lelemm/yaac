//! The durable draft store (docs/05-threads.md#drafts--durable): unsent composer text, its
//! attachments, and the receipts of operations still in flight. User data, not cache — its own
//! file with its own migrations; "clear cache" and a cache schema change never touch it.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use yaac_protocol::{OpId, TargetId, ThreadId};

use crate::migrate::{self, MigrateError};
use crate::protocol::{Draft, Operation, OperationState, ThreadView, ViewSetParams};

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
    // 2: the threads' view preferences (D92): pin, mute (timed or not), dismiss.
    r"
    CREATE TABLE thread_view (
        target_id TEXT NOT NULL,
        thread_id TEXT NOT NULL,
        pinned INTEGER NOT NULL DEFAULT 0,
        muted INTEGER NOT NULL DEFAULT 0,
        muted_until TEXT,
        dismissed INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (target_id, thread_id)
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

    // ── View preferences (D92) ─────────────────────────────────────────────────────────

    /// Every thread's view preferences.
    pub fn views(&self) -> Result<Vec<ThreadView>, DraftsError> {
        let mut stmt = self.conn.prepare("SELECT target_id, thread_id, pinned, muted, muted_until, dismissed FROM thread_view ORDER BY target_id, thread_id")?;
        let rows = stmt.query_map([], view_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn view(&self, target: &TargetId, thread: &ThreadId) -> Result<ThreadView, DraftsError> {
        let found = self
            .conn
            .query_row(
                "SELECT target_id, thread_id, pinned, muted, muted_until, dismissed FROM thread_view WHERE target_id = ?1 AND thread_id = ?2",
                params![target.as_str(), thread.as_str()],
                view_row,
            )
            .optional()?;
        Ok(found.unwrap_or_else(|| ThreadView { target: target.clone(), thread_id: thread.clone(), ..ThreadView::default() }))
    }

    fn write_view(&self, v: &ThreadView) -> Result<(), DraftsError> {
        if v.is_empty() {
            self.conn.execute("DELETE FROM thread_view WHERE target_id = ?1 AND thread_id = ?2", params![v.target.as_str(), v.thread_id.as_str()])?;
        } else {
            self.conn.execute(
                "INSERT OR REPLACE INTO thread_view (target_id, thread_id, pinned, muted, muted_until, dismissed, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![v.target.as_str(), v.thread_id.as_str(), i64::from(v.pinned), i64::from(v.muted), v.muted_until, i64::from(v.dismissed), now_ms()],
            )?;
        }
        Ok(())
    }

    /// Applies patches in one transaction; returns each entry as it is now.
    pub fn set_views(&mut self, patches: &[ViewSetParams]) -> Result<Vec<ThreadView>, DraftsError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut out = Vec::new();
        for p in patches {
            let mut v = self.view(&p.target, &p.thread_id)?;
            if let Some(x) = p.pinned {
                v.pinned = x;
            }
            if let Some(x) = p.dismissed {
                v.dismissed = x;
            }
            if let Some(x) = p.muted {
                v.muted = x;
                if !x {
                    v.muted_until = None;
                }
            }
            if let Some(until) = &p.muted_until {
                v.muted_until = if v.muted { until.clone() } else { None };
            }
            self.write_view(&v)?;
            out.push(v);
        }
        tx.commit()?;
        Ok(out)
    }

    /// A thread is gone from its target: its pin and mute go. A dismissal stays — a thread
    /// trimmed by the target's discovery caps may come back and must stay hidden.
    pub fn view_thread_removed(&mut self, target: &TargetId, thread: &ThreadId) -> Result<Option<ThreadView>, DraftsError> {
        let mut v = self.view(target, thread)?;
        if v.is_empty() || !(v.pinned || v.muted) {
            return Ok(None);
        }
        v.pinned = false;
        v.muted = false;
        v.muted_until = None;
        self.write_view(&v)?;
        Ok(Some(v))
    }

    /// A target was removed: every entry of it goes.
    pub fn view_remove_target(&mut self, target: &TargetId) -> Result<(), DraftsError> {
        self.conn.execute("DELETE FROM thread_view WHERE target_id = ?1", params![target.as_str()])?;
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

fn view_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadView> {
    Ok(ThreadView {
        target: TargetId::new(r.get::<_, String>(0)?),
        thread_id: ThreadId::new(r.get::<_, String>(1)?),
        pinned: r.get::<_, i64>(2)? != 0,
        muted: r.get::<_, i64>(3)? != 0,
        muted_until: r.get(4)?,
        dismissed: r.get::<_, i64>(5)? != 0,
    })
}
