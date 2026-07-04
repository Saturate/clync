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
