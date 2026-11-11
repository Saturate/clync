//! The project ID opencode assigns to a directory, so converted sessions land
//! in the project opencode shows when it's opened there.
//!
//! opencode 2.x picks, for a git repo: sha1 of `git-remote:<host>/<path>` for
//! `origin`, else the ID cached in `<git common dir>/opencode`, else the root
//! commit. A folder outside git gets sha1 of `directory:<path>`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use sha1::{Digest, Sha1};

/// Resolve (and if needed insert) the project row for `dir`. Returns its ID.
pub fn resolve_project(conn: &Connection, dir: &Path) -> Result<String> {
    let (id, worktree, vcs) = match git(dir, &["rev-parse", "--show-toplevel"]) {
        Some(toplevel) => {
            let id = git(dir, &["remote", "get-url", "origin"])
                .and_then(|url| normalize_remote(&url))
                .map(|remote| sha1_hex(&format!("git-remote:{}", remote)))
                .or_else(|| cached_id(dir))
                .or_else(|| root_commit(dir))
                .unwrap_or_else(|| "global".to_string());
            (id, PathBuf::from(toplevel), Some("git"))
        }
        None => {
            let path = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
            let path_str = path.to_string_lossy().to_string();
            // A row recorded while the folder still existed beats recomputing
            // from a path that may no longer resolve the same way.
            let existing: Option<String> = conn
                .query_row(
                    "SELECT id FROM project WHERE worktree = ?1 ORDER BY time_updated DESC LIMIT 1",
                    params![path_str],
                    |row| row.get(0),
                )
                .optional()?;
            let id = existing.unwrap_or_else(|| sha1_hex(&format!("directory:{}", path_str)));
            (id, path, None)
        }
    };

    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT OR IGNORE INTO project (id, worktree, vcs, time_created, time_updated, sandboxes, time_active)
         VALUES (?1, ?2, ?3, ?4, ?4, '[]', ?4)",
        params![id, worktree.to_string_lossy().as_ref(), vcs, now],
    )?;
    Ok(id)
}

/// opencode's remote normalization: `<lowercased host>/<path>` with leading
/// slashes, a trailing `.git` and trailing slashes removed. Handles URLs and
/// scp-style `user@host:path`; `file:` remotes have no stable identity.
pub fn normalize_remote(url: &str) -> Option<String> {
    let v = url.trim();
    if v.is_empty() {
        return None;
    }
    if let Some((scheme, rest)) = split_scheme(v) {
        // Anything that parses as a URL is handled as one, even when it has no
        // host (opencode never falls back to the scp form for those).
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        let rest = rest.strip_prefix("//")?;
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..end];
        let path_and_more = &rest[end..];
        let path = &path_and_more[..path_and_more
            .find(['?', '#'])
            .unwrap_or(path_and_more.len())];
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = if host_port.starts_with('[') {
            &host_port[..host_port.find(']').map_or(host_port.len(), |i| i + 1)]
        } else {
            host_port.split(':').next().unwrap_or("")
        };
        return join_remote(host, path);
    }
    // scp-like syntax: [user@]host:path
    let without_user = match v.split_once('@') {
        Some((user, rest)) if !user.is_empty() && !user.contains(['/', ':']) => rest,
        _ => v,
    };
    let (host, path) = without_user.split_once(':')?;
    if host.is_empty() || host.contains('/') || path.is_empty() {
        return None;
    }
    join_remote(host, path)
}

fn split_scheme(v: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = v.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then_some((scheme, rest))
}

fn join_remote(host: &str, path: &str) -> Option<String> {
    let path = path.trim_start_matches('/');
    let path = path
        .strip_suffix(".git/")
        .or_else(|| path.strip_suffix(".git"))
        .unwrap_or(path)
        .trim_end_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{}", host.to_lowercase(), path))
}

fn cached_id(dir: &Path) -> Option<String> {
    let common = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let id = std::fs::read_to_string(Path::new(&common).join("opencode")).ok()?;
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_string())
}

fn root_commit(dir: &Path) -> Option<String> {
    let roots = git(dir, &["rev-list", "--max-parents=0", "HEAD"])?;
    let mut roots: Vec<&str> = roots.lines().collect();
    roots.sort_unstable();
    roots.first().map(|s| s.to_string())
}

pub fn sha1_hex(input: &str) -> String {
    Sha1::digest(input.as_bytes())
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}
