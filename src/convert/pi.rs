//! pi session reader/writer (JSONL <-> IR).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::tools::{normalize_tool_name, to_pi_tool_name};
use super::{
    ContentBlock, ConvertedMessage, ConvertedSession, MessageRole, SourceTool, TokenUsage,
};

/// Encode a project directory for pi's storage format.
/// `/Users/alkj/code/github/clync` -> `--Users-alkj-code-github-clync--`
pub fn encode_project_dir(path: &Path) -> String {
    let s = path.to_string_lossy();
    let inner = s.replace('/', "-");
    format!("-{}-", inner)
}

/// Decode a pi-encoded project directory back to a real path.
/// `--Users-alkj-code-github-clync--` -> `/Users/alkj/code/github/clync`
pub fn decode_project_dir(encoded: &str) -> PathBuf {
    let trimmed = encoded.trim_matches('-');
    let path_str = trimmed.replace('-', "/");
    PathBuf::from(format!("/{}", path_str))
}

/// List all pi sessions, optionally filtered by project.
pub fn list_sessions(project_filter: Option<&Path>) -> Result<Vec<PiSessionInfo>> {
    let pi_dir = dirs::home_dir()
        .context("cannot find home directory")?
        .join(".pi")
        .join("agent")
        .join("sessions");

    if !pi_dir.exists() {
        return Ok(vec![]);
    }

    let mut sessions = Vec::new();

    for project_entry in fs::read_dir(&pi_dir)? {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }

        let dir_name = project_entry.file_name().to_string_lossy().to_string();
        let decoded_dir = decode_project_dir(&dir_name);

        if let Some(filter) = project_filter
            && decoded_dir != filter {
                continue;
            }

        for session_entry in fs::read_dir(&project_path)? {
            let session_entry = session_entry?;
            let session_path = session_entry.path();
            if session_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let filename = session_entry.file_name().to_string_lossy().to_string();

            // Quick scan for session header
            let info = scan_session_header(&session_path, &decoded_dir);

            sessions.push(info.unwrap_or(PiSessionInfo {
                id: filename.clone(),
                path: session_path,
                project_dir: decoded_dir.clone(),
                title: filename,
                timestamp_ms: 0,
            }));
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp_ms));
    Ok(sessions)
}

fn scan_session_header(path: &Path, project_dir: &Path) -> Result<PiSessionInfo> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    let mut session_id = String::new();
    let mut timestamp_ms: u64 = 0;
    let mut first_user_text = String::new();

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let entry: Value = serde_json::from_str(&line)?;

        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if entry_type == "session" {
            session_id = entry
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(ts) = entry.get("timestamp").and_then(|v| v.as_str())
                && let Ok(dt) = ts.parse::<chrono::DateTime<chrono::Utc>>() {
                    timestamp_ms = dt.timestamp_millis() as u64;
                }
        }

        if entry_type == "message" && first_user_text.is_empty()
            && let Some(role) = entry
                .get("message")
                .and_then(|m| m.get("role"))
                .and_then(|r| r.as_str())
                && role == "user" {
                    first_user_text = entry
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|b| b.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect();
                }

        // Stop after we have both header and first user message
        if !session_id.is_empty() && !first_user_text.is_empty() {
            break;
        }
    }

    let title = if !first_user_text.is_empty() {
        first_user_text
    } else {
        "Untitled session".to_string()
    };

    Ok(PiSessionInfo {
        id: session_id,
        path: path.to_path_buf(),
        project_dir: project_dir.to_path_buf(),
        title,
        timestamp_ms,
    })
}

