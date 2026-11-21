//! The durable draft store (docs/05-threads.md#drafts--durable): unsent composer text, its
//! attachments, and the receipts of operations still in flight. User data, not cache — its own
//! file with its own migrations; "clear cache" and a cache schema change never touch it.
//!
//! A draft's clipboard images exist nowhere but in the window's memory, so their original
//! bytes are kept here too (docs/06-attachments.md#an-upload-belongs-to-one-target): as
//! *blobs*, one file each in `draft-blobs/` beside the database, described by a row. They go
//! when their draft is deleted (the message was sent, or the user emptied it).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest as _, Sha256};
use yaac_protocol::{OpId, Sha256Hex, TargetId, ThreadId};

use crate::migrate::{self, MigrateError};
use crate::protocol::{Draft, DraftBlob, Operation, OperationState, ThreadView, ViewSetParams};

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
    // 3: the original bytes of drafts' clipboard attachments (the bytes are files).
    r"
    CREATE TABLE draft_blobs (
        blob_id TEXT PRIMARY KEY,
        draft_key TEXT NOT NULL,
        name TEXT NOT NULL,
        media_type TEXT,
        size INTEGER NOT NULL,
        sha256 TEXT,
        complete INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX draft_blobs_by_draft ON draft_blobs (draft_key);
    ",
    // 4: a turn ended while no window showed the thread (D114).
    r"
    ALTER TABLE thread_view ADD COLUMN unseen INTEGER NOT NULL DEFAULT 0;
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
    /// A blob file.
    #[error("draft blob: {0}")]
    Io(#[from] std::io::Error),
    /// A blob request the store refuses; the message is shown as-is.
    #[error("{message}")]
    Blob {
        /// An attach error code (`limit_exceeded`, `blob_not_found`, `invalid_params`).
        code: &'static str,
        /// The message.
        message: String,
    },
}

fn refuse(code: &'static str, message: impl Into<String>) -> DraftsError {
    DraftsError::Blob { code, message: message.into() }
}

/// A blob still being written: its file, a running digest, and how much has arrived.
struct OpenBlob {
    file: std::fs::File,
    hash: Sha256,
    written: u64,
    size: u64,
}

impl std::fmt::Debug for OpenBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenBlob").field("written", &self.written).field("size", &self.size).finish_non_exhaustive()
    }
}

