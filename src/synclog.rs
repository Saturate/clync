use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::manifest::get_machine_id;

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncLogEntry {
    pub timestamp: u64,
    pub machine: String,
    pub operation: String,
    pub sessions_pushed: u32,
    pub sessions_pulled: u32,
    pub sessions_merged: u32,
    pub sessions_skipped: u32,
    pub extras: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merges: Vec<MergeRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MergeRecord {
    pub uuid: String,
    pub project: String,
    pub local_entries: usize,
    pub remote_entries: usize,
    pub merged_entries: usize,
    pub edits_resolved: usize,
}

impl SyncLogEntry {
    pub fn new(operation: &str) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            timestamp: now,
            machine: get_machine_id(),
            operation: operation.to_string(),
            sessions_pushed: 0,
            sessions_pulled: 0,
            sessions_merged: 0,
            sessions_skipped: 0,
            extras: 0,
            merges: Vec::new(),
            error: None,
        }
    }
}

const LOG_FILE: &str = "sync.log.jsonl";

const MAX_LOG_ENTRIES: usize = 500;

pub fn append(repo_path: &Path, entry: &SyncLogEntry) -> Result<()> {
    let path = repo_path.join(LOG_FILE);
    let line = serde_json::to_string(entry)? + "\n";
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    std::io::Write::write_all(&mut file, line.as_bytes())?;

    let contents = std::fs::read_to_string(&path)?;
    let lines: Vec<&str> = contents.lines().collect();
    if lines.len() > MAX_LOG_ENTRIES {
        let trimmed = lines[lines.len() - MAX_LOG_ENTRIES..].join("\n") + "\n";
        std::fs::write(&path, trimmed)?;
    }

    Ok(())
}

pub fn read_recent(repo_path: &Path, limit: usize) -> Result<Vec<SyncLogEntry>> {
    let path = repo_path.join(LOG_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = std::fs::read_to_string(&path)?;
    let mut entries: Vec<SyncLogEntry> = contents
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    entries.reverse();
    entries.truncate(limit);
    Ok(entries)
}
