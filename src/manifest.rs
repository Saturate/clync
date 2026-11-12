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

/// Claude Code truncates encoded names longer than this and appends a hash.
const MAX_PROJECT_DIR_LEN: usize = 200;

/// Encode a filesystem path the way Claude Code names its project dirs:
/// every char outside `[a-zA-Z0-9]` becomes '-' (one per UTF-16 unit, as
/// the JS regex sees it), and names over 200 chars are cut and suffixed
/// with a base-36 hash of the original path.
/// `/Users/a/my.app` -> `-Users-a-my-app`, `C:\code\x` -> `C--code-x`.
pub fn encode_project_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    if out.len() <= MAX_PROJECT_DIR_LEN {
        return out;
    }
    // `out` is pure ASCII, so byte slicing is safe
    format!(
        "{}-{}",
        &out[..MAX_PROJECT_DIR_LEN],
        to_base36(js_string_hash(path).unsigned_abs())
    )
}

/// Java-style `h = (h << 5) - h + c` over UTF-16 units with i32 wraparound,
/// matching Claude Code's hash for long project names.
fn js_string_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, c| {
        h.wrapping_shl(5).wrapping_sub(h).wrapping_add(i32::from(c))
    })
}

fn to_base36(mut n: u32) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).expect("base36 digits are ASCII")
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

    #[test]
    fn encode_non_alphanumeric() {
        assert_eq!(
            encode_project_path(r"C:\Users\first.last\my_app v2"),
            "C--Users-first-last-my-app-v2"
        );
        // Reference values from Claude Code's JS regex: astral chars are two
        // UTF-16 units (two dashes), BMP non-ASCII chars are one
        assert_eq!(encode_project_path("/a/\u{1F600}b"), "-a---b");
        assert_eq!(encode_project_path("/a/\u{e6}b"), "-a--b");
    }

    #[test]
    fn encode_long_path_truncates_with_hash() {
        let path = format!("/home/u/{}", "a".repeat(250));
        let encoded = encode_project_path(&path);
        assert_eq!(
            &encoded[..MAX_PROJECT_DIR_LEN],
            &encode_project_path(&path[..200])
        );
        // Reference value from Claude Code's hash in JS
        assert_eq!(&encoded[MAX_PROJECT_DIR_LEN..], "-bkw3mj");
    }

    #[test]
    fn js_hash_matches_reference() {
        assert_eq!(to_base36(js_string_hash("").unsigned_abs()), "0");
        assert_eq!(to_base36(js_string_hash("hello").unsigned_abs()), "1n1e4y");
    }
}
