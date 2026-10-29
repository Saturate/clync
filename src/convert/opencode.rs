//! opencode v2 session reader/writer (SQLite <-> IR).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde_json::Value;

use super::tools::{normalize_tool_name, to_opencode_tool_name};
use super::{
    ContentBlock, ConvertedMessage, ConvertedSession, MessageRole, SourceTool, TokenUsage,
};

/// Open the opencode database.
fn open_db() -> Result<Connection> {
    let db_path = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg).join("opencode").join("opencode.db")
    } else {
        dirs::home_dir()
            .context("cannot find home directory")?
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db")
    };

    Connection::open(&db_path).with_context(|| format!("opening {}", db_path.display()))
}

/// List all opencode v2 sessions.
pub fn list_sessions(project_filter: Option<&Path>) -> Result<Vec<OpencodeSessionInfo>> {
    let conn = open_db()?;

    let mut sessions = Vec::new();
    let query = if project_filter.is_some() {
        "SELECT s.id, s.title, s.directory, s.time_created, s.metadata, p.worktree
         FROM session_v2 s
         JOIN project p ON p.id = s.project_id
         WHERE p.worktree = ?1
         ORDER BY s.time_created DESC"
    } else {
        "SELECT s.id, s.title, s.directory, s.time_created, s.metadata, p.worktree
         FROM session_v2 s
         JOIN project p ON p.id = s.project_id
         ORDER BY s.time_created DESC"
    };

    let mut stmt = conn.prepare(query)?;
    let rows = if let Some(filter) = project_filter {
        stmt.query_map(params![filter.to_string_lossy().as_ref()], row_to_info)?
    } else {
        stmt.query_map([], row_to_info)?
    };

    for row in rows {
        sessions.push(row?);
    }

    Ok(sessions)
}

fn row_to_info(row: &rusqlite::Row) -> rusqlite::Result<OpencodeSessionInfo> {
    let metadata_str: Option<String> = row.get(4)?;
    let provenance = metadata_str.and_then(|s| {
        serde_json::from_str::<Value>(&s).ok().and_then(|v| {
            v.get("clync_source_id")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string())
        })
    });

    Ok(OpencodeSessionInfo {
        id: row.get(0)?,
        title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        directory: row.get(2)?,
        timestamp_ms: row.get(3)?,
        provenance_source_id: provenance,
        worktree: row.get(5)?,
    })
}

