use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Value, json};

use super::super::tools::to_pi_tool_name;
use super::super::{ContentBlock, ConvertedMessage, ConvertedSession, MessageRole, jsonl};
use super::{PROVENANCE_TYPE, STASH_TYPE, encode_project_dir, sessions_dir};

/// Write a ConvertedSession as a pi JSONL file under the project's folder.
pub fn write_session(session: &ConvertedSession) -> Result<PathBuf> {
    anyhow::ensure!(
        session.project_dir.is_absolute(),
        "project directory '{}' is not an absolute path",
        session.project_dir.display()
    );
    let dir = sessions_dir()?.join(encode_project_dir(&session.project_dir));
    fs::create_dir_all(&dir)?;

    // Date the session by its first message so pi's session picker sorts it
    // with the conversations it actually happened alongside.
    let started_ms = session
        .messages
        .first()
        .map(|m| m.timestamp_ms)
        .filter(|ms| *ms > 0)
        .unwrap_or_else(|| u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0));
    let started = jsonl::iso_millis(started_ms);
    let session_uuid = uuid::Uuid::new_v4().to_string();
    let filename = format!(
        "{}_{}.jsonl",
        started.replace([':', '.'], "-"),
        session_uuid
    );
    let output_path = dir.join(filename);

    let mut tree = TreeWriter::default();
    tree.lines.push(json!({
        "type": "session",
        "version": 3,
        "id": session_uuid,
        "timestamp": started,
        "cwd": session.project_dir.to_string_lossy(),
    }));

    let origin = session.origin_key();
    let now = jsonl::iso_millis(u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0));
    tree.push(json!({
        "type": "custom",
        "customType": PROVENANCE_TYPE,
        "timestamp": now,
        "data": {
            "source": origin.tool.to_string(),
            "sourceId": origin.id,
            "convertedAt": now,
        },
    }));
    // Written before the messages so the last message stays the leaf pi resumes from.
    if !session.stash.is_empty() {
        tree.push(json!({
            "type": "custom",
            "customType": STASH_TYPE,
            "timestamp": now,
            "data": { "version": 1, "entries": session.stash },
        }));
    }
    // pi shows this name in its session picker instead of the first prompt.
    tree.push(json!({
        "type": "session_info",
        "timestamp": now,
        "name": session.title.replace(['\r', '\n'], " ").trim(),
    }));

    // Claude results don't record the tool name, but pi's do.
    let call_names: HashMap<&str, &str> = session
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolCall { id, name, .. } => Some((id.as_str(), name.as_str())),
            _ => None,
        })
        .collect();

    for msg in &session.messages {
        let entry = message_entry(msg, session, &call_names);
        tree.push(json!({
            "type": "message",
            "timestamp": jsonl::iso_millis(msg.timestamp_ms),
            "message": entry,
        }));
    }

    jsonl::write_atomic(&output_path, &tree.lines)?;
    Ok(output_path)
}

fn message_entry(
    msg: &ConvertedMessage,
    session: &ConvertedSession,
    call_names: &HashMap<&str, &str>,
) -> Value {
    match msg.role {
        MessageRole::User => json!({
            "role": "user",
            "content": [{ "type": "text", "text": msg.joined_text() }],
            "timestamp": msg.timestamp_ms,
        }),
        MessageRole::Assistant => {
            let mut has_tools = false;
            let content: Vec<Value> = msg
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(json!({ "type": "text", "text": text })),
                    ContentBlock::ToolCall { id, name, input } => {
                        has_tools = true;
                        Some(json!({
                            "type": "toolCall",
                            "id": id,
                            "name": to_pi_tool_name(name),
                            "arguments": input,
                        }))
                    }
                    ContentBlock::ToolResult { .. } => None,
                })
                .collect();
            // pi's footer sums `usage` over every assistant message without a
            // guard, so a missing object crashes the TUI on open.
            json!({
                "role": "assistant",
                "content": content,
                "stopReason": if has_tools { "toolUse" } else { "stop" },
                "timestamp": msg.timestamp_ms,
                "model": session.model.as_deref().unwrap_or("unknown"),
                "provider": session.provider.as_deref().unwrap_or("unknown"),
                "usage": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
                },
            })
        }
        MessageRole::ToolResult => {
            let (call_id, output, is_error) = msg.result_block().unwrap_or(("", "", false));
            let name = msg
                .tool_name
                .as_deref()
                .or_else(|| call_names.get(call_id).copied())
                .map(to_pi_tool_name)
                .unwrap_or_default();
            json!({
                "role": "toolResult",
                "toolCallId": call_id,
                "toolName": name,
                "content": [{ "type": "text", "text": output }],
                "isError": is_error,
                "timestamp": msg.timestamp_ms,
            })
        }
    }
}

/// pi sessions are a tree of entries linked by 8-hex-char ids; clync writes
/// a single linear branch starting from a null parent.
#[derive(Default)]
struct TreeWriter {
    lines: Vec<Value>,
    parent: Option<String>,
    used: HashSet<String>,
}

impl TreeWriter {
    fn push(&mut self, mut entry: Value) {
        let id = loop {
            let candidate = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
            if self.used.insert(candidate.clone()) {
                break candidate;
            }
        };
        entry["id"] = Value::String(id.clone());
        entry["parentId"] = self.parent.clone().map_or(Value::Null, Value::String);
        self.lines.push(entry);
        self.parent = Some(id);
    }
}
