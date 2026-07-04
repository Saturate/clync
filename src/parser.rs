use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(
        default,
        rename = "parentUuid",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_uuid: Option<String>,
    #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<Value>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub entry_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

impl ConversationEntry {
    pub fn content_hash(&self) -> u64 {
        let serialized = serde_json::to_string(self).unwrap_or_default();
        fnv1a_hash(serialized.as_bytes())
    }

    pub fn timestamp_millis(&self) -> u64 {
        match &self.timestamp {
            Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
            Some(Value::String(s)) => s.parse().unwrap_or(0),
            _ => 0,
        }
    }
}

pub fn parse_jsonl(data: &[u8]) -> Result<Vec<ConversationEntry>> {
    let text = std::str::from_utf8(data).context("session file is not valid UTF-8")?;
    let mut entries = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<ConversationEntry>(trimmed) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                eprintln!("warning: skipping unparseable JSONL line: {e}");
            }
        }
    }
    Ok(entries)
}

pub fn entries_to_jsonl(entries: &[ConversationEntry]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for entry in entries {
        let line = serde_json::to_string(entry)?;
        output.extend_from_slice(line.as_bytes());
        output.push(b'\n');
    }
    Ok(output)
}

pub fn parse_jsonl_file(path: &Path) -> Result<Vec<ConversationEntry>> {
    let data = std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    parse_jsonl(&data)
}

pub fn file_content_hash(path: &Path) -> Result<u64> {
    let data = std::fs::read(path)?;
    Ok(fnv1a_hash(&data))
}

fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty() {
        let entries = parse_jsonl(b"").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_blank_lines() {
        let input = b"\n\n{\"type\":\"mode\"}\n\n";
        let entries = parse_jsonl(input).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry_type.as_deref(), Some("mode"));
    }

    #[test]
    fn parse_conversation_entry() {
        let input = br#"{"uuid":"abc-123","parentUuid":"parent-1","sessionId":"sess-1","timestamp":1700000000,"type":"user","message":{"content":"hello"}}"#;
        let entries = parse_jsonl(input).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].uuid.as_deref(), Some("abc-123"));
        assert_eq!(entries[0].parent_uuid.as_deref(), Some("parent-1"));
        assert_eq!(entries[0].session_id.as_deref(), Some("sess-1"));
        assert_eq!(entries[0].timestamp_millis(), 1700000000);
        assert_eq!(entries[0].entry_type.as_deref(), Some("user"));
    }

    #[test]
    fn roundtrip() {
        let input = b"{\"uuid\":\"a\",\"type\":\"user\",\"timestamp\":100}\n{\"uuid\":\"b\",\"type\":\"assistant\",\"timestamp\":200}\n";
        let entries = parse_jsonl(input).unwrap();
        let output = entries_to_jsonl(&entries).unwrap();
        let reparsed = parse_jsonl(&output).unwrap();
        assert_eq!(entries.len(), reparsed.len());
        assert_eq!(entries[0].uuid, reparsed[0].uuid);
        assert_eq!(entries[1].uuid, reparsed[1].uuid);
    }

    #[test]
    fn content_hash_differs() {
        let a: ConversationEntry =
            serde_json::from_str(r#"{"uuid":"a","type":"user","timestamp":100}"#).unwrap();
        let b: ConversationEntry =
            serde_json::from_str(r#"{"uuid":"a","type":"user","timestamp":200}"#).unwrap();
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn content_hash_same() {
        let a: ConversationEntry =
            serde_json::from_str(r#"{"uuid":"x","type":"user","timestamp":100}"#).unwrap();
        let b: ConversationEntry =
            serde_json::from_str(r#"{"uuid":"x","type":"user","timestamp":100}"#).unwrap();
        assert_eq!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn flatten_preserves_unknown_fields() {
        let input = r#"{"uuid":"a","type":"user","custom_field":"value","nested":{"key":1}}"#;
        let entry: ConversationEntry = serde_json::from_str(input).unwrap();
        let serialized = serde_json::to_string(&entry).unwrap();
        assert!(serialized.contains("custom_field"));
        assert!(serialized.contains("nested"));
    }
}
