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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
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

/// Encode a filesystem path the way Claude Code names its project dirs:
/// path separators and the Windows drive colon become '-'.
/// `/Users/a/code/x` -> `-Users-a-code-x`, `C:\code\x` -> `C--code-x`.
pub fn encode_project_path(path: &str) -> String {
    path.replace(['/', '\\', ':'], "-")
}

fn encoded_home() -> String {
    crate::config::home_dir()
        .map(|h| encode_project_path(&h.to_string_lossy()))
        .unwrap_or_default()
}

pub fn normalize_project_path(claude_dir_encoded: &str) -> String {
    let home_encoded = encoded_home();
    let home_encoded = home_encoded.trim_start_matches('-');

    let path = claude_dir_encoded.trim_start_matches('-');
    if let Some(rest) = path.strip_prefix(home_encoded) {
        rest.trim_start_matches('-').to_string()
    } else {
        path.to_string()
    }
}

pub fn denormalize_project_path(normalized: &str) -> String {
    format!("{}-{}", encoded_home(), normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_unix_path() {
        assert_eq!(encode_project_path("/Users/a/code/x"), "-Users-a-code-x");
    }

    #[test]
    fn encode_windows_path() {
        assert_eq!(encode_project_path(r"C:\code\clync"), "C--code-clync");
        assert_eq!(encode_project_path("C:/code/clync"), "C--code-clync");
    }
}