/// Read an opencode v2 session into the IR.
#[allow(clippy::too_many_lines)]
pub fn read_session(session_id: &str) -> Result<ConvertedSession> {
    let conn = open_db()?;

    // Get session metadata
    let (
        title,
        directory,
        model_json,
        tokens_input,
        tokens_output,
        tokens_reasoning,
        tokens_cache_read,
        tokens_cache_write,
    ): (
        Option<String>,
        String,
        Option<String>,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = conn
        .query_row(
            "SELECT title, directory, model, tokens_input, tokens_output, tokens_reasoning,
                    tokens_cache_read, tokens_cache_write
             FROM session_v2 WHERE id = ?1",
            params![session_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .with_context(|| format!("session {} not found", session_id))?;

    // Parse model and provider from JSON
    let (model, provider) = model_json
        .as_ref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .map(|v| {
            let m = v
                .get("id")
                .and_then(|id| id.as_str())
                .map(|s| s.to_string());
            let p = v
                .get("providerID")
                .and_then(|p| p.as_str())
                .map(|s| s.to_string());
            (m, p)
        })
        .unwrap_or((None, None));

    // Read session_message rows ordered by seq
    let mut stmt = conn.prepare(
        "SELECT type, data, time_created, seq FROM session_message
         WHERE session_id = ?1
         ORDER BY seq",
    )?;

    let mut messages = Vec::new();
    let mut stash: Vec<Value> = Vec::new();

    let rows = stmt.query_map(params![session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;

    for row in rows {
        let (msg_type, data_str, time_created, seq) = row?;
        let data: Value = match serde_json::from_str(&data_str) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("warning: skipping unparseable message: {}", e);
                continue;
            }
        };

        match msg_type.as_str() {
            "user" => {
                let text = data
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !text.is_empty() {
                    messages.push(ConvertedMessage {
                        role: MessageRole::User,
                        timestamp_ms: time_created,
                        content: vec![ContentBlock::Text { text }],
                        tool_call_id: None,
                        tool_name: None,
                        is_error: false,
                        raw: Some(data.clone()),
                    });
                }
            }
            "assistant" => {
                let content_arr = data.get("content").and_then(|v| v.as_array());
                if let Some(blocks) = content_arr {
                    let mut ir_blocks = Vec::new();
                    let mut tool_results: Vec<(String, ConvertedMessage)> = Vec::new();

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
                                    ir_blocks.push(ContentBlock::Text { text });
                                }
                            }
                            "tool" => {
                                let tool_id = block
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let tool_name = block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let state = block.get("state");
                                let input = state
                                    .and_then(|s| s.get("input"))
                                    .cloned()
                                    .unwrap_or(Value::Null);
                                let status = state
                                    .and_then(|s| s.get("status"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("completed");

                                let normalized = normalize_tool_name(&tool_name);

                                ir_blocks.push(ContentBlock::ToolCall {
                                    id: tool_id.clone(),
                                    name: normalized.clone(),
                                    input,
                                });

                                // Extract tool result from state.content
                                let output = extract_tool_output(state);
                                let is_error = status == "error";

                                // Create a separate ToolResult IR message
                                let tool_time = block
                                    .get("time")
                                    .and_then(|t| t.get("completed"))
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(time_created);

                                tool_results.push((
                                    tool_id.clone(),
                                    ConvertedMessage {
                                        role: MessageRole::ToolResult,
                                        timestamp_ms: tool_time,
                                        content: vec![ContentBlock::ToolResult {
                                            call_id: tool_id.clone(),
                                            output,
                                            is_error,
                                        }],
                                        tool_call_id: Some(tool_id),
                                        tool_name: Some(normalized),
                                        is_error,
                                        raw: None, // synthetic, derived from tool state
                                    },
                                ));
                            }
                            "reasoning" => {
                                // Stash reasoning blocks so they survive a roundtrip
                                stash.push(serde_json::json!({
                                    "stash_type": "reasoning_block",
                                    "seq": seq,
                                    "time_created": time_created,
                                    "block": block.clone(),
                                }));
                            }
                            _ => {}
                        }
                    }

                    if !ir_blocks.is_empty() {
                        messages.push(ConvertedMessage {
                            role: MessageRole::Assistant,
                            timestamp_ms: time_created,
                            content: ir_blocks,
                            tool_call_id: None,
                            tool_name: None,
                            is_error: false,
                            raw: Some(data.clone()),
                        });
                    }

                    // Add tool results after the assistant message
                    for (_, result) in tool_results {
                        messages.push(result);
                    }
                }
            }
            _ => {
                // Stash non-convertible message types (idle, synthetic,
                // system, compaction, model-switched) for lossless roundtrip
                stash.push(serde_json::json!({
                    "type": msg_type,
                    "seq": seq,
                    "data": data,
                    "time_created": time_created,
                }));
            }
        }
    }

    let title = title.unwrap_or_else(|| {
        messages
            .iter()
            .find(|m| m.role == MessageRole::User)
            .and_then(|m| m.content.first())
            .and_then(|b| match b {
                ContentBlock::Text { text } => Some(text.chars().take(80).collect()),
                _ => None,
            })
            .unwrap_or_else(|| "Untitled session".to_string())
    });

    let has_tokens = tokens_input > 0 || tokens_output > 0;

    Ok(ConvertedSession {
        source_id: session_id.to_string(),
        source_tool: SourceTool::Opencode,
        title,
        project_dir: PathBuf::from(&directory),
        messages,
        model,
        provider,
        tokens: if has_tokens {
            Some(TokenUsage {
                input: tokens_input as u64,
                output: tokens_output as u64,
                cache_read: tokens_cache_read as u64,
                cache_write: tokens_cache_write as u64,
                reasoning: tokens_reasoning as u64,
            })
        } else {
            None
        },
        stash,
    })
}

fn extract_tool_output(state: Option<&Value>) -> String {
    let state = match state {
        Some(s) => s,
        None => return String::new(),
    };

    // v2: state.content is array of {type: "text", text: "..."}
    if let Some(content) = state.get("content").and_then(|v| v.as_array()) {
        let parts: Vec<&str> = content
            .iter()
            .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
            .collect();
        if !parts.is_empty() {
            return parts.join("\n");
        }
    }

    // Fallback: state.output (v1 compat, shouldn't hit for v2)
    state
        .get("output")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Write a ConvertedSession to the opencode v2 database.
pub fn write_session(session: &ConvertedSession) -> Result<String> {
    let conn = open_db()?;

    // Ensure project exists
    let project_id = ensure_project(&conn, &session.project_dir)?;

    // Generate session ID
    let session_id = generate_opencode_id("ses");
    let slug = generate_slug();
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Build model JSON
    let model_json = session.model.as_ref().map(|m| {
        serde_json::json!({
            "id": m,
            "providerID": session.provider.as_deref().unwrap_or("unknown"),
            "variant": "default",
        })
        .to_string()
    });

    // Aggregate tokens
    let (ti, to, tr, tcr, tcw) = session
        .tokens
        .as_ref()
        .map(|t| {
            (
                t.input as i64,
                t.output as i64,
                t.reasoning as i64,
                t.cache_read as i64,
                t.cache_write as i64,
            )
        })
        .unwrap_or((0, 0, 0, 0, 0));

    // Provenance metadata (includes stash for lossless roundtrip)
    let mut meta = serde_json::json!({
        "clync_source": session.source_tool.to_string(),
        "clync_source_id": session.source_id,
        "clync_converted_at": chrono::Utc::now().to_rfc3339(),
    });
    if !session.stash.is_empty() {
        meta["clync_stash"] = Value::Array(session.stash.clone());
    }
    let metadata = meta.to_string();

    // Insert session_v2
    conn.execute(
        "INSERT INTO session_v2 (id, project_id, slug, directory, title, version, metadata,
         cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write,
         model, time_created, time_updated, resume_attempts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0)",
        params![
            session_id,
            project_id,
            slug,
            session.project_dir.to_string_lossy().as_ref(),
            session.title,
            "clync-converted",
            metadata,
            ti,
            to,
            tr,
            tcr,
            tcw,
            model_json,
            now_ms,
            now_ms,
        ],
    )?;

    // Insert session_messages
    let mut seq = 1i64;

    for msg in &session.messages {
        let msg_id = generate_opencode_id("msg");
        let ts = msg.timestamp_ms as i64;

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

                let data = serde_json::json!({
                    "time": {"created": ts},
                    "text": text,
                    "files": [],
                    "agents": [],
                });

                conn.execute(
                    "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
                     VALUES (?1, ?2, 'user', ?3, ?4, ?5, ?6)",
                    params![msg_id, session_id, seq, ts, ts, data.to_string()],
                )?;
                seq += 1;
            }
            MessageRole::Assistant => {
                // Build content array with tool blocks that include their results
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
                            let oc_name = to_opencode_tool_name(name);

                            // Find matching tool result in subsequent messages
                            let tool_result = find_tool_result(&session.messages, id);
                            let (output_content, status, _is_error) = match &tool_result {
                                Some(tr) => {
                                    let output = tr
                                        .content
                                        .iter()
                                        .filter_map(|b| match b {
                                            ContentBlock::ToolResult { output, .. } => {
                                                Some(output.as_str())
                                            }
                                            _ => None,
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    let err = tr.is_error;
                                    (output, if err { "error" } else { "completed" }, err)
                                }
                                None => (String::new(), "pending", false),
                            };

                            let output_blocks = vec![serde_json::json!({
                                "type": "text",
                                "text": output_content,
                            })];

                            content_blocks.push(serde_json::json!({
                                "type": "tool",
                                "id": id,
                                "name": oc_name,
                                "executed": false,
                                "state": {
                                    "status": status,
                                    "input": input,
                                    "content": output_blocks,
                                    "metadata": {
                                        "status": status,
                                        "truncated": false,
                                    },
                                },
                                "time": {
                                    "created": ts,
                                    "completed": ts,
                                },
                            }));
                        }
                        ContentBlock::ToolResult { .. } => {
                            // Tool results are merged into the tool block above
                        }
                    }
                }

                let has_tools = content_blocks
                    .iter()
                    .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool"));

                let model_data = session.model.as_ref().map(|m| {
                    serde_json::json!({
                        "id": m,
                        "providerID": session.provider.as_deref().unwrap_or("unknown"),
                        "variant": "default",
                    })
                });

                let data = serde_json::json!({
                    "time": {"created": ts, "completed": ts},
                    "agent": "build",
                    "model": model_data,
                    "content": content_blocks,
                    "finish": if has_tools { "tool-calls" } else { "stop" },
                    "cost": 0,
                    "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"write": 0, "read": 0}},
                });

                conn.execute(
                    "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
                     VALUES (?1, ?2, 'assistant', ?3, ?4, ?5, ?6)",
                    params![msg_id, session_id, seq, ts, ts, data.to_string()],
                )?;
                seq += 1;
            }
            MessageRole::ToolResult => {
                // Tool results are merged into the assistant message's tool blocks.
                // Don't write a separate row.
            }
        }
    }

    // Write an idle message to mark the session as finished
    let idle_id = generate_opencode_id("msg");
    let idle_data = serde_json::json!({
        "time": {"created": now_ms},
        "outcome": "succeeded",
    });
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
         VALUES (?1, ?2, 'idle', ?3, ?4, ?5, ?6)",
        params![
            idle_id,
            session_id,
            seq,
            now_ms,
            now_ms,
            idle_data.to_string()
        ],
    )?;

    Ok(session_id)
}

