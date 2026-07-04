use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub sessions: HashMap<String, SessionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub uuid: String,
    pub project_path: String,
    pub mtime: u64,
    pub size: u64,
    pub content_hash: u64,
    #[serde(default)]
    pub has_companion: bool,
    #[serde(default)]
    pub last_pushed_by: String,
}

impl Manifest {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }
}

pub fn get_machine_id() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

pub fn normalize_project_path(claude_dir_encoded: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let home_encoded = home.replace('/', "-");
    let home_encoded = home_encoded.trim_start_matches('-');

    let path = claude_dir_encoded.trim_start_matches('-');
    if let Some(rest) = path.strip_prefix(home_encoded) {
        rest.trim_start_matches('-').to_string()
    } else {
        path.to_string()
    }
}

pub fn denormalize_project_path(normalized: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let home_encoded = home.replace('/', "-");
    format!("{}-{}", home_encoded, normalized)
}
