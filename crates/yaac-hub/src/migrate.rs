//! Ordered schema migrations for the durable stores (drafts, relay queue). Unlike the cache,
//! these hold user data and are never dropped: each migration runs once, in order, inside a
//! transaction, and a file from a newer build is refused rather than misread.

use rusqlite::Connection;

/// A migration failed, or the file is from a newer build.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    /// SQLite.
    #[error("{store}: {source}")]
    Sql {
        /// Which store.
        store: &'static str,
        /// Cause.
        #[source]
        source: rusqlite::Error,
    },
    /// The file was written by a newer build. Update YAAC; nothing was changed.
    #[error("{store} was written by a newer YAAC (schema {found}, this build knows {known}); update YAAC")]
    Newer {
        /// Which store.
        store: &'static str,
        /// The file's schema.
        found: i64,
        /// This build's newest schema.
        known: i64,
    },
}

/// Opens a durable store and brings it to the newest schema.
pub(crate) fn open(path: &std::path::Path, store: &'static str, migrations: &[&str]) -> Result<Connection, MigrateError> {
    let sql = |source| MigrateError::Sql { store, source };
    let mut conn = Connection::open(path).map_err(sql)?;
    let _ = crate::paths::make_private_file(path);
    conn.pragma_update(None, "journal_mode", "WAL").map_err(sql)?;
    // Once acknowledged, a relayed message exists only here: every commit reaches the disk.
    conn.pragma_update(None, "synchronous", "FULL").map_err(sql)?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(sql)?;
    let known = i64::try_from(migrations.len()).unwrap_or(i64::MAX);
    let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(sql)?;
    if found > known {
        return Err(MigrateError::Newer { store, found, known });
    }
    for (index, migration) in migrations.iter().enumerate().skip(usize::try_from(found).unwrap_or(0)) {
        let tx = conn.transaction().map_err(sql)?;
        tx.execute_batch(migration).map_err(sql)?;
        tx.pragma_update(None, "user_version", i64::try_from(index + 1).unwrap_or(i64::MAX)).map_err(sql)?;
        tx.commit().map_err(sql)?;
    }
    Ok(conn)
}
