use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Value, json};

use super::super::tools::to_claude_tool_name;
use super::super::{ContentBlock, ConvertedMessage, ConvertedSession, MessageRole, jsonl};
use super::{encode_project_dir, projects_dir};

/// Write a ConvertedSession as a Claude JSONL file under the project's folder.
pub fn write_session(session: &ConvertedSession) -> Result<PathBuf> {
    anyhow::ensure!(
        session.project_dir.is_absolute(),
        "project directory '{}' is not an absolute path",
        session.project_dir.display()
    );
    let dir = projects_dir()?.join(encode_project_dir(&session.project_dir));
    fs::create_dir_all(&dir)?;

    let session_uuid = uuid::Uuid::new_v4().to_string();
    let output_path = dir.join(format!("{}.jsonl", session_uuid));
    let mut writer = EntryWriter {
        session: session_uuid.clone(),
        cwd: session.project_dir.to_string_lossy().to_string(),
        model: session.model.clone(),
        tool_ids: claude_tool_ids(&session.messages),
        parent: None,
        lines: Vec::new(),
    };

    let origin = session.origin_key();
    writer.lines.push(json!({
        "type": "ai-title",
        "aiTitle": session.title,
        "sessionId": session_uuid,
    }));
    writer.lines.push(json!({
        "type": "clync-provenance",
        "source": origin.tool.to_string(),
        "sourceId": origin.id,
        "convertedAt": chrono::Utc::now().to_rfc3339(),
    }));
    for msg in &session.messages {
        writer.message(msg);
    }

    // Claude Code ignores unknown entry types. The stash goes between the
    // messages and `last-prompt` because the session picker reads the first
    // prompt from the head of the file and `lastPrompt` from its tail.
    if !session.stash.is_empty() {
        writer.lines.push(json!({
            "type": "clync-stash",
            "version": 1,
            "source": origin.tool.to_string(),
            "sourceId": origin.id,
            "entries": session.stash,
        }));
    }

    let leaf = writer.parent.clone();
    let last_prompt = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::User)
        .map(|m| m.joined_text());
    writer.lines.push(json!({
        "type": "last-prompt",
        "lastPrompt": last_prompt,
        "leafUuid": leaf,
        "sessionId": session_uuid,
    }));

    jsonl::write_atomic(&output_path, &writer.lines)?;
    Ok(output_path)
}

/// Claude's API only accepts tool IDs matching `^[a-zA-Z0-9_-]{1,64}$`; pi's
/// OpenAI adapter writes `call|item` IDs. Clean them the way pi's own Anthropic
/// adapter does, keeping them unique so results still find their calls.
fn claude_tool_ids(messages: &[ConvertedMessage]) -> HashMap<String, String> {
    let mut used = HashSet::new();
    let mut map = HashMap::new();
    for block in messages.iter().flat_map(|m| &m.content) {
        let ContentBlock::ToolCall { id, .. } = block else {
            continue;
        };
        let base: String = id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .take(64)
            .collect();
        let base = if base.is_empty() {
            "call".to_string()
        } else {
            base
        };
        let mut clean = base.clone();
        let mut n = 1;
        while !used.insert(clean.clone()) {
            let suffix = format!("_{}", n);
            clean = format!("{}{}", &base[..base.len().min(64 - suffix.len())], suffix);
            n += 1;
        }
        map.insert(id.clone(), clean);
    }
    map
}

/// Emits conversation entries as a single parentUuid chain.
struct EntryWriter {
    session: String,
    cwd: String,
    model: Option<String>,
    tool_ids: HashMap<String, String>,
    parent: Option<String>,
    lines: Vec<Value>,
}

impl EntryWriter {
    fn message(&mut self, msg: &ConvertedMessage) {
        let ts = jsonl::iso_millis(msg.timestamp_ms);
        match msg.role {
            MessageRole::User => {
                self.push(
                    "user",
                    &ts,
                    json!({ "role": "user", "content": msg.joined_text() }),
                );
            }
            MessageRole::ToolResult => {
                if let Some((call_id, output, is_error)) = msg.result_block() {
                    let content = json!([{
                        "type": "tool_result",
                        "tool_use_id": self.tool_id(call_id),
                        "content": output,
                        "is_error": is_error,
                    }]);
                    self.push("user", &ts, json!({ "role": "user", "content": content }));
                }
            }
            MessageRole::Assistant => self.assistant(msg, &ts),
        }
    }

    /// Claude stores one content block per entry. Blocks of one API message share
    /// `message.id`, which is how Claude Code stitches them back together on resume.
    fn assistant(&mut self, msg: &ConvertedMessage, ts: &str) {
        let message_id = format!("msg_clync_{}", uuid::Uuid::new_v4().simple());
        let has_tools = msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolCall { .. }));
        let stop_reason = if has_tools { "tool_use" } else { "end_turn" };

        for block in &msg.content {
            let content = match block {
                ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
                ContentBlock::ToolCall { id, name, input } => json!({
                    "type": "tool_use",
                    "id": self.tool_id(id),
                    "name": to_claude_tool_name(name),
                    "input": input,
                }),
                ContentBlock::ToolResult { .. } => continue,
            };
            let mut message = json!({
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": [content],
                "stop_reason": stop_reason,
                "stop_sequence": null,
                "usage": { "input_tokens": 0, "output_tokens": 0 },
            });
            if let Some(model) = &self.model {
                message["model"] = Value::String(model.clone());
            }
            self.push("assistant", ts, message);
        }
    }

    fn tool_id(&self, id: &str) -> String {
        self.tool_ids
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    fn push(&mut self, entry_type: &str, ts: &str, message: Value) {
        let uuid = uuid::Uuid::new_v4().to_string();
        self.lines.push(json!({
            "parentUuid": self.parent,
            "isSidechain": false,
            "userType": "external",
            "cwd": self.cwd,
            "sessionId": self.session,
            "type": entry_type,
            "uuid": uuid,
            "timestamp": ts,
            "message": message,
        }));
        self.parent = Some(uuid);
    }
}
