use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Transaction, params};
use serde_json::{Value, json};

use super::super::tools::to_opencode_tool_name;
use super::super::{ContentBlock, ConvertedMessage, ConvertedSession, MessageRole};
use super::project::resolve_project;
use super::{open_db, opencode_id, random_slug};

/// Write a ConvertedSession to the opencode v2 database. Returns the session ID.
pub fn write_session(session: &ConvertedSession) -> Result<String> {
    anyhow::ensure!(
        session.project_dir.is_absolute(),
        "project directory '{}' is not an absolute path",
        session.project_dir.display()
    );
    let mut conn = open_db()?;
    let tx = conn.transaction()?;
    let project_id = resolve_project(&tx, &session.project_dir)?;

    let first_ts = session.messages.first().map_or(0, |m| ms(m.timestamp_ms));
    let last_ts = session.messages.last().map_or(0, |m| ms(m.timestamp_ms));
    let now = chrono::Utc::now().timestamp_millis();
    let (created, updated) = if first_ts > 0 {
        (first_ts, last_ts.max(first_ts))
    } else {
        (now, now)
    };

    let session_id = opencode_id("ses", u64::try_from(created).unwrap_or(0), true);
    insert_session(&tx, session, &session_id, &project_id, created, updated)?;

    let results = tool_results(&session.messages);
    let mut seq = 1i64;
    for msg in &session.messages {
        let (msg_type, data) = match msg.role {
            MessageRole::User => (
                "user",
                json!({
                    "time": { "created": ms(msg.timestamp_ms) },
                    "text": msg.joined_text(),
                    "files": [],
                    "agents": [],
                }),
            ),
            MessageRole::Assistant => ("assistant", assistant_data(msg, session, &results)),
            // Results live inside the assistant's tool blocks.
            MessageRole::ToolResult => continue,
        };
        insert_message(&tx, &session_id, seq, msg_type, ms(msg.timestamp_ms), &data)?;
        seq += 1;
    }
    let idle = json!({ "time": { "created": updated }, "outcome": "succeeded" });
    insert_message(&tx, &session_id, seq, "idle", updated, &idle)?;
    // opencode allocates the next message seq from this watermark; without it
    // continuing the session collides with the rows written here.
    tx.execute(
        "INSERT OR REPLACE INTO event_sequence (aggregate_id, seq) VALUES (?1, ?2)",
        params![session_id, seq],
    )?;

    tx.commit()?;
    Ok(session_id)
}

fn insert_session(
    tx: &Transaction,
    session: &ConvertedSession,
    session_id: &str,
    project_id: &str,
    created: i64,
    updated: i64,
) -> Result<()> {
    let model_json = model_value(session).to_string();
    let tokens = session.tokens.clone().unwrap_or_default();
    let origin = session.origin_key();
    let mut meta = json!({
        "clync_source": origin.tool.to_string(),
        "clync_source_id": origin.id,
        "clync_converted_at": chrono::Utc::now().to_rfc3339(),
    });
    if !session.stash.is_empty() {
        meta["clync_stash"] = Value::Array(session.stash.clone());
    }
    let n = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);

    tx.execute(
        "INSERT INTO session_v2 (id, project_id, slug, directory, title, version, metadata,
         cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write,
         agent, model, time_created, time_updated, time_idle, idle_outcome, resume_attempts)
         VALUES (?1, ?2, ?3, ?4, ?5, 'clync', ?6, 0, ?7, ?8, ?9, ?10, ?11, 'build', ?12, ?13, ?14,
                 ?14, 'succeeded', 0)",
        params![
            session_id,
            project_id,
            random_slug(),
            session.project_dir.to_string_lossy().as_ref(),
            session.title,
            meta.to_string(),
            n(tokens.input),
            n(tokens.output),
            n(tokens.reasoning),
            n(tokens.cache_read),
            n(tokens.cache_write),
            model_json,
            created,
            updated,
        ],
    )?;
    Ok(())
}

fn insert_message(
    tx: &Transaction,
    session_id: &str,
    seq: i64,
    msg_type: &str,
    ts: i64,
    data: &Value,
) -> Result<()> {
    let id = opencode_id("msg", u64::try_from(ts).unwrap_or(0), false);
    tx.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
        params![id, session_id, msg_type, seq, ts, data.to_string()],
    )?;
    Ok(())
}

struct ToolOutcome<'a> {
    output: &'a str,
    is_error: bool,
    completed: i64,
}

fn tool_results(messages: &[ConvertedMessage]) -> HashMap<&str, ToolOutcome<'_>> {
    messages
        .iter()
        .filter_map(|m| {
            let (call_id, output, is_error) = m.result_block()?;
            Some((
                call_id,
                ToolOutcome {
                    output,
                    is_error,
                    completed: ms(m.timestamp_ms),
                },
            ))
        })
        .collect()
}

fn assistant_data(
    msg: &ConvertedMessage,
    session: &ConvertedSession,
    results: &HashMap<&str, ToolOutcome>,
) -> Value {
    let ts = ms(msg.timestamp_ms);
    let mut has_tools = false;
    let mut completed = ts;
    let content: Vec<Value> = msg
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(json!({ "type": "text", "text": text })),
            ContentBlock::ToolCall { id, name, input } => {
                has_tools = true;
                let outcome = results.get(id.as_str());
                let done = outcome.map_or(ts, |o| o.completed.max(ts));
                completed = completed.max(done);
                Some(json!({
                    "type": "tool",
                    "id": id,
                    "name": to_opencode_tool_name(name),
                    "executed": false,
                    "state": tool_state(input, outcome),
                    "time": { "created": ts, "ran": ts, "completed": done },
                }))
            }
            ContentBlock::ToolResult { .. } => None,
        })
        .collect();

    json!({
        "time": { "created": ts, "completed": completed },
        "agent": "build",
        "model": model_value(session),
        "content": content,
        "finish": if has_tools { "tool-calls" } else { "stop" },
        "cost": 0,
        "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
    })
}

/// opencode decodes tool state strictly by status: completed calls carry
/// `content`, failed ones carry `error` and nothing else.
fn tool_state(input: &Value, outcome: Option<&ToolOutcome>) -> Value {
    match outcome {
        Some(o) if !o.is_error => json!({
            "status": "completed",
            "input": input,
            "content": [{ "type": "text", "text": o.output }],
        }),
        _ => json!({
            "status": "error",
            "input": input,
            "error": {
                "type": "tool.execution",
                "message": outcome.map_or("No result recorded", |o| o.output),
            },
        }),
    }
}

/// opencode can't decode an assistant message without a model reference.
fn model_value(session: &ConvertedSession) -> Value {
    json!({
        "id": session.model.as_deref().unwrap_or("unknown"),
        "providerID": session.provider.as_deref().unwrap_or("unknown"),
        "variant": "default",
    })
}

fn ms(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}
