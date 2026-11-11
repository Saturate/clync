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
    Ok(dirs::home_dir()
        .context("cannot find home directory")?
        .join(".claude")
        .join("projects"))
}

/// Encode a project directory path the way Claude Code does: every character
/// that isn't an ASCII letter or digit becomes '-'.
/// `/Users/alkj/code/DCC.Frontends` -> `-Users-alkj-code-DCC-Frontends`
/// Claude works on UTF-16 code units, so a character outside the BMP becomes
/// two dashes, and names over 200 units are cut and suffixed with a hash.
pub fn encode_project_dir(path: &Path) -> String {
    const MAX_LEN: usize = 200;
    let raw = path.to_string_lossy();
    let units: Vec<u16> = raw.encode_utf16().collect();
    let encoded: String = units
        .iter()
        .map(|&u| match char::from_u32(u32::from(u)) {
            Some(c) if c.is_ascii_alphanumeric() => c,
            _ => '-',
        })
        .collect();
    if encoded.len() <= MAX_LEN {
        return encoded;
    }
    // Java-style string hash over UTF-16 units, as Claude Code computes it.
    let hash = units.iter().fold(0i32, |h, &u| {
        h.wrapping_shl(5).wrapping_sub(h).wrapping_add(i32::from(u))
    });
    format!("{}-{}", &encoded[..MAX_LEN], to_base36(hash.unsigned_abs()))
}

fn to_base36(mut n: u32) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
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

/// Converted sessions already in Claude, keyed by origin. Reads only the
/// first few lines of each file since the provenance entry is written near the top.
pub fn provenance_index() -> Result<HashMap<Origin, PathBuf>> {
    let mut index = HashMap::new();
    for path in session_files(&projects_dir()?)? {
        let origin = jsonl::read_head(&path, 5).into_iter().find_map(|e| {
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