/// The draft store.
#[derive(Debug)]
pub struct DraftStore {
    conn: Connection,
    blob_dir: PathBuf,
    open_blobs: HashMap<String, OpenBlob>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl DraftStore {
    /// Opens the store, running any pending migrations. Blobs a previous run did not finish
    /// are removed.
    pub fn open(path: &Path) -> Result<Self, DraftsError> {
        let conn = migrate::open(path, "drafts", MIGRATIONS)?;
        let blob_dir = path.parent().unwrap_or_else(|| Path::new(".")).join("draft-blobs");
        let mut store = Self { conn, blob_dir, open_blobs: HashMap::new() };
        store.sweep_blobs()?;
        Ok(store)
    }

    /// Where blob files live.
    pub fn blob_dir(&self) -> &Path {
        &self.blob_dir
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

    /// Deletes a draft (sent, or emptied by the user), and its blobs.
    pub fn delete(&mut self, key: &str) -> Result<(), DraftsError> {
        self.conn.execute("DELETE FROM drafts WHERE key = ?1", params![key])?;
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT blob_id FROM draft_blobs WHERE draft_key = ?1")?;
            let rows = stmt.query_map(params![key], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        for id in ids {
            self.blob_delete(&id)?;
        }
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
        let mut stmt = self.conn.prepare("SELECT target_id, thread_id, pinned, muted, muted_until, dismissed, unseen FROM thread_view ORDER BY target_id, thread_id")?;
        let rows = stmt.query_map([], view_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn view(&self, target: &TargetId, thread: &ThreadId) -> Result<ThreadView, DraftsError> {
        let found = self
            .conn
            .query_row(
                "SELECT target_id, thread_id, pinned, muted, muted_until, dismissed, unseen FROM thread_view WHERE target_id = ?1 AND thread_id = ?2",
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
                "INSERT OR REPLACE INTO thread_view (target_id, thread_id, pinned, muted, muted_until, dismissed, unseen, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![v.target.as_str(), v.thread_id.as_str(), i64::from(v.pinned), i64::from(v.muted), v.muted_until, i64::from(v.dismissed), i64::from(v.unseen), now_ms()],
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
            if let Some(x) = p.unseen {
                v.unseen = x;
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

    /// A thread is gone from its target: its pin, mute, and unseen mark go. A dismissal stays
    /// — a thread trimmed by the target's discovery caps may come back and must stay hidden.
    pub fn view_thread_removed(&mut self, target: &TargetId, thread: &ThreadId) -> Result<Option<ThreadView>, DraftsError> {
        let mut v = self.view(target, thread)?;
        if v.is_empty() || !(v.pinned || v.muted || v.unseen) {
            return Ok(None);
        }
        v.pinned = false;
        v.muted = false;
        v.unseen = false;
        v.muted_until = None;
        self.write_view(&v)?;
        Ok(Some(v))
    }

    /// A target was removed: every entry of it goes.
    pub fn view_remove_target(&mut self, target: &TargetId) -> Result<(), DraftsError> {
        self.conn.execute("DELETE FROM thread_view WHERE target_id = ?1", params![target.as_str()])?;
        Ok(())
    }

    // ── Blobs: a draft's clipboard bytes ────────────────────────────────────────────────

    fn blob_file(&self, id: &str, partial: bool) -> PathBuf {
        self.blob_dir.join(if partial { format!("{id}.partial") } else { id.to_owned() })
    }

    /// Removes blobs that were never finished, and files no row describes.
    fn sweep_blobs(&mut self) -> Result<(), DraftsError> {
        self.conn.execute("DELETE FROM draft_blobs WHERE complete = 0", [])?;
        let Ok(entries) = std::fs::read_dir(&self.blob_dir) else { return Ok(()) };
        let known: std::collections::HashSet<String> = {
            let mut stmt = self.conn.prepare("SELECT blob_id FROM draft_blobs")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !known.contains(&name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }

    /// Starts a blob of `size` bytes (at most `max` bytes) for a draft.
    pub fn blob_begin(&mut self, draft: &str, name: &str, media_type: Option<&str>, size: u64, max: u64) -> Result<String, DraftsError> {
        if size > max {
            return Err(refuse("limit_exceeded", format!("a pasted attachment is limited to {} (this one is {})", human(max), human(size))));
        }
        if name.trim().is_empty() {
            return Err(refuse("invalid_params", "a blob needs a name"));
        }
        crate::paths::ensure_private_dir(&self.blob_dir)?;
        let id = crate::settings::new_id("b");
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(self.blob_file(&id, true))?;
        self.conn.execute(
            "INSERT INTO draft_blobs (blob_id, draft_key, name, media_type, size, complete, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
            params![id, draft, name, media_type, i64::try_from(size).unwrap_or(i64::MAX), now_ms()],
        )?;
        self.open_blobs.insert(id.clone(), OpenBlob { file, hash: Sha256::new(), written: 0, size });
        Ok(id)
    }

    /// Appends the next chunk. Chunks must arrive in order and not pass the declared size.
    pub fn blob_append(&mut self, id: &str, offset: u64, bytes: &[u8]) -> Result<u64, DraftsError> {
        let blob = self.open_blobs.get_mut(id).ok_or_else(|| refuse("blob_not_found", format!("no blob {id} is being written")))?;
        if offset != blob.written {
            return Err(refuse("invalid_params", format!("expected the chunk at offset {}, got {offset}", blob.written)));
        }
        if blob.written + bytes.len() as u64 > blob.size {
            return Err(refuse("invalid_params", format!("more bytes than the declared {}", blob.size)));
        }
        blob.file.write_all(bytes)?;
        blob.hash.update(bytes);
        blob.written += bytes.len() as u64;
        Ok(blob.written)
    }

    /// Finishes a blob: every declared byte has arrived. It is durable from here on.
    pub fn blob_end(&mut self, id: &str) -> Result<DraftBlob, DraftsError> {
        let blob = self.open_blobs.remove(id).ok_or_else(|| refuse("blob_not_found", format!("no blob {id} is being written")))?;
        if blob.written != blob.size {
            let _ = self.blob_delete(id);
            return Err(refuse("invalid_params", format!("received {} bytes of the declared {}", blob.written, blob.size)));
        }
        blob.file.sync_all()?;
        drop(blob.file);
        let sha = hex(&blob.hash.finalize());
        std::fs::rename(self.blob_file(id, true), self.blob_file(id, false))?;
        self.conn.execute("UPDATE draft_blobs SET complete = 1, sha256 = ?2 WHERE blob_id = ?1", params![id, sha])?;
        self.blob(id)?.map(|(b, _)| b).ok_or_else(|| refuse("blob_not_found", format!("no blob {id}")))
    }

    /// A complete blob and its file.
    pub fn blob(&self, id: &str) -> Result<Option<(DraftBlob, PathBuf)>, DraftsError> {
        let found = self
            .conn
            .query_row(
                "SELECT blob_id, draft_key, name, media_type, size, sha256 FROM draft_blobs WHERE blob_id = ?1 AND complete = 1",
                params![id],
                blob_row,
            )
            .optional()?;
        Ok(found.map(|b| {
            let file = self.blob_file(&b.blob_id, false);
            (b, file)
        }))
    }

    /// Complete blobs, of one draft or all, oldest first.
    pub fn blobs(&self, draft: Option<&str>) -> Result<Vec<DraftBlob>, DraftsError> {
        let mut stmt = self.conn.prepare(
            "SELECT blob_id, draft_key, name, media_type, size, sha256 FROM draft_blobs
             WHERE complete = 1 AND (?1 IS NULL OR draft_key = ?1) ORDER BY created_at, blob_id",
        )?;
        let rows = stmt.query_map(params![draft], blob_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Forgets a blob (finished or not) and removes its file.
    pub fn blob_delete(&mut self, id: &str) -> Result<(), DraftsError> {
        self.open_blobs.remove(id);
        self.conn.execute("DELETE FROM draft_blobs WHERE blob_id = ?1", params![id])?;
        for partial in [true, false] {
            match std::fs::remove_file(self.blob_file(id, partial)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
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

fn blob_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DraftBlob> {
    Ok(DraftBlob {
        blob_id: r.get(0)?,
        draft: r.get(1)?,
        name: r.get(2)?,
        media_type: r.get(3)?,
        size: u64::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
        sha256: Sha256Hex::new(r.get::<_, Option<String>>(5)?.unwrap_or_default()),
    })
}

/// Lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// A byte count for a message ("64 MiB").
pub(crate) fn human(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= MIB {
        let tenths = bytes * 10 / MIB;
        if tenths.is_multiple_of(10) { format!("{} MiB", tenths / 10) } else { format!("{}.{} MiB", tenths / 10, tenths % 10) }
    } else if bytes >= 1024 {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{bytes} bytes")
    }
}

fn view_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadView> {
    Ok(ThreadView {
        target: TargetId::new(r.get::<_, String>(0)?),
        thread_id: ThreadId::new(r.get::<_, String>(1)?),
        pinned: r.get::<_, i64>(2)? != 0,
        muted: r.get::<_, i64>(3)? != 0,
        muted_until: r.get(4)?,
        dismissed: r.get::<_, i64>(5)? != 0,
        unseen: r.get::<_, i64>(6)? != 0,
    })
}
