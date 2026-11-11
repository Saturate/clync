//! opencode v2 session reader/writer (SQLite <-> IR).

pub(super) mod project;
mod read;
mod write;

pub use read::read_session;
pub use write::write_session;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;

use super::Origin;

const XDG_DATA_HOME: &str = "XDG_DATA_HOME";

pub fn db_path() -> Result<PathBuf> {
    let data_dir = match std::env::var_os(XDG_DATA_HOME) {
        Some(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
        _ => dirs::home_dir()
            .context("cannot find home directory")?
            .join(".local")
            .join("share"),
    };
    Ok(data_dir.join("opencode").join("opencode.db"))
}

/// Open the existing opencode database. Never creates one: an empty file
/// without opencode's migrations would only produce confusing SQL errors.
fn open_db() -> Result<Connection> {
    let path = db_path()?;
    anyhow::ensure!(
        path.exists(),
        "opencode database not found at {}; run opencode once first",
        path.display()
    );
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {}", path.display()))?;
    // opencode may hold the database open while we write.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

#[derive(Debug)]
pub struct OpencodeSessionInfo {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub converted: bool,
}

/// List all top-level opencode v2 sessions, optionally filtered by project.
pub fn list_sessions(project_filter: Option<&Path>) -> Result<Vec<OpencodeSessionInfo>> {
    let conn = open_db()?;
    let filter = project_filter.map(|p| p.to_string_lossy().to_string());
    let mut stmt = conn.prepare(
        "SELECT s.id, s.title, s.directory, s.metadata
         FROM session_v2 s
         JOIN project p ON p.id = s.project_id
         WHERE s.parent_id IS NULL AND (?1 IS NULL OR p.worktree = ?1 OR s.directory = ?1)
         ORDER BY s.time_created DESC",
    )?;
    let rows = stmt.query_map(params![filter], |row| {
        let metadata: Option<String> = row.get(3)?;
        Ok(OpencodeSessionInfo {
            id: row.get(0)?,
            title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            directory: row.get(2)?,
            converted: metadata.as_deref().and_then(metadata_origin).is_some(),
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Find a session by full ID or unique ID prefix.
pub fn resolve_session(arg: &str) -> Result<String> {
    let conn = open_db()?;
    // Exact prefix match: LIKE would treat `_` as a wildcard. Child sessions
    // are subagent runs, not conversations to convert.
    let mut stmt = conn.prepare(
        "SELECT id FROM session_v2 WHERE substr(id, 1, length(?1)) = ?1 AND parent_id IS NULL LIMIT 2",
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![arg], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    match ids.as_slice() {
        [one] => Ok(one.clone()),
        [] => anyhow::bail!("no opencode session matches '{}'", arg),
        _ => anyhow::bail!("'{}' matches more than one opencode session", arg),
    }
}

/// Converted sessions already in opencode, keyed by origin.
pub fn provenance_index() -> Result<HashMap<Origin, String>> {
    let conn = open_db()?;
    let mut stmt = conn.prepare(
        "SELECT id, metadata FROM session_v2 WHERE metadata LIKE '%\"clync_source_id\"%'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut index = HashMap::new();
    for row in rows {
        let (id, metadata) = row?;
        if let Some(origin) = metadata_origin(&metadata) {
            index.insert(origin, id);
        }
    }
    Ok(index)
}

/// Delete a session clync wrote earlier, so `--force` can replace it. Child
/// rows are removed explicitly rather than trusting the connection's
/// foreign-key setting, and `event` hangs off `event_sequence`, not the session.
pub fn delete_session(id: &str) -> Result<()> {
    let mut conn = open_db()?;
    let tx = conn.transaction()?;
    for sql in [
        "DELETE FROM event WHERE aggregate_id = ?1",
        "DELETE FROM event_sequence WHERE aggregate_id = ?1",
        "DELETE FROM session_pending WHERE session_id = ?1",
        "DELETE FROM session_inbox WHERE session_id = ?1",
        "DELETE FROM session_message WHERE session_id = ?1",
        "DELETE FROM session_v2 WHERE id = ?1",
    ] {
        tx.execute(sql, params![id])?;
    }
    tx.commit()?;
    Ok(())
}

fn metadata_origin(metadata: &str) -> Option<Origin> {
    let meta: Value = serde_json::from_str(metadata).ok()?;
    Origin::parse(
        meta.get("clync_source")?.as_str()?,
        meta.get("clync_source_id")?.as_str()?,
    )
}

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// opencode's identifier format: prefix, 12 hex chars of
/// `timestamp_ms * 0x1000 + counter` (bit-inverted for descending IDs, which
/// opencode uses for sessions so newer ones sort first), then 14 random base62 chars.
/// Only the low 48 bits are kept, exactly as opencode does, so ordering holds
/// within a ~2 year window.
pub(super) fn opencode_id(prefix: &str, timestamp_ms: u64, descending: bool) -> String {
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed) % 0x1000;
    let mut n = timestamp_ms.wrapping_mul(0x1000).wrapping_add(counter);
    if descending {
        n = !n;
    }
    format!(
        "{}_{:012x}{}",
        prefix,
        n & 0xffff_ffff_ffff,
        random_base62(14)
    )
}

fn random_base62(len: usize) -> String {
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let bytes = uuid::Uuid::new_v4();
    bytes
        .as_bytes()
        .iter()
        .take(len)
        .map(|b| CHARS[usize::from(*b) % CHARS.len()] as char)
        .collect()
}

fn random_slug() -> String {
    const ADJECTIVES: &[&str] = &[
        "amber", "bold", "calm", "dark", "eager", "fair", "glad", "hazy", "keen", "late", "mild",
        "neat", "odd", "pale", "rare", "safe", "tall", "vast", "warm", "wise",
    ];
    const NOUNS: &[&str] = &[
        "bear", "crab", "deer", "dove", "fawn", "frog", "goat", "hawk", "lion", "lynx", "moth",
        "newt", "orca", "puma", "rook", "seal", "swan", "toad", "vole", "wolf",
    ];
    let bytes = uuid::Uuid::new_v4();
    let b = bytes.as_bytes();
    format!(
        "{}-{}",
        ADJECTIVES[usize::from(b[0]) % ADJECTIVES.len()],
        NOUNS[usize::from(b[1]) % NOUNS.len()]
    )
}
