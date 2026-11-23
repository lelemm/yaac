//! The host cache: one SQLite file, disposable (docs/05-threads.md#cache--disposable).
//!
//! The hub is its only writer; the window opens it read-only ([`Cache::open_read_only`]).
//!
//! | Table | Contents |
//! | --- | --- |
//! | `targets` | Last known target facts and connection state |
//! | `providers` | One descriptor per instance, with its `revision` |
//! | `threads` | One `ThreadSummary` per thread; `sync_tokens` holds the per-target token |
//! | `thread_positions` | For each cached transcript: the journal position it is valid at, and the thread's live state |
//! | `items` | Cached transcripts, in order |
//! | `projects` | Each target's user-created projects (one JSON list per target) |
//! | `discovery` | Each target's discovery caps (`discovery/caps`) |
//! | `ui_state` | The window's grouping toggles, collapsed groups, last selection |
//!
//! Rules kept here:
//!
//! - **A thread's items and its position commit in one transaction**
//!   ([`Cache::commit_events`], [`Cache::replace_thread`]); a reader sees both or neither.
//! - **A schema-version mismatch drops and rebuilds** the file — there is no migration path.
//! - **Removing a target deletes its rows** ([`Cache::remove_target`]).
//! - **Secret answers are never written** ([`apply::redact_secrets`] runs on every item).
//! - Transcripts are bounded: [`Cache::evict`] drops least-recently-used threads over a cap.

pub mod apply;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde_json::Value;
use yaac_protocol::attach::ConnectionState;
use yaac_protocol::{
    DiscoveryCaps, Epoch, Item, ItemBody, JournalPosition, Project, ProviderDescriptor, Seq, SyncToken, TargetFacts, TargetId, ThreadEvent,
    ThreadEventParams, ThreadId, ThreadState, ThreadSummary,
};

pub use apply::LiveState;

/// The cache schema. Any change to the tables bumps it; a file with another number is
/// dropped and rebuilt from the targets.
pub const CACHE_SCHEMA: i64 = 3;

const SCHEMA: &str = r"
CREATE TABLE targets (
    target_id TEXT PRIMARY KEY,
    facts TEXT,
    state TEXT,
    updated_at INTEGER NOT NULL
);
CREATE TABLE providers (
    target_id TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    descriptor TEXT NOT NULL,
    PRIMARY KEY (target_id, instance_id)
);
CREATE TABLE threads (
    target_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    summary TEXT NOT NULL,
    PRIMARY KEY (target_id, thread_id)
);
CREATE TABLE sync_tokens (
    target_id TEXT PRIMARY KEY,
    token TEXT NOT NULL
);
CREATE TABLE thread_positions (
    target_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    live TEXT NOT NULL,
    bytes INTEGER NOT NULL DEFAULT 0,
    last_used INTEGER NOT NULL,
    PRIMARY KEY (target_id, thread_id)
);
CREATE TABLE items (
    target_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    ord INTEGER NOT NULL,
    turn_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    item TEXT NOT NULL,
    PRIMARY KEY (target_id, thread_id, item_id)
);
CREATE INDEX items_by_ord ON items (target_id, thread_id, ord);
CREATE TABLE projects (
    target_id TEXT PRIMARY KEY,
    projects TEXT NOT NULL
);
CREATE TABLE discovery (
    target_id TEXT PRIMARY KEY,
    caps TEXT NOT NULL
);
CREATE TABLE ui_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// A cache error.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// SQLite.
    #[error("cache: {0}")]
    Sql(#[from] rusqlite::Error),
    /// A stored value did not decode (the file is rebuilt on the next schema change; the row is
    /// skipped meanwhile).
    #[error("cache: {0}")]
    Json(#[from] serde_json::Error),
    /// Removing a stale file failed.
    #[error("cache: {0}")]
    Io(#[from] std::io::Error),
    /// A fault injected by a test.
    #[error("cache: injected fault")]
    Injected,
}

/// A cached transcript: items in order, the position they are valid at, and live state.
#[derive(Clone, Debug, PartialEq)]
pub struct CachedThread {
    /// Items, oldest first.
    pub items: Vec<Item>,
    /// The journal position the items are valid at.
    pub position: JournalPosition,
    /// Control, running turn, effective capabilities, usage.
    pub live: LiveState,
}

/// A target's last known facts and connection state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedTarget {
    /// From the last `initialize`.
    pub facts: Option<TargetFacts>,
    /// The last state the hub recorded.
    pub state: Option<ConnectionState>,
}

/// Where a test wants a commit to fail (crash-consistency tests).
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Fail after the items are written but before the position is.
    BeforePosition,
}

