use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::super::tools::normalize_tool_name;
use super::super::{
    ContentBlock, ConvertedMessage, ConvertedSession, MessageRole, Origin, SourceTool, TokenUsage,
    jsonl, thinking_stash, title_from_messages,
};
use super::branch::live_branch;
use super::{decode_project_dir, entry_cwd};

/// System entries worth carrying over; the rest are UI or timing bookkeeping.
const STASHED_SYSTEM_SUBTYPES: &[&str] = &["compact_boundary", "away_summary"];

/// Read a Claude session JSONL file into the IR.
pub fn read_session(path: &Path) -> Result<ConvertedSession> {
    if path.parent().and_then(|p| p.file_name()) == Some("subagents".as_ref()) {
        anyhow::bail!(
            "{} is a subagent transcript, not a session; convert its parent session instead",
            path.display()
        );
    }

    let entries = jsonl::read_values(path)?;
    let live = live_branch(&entries, path);
    let is_message = |e: &Value| {
        matches!(
            e.get("type").and_then(|v| v.as_str()),
            Some("user" | "assistant")
        )
    };
    let mut state = ReadState::default();
    // Metadata in file order, then the conversation in replay order.
    for entry in entries.iter().filter(|e| !is_message(e)) {
        state.ingest(entry.clone());
    }
    for &i in &live {
        if is_message(&entries[i]) {
            state.ingest(entries[i].clone());
        }
    }

    let uuid = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let project_dir = state.cwd.clone().unwrap_or_else(|| {
        let folder = path.parent().and_then(|p| p.file_name());
        decode_project_dir(&folder.unwrap_or_default().to_string_lossy())
    });
    Ok(state.finish(uuid, project_dir))
}

#[derive(Default)]
struct ReadState {
    ai_title: Option<String>,
    custom_title: Option<String>,
    cwd: Option<PathBuf>,
    model: Option<String>,
    usage: Option<TokenUsage>,
    origin: Option<Origin>,
    carried_stash: Option<Vec<Value>>,
    entries: Vec<(u64, Value)>,
    last_ts: u64,
    counted_messages: HashSet<String>,
    stash: Vec<Value>,
}

impl ReadState {
    fn ingest(&mut self, entry: Value) {
        if self.cwd.is_none() {
            self.cwd = entry_cwd(&entry);
        }
        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match entry_type {
            "ai-title" => self.ai_title = str_field(&entry, "aiTitle"),
            "custom-title" => self.custom_title = str_field(&entry, "customTitle"),
            "clync-provenance" => {
                self.origin = Origin::parse(
                    entry.get("source").and_then(|v| v.as_str()).unwrap_or(""),
                    entry.get("sourceId").and_then(|v| v.as_str()).unwrap_or(""),
                );
            }
            "clync-stash" => {
                if let Some(Value::Array(items)) = entry.get("entries") {
                    self.carried_stash = Some(items.clone());
                }
            }
            "summary" => self.stash.push(entry),
            "system" => {
                let subtype = entry.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                if STASHED_SYSTEM_SUBTYPES.contains(&subtype) {
                    self.stash.push(entry);
                }
            }
            // Sidechains are subagent transcripts, not the main conversation.
            // A compaction summary restates history that's converted in full
            // anyway, so it goes to the stash rather than the conversation.
            "user" if entry.get("isCompactSummary") == Some(&Value::Bool(true)) => {
                self.stash.push(entry);
            }
            "user" | "assistant" if entry.get("isSidechain") != Some(&Value::Bool(true)) => {
                if entry_type == "assistant" {
                    self.record_assistant_meta(&entry);
                }
                // Entries arrive in replay order; a missing timestamp borrows
                // the previous one rather than jumping to the epoch.
                let ts = jsonl::entry_timestamp(&entry).unwrap_or(self.last_ts);
                self.last_ts = ts;
                self.entries.push((ts, entry));
            }
            _ => {}
        }
    }

    fn record_assistant_meta(&mut self, entry: &Value) {
        let Some(message) = entry.get("message") else {
            return;
        };
        // The last model used is the one a resume continues with. Claude labels
        // locally generated messages (interrupts, errors) "<synthetic>", which
        // isn't a model any target can resume with.
        if let Some(model) = str_field(message, "model").filter(|m| m != "<synthetic>") {
            self.model = Some(model);
        }
        // Every content-block entry of one API message repeats its usage.
        let first_sight = match str_field(message, "id") {
            Some(id) => self.counted_messages.insert(id),
            None => true,
        };
        if let Some(u) = message.get("usage").filter(|_| first_sight) {
            let n = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            let usage = self.usage.get_or_insert_with(TokenUsage::default);
            usage.input += n("input_tokens");
            usage.output += n("output_tokens");
            usage.cache_read += n("cache_read_input_tokens");
            usage.cache_write += n("cache_creation_input_tokens");
        }
    }