fn find_tool_result<'a>(
    messages: &'a [ConvertedMessage],
    tool_call_id: &str,
) -> Option<&'a ConvertedMessage> {
    messages.iter().find(|m| {
        m.role == MessageRole::ToolResult
            && m.tool_call_id
                .as_ref()
                .map(|id| id == tool_call_id)
                .unwrap_or(false)
    })
}

fn ensure_project(conn: &Connection, worktree: &Path) -> Result<String> {
    let worktree_str = worktree.to_string_lossy();

    // Check if project already exists
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM project WHERE worktree = ?1",
            params![worktree_str.as_ref()],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        return Ok(id);
    }

    // Create new project
    let id = generate_hex_id(40);
    let name = worktree
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let now_ms = chrono::Utc::now().timestamp_millis();

    conn.execute(
        "INSERT INTO project (id, worktree, vcs, name, time_created, time_updated, sandboxes, time_active)
         VALUES (?1, ?2, 'git', ?3, ?4, ?5, '[]', 0)",
        params![id, worktree_str.as_ref(), name, now_ms, now_ms],
    )?;

    Ok(id)
}

/// Build an index of existing opencode sessions by their provenance source ID.
pub fn provenance_index() -> Result<HashMap<String, String>> {
    let conn = open_db()?;

    let mut index = HashMap::new();
    let mut stmt =
        conn.prepare("SELECT id, metadata FROM session_v2 WHERE metadata IS NOT NULL")?;

    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (id, metadata_str) = row?;
        if let Ok(meta) = serde_json::from_str::<Value>(&metadata_str)
            && let Some(source_id) = meta.get("clync_source_id").and_then(|v| v.as_str()) {
                index.insert(source_id.to_string(), id);
            }
    }

    Ok(index)
}

