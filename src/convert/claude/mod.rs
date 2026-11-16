//! Claude Code session reader/writer (JSONL <-> IR).

mod branch;
mod read;
mod write;

pub use read::read_session;
pub use write::write_session;

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::{Origin, jsonl};

pub fn projects_dir() -> Result<PathBuf> {
    Ok(crate::config::home_dir()
        .context("cannot find home directory")?
        .join(".claude")
        .join("projects"))
}

/// Claude Code's project folder name for `path`.
pub fn encode_project_dir(path: &Path) -> String {
    crate::manifest::encode_project_path(&path.to_string_lossy())
}

/// Best-effort decode of a project folder name. Lossy for paths that contain
/// '-', so the `cwd` recorded in the session entries is preferred when present.
pub fn decode_project_dir(encoded: &str) -> PathBuf {
    PathBuf::from(encoded.replace('-', "/"))
}

#[derive(Debug)]
pub struct ClaudeSessionInfo {
    pub uuid: String,
    pub path: PathBuf,
    pub project_dir: PathBuf,
    pub title: String,
    pub timestamp_ms: u64,
}

/// List all top-level Claude sessions (subagent transcripts are excluded),
/// optionally filtered by project.
pub fn list_sessions(project_filter: Option<&Path>) -> Result<Vec<ClaudeSessionInfo>> {
    let root = projects_dir()?;
    let mut sessions = Vec::new();
    for path in session_files(&root)? {
        let uuid = file_stem(&path);
        let meta = scan_session_metadata(&path).unwrap_or_default();
        let project_dir = meta.cwd.unwrap_or_else(|| {
            let folder = path.parent().and_then(|p| p.file_name());
            decode_project_dir(&folder.unwrap_or_default().to_string_lossy())
        });
        if project_filter.is_some_and(|f| f != project_dir) {
            continue;
        }
        sessions.push(ClaudeSessionInfo {
            uuid,
            path,
            project_dir,
            // A user-set custom title wins over the generated one, matching Claude's UI.
            title: meta
                .custom_title
                .or(meta.ai_title)
                .unwrap_or_else(|| "Untitled session".to_string()),
            timestamp_ms: meta.earliest_ts.unwrap_or(0),
        });
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp_ms));
    Ok(sessions)
}

/// Find a session by file path, full UUID, or unique UUID prefix.
pub fn resolve_session(arg: &str) -> Result<PathBuf> {
    let as_path = PathBuf::from(shellexpand::tilde(arg).as_ref());
    if as_path.is_file() {
        return Ok(as_path);
    }
    let matches: Vec<PathBuf> = session_files(&projects_dir()?)?
        .into_iter()
        .filter(|p| file_stem(p).starts_with(arg))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] => anyhow::bail!("no Claude session matches '{}'", arg),
        _ => anyhow::bail!("'{}' matches {} Claude sessions", arg, matches.len()),
    }
}

/// Converted sessions already in Claude, keyed by origin. Scans whole files:
/// clync's sync merge moves entries without a uuid, provenance included, to
/// the end.
pub fn provenance_index() -> Result<HashMap<Origin, PathBuf>> {
    let mut index = HashMap::new();
    for path in session_files(&projects_dir()?)? {
        let origin = find_provenance(&path).and_then(|e| {
            if e.get("type").and_then(|v| v.as_str()) != Some("clync-provenance") {
                return None;
            }
            Origin::parse(
                e.get("source").and_then(|v| v.as_str()).unwrap_or(""),
                e.get("sourceId").and_then(|v| v.as_str()).unwrap_or(""),
            )
        });
        if let Some(origin) = origin {
            index.insert(origin, path);
        }
    }
    Ok(index)
}

fn session_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    for project in fs::read_dir(root)? {
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

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

#[derive(Default)]
struct SessionMeta {
    ai_title: Option<String>,
    custom_title: Option<String>,
    earliest_ts: Option<u64>,
    cwd: Option<PathBuf>,
}

/// Only lines mentioning the marker get parsed; session files can be hundreds of MB.
pub(crate) fn find_provenance(path: &Path) -> Option<Value> {
    let reader = BufReader::new(fs::File::open(path).ok()?);
    reader
        .split(b'\n')
        .map_while(Result::ok)
        .map(|line| String::from_utf8_lossy(&line).into_owned())
        .filter(|line| line.contains("\"clync-provenance\""))
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .find(|e| e.get("type").and_then(|v| v.as_str()) == Some("clync-provenance"))
}

/// Cheap scan: only lines that mention the fields we need get parsed, since
/// sessions can be hundreds of MB and listing touches every one of them.
fn scan_session_metadata(path: &Path) -> Result<SessionMeta> {
    let reader = BufReader::new(fs::File::open(path)?);
    let mut meta = SessionMeta::default();
    // Raw byte lines: one invalid UTF-8 line shouldn't hide the whole session.
    for line in reader.split(b'\n') {
        let line = String::from_utf8_lossy(&line?).into_owned();
        let wants_title = line.contains("\"aiTitle\"") || line.contains("\"customTitle\"");
        let wants_ts = meta.earliest_ts.is_none() && line.contains("\"timestamp\"");
        let wants_cwd = meta.cwd.is_none() && line.contains("\"cwd\"");
        if !(wants_title || wants_ts || wants_cwd) {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(t) = entry.get("customTitle").and_then(|v| v.as_str()) {
            meta.custom_title = Some(t.to_string());
        } else if let Some(t) = entry.get("aiTitle").and_then(|v| v.as_str()) {
            meta.ai_title = Some(t.to_string());
        }
        if meta.earliest_ts.is_none() {
            meta.earliest_ts = jsonl::entry_timestamp(&entry);
        }
        if meta.cwd.is_none() {
            meta.cwd = entry_cwd(&entry);
        }
    }
    Ok(meta)
}

fn entry_cwd(entry: &Value) -> Option<PathBuf> {
    entry
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}