    fn finish(mut self, uuid: String, project_dir: PathBuf) -> ConvertedSession {
        let mut converter = Converter::default();
        for (ts, entry) in &self.entries {
            converter.ingest(*ts, entry);
        }
        let (messages, dropped) = converter.finish();
        self.stash.extend(dropped);

        let title = self
            .custom_title
            .or(self.ai_title)
            .unwrap_or_else(|| title_from_messages(&messages));

        ConvertedSession {
            source_id: uuid,
            source_tool: SourceTool::Claude,
            origin: self.origin,
            title,
            project_dir,
            messages,
            model: self.model,
            provider: Some("anthropic".to_string()),
            tokens: self.usage,
            // A converted session's stash belongs to the original source;
            // anything dropped from the intermediate copy is conversion noise.
            stash: self.carried_stash.unwrap_or(self.stash),
        }
    }
}

/// Turns Claude's one-block-per-entry stream into IR messages, merging
/// consecutive assistant entries into a single message.
#[derive(Default)]
struct Converter {
    messages: Vec<ConvertedMessage>,
    stash: Vec<Value>,
    pending: Vec<ContentBlock>,
    pending_ts: u64,
}

impl Converter {
    fn ingest(&mut self, ts: u64, entry: &Value) {
        let content = entry.get("message").and_then(|m| m.get("content"));
        match entry.get("type").and_then(|v| v.as_str()) {
            Some("user") => {
                self.flush();
                self.ingest_user(ts, content);
            }
            Some("assistant") => {
                if let Some(Value::Array(blocks)) = content {
                    for block in blocks {
                        self.ingest_assistant_block(ts, block);
                    }
                }
            }
            _ => {}
        }
    }

    fn ingest_user(&mut self, ts: u64, content: Option<&Value>) {
        let Some(Value::Array(blocks)) = content else {
            let text = jsonl::text_blocks(content);
            if !text.trim().is_empty() {
                self.messages
                    .push(ConvertedMessage::text(MessageRole::User, ts, text));
            }
            return;
        };

        let mut text_parts = Vec::new();
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("tool_result") => {
                    // Screenshots and other non-text output go to the stash.
                    for part in block
                        .get("content")
                        .and_then(|c| c.as_array())
                        .into_iter()
                        .flatten()
                    {
                        if part.get("type").and_then(|t| t.as_str()) != Some("text") {
                            self.stash.push(block_stash(ts, part));
                        }
                    }
                    self.messages.push(ConvertedMessage::tool_result(
                        ts,
                        str_field(block, "tool_use_id").unwrap_or_default(),
                        None,
                        jsonl::text_blocks(block.get("content")),
                        block.get("is_error").and_then(|v| v.as_bool()) == Some(true),
                    ));
                }
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(t.to_string());
                    }
                }
                // Images and documents carry content the IR can't express.
                _ => self.stash.push(block_stash(ts, block)),
            }
        }
        let text = text_parts.join("\n");
        // Claude's API rejects whitespace-only text blocks on resume.
        if !text.trim().is_empty() {
            self.messages
                .push(ConvertedMessage::text(MessageRole::User, ts, text));
        }
    }

    fn ingest_assistant_block(&mut self, ts: u64, block: &Value) {
        let new_block = match block.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                let text = str_field(block, "text").unwrap_or_default();
                if text.trim().is_empty() {
                    return;
                }
                ContentBlock::Text { text }
            }
            Some("tool_use") => ContentBlock::ToolCall {
                id: str_field(block, "id").unwrap_or_default(),
                name: normalize_tool_name(&str_field(block, "name").unwrap_or_default()),
                input: block.get("input").cloned().unwrap_or(Value::Null),
            },
            Some("thinking") => {
                let text = str_field(block, "thinking").unwrap_or_default();
                if !text.is_empty() {
                    self.stash.push(thinking_stash(ts, &text));
                }
                return;
            }
            Some("redacted_thinking") => return,
            _ => {
                self.stash.push(block_stash(ts, block));
                return;
            }
        };
        if self.pending.is_empty() {
            self.pending_ts = ts;
        }
        self.pending.push(new_block);
    }

    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        self.messages.push(ConvertedMessage {
            role: MessageRole::Assistant,
            timestamp_ms: self.pending_ts,
            content: std::mem::take(&mut self.pending),
            tool_call_id: None,
            tool_name: None,
        });
    }

    fn finish(mut self) -> (Vec<ConvertedMessage>, Vec<Value>) {
        self.flush();
        (self.messages, self.stash)
    }
}

fn block_stash(ts: u64, block: &Value) -> Value {
    serde_json::json!({ "kind": "block", "timestamp_ms": ts, "block": block })
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|v| v.as_str()).map(str::to_string)
}
