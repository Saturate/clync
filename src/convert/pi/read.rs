use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::super::tools::normalize_tool_name;
use super::super::{
    ContentBlock, ConvertedMessage, ConvertedSession, MessageRole, Origin, SourceTool, TokenUsage,
    branch_path, jsonl, thinking_stash, title_from_messages,
};
use super::{PROVENANCE_TYPE, STASH_TYPE, custom_data, provenance_origin};

/// Entry types that carry conversation content; model and thinking-level
/// switches are session settings and don't survive into another tool anyway.
const STASHED_ENTRY_TYPES: &[&str] = &["compaction", "branch_summary", "custom", "custom_message"];

/// Entry types that belong to one branch of the session tree.
const BRANCHED_ENTRY_TYPES: &[&str] =
    &["message", "custom_message", "compaction", "branch_summary"];

/// Read a pi session JSONL file into the IR.
pub fn read_session(path: &Path) -> Result<ConvertedSession> {
    let entries = jsonl::read_values(path)?;
    let live = live_branch(&entries, path);
    let mut state = ReadState::default();
    for (i, entry) in entries.into_iter().enumerate() {
        let branched = entry
            .get("type")
            .and_then(|v| v.as_str())
            .is_some_and(|t| BRANCHED_ENTRY_TYPES.contains(&t));
        if branched && !live.contains(&i) {
            continue;
        }
        state.ingest(entry);
    }
    anyhow::ensure!(
        !state.session_id.is_empty(),
        "{} has no pi session header",
        path.display()
    );

    let title = state
        .name
        .clone()
        .unwrap_or_else(|| title_from_messages(&state.messages));
    Ok(ConvertedSession {
        source_id: state.session_id,
        source_tool: SourceTool::Pi,
        origin: state.origin,
        title,
        project_dir: state.project_dir,
        messages: state.messages,
        model: state.model,
        provider: state.provider,
        tokens: state.usage,
        // A converted session's stash belongs to the original source.
        stash: state.carried_stash.unwrap_or(state.stash),
    })
}

/// Indices on the branch pi resumes: its leaf is the last entry in the file,
/// followed back through `parentId`. Abandoned branches stay in the file.
fn live_branch(entries: &[Value], path: &Path) -> HashSet<usize> {
    let has_id = |e: &Value| {
        e.get("type").and_then(|v| v.as_str()) != Some("session") && e.get("id").is_some()
    };
    // v1 sessions predate the tree; pi adds ids only when it rewrites them.
    if !entries.iter().any(has_id) {
        return (0..entries.len()).collect();
    }
    let index_of: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.get("id").and_then(|v| v.as_str()).map(|id| (id, i)))
        .collect();
    let leaf = entries.iter().rposition(has_id);
    let (live, repaired) = branch_path(
        leaf,
        &index_of,
        |i| {
            entries[i]
                .get("parentId")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        },
        |i, _| (0..i).rev().find(|&j| has_id(&entries[j])),
    );
    if repaired {
        eprintln!(
            "warning: {} references entries that aren't in the file; bridged the gap in file order",
            path.display()
        );
    }
    live.into_iter().collect()
}

#[derive(Default)]
struct ReadState {
    session_id: String,
    is_fork: bool,
    name: Option<String>,
    project_dir: PathBuf,
    model: Option<String>,
    provider: Option<String>,
    usage: Option<TokenUsage>,
    origin: Option<Origin>,
    carried_stash: Option<Vec<Value>>,
    messages: Vec<ConvertedMessage>,
    stash: Vec<Value>,
}

