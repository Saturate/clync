//! pi session reader/writer (JSONL <-> IR).

mod read;
mod write;

pub use read::read_session;
pub use write::write_session;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::{Origin, jsonl};

/// Custom entry types clync writes. pi keeps `custom` entries in the session
/// tree but leaves them out of the model context.
pub const PROVENANCE_TYPE: &str = "clync-provenance";
pub const STASH_TYPE: &str = "clync-stash";

pub fn sessions_dir() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("cannot find home directory")?
        .join(".pi")
        .join("agent")
        .join("sessions"))
}

/// Encode a project directory the way pi does: drop one leading separator,
/// then replace `/`, `\\` and `:`.
/// `/Users/alkj/code/github/clync` -> `--Users-alkj-code-github-clync--`
pub fn encode_project_dir(path: &Path) -> String {
    let s = path.to_string_lossy();
    let trimmed = s.strip_prefix(['/', '\\']).unwrap_or(&s);
    format!("--{}--", trimmed.replace(['/', '\\', ':'], "-"))
}

/// Best-effort decode of a project folder name. Lossy for paths that contain
/// '-', so the `cwd` in the session header is preferred.
pub fn decode_project_dir(encoded: &str) -> PathBuf {
    let trimmed = encoded.trim_matches('-');
    PathBuf::from(format!("/{}", trimmed.replace('-', "/")))
}

#[derive(Debug)]
pub struct PiSessionInfo {
    pub id: String,
    pub path: PathBuf,
    pub project_dir: PathBuf,
    pub title: String,
    pub timestamp_ms: u64,
}

/// List all pi sessions, optionally filtered by project.
pub fn list_sessions(project_filter: Option<&Path>) -> Result<Vec<PiSessionInfo>> {
    let mut sessions = Vec::new();
    for path in session_files()? {
        let info = scan_session(&path);
        if project_filter.is_some_and(|f| f != info.project_dir) {
            continue;
        }
        sessions.push(info);
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp_ms));
    Ok(sessions)
}

/// Find a session by file path, full session ID, or unique ID prefix.
pub fn resolve_session(arg: &str) -> Result<PathBuf> {
    let as_path = PathBuf::from(shellexpand::tilde(arg).as_ref());
    if as_path.is_file() {
        return Ok(as_path);
    }
    let matches: Vec<PathBuf> = session_files()?
        .into_iter()
        .filter(|p| header_id(p).is_some_and(|id| id.starts_with(arg)))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] => anyhow::bail!("no pi session matches '{}'", arg),
        _ => anyhow::bail!("'{}' matches {} pi sessions", arg, matches.len()),
    }
}

/// Converted sessions already in pi, keyed by origin.
pub fn provenance_index() -> Result<HashMap<Origin, PathBuf>> {
    let mut index = HashMap::new();
    for path in session_files()? {
        let head = jsonl::read_head(&path, 5);
        // A fork of a converted session copies its provenance entry, but it's
        // the user's session, not clync's to replace.
        let is_fork = head
            .first()
            .is_some_and(|h| h.get("parentSession").is_some());
        let origin = head
            .iter()
            .find_map(|e| custom_data(e, PROVENANCE_TYPE).and_then(provenance_origin))
            .filter(|_| !is_fork);
        if let Some(origin) = origin {
            index.insert(origin, path);
        }
    }
    Ok(index)
}

/// `data` of a `custom` entry with the given customType.
pub fn custom_data<'a>(entry: &'a Value, custom_type: &str) -> Option<&'a Value> {
    if entry.get("type").and_then(|v| v.as_str()) != Some("custom")
        || entry.get("customType").and_then(|v| v.as_str()) != Some(custom_type)
    {
        return None;
    }
    entry.get("data")
}

pub fn provenance_origin(data: &Value) -> Option<Origin> {
    Origin::parse(
        data.get("source").and_then(|v| v.as_str()).unwrap_or(""),
        data.get("sourceId").and_then(|v| v.as_str()).unwrap_or(""),
    )
}

fn session_files() -> Result<Vec<PathBuf>> {
    let root = sessions_dir()?;
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    for project in fs::read_dir(&root)? {
        let project = project?.path();
        if !project.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&project)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn header_id(path: &Path) -> Option<String> {
    let header = jsonl::read_head(path, 1).into_iter().next()?;
    header
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn scan_session(path: &Path) -> PiSessionInfo {
    let head = jsonl::read_head(path, 20);
    let header = head
        .iter()
        .find(|e| e.get("type").and_then(|v| v.as_str()) == Some("session"));
    let folder = path
        .parent()
        .and_then(|p| p.file_name())
        .unwrap_or_default();
    let project_dir = header
        .and_then(|h| h.get("cwd"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| decode_project_dir(&folder.to_string_lossy()));
    let first_user = head.iter().find(|e| {
        e.get("message")
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            == Some("user")
    });
    let title: String = first_user
        .map(|e| jsonl::text_blocks(e.get("message").and_then(|m| m.get("content"))))
        .unwrap_or_default()
        .chars()
        .take(80)
        .collect();

    PiSessionInfo {
        id: header
            .and_then(|h| h.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        path: path.to_path_buf(),
        project_dir,
        title: if title.is_empty() {
            "Untitled session".to_string()
        } else {
            title
        },
        timestamp_ms: header.and_then(jsonl::entry_timestamp).unwrap_or(0),
    }
}
