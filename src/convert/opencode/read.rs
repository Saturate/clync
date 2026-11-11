use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::params;
use serde_json::Value;

use super::super::tools::normalize_tool_name;
use super::super::{
    ContentBlock, ConvertedMessage, ConvertedSession, MessageRole, SourceTool, TokenUsage, jsonl,
    thinking_stash, title_from_messages,
};
use super::{metadata_origin, open_db};

/// Message types with no conversation content.
const SKIPPED_MESSAGE_TYPES: &[&str] = &["idle"];

/// Read an opencode v2 session into the IR.
pub fn read_session(session_id: &str) -> Result<ConvertedSession> {
    let conn = open_db()?;
    let row = conn
        .query_row(
            "SELECT title, directory, model, metadata, tokens_input, tokens_output,
                    tokens_reasoning, tokens_cache_read, tokens_cache_write
             FROM session_v2 WHERE id = ?1",
            params![session_id],
            |row| {
                Ok(SessionRow {
                    title: row.get(0)?,
                    directory: row.get(1)?,
                    model: row.get(2)?,
                    metadata: row.get(3)?,
                    tokens: [
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ],
                })
            },
        )
        .with_context(|| format!("opencode session {} not found", session_id))?;

    let mut stmt = conn.prepare(
        "SELECT type, data, time_created FROM session_message
         WHERE session_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;

    let mut reader = MessageReader::default();
    for row in rows {
        let (msg_type, data, time_created) = row?;
        match serde_json::from_str::<Value>(&data) {
            Ok(data) => reader.ingest(&msg_type, data, u64::try_from(time_created).unwrap_or(0)),
            Err(e) => eprintln!("warning: skipping unparseable opencode message: {}", e),
        }
    }

    let (model, provider) = parse_model(row.model.as_deref());
    let metadata: Option<Value> = row
        .metadata
        .as_deref()
        .and_then(|m| serde_json::from_str(m).ok());
    let carried_stash = metadata
        .as_ref()
        .and_then(|m| m.get("clync_stash"))
        .and_then(|s| s.as_array())
        .cloned();
    let [input, output, reasoning, cache_read, cache_write] =
        row.tokens.map(|t| u64::try_from(t).unwrap_or(0));
    let has_tokens = input > 0 || output > 0;
    let title = row
        .title
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| title_from_messages(&reader.messages));

    Ok(ConvertedSession {
        source_id: session_id.to_string(),
        source_tool: SourceTool::Opencode,
        origin: row.metadata.as_deref().and_then(metadata_origin),
        title,
        project_dir: PathBuf::from(&row.directory),
        messages: reader.messages,
        model,
        provider,
        tokens: has_tokens.then_some(TokenUsage {
            input,
            output,
            cache_read,
            cache_write,
            reasoning,
        }),
        // A converted session's stash belongs to the original source.
        stash: carried_stash.unwrap_or(reader.stash),
    })
}

struct SessionRow {
    title: Option<String>,
    directory: String,
    model: Option<String>,
    metadata: Option<String>,
    tokens: [i64; 5],
}

fn parse_model(model_json: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(v) = model_json.and_then(|s| serde_json::from_str::<Value>(s).ok()) else {
        return (None, None);
    };
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    (get("id"), get("providerID"))
}

#[derive(Default)]
struct MessageReader {
    messages: Vec<ConvertedMessage>,
    stash: Vec<Value>,
}

impl MessageReader {
    fn ingest(&mut self, msg_type: &str, data: Value, time_created: u64) {
        match msg_type {
            "user" => {
                if let Some(files) = data.get("files").and_then(|f| f.as_array())
                    && !files.is_empty()
                {
                    self.stash.push(serde_json::json!({
                        "kind": "files", "timestamp_ms": time_created, "files": files,
                    }));
                }
                let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if !text.trim().is_empty() {
                    self.messages.push(ConvertedMessage::text(
                        MessageRole::User,
                        time_created,
                        text.to_string(),
                    ));
                }
            }
            "assistant" => self.ingest_assistant(&data, time_created),
            t if SKIPPED_MESSAGE_TYPES.contains(&t) => {}
            // compaction, synthetic and system rows carry content the IR can't express
            _ => self.stash.push(serde_json::json!({
                "kind": "message",
                "type": msg_type,
                "timestamp_ms": time_created,
                "data": data,
            })),
        }
    }

    fn ingest_assistant(&mut self, data: &Value, time_created: u64) {
        let mut blocks = Vec::new();
        let mut results = Vec::new();
        for block in data
            .get("content")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    if !text.trim().is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                }
                Some("tool") => {
                    let id = str_field(block, "id");
                    let name = normalize_tool_name(&str_field(block, "name"));
                    let state = block.get("state");
                    blocks.push(ContentBlock::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: state
                            .and_then(|s| s.get("input"))
                            .cloned()
                            .unwrap_or(Value::Null),
                    });
                    let status = state.and_then(|s| s.get("status")).and_then(|v| v.as_str());
                    let completed = block
                        .get("time")
                        .and_then(|t| t.get("completed"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(time_created);
                    // Only completed calls succeeded; pending or running ones were
                    // cut off and never produced output.
                    let finished = matches!(status, Some("completed" | "error"));
                    let output = if finished {
                        tool_output(state)
                    } else {
                        "No result recorded".to_string()
                    };
                    results.push(ConvertedMessage::tool_result(
                        completed,
                        id,
                        Some(name),
                        output,
                        status != Some("completed"),
                    ));
                }
                Some("reasoning") => {
                    let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    if !text.is_empty() {
                        self.stash.push(thinking_stash(time_created, text));
                    }
                }
                _ => self.stash.push(serde_json::json!({
                    "kind": "block", "timestamp_ms": time_created, "block": block,
                })),
            }
        }
        if !blocks.is_empty() {
            self.messages.push(ConvertedMessage {
                role: MessageRole::Assistant,
                timestamp_ms: time_created,
                content: blocks,
                tool_call_id: None,
                tool_name: None,
            });
        }
        self.messages.extend(results);
    }
}

/// Tool output lives in `state.content` when the call completed and in
/// `state.error.message` when it failed.
fn tool_output(state: Option<&Value>) -> String {
    let Some(state) = state else {
        return String::new();
    };
    let content = jsonl::text_blocks(state.get("content"));
    if !content.is_empty() {
        return content;
    }
    if let Some(msg) = state
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|v| v.as_str())
    {
        return msg.to_string();
    }
    state
        .get("metadata")
        .and_then(|m| m.get("output"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}