/// The cache. See the module docs.
#[derive(Debug)]
pub struct Cache {
    conn: Connection,
    path: PathBuf,
    fault: Option<Fault>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn to_u64(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

fn remove_db_files(path: &Path) -> std::io::Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        match std::fs::remove_file(PathBuf::from(file)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

impl Cache {
    /// Opens (or creates) the cache for writing. A file with another schema version, or one
    /// that cannot be read at all, is deleted and rebuilt empty.
    pub fn open(path: &Path) -> Result<Self, CacheError> {
        match Self::try_open(path) {
            Ok(Some(cache)) => Ok(cache),
            Ok(None) | Err(_) => {
                remove_db_files(path)?;
                Self::try_open(path)?.ok_or(CacheError::Sql(rusqlite::Error::InvalidQuery))
            }
        }
    }

    /// `Ok(None)`: the file has another schema and must be rebuilt.
    fn try_open(path: &Path) -> Result<Option<Self>, CacheError> {
        let conn = Connection::open(path)?;
        crate::paths::make_private_file(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table'", [], |r| r.get(0))?;
        if version == 0 && tables == 0 {
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", CACHE_SCHEMA)?;
        } else if version != CACHE_SCHEMA {
            return Ok(None);
        }
        Ok(Some(Self { conn, path: path.to_owned(), fault: None }))
    }

    /// Opens the cache read-only (the window). Fails if the file is missing or has another
    /// schema — show nothing cached rather than misread it.
    pub fn open_read_only(path: &Path) -> Result<Self, CacheError> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(std::time::Duration::from_secs(1))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != CACHE_SCHEMA {
            return Err(CacheError::Sql(rusqlite::Error::InvalidQuery));
        }
        Ok(Self { conn, path: path.to_owned(), fault: None })
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[doc(hidden)]
    pub fn inject_fault(&mut self, fault: Option<Fault>) {
        self.fault = fault;
    }

    // ── Writes ──────────────────────────────────────────────────────────────────────────

    /// Applies a batch of one thread's journal events and moves its position past the last
    /// one applied, in one transaction. Events whose position is not after the stored one are
    /// skipped, so a repeated batch changes nothing. `position` is used only when the thread
    /// has no stored position and no event applies.
    pub fn commit_events(
        &mut self,
        target: &TargetId,
        thread: &ThreadId,
        events: &[ThreadEventParams],
        position: JournalPosition,
    ) -> Result<(), CacheError> {
        let fault = self.fault;
        let tx = self.conn.transaction()?;
        let stored = read_position(&tx, target, thread)?;
        let (mut live, mut bytes) = match &stored {
            Some((_, live, bytes)) => (live.clone(), *bytes),
            None => (LiveState::default(), 0),
        };
        let mut at = stored.as_ref().map(|(p, _, _)| *p);
        for event in events {
            let after = event.position_after();
            let fresh = match at {
                Some(p) => p.epoch == event.epoch && after.seq > p.seq,
                None => true,
            };
            if !fresh {
                continue;
            }
            apply_one(&tx, target, thread, &event.event, &mut live, &mut bytes)?;
            at = Some(after);
        }
        if fault == Some(Fault::BeforePosition) {
            return Err(CacheError::Injected); // dropping `tx` rolls everything back
        }
        // The furthest position reached: the last fresh event, else what was stored.
        let position = at.unwrap_or(position);
        write_position(&tx, target, thread, position, &live, bytes)?;
        tx.commit()?;
        Ok(())
    }

    /// Replaces a thread's cached transcript with a snapshot valid at `position`, in one
    /// transaction (the rebuild after a `reset`).
    pub fn replace_thread(
        &mut self,
        target: &TargetId,
        thread: &ThreadId,
        items_oldest_first: &[Item],
        position: JournalPosition,
        live: &LiveState,
    ) -> Result<(), CacheError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM items WHERE target_id = ?1 AND thread_id = ?2", params![target.as_str(), thread.as_str()])?;
        let mut bytes = 0i64;
        for (ord, item) in items_oldest_first.iter().enumerate() {
            let mut item = item.clone();
            apply::redact_secrets(&mut item);
            let json = serde_json::to_string(&item)?;
            bytes += to_i64(json.len() as u64);
            tx.execute(
                "INSERT OR REPLACE INTO items (target_id, thread_id, item_id, ord, turn_id, kind, item) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![target.as_str(), thread.as_str(), item.id.as_str(), to_i64(ord as u64 + 1), item.turn_id.as_str(), item.kind(), json],
            )?;
        }
        write_position(&tx, target, thread, position, live, bytes)?;
        tx.commit()?;
        Ok(())
    }

    /// Updates a thread's live state without moving its position (a new subscription result).
    pub fn set_live(&mut self, target: &TargetId, thread: &ThreadId, live: &LiveState) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE thread_positions SET live = ?3 WHERE target_id = ?1 AND thread_id = ?2",
            params![target.as_str(), thread.as_str(), serde_json::to_string(live)?],
        )?;
        Ok(())
    }

    /// Stores the target's facts and connection state.
    pub fn put_target(
        &mut self,
        target: &TargetId,
        facts: Option<&TargetFacts>,
        state: &ConnectionState,
    ) -> Result<(), CacheError> {
        let facts = facts.map(serde_json::to_string).transpose()?;
        self.conn.execute(
            "INSERT INTO targets (target_id, facts, state, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (target_id) DO UPDATE SET facts = COALESCE(excluded.facts, targets.facts), state = excluded.state, updated_at = excluded.updated_at",
            params![target.as_str(), facts, serde_json::to_string(state)?, now_ms()],
        )?;
        Ok(())
    }

    /// Replaces the target's provider descriptors (`provider/list` on connect).
    pub fn replace_providers(&mut self, target: &TargetId, providers: &[ProviderDescriptor]) -> Result<(), CacheError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM providers WHERE target_id = ?1", params![target.as_str()])?;
        for p in providers {
            upsert_provider(&tx, target, p)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Stores one descriptor (`provider/updated`), unless a newer revision is already stored.
    pub fn put_provider(&mut self, target: &TargetId, provider: &ProviderDescriptor) -> Result<(), CacheError> {
        let tx = self.conn.transaction()?;
        upsert_provider(&tx, target, provider)?;
        tx.commit()?;
        Ok(())
    }

    /// Applies one `thread/sync` page (or a `thread/updated` / `thread/removed`) and stores the
    /// token, in one transaction. `full` replaces the target's whole index.
    pub fn apply_index(
        &mut self,
        target: &TargetId,
        changed: &[ThreadSummary],
        removed: &[ThreadId],
        token: &SyncToken,
        full_first_page: bool,
    ) -> Result<(), CacheError> {
        let tx = self.conn.transaction()?;
        if full_first_page {
            tx.execute("DELETE FROM threads WHERE target_id = ?1", params![target.as_str()])?;
        }
        for summary in changed {
            tx.execute(
                "INSERT OR REPLACE INTO threads (target_id, thread_id, state, updated_at, summary) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    target.as_str(),
                    summary.id.as_str(),
                    summary.state.as_str(),
                    summary.updated_at.as_str(),
                    serde_json::to_string(summary)?
                ],
            )?;
        }
        for id in removed {
            tx.execute("DELETE FROM threads WHERE target_id = ?1 AND thread_id = ?2", params![target.as_str(), id.as_str()])?;
            tx.execute("DELETE FROM items WHERE target_id = ?1 AND thread_id = ?2", params![target.as_str(), id.as_str()])?;
            tx.execute(
                "DELETE FROM thread_positions WHERE target_id = ?1 AND thread_id = ?2",
                params![target.as_str(), id.as_str()],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO sync_tokens (target_id, token) VALUES (?1, ?2)",
            params![target.as_str(), token.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Replaces a target's projects (`project/list` on connect, `project/updated`).
    pub fn replace_projects(&mut self, target: &TargetId, projects: &[Project]) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO projects (target_id, projects) VALUES (?1, ?2)",
            params![target.as_str(), serde_json::to_string(projects)?],
        )?;
        Ok(())
    }

    /// Stores a target's discovery caps (`discovery/caps` on connect, `discovery/updated`).
    pub fn put_discovery(&mut self, target: &TargetId, caps: &DiscoveryCaps) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO discovery (target_id, caps) VALUES (?1, ?2)",
            params![target.as_str(), serde_json::to_string(caps)?],
        )?;
        Ok(())
    }

    /// A target's discovery caps, as last read from it.
    pub fn discovery(&self, target: &TargetId) -> Result<Option<DiscoveryCaps>, CacheError> {
        let row = self
            .conn
            .query_row("SELECT caps FROM discovery WHERE target_id = ?1", params![target.as_str()], |r| r.get::<_, String>(0))
            .optional()?;
        Ok(row.map(|s| serde_json::from_str(&s)).transpose()?)
    }

    /// Deletes every row of a target.
    pub fn remove_target(&mut self, target: &TargetId) -> Result<(), CacheError> {
        let tx = self.conn.transaction()?;
        for table in ["targets", "providers", "threads", "sync_tokens", "thread_positions", "items", "projects", "discovery"] {
            tx.execute(&format!("DELETE FROM {table} WHERE target_id = ?1"), params![target.as_str()])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// "Clear cache": every row of every table (drafts live elsewhere and are not touched).
    pub fn clear(&mut self) -> Result<(), CacheError> {
        let tx = self.conn.transaction()?;
        for table in ["targets", "providers", "threads", "sync_tokens", "thread_positions", "items", "projects", "discovery", "ui_state"] {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Sets (or, with `null`, deletes) a `ui_state` value.
    pub fn set_ui_state(&mut self, key: &str, value: &Value) -> Result<(), CacheError> {
        if value.is_null() {
            self.conn.execute("DELETE FROM ui_state WHERE key = ?1", params![key])?;
        } else {
            self.conn.execute(
                "INSERT OR REPLACE INTO ui_state (key, value) VALUES (?1, ?2)",
                params![key, serde_json::to_string(value)?],
            )?;
        }
        Ok(())
    }

    /// Marks a thread as just used (for least-recently-used eviction).
    pub fn touch(&mut self, target: &TargetId, thread: &ThreadId) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE thread_positions SET last_used = ?3 WHERE target_id = ?1 AND thread_id = ?2",
            params![target.as_str(), thread.as_str(), now_ms()],
        )?;
        Ok(())
    }

    /// Evicts least-recently-used transcripts until the cached items total at most
    /// `cap_bytes`, never touching `protected` threads. Returns what was evicted.
    pub fn evict(&mut self, cap_bytes: u64, protected: &[(TargetId, ThreadId)]) -> Result<Vec<(TargetId, ThreadId)>, CacheError> {
        let total: i64 = self.conn.query_row("SELECT COALESCE(SUM(bytes), 0) FROM thread_positions", [], |r| r.get(0))?;
        let mut over = to_u64(total).saturating_sub(cap_bytes);
        if over == 0 {
            return Ok(Vec::new());
        }
        let candidates: Vec<(String, String, i64)> = {
            let mut stmt = self.conn.prepare("SELECT target_id, thread_id, bytes FROM thread_positions ORDER BY last_used ASC")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?
        };
        let tx = self.conn.transaction()?;
        let mut evicted = Vec::new();
        for (target, thread, bytes) in candidates {
            if over == 0 {
                break;
            }
            let key = (TargetId::new(&target), ThreadId::new(&thread));
            if protected.contains(&key) {
                continue;
            }
            tx.execute("DELETE FROM items WHERE target_id = ?1 AND thread_id = ?2", params![target, thread])?;
            tx.execute("DELETE FROM thread_positions WHERE target_id = ?1 AND thread_id = ?2", params![target, thread])?;
            over = over.saturating_sub(to_u64(bytes));
            evicted.push(key);
        }
        tx.commit()?;
        Ok(evicted)
    }

    /// Flushes the write-ahead log into the main file.
    pub fn checkpoint(&mut self) -> Result<(), CacheError> {
        self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    // ── Reads (the window uses these on a read-only handle) ─────────────────────────────

    /// A cached transcript, items and position read in one transaction.
    pub fn thread(&mut self, target: &TargetId, thread: &ThreadId) -> Result<Option<CachedThread>, CacheError> {
        let tx = self.conn.transaction()?;
        let Some((position, live, _)) = read_position(&tx, target, thread)? else {
            return Ok(None);
        };
        let items = {
            let mut stmt = tx.prepare("SELECT item FROM items WHERE target_id = ?1 AND thread_id = ?2 ORDER BY ord ASC")?;
            let rows = stmt.query_map(params![target.as_str(), thread.as_str()], |r| r.get::<_, String>(0))?;
            let mut items = Vec::new();
            for row in rows {
                items.push(serde_json::from_str::<Item>(&row?)?);
            }
            items
        };
        tx.commit()?;
        Ok(Some(CachedThread { items, position, live }))
    }

    /// Every stored position (the hub loads these at start).
    pub fn positions(&self) -> Result<Vec<(TargetId, ThreadId, JournalPosition)>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT target_id, thread_id, epoch, seq FROM thread_positions")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                TargetId::new(r.get::<_, String>(0)?),
                ThreadId::new(r.get::<_, String>(1)?),
                JournalPosition { epoch: Epoch(to_u64(r.get(2)?)), seq: Seq(to_u64(r.get(3)?)) },
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The target's sync token.
    pub fn sync_token(&self, target: &TargetId) -> Result<Option<SyncToken>, CacheError> {
        Ok(self
            .conn
            .query_row("SELECT token FROM sync_tokens WHERE target_id = ?1", params![target.as_str()], |r| r.get::<_, String>(0))
            .optional()?
            .map(SyncToken::new))
    }

    /// Every target's sync token.
    pub fn sync_tokens(&self) -> Result<Vec<(TargetId, SyncToken)>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT target_id, token FROM sync_tokens")?;
        let rows = stmt.query_map([], |r| Ok((TargetId::new(r.get::<_, String>(0)?), SyncToken::new(r.get::<_, String>(1)?))))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The target's cached thread index.
    pub fn threads(&self, target: &TargetId) -> Result<Vec<ThreadSummary>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT summary FROM threads WHERE target_id = ?1 ORDER BY updated_at DESC")?;
        let rows = stmt.query_map(params![target.as_str()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// One summary from the cached index.
    pub fn thread_summary(&self, target: &TargetId, thread: &ThreadId) -> Result<Option<ThreadSummary>, CacheError> {
        let summary = self.conn.query_row(
            "SELECT summary FROM threads WHERE target_id = ?1 AND thread_id = ?2",
            params![target.as_str(), thread.as_str()],
            |row| row.get::<_, String>(0),
        ).optional()?;
        Ok(summary.as_deref().map(serde_json::from_str).transpose()?)
    }

    /// Every thread's last known state, for the hub's lifecycle.
    pub fn thread_states(&self) -> Result<Vec<(TargetId, ThreadId, ThreadState)>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT target_id, thread_id, state FROM threads")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                TargetId::new(r.get::<_, String>(0)?),
                ThreadId::new(r.get::<_, String>(1)?),
                ThreadState::from_wire(&r.get::<_, String>(2)?),
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The target's cached projects.
    pub fn projects(&self, target: &TargetId) -> Result<Vec<Project>, CacheError> {
        let text = self
            .conn
            .query_row("SELECT projects FROM projects WHERE target_id = ?1", params![target.as_str()], |r| r.get::<_, String>(0))
            .optional()?;
        Ok(text.map(|t| serde_json::from_str(&t)).transpose()?.unwrap_or_default())
    }

    /// The target's cached provider descriptors.
    pub fn providers(&self, target: &TargetId) -> Result<Vec<ProviderDescriptor>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT descriptor FROM providers WHERE target_id = ?1 ORDER BY instance_id")?;
        let rows = stmt.query_map(params![target.as_str()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// The target's last known facts and state.
    pub fn target(&self, target: &TargetId) -> Result<Option<CachedTarget>, CacheError> {
        let row = self
            .conn
            .query_row("SELECT facts, state FROM targets WHERE target_id = ?1", params![target.as_str()], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .optional()?;
        let Some((facts, state)) = row else { return Ok(None) };
        let facts = facts.map(|f| serde_json::from_str(&f)).transpose()?;
        let state = state.map(|s| serde_json::from_str(&s)).transpose()?;
        Ok(Some(CachedTarget { facts, state }))
    }

    /// A `ui_state` value.
    pub fn ui_state(&self, key: &str) -> Result<Option<Value>, CacheError> {
        let text = self
            .conn
            .query_row("SELECT value FROM ui_state WHERE key = ?1", params![key], |r| r.get::<_, String>(0))
            .optional()?;
        Ok(text.map(|t| serde_json::from_str(&t)).transpose()?)
    }

    /// Number of rows a target has across the cache (tests, diagnostics).
    pub fn target_row_count(&self, target: &TargetId) -> Result<u64, CacheError> {
        let mut total = 0;
        for table in ["targets", "providers", "threads", "sync_tokens", "thread_positions", "items", "projects", "discovery"] {
            let n: i64 =
                self.conn.query_row(&format!("SELECT count(*) FROM {table} WHERE target_id = ?1"), params![target.as_str()], |r| {
                    r.get(0)
                })?;
            total += to_u64(n);
        }
        Ok(total)
    }
}

fn upsert_provider(tx: &Transaction<'_>, target: &TargetId, p: &ProviderDescriptor) -> Result<(), CacheError> {
    tx.execute(
        "INSERT INTO providers (target_id, instance_id, revision, descriptor) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (target_id, instance_id) DO UPDATE SET revision = excluded.revision, descriptor = excluded.descriptor
         WHERE excluded.revision >= providers.revision",
        params![target.as_str(), p.instance.as_str(), to_i64(p.revision), serde_json::to_string(p)?],
    )?;
    Ok(())
}

fn read_position(
    tx: &Transaction<'_>,
    target: &TargetId,
    thread: &ThreadId,
) -> Result<Option<(JournalPosition, LiveState, i64)>, CacheError> {
    let row = tx
        .query_row(
            "SELECT epoch, seq, live, bytes FROM thread_positions WHERE target_id = ?1 AND thread_id = ?2",
            params![target.as_str(), thread.as_str()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)),
        )
        .optional()?;
    let Some((epoch, seq, live, bytes)) = row else { return Ok(None) };
    let live = serde_json::from_str(&live).unwrap_or_default();
    Ok(Some((JournalPosition { epoch: Epoch(to_u64(epoch)), seq: Seq(to_u64(seq)) }, live, bytes)))
}

fn write_position(
    tx: &Transaction<'_>,
    target: &TargetId,
    thread: &ThreadId,
    position: JournalPosition,
    live: &LiveState,
    bytes: i64,
) -> Result<(), CacheError> {
    tx.execute(
        "INSERT OR REPLACE INTO thread_positions (target_id, thread_id, epoch, seq, live, bytes, last_used) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            target.as_str(),
            thread.as_str(),
            to_i64(position.epoch.0),
            to_i64(position.seq.0),
            serde_json::to_string(live)?,
            bytes.max(0),
            now_ms()
        ],
    )?;
    Ok(())
}

fn load_item(tx: &Transaction<'_>, target: &TargetId, thread: &ThreadId, item: &str) -> Result<Option<Item>, CacheError> {
    let text = tx
        .query_row(
            "SELECT item FROM items WHERE target_id = ?1 AND thread_id = ?2 AND item_id = ?3",
            params![target.as_str(), thread.as_str(), item],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    Ok(text.map(|t| serde_json::from_str(&t)).transpose()?)
}

/// Writes an item, keeping its place if it exists; returns the change in stored bytes.
fn store_item(tx: &Transaction<'_>, target: &TargetId, thread: &ThreadId, item: &Item) -> Result<i64, CacheError> {
    let mut item = item.clone();
    apply::redact_secrets(&mut item);
    let json = serde_json::to_string(&item)?;
    let existing: Option<(i64, i64)> = tx
        .query_row(
            "SELECT ord, length(item) FROM items WHERE target_id = ?1 AND thread_id = ?2 AND item_id = ?3",
            params![target.as_str(), thread.as_str(), item.id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (ord, old_len) = match existing {
        Some(found) => found,
        None => {
            let max: i64 = tx.query_row(
                "SELECT COALESCE(MAX(ord), 0) FROM items WHERE target_id = ?1 AND thread_id = ?2",
                params![target.as_str(), thread.as_str()],
                |r| r.get(0),
            )?;
            (max + 1, 0)
        }
    };
    tx.execute(
        "INSERT OR REPLACE INTO items (target_id, thread_id, item_id, ord, turn_id, kind, item) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![target.as_str(), thread.as_str(), item.id.as_str(), ord, item.turn_id.as_str(), item.kind(), json],
    )?;
    Ok(to_i64(json.len() as u64) - old_len)
}

fn apply_one(
    tx: &Transaction<'_>,
    target: &TargetId,
    thread: &ThreadId,
    event: &ThreadEvent,
    live: &mut LiveState,
    bytes: &mut i64,
) -> Result<(), CacheError> {
    match event {
        ThreadEvent::ItemStarted(e) | ThreadEvent::ItemCompleted(e) => *bytes += store_item(tx, target, thread, &e.item)?,
        ThreadEvent::ItemUpdated(e) => *bytes += store_item(tx, target, thread, &e.item)?,
        ThreadEvent::ItemDelta(e) => {
            if let Some(mut item) = load_item(tx, target, thread, e.item_id.as_str())?
                && apply::apply_delta(&mut item, &e.field, &e.delta)
            {
                *bytes += store_item(tx, target, thread, &item)?;
            }
        }
        ThreadEvent::RequestChanged(e) => {
            if let Some(mut item) = load_item(tx, target, thread, e.item_id.as_str())?
                && let ItemBody::Request(request) = &mut item.body
            {
                request.state = e.state.clone();
                if e.resolution.is_some() {
                    request.resolution = e.resolution.clone();
                }
                *bytes += store_item(tx, target, thread, &item)?;
            }
        }
        ThreadEvent::TurnStarted(e) => live.active_turn = Some(e.turn_id.clone()),
        ThreadEvent::TurnCompleted(e) => {
            if live.active_turn.as_ref() == Some(&e.turn_id) {
                live.active_turn = None;
            }
            live.last_turn = Some((e.turn_id.clone(), e.status.clone()));
        }
        ThreadEvent::ControlChanged(e) => {
            live.control = Some(e.control.clone());
            live.control_holder = e.holder.clone();
            live.active_turn = e.active_turn.clone();
        }
        ThreadEvent::SettingsChanged(e) => live.effective = Some(e.effective.clone()),
        ThreadEvent::UsageUpdated(e) => {
            if e.tokens.is_some() {
                live.tokens = e.tokens.clone();
            }
            if e.context.is_some() {
                live.context = e.context.clone();
            }
        }
        ThreadEvent::QueueChanged(e) => apply::apply_queue(&mut live.queued, e),
        ThreadEvent::HoldChanged(e) => live.hold = e.hold.clone(),
        ThreadEvent::Unknown(_) => {}
    }
    Ok(())
}
