//! Claude Code session reader/writer (JSONL <-> IR).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::{
    ContentBlock, ConvertedMessage, ConvertedSession, MessageRole, SourceTool, TokenUsage,
};
use super::tools::normalize_tool_name;

/// Encode a project directory path for Claude's storage format.
/// `/Users/alkj/code/github/clync` -> `-Users-alkj-code-github-clync`
pub fn encode_project_dir(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.replace('/', "-")
}

/// Decode a Claude-encoded project directory back to a real path.
/// `-Users-alkj-code-github-clync` -> `/Users/alkj/code/github/clync`
pub fn decode_project_dir(encoded: &str) -> PathBuf {
    // The encoding replaces '/' with '-', but the leading '-' represents the root '/'
    let path_str = encoded.replace('-', "/");
    PathBuf::from(path_str)
}

/// List all Claude sessions, optionally filtered by project.
pub fn list_sessions(project_filter: Option<&Path>) -> Result<Vec<ClaudeSessionInfo>> {
    let claude_dir = dirs::home_dir()
        .context("cannot find home directory")?
        .join(".claude")
        .join("projects");

    if !claude_dir.exists() {
        return Ok(vec![]);
    }

    let mut sessions = Vec::new();

    for project_entry in fs::read_dir(&claude_dir)? {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }

        let dir_name = project_entry.file_name().to_string_lossy().to_string();
        let decoded_dir = decode_project_dir(&dir_name);

        if let Some(filter) = project_filter {
            if decoded_dir != filter {
                continue;
            }
        }

        for session_entry in fs::read_dir(&project_path)? {
            let session_entry = session_entry?;
            let session_path = session_entry.path();
            if session_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let uuid = session_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            // Quick scan for title and first timestamp
            let (title, timestamp) = scan_session_metadata(&session_path).unwrap_or_default();

            sessions.push(ClaudeSessionInfo {
                uuid: uuid.clone(),
                path: session_path,
                project_dir: decoded_dir.clone(),
                title,
                timestamp_ms: timestamp,
            });
        }
    }

    sessions.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms));
    Ok(sessions)
}

/// Quick metadata scan without full parse.
fn scan_session_metadata(path: &Path) -> Result<(String, u64)> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    let mut title = String::new();
    let mut earliest_ts: u64 = u64::MAX;

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let entry: Value = serde_json::from_str(&line)?;

        if entry.get("aiTitle").is_some() {
            title = entry["aiTitle"].as_str().unwrap_or("").to_string();
        }

        if let Some(ts) = parse_timestamp(&entry) {
            if ts < earliest_ts {
                earliest_ts = ts;
            }
        }
    }

    if title.is_empty() {
        title = "Untitled session".to_string();
    }
    if earliest_ts == u64::MAX {
        earliest_ts = 0;
    }

    Ok((title, earliest_ts))
}