/// Read a pi session JSONL file into the IR.
#[allow(clippy::too_many_lines)]
pub fn read_session(path: &Path) -> Result<ConvertedSession> {
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut session_id = String::new();
    let mut project_dir = PathBuf::new();
    let mut model: Option<String> = None;
    let mut provider: Option<String> = None;
    let mut total_usage = TokenUsage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reasoning: 0,
    };
    let mut has_usage = false;
    let mut messages = Vec::new();
    let mut stash: Vec<Value> = Vec::new();

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

        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match entry_type {
            "session" => {
                session_id = entry
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                project_dir = entry
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_default();
            }
            "message" => {
                let msg = entry.get("message").cloned().unwrap_or(Value::Null);
                let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                let ts = parse_pi_timestamp(&entry);

                match role {
                    "user" => {
                        let content = msg.get("content").and_then(|c| c.as_array());
                        let text = content
                            .map(|blocks| {
                                blocks
                                    .iter()
                                    .filter_map(|b| {
                                        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                                            b.get("text").and_then(|t| t.as_str())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .unwrap_or_default();

                        if !text.is_empty() {
                            messages.push(ConvertedMessage {
                                role: MessageRole::User,
                                timestamp_ms: ts,
                                content: vec![ContentBlock::Text { text }],
                                tool_call_id: None,
                                tool_name: None,
                                is_error: false,
                                raw: Some(entry.clone()),
                            });
                        }
                    }
                    "assistant" => {
                        // Capture model/provider from first assistant message
                        if model.is_none() {
                            model = msg
                                .get("model")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            provider = msg
                                .get("provider")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }

                        // Accumulate usage
                        if let Some(usage) = msg.get("usage") {
                            has_usage = true;
                            total_usage.input +=
                                usage.get("input").and_then(|v| v.as_u64()).unwrap_or(0);
                            total_usage.output +=
                                usage.get("output").and_then(|v| v.as_u64()).unwrap_or(0);
                            total_usage.reasoning +=
                                usage.get("reasoning").and_then(|v| v.as_u64()).unwrap_or(0);
                        }

                        let content = msg.get("content").and_then(|c| c.as_array());
                        let mut ir_blocks = Vec::new();

                        if let Some(blocks) = content {
                            for block in blocks {
                                let block_type =
                                    block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                match block_type {
                                    "text" => {
                                        let text = block
                                            .get("text")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        if !text.is_empty() {
                                            ir_blocks.push(ContentBlock::Text { text });
                                        }
                                    }
                                    "toolCall" => {
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
                                        // pi uses "arguments" not "input"
                                        let input =
                                            block.get("arguments").cloned().unwrap_or(Value::Null);

                                        ir_blocks.push(ContentBlock::ToolCall {
                                            id,
                                            name: normalize_tool_name(&name),
                                            input,
                                        });
                                    }
                                    "thinking" => {
                                        // Stash thinking blocks with context
                                        let msg_id =
                                            entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                        stash.push(serde_json::json!({
                                            "kind": "thinking_block",
                                            "message_id": msg_id,
                                            "timestamp_ms": ts,
                                            "block": block,
                                        }));
                                    }
                                    _ => {}
                                }
                            }
                        }

                        if !ir_blocks.is_empty() {
                            messages.push(ConvertedMessage {
                                role: MessageRole::Assistant,
                                timestamp_ms: ts,
                                content: ir_blocks,
                                tool_call_id: None,
                                tool_name: None,
                                is_error: false,
                                raw: Some(entry.clone()),
                            });
                        }
                    }
                    "toolResult" => {
                        let call_id = msg
                            .get("toolCallId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let tool_name = msg
                            .get("toolName")
                            .and_then(|v| v.as_str())
                            .map(normalize_tool_name);
                        let is_error = msg
                            .get("isError")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        let output = msg
                            .get("content")
                            .and_then(|c| c.as_array())
                            .map(|blocks| {
                                blocks
                                    .iter()
                                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .unwrap_or_default();

                        messages.push(ConvertedMessage {
                            role: MessageRole::ToolResult,
                            timestamp_ms: ts,
                            content: vec![ContentBlock::ToolResult {
                                call_id: call_id.clone(),
                                output,
                                is_error,
                            }],
                            tool_call_id: Some(call_id),
                            tool_name,
                            is_error,
                            raw: Some(entry.clone()),
                        });
                    }
                    _ => {}
                }
            }
            _ => {
                // Stash non-convertible entries (model_change, thinking_level_change,
                // compaction, clync-provenance, etc.) for lossless round-trips
                stash.push(entry);
            }
        }
    }

    let title = messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .and_then(|m| m.content.first())
        .and_then(|b| match b {
            ContentBlock::Text { text } => Some(text.chars().take(80).collect()),
            _ => None,
        })
        .unwrap_or_else(|| "Untitled session".to_string());

    Ok(ConvertedSession {
        source_id: session_id,
        source_tool: SourceTool::Pi,
        title,
        project_dir,
        messages,
        model,
        provider,
        tokens: if has_usage { Some(total_usage) } else { None },
        stash,
    })
}

fn parse_pi_timestamp(entry: &Value) -> u64 {
    // Try ISO string timestamp
    if let Some(ts_str) = entry.get("timestamp").and_then(|v| v.as_str())
        && let Ok(dt) = ts_str.parse::<chrono::DateTime<chrono::Utc>>() {
            return dt.timestamp_millis() as u64;
        }
    // Try message.timestamp (epoch ms)
    if let Some(ts) = entry
        .get("message")
        .and_then(|m| m.get("timestamp"))
        .and_then(|v| v.as_u64())
    {
        return ts;
    }
    0
}

/// Write a ConvertedSession as a pi JSONL file.
pub fn write_session(session: &ConvertedSession, output_dir: Option<&Path>) -> Result<PathBuf> {
    let dir = if let Some(d) = output_dir {
        d.to_path_buf()
    } else {
        let pi_dir = dirs::home_dir()
            .context("cannot find home directory")?
            .join(".pi")
            .join("agent")
            .join("sessions");
        let encoded = encode_project_dir(&session.project_dir);
        pi_dir.join(encoded)
    };

    fs::create_dir_all(&dir)?;

    let session_uuid = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let filename = format!(
        "{}_{}.jsonl",
        now.format("%Y-%m-%dT%H-%M-%S-%3fZ"),
        session_uuid
    );
    let output_path = dir.join(&filename);

    let mut file = fs::File::create(&output_path)?;

    // Write session header
    write_jsonl_line(
        &mut file,
        &serde_json::json!({
            "type": "session",
            "version": 3,
            "id": session_uuid,
            "timestamp": now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "cwd": session.project_dir.to_string_lossy(),
        }),
    )?;

    // Write provenance
    write_jsonl_line(
        &mut file,
        &serde_json::json!({
            "type": "clync-provenance",
            "source": session.source_tool.to_string(),
            "sourceId": session.source_id,
            "convertedAt": now.to_rfc3339(),
        }),
    )?;

    // Write messages
    let mut prev_id = session_uuid[..8].to_string();

    for msg in &session.messages {
        let ts = chrono::DateTime::from_timestamp_millis(msg.timestamp_ms as i64)
            .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .unwrap_or_default();
        let entry_id = generate_short_hex_id();

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

                write_jsonl_line(
                    &mut file,
                    &serde_json::json!({
                        "type": "message",
                        "id": entry_id,
                        "parentId": prev_id,
                        "timestamp": ts,
                        "message": {
                            "role": "user",
                            "content": [{"type": "text", "text": text}],
                            "timestamp": msg.timestamp_ms,
                        },
                    }),
                )?;
                prev_id = entry_id;
            }
            MessageRole::Assistant => {
                let mut content_blocks = Vec::new();

                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            content_blocks.push(serde_json::json!({
                                "type": "text",
                                "text": text,
                            }));
                        }
                        ContentBlock::ToolCall { id, name, input } => {
                            let pi_name = to_pi_tool_name(name);
                            content_blocks.push(serde_json::json!({
                                "type": "toolCall",
                                "id": id,
                                "name": pi_name,
                                "arguments": input,
                            }));
                        }
                        ContentBlock::ToolResult { .. } => {}
                    }
                }

                let mut msg_data = serde_json::json!({
                    "role": "assistant",
                    "content": content_blocks,
                });

                if let Some(ref m) = session.model {
                    msg_data["model"] = Value::String(m.clone());
                }
                if let Some(ref p) = session.provider {
                    msg_data["provider"] = Value::String(p.clone());
                }

                write_jsonl_line(
                    &mut file,
                    &serde_json::json!({
                        "type": "message",
                        "id": entry_id,
                        "parentId": prev_id,
                        "timestamp": ts,
                        "message": msg_data,
                    }),
                )?;
                prev_id = entry_id;
            }
            MessageRole::ToolResult => {
                let (call_id, output, is_error) = msg
                    .content
                    .first()
                    .and_then(|b| match b {
                        ContentBlock::ToolResult {
                            call_id,
                            output,
                            is_error,
                        } => Some((call_id.as_str(), output.as_str(), *is_error)),
                        _ => None,
                    })
                    .unwrap_or(("", "", false));

                let tool_name = msg
                    .tool_name
                    .as_ref()
                    .map(|n| to_pi_tool_name(n))
                    .unwrap_or_default();

                write_jsonl_line(
                    &mut file,
                    &serde_json::json!({
                        "type": "message",
                        "id": entry_id,
                        "parentId": prev_id,
                        "timestamp": ts,
                        "message": {
                            "role": "toolResult",
                            "toolCallId": call_id,
                            "toolName": tool_name,
                            "content": [{"type": "text", "text": output}],
                            "isError": is_error,
                        },
                    }),
                )?;
                prev_id = entry_id;
            }
        }
    }

    // Write stash for lossless round-trips (pi ignores unknown entry types)
    if !session.stash.is_empty() {
        write_jsonl_line(
            &mut file,
            &serde_json::json!({
                "type": "clync-stash",
                "version": 1,
                "source": session.source_tool.to_string(),
                "sourceId": session.source_id,
                "entries": session.stash,
            }),
        )?;
    }

    Ok(output_path)
}

fn write_jsonl_line(file: &mut fs::File, value: &Value) -> Result<()> {
    let line = serde_json::to_string(value)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

fn generate_short_hex_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:08x}", ts as u32)
}

/// Check if a pi session was already converted from this source.
pub fn find_provenance(path: &Path) -> Result<Option<ProvenanceInfo>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    for line in reader.lines().take(5) {
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

/// Build an index of pi sessions by their provenance source ID.
pub fn provenance_index(project_dir: &Path) -> Result<HashMap<String, PathBuf>> {
    let pi_dir = dirs::home_dir()
        .context("cannot find home directory")?
        .join(".pi")
        .join("agent")
        .join("sessions");
    let encoded = encode_project_dir(project_dir);
    let dir = pi_dir.join(encoded);

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

#[derive(Debug)]
pub struct PiSessionInfo {
    pub id: String,
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