impl ReadState {
    fn ingest(&mut self, entry: Value) {
        match entry.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "session" => {
                self.session_id = str_field(&entry, "id").unwrap_or_default();
                // A fork copies the original's clync entries, but it's the
                // user's own session from here on.
                self.is_fork = entry.get("parentSession").is_some();
                self.project_dir = str_field(&entry, "cwd")
                    .map(PathBuf::from)
                    .unwrap_or_default();
            }
            "message" => self.ingest_message(&entry),
            // The latest session_info wins; an empty name clears it, as in pi.
            "session_info" => {
                self.name = str_field(&entry, "name")
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty());
            }
            "custom" => {
                if let Some(data) = custom_data(&entry, PROVENANCE_TYPE) {
                    if !self.is_fork {
                        self.origin = provenance_origin(data);
                    }
                } else if let Some(data) = custom_data(&entry, STASH_TYPE) {
                    if let Some(Value::Array(items)) = data.get("entries") {
                        self.carried_stash = Some(items.clone());
                    }
                } else {
                    self.stash.push(entry);
                }
            }
            t if STASHED_ENTRY_TYPES.contains(&t) => self.stash.push(entry),
            _ => {}
        }
    }

    fn ingest_message(&mut self, entry: &Value) {
        let Some(msg) = entry.get("message") else {
            return;
        };
        let ts = jsonl::entry_timestamp(entry).unwrap_or(0);
        match msg.get("role").and_then(|r| r.as_str()) {
            Some("user") => {
                let content = msg.get("content");
                self.stash_non_text(content, ts);
                let text = jsonl::text_blocks(content);
                if !text.trim().is_empty() {
                    self.messages
                        .push(ConvertedMessage::text(MessageRole::User, ts, text));
                }
            }
            Some("assistant") => self.ingest_assistant(msg, ts),
            Some("toolResult") => {
                self.stash_non_text(msg.get("content"), ts);
                self.messages.push(ConvertedMessage::tool_result(
                    ts,
                    str_field(msg, "toolCallId").unwrap_or_default(),
                    str_field(msg, "toolName").map(|n| normalize_tool_name(&n)),
                    jsonl::text_blocks(msg.get("content")),
                    msg.get("isError").and_then(|v| v.as_bool()) == Some(true),
                ));
            }
            // bashExecution, custom messages and the like have no IR equivalent.
            _ => self.stash.push(entry.clone()),
        }
    }

    /// Images and other non-text blocks the IR can't carry go to the stash.
    fn stash_non_text(&mut self, content: Option<&Value>, ts: u64) {
        for block in content.and_then(|c| c.as_array()).into_iter().flatten() {
            if block.get("type").and_then(|t| t.as_str()) != Some("text") {
                self.stash.push(serde_json::json!({
                    "kind": "block", "timestamp_ms": ts, "block": block,
                }));
            }
        }
    }

    fn ingest_assistant(&mut self, msg: &Value, ts: u64) {
        // The last model used is the one pi resumes with.
        if let Some(model) = str_field(msg, "model") {
            self.model = Some(model);
            self.provider = str_field(msg, "provider");
        }
        if let Some(u) = msg.get("usage") {
            let n = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            let usage = self.usage.get_or_insert_with(TokenUsage::default);
            usage.input += n("input");
            usage.output += n("output");
            usage.reasoning += n("reasoning");
            usage.cache_read += n("cacheRead");
            usage.cache_write += n("cacheWrite");
        }

        let mut blocks = Vec::new();
        for block in msg
            .get("content")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    let text = str_field(block, "text").unwrap_or_default();
                    if !text.trim().is_empty() {
                        blocks.push(ContentBlock::Text { text });
                    }
                }
                Some("toolCall") => blocks.push(ContentBlock::ToolCall {
                    id: str_field(block, "id").unwrap_or_default(),
                    name: normalize_tool_name(&str_field(block, "name").unwrap_or_default()),
                    // pi calls the tool input "arguments"
                    input: block.get("arguments").cloned().unwrap_or(Value::Null),
                }),
                Some("thinking") => {
                    let text = str_field(block, "thinking").unwrap_or_default();
                    if !text.is_empty() {
                        self.stash.push(thinking_stash(ts, &text));
                    }
                }
                _ => self.stash.push(serde_json::json!({
                    "kind": "block", "timestamp_ms": ts, "block": block,
                })),
            }
        }
        if !blocks.is_empty() {
            self.messages.push(ConvertedMessage {
                role: MessageRole::Assistant,
                timestamp_ms: ts,
                content: blocks,
                tool_call_id: None,
                tool_name: None,
            });
        }
    }
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|v| v.as_str()).map(str::to_string)
}