fn generate_opencode_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    // Mimic opencode's ID format: prefix + hex timestamp + random chars
    let ts_hex = format!("{:x}", ts);
    let random_part = generate_base62(20);
    format!("{}_{}{}", prefix, ts_hex, random_part)
}

fn generate_slug() -> String {
    const ADJECTIVES: &[&str] = &[
        "amber", "bold", "calm", "dark", "eager", "fair", "glad", "hazy", "keen", "late", "mild",
        "neat", "odd", "pale", "rare", "safe", "tall", "vast", "warm", "wise",
    ];
    const NOUNS: &[&str] = &[
        "bear", "crab", "deer", "dove", "fawn", "frog", "goat", "hawk", "lion", "lynx", "moth",
        "newt", "orca", "puma", "rook", "seal", "swan", "toad", "vole", "wolf",
    ];

    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as usize;
    let adj = ADJECTIVES[seed % ADJECTIVES.len()];
    let noun = NOUNS[(seed / ADJECTIVES.len()) % NOUNS.len()];
    format!("{}-{}", adj, noun)
}

fn generate_hex_id(len: usize) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut result = format!("{:032x}", ts);
    // Pad or truncate to desired length
    while result.len() < len {
        result.push_str(&format!("{:016x}", ts.wrapping_mul(result.len() as u128)));
    }
    result.truncate(len);
    result
}

fn generate_base62(len: usize) -> String {
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut result = String::with_capacity(len);
    for _ in 0..len {
        result.push(CHARS[(seed % 62) as usize] as char);
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    }
    result
}

#[derive(Debug)]
pub struct OpencodeSessionInfo {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub timestamp_ms: u64,
    pub provenance_source_id: Option<String>,
    pub worktree: String,
}
