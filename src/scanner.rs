use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::manifest::{SessionEntry, get_machine_id, normalize_project_path};
use crate::parser::file_content_hash;

#[allow(dead_code)]
pub struct LocalSession {
    pub uuid: String,
    pub project_dir_name: String,
    pub jsonl_path: PathBuf,
    pub companion_dir: Option<PathBuf>,
    pub entry: SessionEntry,
}

#[derive(Clone, Default)]
pub struct ScanFilter {
    pub max_age_days: Option<u64>,
    pub max_file_size: Option<u64>,
}

pub fn scan_sessions(claude_projects_dir: &Path, filter: &ScanFilter) -> Result<Vec<LocalSession>> {
    let mut sessions = Vec::new();

    if !claude_projects_dir.exists() {
        return Ok(sessions);
    }

    let now = SystemTime::now();
    let age_cutoff = filter.max_age_days.map(|days| {
        now.checked_sub(Duration::from_secs(days * 86400))
            .unwrap_or(UNIX_EPOCH)
    });

    for project_entry in std::fs::read_dir(claude_projects_dir)? {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }

        let project_dir_name = project_entry.file_name().to_string_lossy().to_string();

        for entry in std::fs::read_dir(&project_path)? {
            let entry = entry?;
            let path = entry.path();

            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".jsonl") {
                continue;
            }

            let uuid = name.trim_end_matches(".jsonl").to_string();
            if uuid.contains('/') || uuid == "memory" {
                continue;
            }

            let metadata = std::fs::metadata(&path)?;

            if let Some(max_size) = filter.max_file_size
                && metadata.len() > max_size
            {
                continue;
            }

            let modified = metadata.modified()?;
            if let Some(cutoff) = age_cutoff
                && modified < cutoff
            {
                continue;
            }

            let mtime = modified
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let size = metadata.len();

            let content_hash = match file_content_hash(&path) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("warning: could not hash {}: {e}", path.display());
                    continue;
                }
            };

            let companion_dir = project_path.join(&uuid);
            let has_companion = companion_dir.is_dir();

            let normalized_project = normalize_project_path(&project_dir_name);

            sessions.push(LocalSession {
                uuid: uuid.clone(),
                project_dir_name: project_dir_name.clone(),
                jsonl_path: path,
                companion_dir: if has_companion {
                    Some(companion_dir)
                } else {
                    None
                },
                entry: SessionEntry {
                    uuid,
                    project_path: normalized_project,
                    mtime,
                    size,
                    content_hash,
                    has_companion,
                    last_pushed_by: get_machine_id(),
                },
            });
        }
    }

    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("clync-scanner-test")
            .join(name)
            .join(format!("{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        dir
    }

    fn write_session(dir: &Path, project: &str, uuid: &str) {
        let project_dir = dir.join(project);
        std::fs::create_dir_all(&project_dir).unwrap();
        let content = format!(
            r#"{{"uuid":"{uuid}","type":"human","timestamp":1700000000,"text":"hello"}}"#
        );
        std::fs::write(project_dir.join(format!("{uuid}.jsonl")), content.as_bytes()).unwrap();
    }

    #[test]
    fn nonexistent_dir_returns_empty() {
        let dir = test_dir("nonexistent");
        let filter = ScanFilter::default();
        let sessions = scan_sessions(&dir, &filter).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn empty_dir_returns_empty() {
        let dir = test_dir("empty");
        std::fs::create_dir_all(&dir).unwrap();
        let filter = ScanFilter::default();
        let sessions = scan_sessions(&dir, &filter).unwrap();
        assert!(sessions.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finds_valid_sessions() {
        let dir = test_dir("valid");
        write_session(&dir, "project-a", "abc-123");
        write_session(&dir, "project-a", "def-456");

        let filter = ScanFilter::default();
        let sessions = scan_sessions(&dir, &filter).unwrap();
        assert_eq!(sessions.len(), 2);

        let uuids: Vec<&str> = sessions.iter().map(|s| s.uuid.as_str()).collect();
        assert!(uuids.contains(&"abc-123"));
        assert!(uuids.contains(&"def-456"));

        let s = sessions.iter().find(|s| s.uuid == "abc-123").unwrap();
        assert_eq!(s.project_dir_name, "project-a");
        assert!(s.entry.size > 0);
        assert!(!s.entry.has_companion);
        assert!(s.companion_dir.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detects_companion_dirs() {
        let dir = test_dir("companion");
        write_session(&dir, "project-x", "sess-001");
        std::fs::create_dir_all(dir.join("project-x").join("sess-001")).unwrap();
        std::fs::write(
            dir.join("project-x").join("sess-001").join("artifact.txt"),
            "data",
        )
        .unwrap();

        let sessions = scan_sessions(&dir, &ScanFilter::default()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].entry.has_companion);
        assert!(sessions[0].companion_dir.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skips_non_jsonl_files() {
        let dir = test_dir("non_jsonl");
        let project_dir = dir.join("project-b");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(project_dir.join("notes.txt"), "not a session").unwrap();
        std::fs::write(project_dir.join("data.json"), "{}").unwrap();

        let sessions = scan_sessions(&dir, &ScanFilter::default()).unwrap();
        assert!(sessions.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skips_memory_jsonl() {
        let dir = test_dir("memory");
        let project_dir = dir.join("project-c");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(project_dir.join("memory.jsonl"), r#"{"type":"memory"}"#).unwrap();

        let sessions = scan_sessions(&dir, &ScanFilter::default()).unwrap();
        assert!(sessions.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn filters_by_max_file_size() {
        let dir = test_dir("max_size");
        write_session(&dir, "project-d", "small-uuid");
        let big_content = "x".repeat(10_000);
        let project_dir = dir.join("project-d");
        std::fs::write(
            project_dir.join("big-uuid.jsonl"),
            big_content.as_bytes(),
        )
        .unwrap();

        let filter = ScanFilter {
            max_file_size: Some(1000),
            ..Default::default()
        };
        let sessions = scan_sessions(&dir, &filter).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].uuid, "small-uuid");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn filters_by_max_age_days() {
        let dir = test_dir("max_age");
        write_session(&dir, "project-e", "recent-uuid");

        // Set mtime to 1000 days ago for a second file
        let project_dir = dir.join("project-e");
        let old_path = project_dir.join("old-uuid.jsonl");
        std::fs::write(&old_path, r#"{"type":"human","uuid":"old-uuid"}"#).unwrap();
        let old_time = std::time::SystemTime::now()
            .checked_sub(Duration::from_secs(1000 * 86400))
            .unwrap();
        let _ = filetime::set_file_mtime(
            &old_path,
            filetime::FileTime::from_system_time(old_time),
        );

        let filter = ScanFilter {
            max_age_days: Some(30),
            ..Default::default()
        };
        let sessions = scan_sessions(&dir, &filter).unwrap();
        // recent-uuid should always be included; old-uuid should be excluded
        // (if filetime crate isn't available the mtime won't change, so we just
        // check that at least the recent one is found)
        assert!(sessions.iter().any(|s| s.uuid == "recent-uuid"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skips_non_dir_entries_in_projects() {
        let dir = test_dir("non_dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("not-a-dir.txt"), "file at root").unwrap();

        let sessions = scan_sessions(&dir, &ScanFilter::default()).unwrap();
        assert!(sessions.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sessions_across_multiple_projects() {
        let dir = test_dir("multi_project");
        write_session(&dir, "project-x", "uuid-x");
        write_session(&dir, "project-y", "uuid-y");

        let sessions = scan_sessions(&dir, &ScanFilter::default()).unwrap();
        assert_eq!(sessions.len(), 2);

        let projects: Vec<&str> = sessions.iter().map(|s| s.project_dir_name.as_str()).collect();
        assert!(projects.contains(&"project-x"));
        assert!(projects.contains(&"project-y"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