/// Read a Claude session JSONL file into the IR.
pub fn read_session(path: &Path) -> Result<ConvertedSession> {
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);

    let uuid = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Derive project dir from the parent directory name
    let project_dir = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|n| decode_project_dir(n))
        .unwrap_or_default();

    let mut title = String::new();
    let mut model: Option<String> = None;
    let mut total_usage = TokenUsage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reasoning: 0,
    };
    let mut has_usage = false;

    // Collect conversation entries (user + assistant only)
    let mut entries: Vec<(u64, Value)> = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let entry: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("warning: skipping unparseable line: {}", e);
                continue;
            }
        };

        // Extract title from ai-title entries
        if let Some(t) = entry.get("aiTitle").and_then(|v| v.as_str()) {
            title = t.to_string();
        }

        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");

        // Skip sidechains
        if entry.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }

        match entry_type {
            "user" | "assistant" => {
                let ts = parse_timestamp(&entry).unwrap_or(0);

                // Accumulate token usage from assistant entries
                if entry_type == "assistant" {
                    if let Some(usage) = entry
                        .get("message")
                        .and_then(|m| m.get("usage"))
                    {
                        has_usage = true;
                        total_usage.input +=
                            usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                        total_usage.output +=
                            usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                        total_usage.cache_read += usage
                            .get("cache_read_input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        total_usage.cache_write += usage
                            .get("cache_creation_input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                    }

                    // Capture model from first assistant entry
                    if model.is_none() {
                        model = entry
                            .get("message")
                            .and_then(|m| m.get("model"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                }

                entries.push((ts, entry));
            }
            _ => {} // Skip all other entry types
        }
    }

    // Sort by timestamp
    entries.sort_by_key(|(ts, _)| *ts);

    // Convert entries to IR messages, merging consecutive assistant entries
    let messages = merge_and_convert_entries(&entries)?;

    // Apply title fallback
    if title.is_empty() {
        title = messages
            .iter()
            .find(|m| m.role == MessageRole::User)
            .and_then(|m| {
                m.content.first().and_then(|b| match b {
                    ContentBlock::Text { text } => {
                        let truncated: String = text.chars().take(80).collect();
                        Some(truncated)
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(|| "Untitled session".to_string());
    }

    Ok(ConvertedSession {
        source_id: uuid,
        source_tool: SourceTool::Claude,
        title,
        project_dir,
        messages,
        model,
        provider: Some("anthropic".to_string()),
        tokens: if has_usage { Some(total_usage) } else { None },
    })
}

/// Merge consecutive assistant entries and convert all entries to IR messages.
fn merge_and_convert_entries(entries: &[(u64, Value)]) -> Result<Vec<ConvertedMessage>> {
    let mut messages: Vec<ConvertedMessage> = Vec::new();
    let mut pending_assistant_blocks: Vec<ContentBlock> = Vec::new();
    let mut pending_assistant_ts: u64 = 0;

    for (ts, entry) in entries {
        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let message = entry.get("message").cloned().unwrap_or(Value::Null);
        let content = message.get("content");

        match entry_type {
            "user" => {
                // Flush any pending assistant blocks
                flush_assistant(&mut messages, &mut pending_assistant_blocks, pending_assistant_ts);

                // Check if this is a tool_result user entry
                if let Some(Value::Array(blocks)) = content {
                    if blocks
                        .first()
                        .and_then(|b| b.get("type"))
                        .and_then(|t| t.as_str())
                        == Some("tool_result")
                    {
                        // Tool result entries: one IR message per tool_result block
                        for block in blocks {
                            let call_id = block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let output = extract_tool_result_content(block);
                            let is_error = block
                                .get("is_error")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            messages.push(ConvertedMessage {
                                role: MessageRole::ToolResult,
                                timestamp_ms: *ts,
                                content: vec![ContentBlock::ToolResult {
                                    call_id: call_id.clone(),
                                    output,
                                    is_error,
                                }],
                                tool_call_id: Some(call_id),
                                tool_name: None,
                                is_error,
                            });
                        }
                        continue;
                    }
                }

                // Plain text user message
                let text = match content {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Array(blocks)) => blocks
                        .iter()
                        .filter_map(|b| {
                            if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                                b.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };

                if !text.is_empty() {
                    messages.push(ConvertedMessage {
                        role: MessageRole::User,
                        timestamp_ms: *ts,
                        content: vec![ContentBlock::Text { text }],
                        tool_call_id: None,
                        tool_name: None,
                        is_error: false,
                    });
                }
            }
            "assistant" => {
                // Each assistant entry has one content block; accumulate them
                if let Some(Value::Array(blocks)) = content {
                    for block in blocks {
                        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match block_type {
                            "text" => {
                                let text = block
                                    .get("text")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                if !text.is_empty() {
                                    if pending_assistant_blocks.is_empty() {
                                        pending_assistant_ts = *ts;
                                    }
                                    pending_assistant_blocks
                                        .push(ContentBlock::Text { text });
                                }
                            }
                            "tool_use" => {
                                let id = block
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let name = block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let input = block.get("input").cloned().unwrap_or(Value::Null);

                                if pending_assistant_blocks.is_empty() {
                                    pending_assistant_ts = *ts;
                                }
                                pending_assistant_blocks.push(ContentBlock::ToolCall {
                                    id,
                                    name: normalize_tool_name(&name),
                                    input,
                                });
                            }
                            "thinking" => {
                                // Drop thinking blocks (not universally displayable)
                            }
                            _ => {
                                // Unknown block type, skip
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Flush any remaining assistant blocks
    flush_assistant(&mut messages, &mut pending_assistant_blocks, pending_assistant_ts);

    Ok(messages)
}

fn flush_assistant(
    messages: &mut Vec<ConvertedMessage>,
    blocks: &mut Vec<ContentBlock>,
    ts: u64,
) {
    if blocks.is_empty() {
        return;
    }
    messages.push(ConvertedMessage {
        role: MessageRole::Assistant,
        timestamp_ms: ts,
        content: blocks.drain(..).collect(),
        tool_call_id: None,
        tool_name: None,
        is_error: false,
    });
}

fn extract_tool_result_content(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Parse a timestamp from a Claude session entry. Returns epoch milliseconds.
fn parse_timestamp(entry: &Value) -> Option<u64> {
    // Try ISO string timestamp field
    if let Some(ts_str) = entry.get("timestamp").and_then(|v| v.as_str()) {
        if let Ok(dt) = ts_str.parse::<chrono::DateTime<chrono::Utc>>() {
            return Some(dt.timestamp_millis() as u64);
        }
    }
    // Try message.timestamp (epoch ms)
    if let Some(ts) = entry
        .get("message")
        .and_then(|m| m.get("timestamp"))
        .and_then(|v| v.as_u64())
    {
        return Some(ts);
    }
    None
}

/// Write a ConvertedSession as a Claude JSONL file.
pub fn write_session(session: &ConvertedSession, output_dir: Option<&Path>) -> Result<PathBuf> {
    let dir = if let Some(d) = output_dir {
        d.to_path_buf()
    } else {
        let claude_dir = dirs::home_dir()
            .context("cannot find home directory")?
            .join(".claude")
            .join("projects");
        let encoded = encode_project_dir(&session.project_dir);
        claude_dir.join(encoded)
    };

    fs::create_dir_all(&dir)?;

    let session_uuid = uuid::Uuid::new_v4().to_string();
    let output_path = dir.join(format!("{}.jsonl", session_uuid));

    let mut file = fs::File::create(&output_path)?;

    // Write metadata entries
    write_jsonl_line(
        &mut file,
        &serde_json::json!({
            "type": "mode",
            "mode": "normal",
            "sessionId": session_uuid,
        }),
    )?;

    write_jsonl_line(
        &mut file,
        &serde_json::json!({
            "type": "ai-title",
            "aiTitle": session.title,
            "sessionId": session_uuid,
        }),
    )?;

    write_jsonl_line(
        &mut file,
        &serde_json::json!({
            "type": "clync-provenance",
            "source": session.source_tool.to_string(),
            "sourceId": session.source_id,
            "convertedAt": chrono::Utc::now().to_rfc3339(),
        }),
    )?;

    // Write conversation entries. Each content block becomes its own entry.
    let mut prev_uuid = session_uuid.clone();

    for msg in &session.messages {
        let ts = chrono::DateTime::from_timestamp_millis(msg.timestamp_ms as i64)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        match msg.role {
            MessageRole::User => {
                let text = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                let entry_uuid = uuid::Uuid::new_v4().to_string();
                write_jsonl_line(
                    &mut file,
                    &serde_json::json!({
                        "parentUuid": prev_uuid,
                        "isSidechain": false,
                        "type": "user",
                        "uuid": entry_uuid,
                        "timestamp": ts,
                        "message": {
                            "role": "user",
                            "content": text,
                        },
                        "sessionId": session_uuid,
                    }),
                )?;
                prev_uuid = entry_uuid;
            }
            MessageRole::Assistant => {
                // One entry per content block
                for block in &msg.content {
                    let entry_uuid = uuid::Uuid::new_v4().to_string();
                    let (content_block, stop_reason) = match block {
                        ContentBlock::Text { text } => (
                            serde_json::json!([{"type": "text", "text": text}]),
                            "end_turn",
                        ),
                        ContentBlock::ToolCall { id, name, input } => {
                            let claude_name =
                                super::tools::to_claude_tool_name(name);
                            (
                                serde_json::json!([{
                                    "type": "tool_use",
                                    "id": id,
                                    "name": claude_name,
                                    "input": input,
                                }]),
                                "tool_use",
                            )
                        }
                        ContentBlock::ToolResult { .. } => continue,
                    };

                    let mut entry = serde_json::json!({
                        "parentUuid": prev_uuid,
                        "isSidechain": false,
                        "type": "assistant",
                        "uuid": entry_uuid,
                        "timestamp": ts,
                        "message": {
                            "role": "assistant",
                            "content": content_block,
                            "stop_reason": stop_reason,
                        },
                        "sessionId": session_uuid,
                    });

                    // Add model if available
                    if let Some(ref m) = session.model {
                        entry["message"]["model"] = Value::String(m.clone());
                    }

                    write_jsonl_line(&mut file, &entry)?;
                    prev_uuid = entry_uuid;
                }
            }
            MessageRole::ToolResult => {
                // Write as type=user with tool_result content blocks
                let entry_uuid = uuid::Uuid::new_v4().to_string();
                let blocks: Vec<Value> = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolResult {
                            call_id,
                            output,
                            is_error,
                        } => Some(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": call_id,
                            "content": output,
                            "is_error": is_error,
                        })),
                        _ => None,
                    })
                    .collect();

                if !blocks.is_empty() {
                    write_jsonl_line(
                        &mut file,
                        &serde_json::json!({
                            "parentUuid": prev_uuid,
                            "isSidechain": false,
                            "type": "user",
                            "uuid": entry_uuid,
                            "timestamp": ts,
                            "message": {
                                "role": "user",
                                "content": blocks,
                            },
                            "sessionId": session_uuid,
                        }),
                    )?;
                    prev_uuid = entry_uuid;
                }
            }
        }
    }

    // Write last-prompt so Claude Code knows the leaf
    write_jsonl_line(
        &mut file,
        &serde_json::json!({
            "type": "last-prompt",
            "leafUuid": prev_uuid,
            "sessionId": session_uuid,
        }),
    )?;

    Ok(output_path)
}

fn write_jsonl_line(file: &mut fs::File, value: &Value) -> Result<()> {
    let line = serde_json::to_string(value)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

/// Check if a Claude session was already converted from this source.
pub fn find_provenance(path: &Path) -> Result<Option<ProvenanceInfo>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let entry: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if entry.get("type").and_then(|v| v.as_str()) == Some("clync-provenance") {
            return Ok(Some(ProvenanceInfo {
                source: entry
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                source_id: entry
                    .get("sourceId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            }));
        }
    }
    Ok(None)
}

#[derive(Debug)]
pub struct ClaudeSessionInfo {
    pub uuid: String,
    pub path: PathBuf,
    pub project_dir: PathBuf,
    pub title: String,
    pub timestamp_ms: u64,
}

#[derive(Debug)]
pub struct ProvenanceInfo {
    pub source: String,
    pub source_id: String,
}

/// Build an index of existing Claude sessions by their provenance source ID.
/// Used to check for duplicates before writing.
pub fn provenance_index(project_dir: &Path) -> Result<HashMap<String, PathBuf>> {
    let claude_dir = dirs::home_dir()
        .context("cannot find home directory")?
        .join(".claude")
        .join("projects");
    let encoded = encode_project_dir(project_dir);
    let dir = claude_dir.join(encoded);

    let mut index = HashMap::new();

    if !dir.exists() {
        return Ok(index);
    }

    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Ok(Some(prov)) = find_provenance(&path) {
            index.insert(prov.source_id, path);
        }
    }

    Ok(index)
}
