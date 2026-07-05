use anyhow::Result;
use serde::Serialize;
use std::path::Path;

use crate::parser::parse_jsonl_file;
use crate::scanner::{ScanFilter, scan_sessions};

#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub uuid: String,
    pub project: String,
    pub messages: usize,
    pub first_message: Option<String>,
    pub size_bytes: u64,
    pub mtime: u64,
}

pub fn list_sessions(
    claude_projects_dir: &Path,
    query: Option<&str>,
    filter: &ScanFilter,
) -> Result<Vec<SessionSummary>> {
    let sessions = scan_sessions(claude_projects_dir, filter)?;
    let mut summaries = Vec::new();

    for session in &sessions {
        let entries = parse_jsonl_file(&session.jsonl_path).unwrap_or_default();

        let first_user_message = entries
            .iter()
            .find(|e| e.entry_type.as_deref() == Some("user"))
            .and_then(|e| {
                e.extra.get("message").and_then(|m| {
                    m.get("content")
                        .and_then(|c| c.as_str().map(|s| truncate(s, 120).to_string()))
                        .or_else(|| {
                            m.get("content").and_then(|c| {
                                c.as_array().and_then(|arr| {
                                    arr.iter().find_map(|item| {
                                        item.get("text")
                                            .and_then(|t| t.as_str())
                                            .map(|s| truncate(s, 120).to_string())
                                    })
                                })
                            })
                        })
                })
            });

        let message_count = entries
            .iter()
            .filter(|e| matches!(e.entry_type.as_deref(), Some("user") | Some("assistant")))
            .count();

        let summary = SessionSummary {
            uuid: session.uuid.clone(),
            project: session.entry.project_path.clone(),
            messages: message_count,
            first_message: first_user_message,
            size_bytes: session.entry.size,
            mtime: session.entry.mtime,
        };

        if let Some(q) = query {
            let q_lower = q.to_lowercase();
            let matches = summary.project.to_lowercase().contains(&q_lower)
                || summary.uuid.contains(&q_lower)
                || summary
                    .first_message
                    .as_ref()
                    .is_some_and(|m| m.to_lowercase().contains(&q_lower));
            if !matches {
                continue;
            }
        }

        summaries.push(summary);
    }

    summaries.sort_by_key(|s| std::cmp::Reverse(s.mtime));
    Ok(summaries)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_long_string() {
        assert_eq!(truncate("hello world", 5), "hello");
    }

    #[test]
    fn truncate_multibyte_utf8() {
        let s = "hello\u{00FC}world";
        let result = truncate(s, 6);
        assert_eq!(result, "hello");
    }

    #[test]
    fn truncate_empty() {
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn truncate_zero_max() {
        assert_eq!(truncate("hello", 0), "");
    }
}
