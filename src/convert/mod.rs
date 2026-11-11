//! Session format conversion between Claude Code, opencode (v2), and pi.
//!
//! All conversions go through a common Intermediate Representation (IR).
//! Adding a new tool means implementing `read(source) -> IR` and `write(IR) -> target`.

pub mod claude;
mod cli;
mod jsonl;
pub mod opencode;
pub mod pi;
pub mod tools;

pub use cli::{ConvertArgs, run};

use std::path::PathBuf;
use std::str::FromStr;

use serde_json::Value;

/// Session metadata after conversion.
#[derive(Debug, Clone)]
pub struct ConvertedSession {
    pub source_id: String,
    pub source_tool: SourceTool,
    /// Set when the source session was itself produced by clync, so chains like
    /// claude -> pi -> opencode still dedupe against the original Claude session.
    pub origin: Option<Origin>,
    pub title: String,
    pub project_dir: PathBuf,
    pub messages: Vec<ConvertedMessage>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub tokens: Option<TokenUsage>,
    /// Source entries that carry conversation content but don't map to the IR
    /// (thinking text, compaction summaries, images). Written into the target
    /// so a future reverse conversion can recover them.
    pub stash: Vec<Value>,
}

impl ConvertedSession {
    /// The session this one ultimately came from. Used as the dedupe key.
    pub fn origin_key(&self) -> Origin {
        self.origin.clone().unwrap_or_else(|| Origin {
            tool: self.source_tool.clone(),
            id: self.source_id.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    pub tool: SourceTool,
    pub id: String,
}

impl Origin {
    pub fn parse(tool: &str, id: &str) -> Option<Self> {
        let tool = tool.parse().ok()?;
        if id.is_empty() {
            return None;
        }
        Some(Origin {
            tool,
            id: id.to_string(),
        })
    }
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.tool, self.id)
    }
}

/// A single conversation message in the IR.
#[derive(Debug, Clone)]
pub struct ConvertedMessage {
    pub role: MessageRole,
    pub timestamp_ms: u64,
    pub content: Vec<ContentBlock>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
}

impl ConvertedMessage {
    pub fn text(role: MessageRole, timestamp_ms: u64, text: String) -> Self {
        ConvertedMessage {
            role,
            timestamp_ms,
            content: vec![ContentBlock::Text { text }],
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn tool_result(
        timestamp_ms: u64,
        call_id: String,
        tool_name: Option<String>,
        output: String,
        is_error: bool,
    ) -> Self {
        ConvertedMessage {
            role: MessageRole::ToolResult,
            timestamp_ms,
            content: vec![ContentBlock::ToolResult {
                call_id: call_id.clone(),
                output,
                is_error,
            }],
            tool_call_id: Some(call_id),
            tool_name,
        }
    }

    /// Joined text of all text blocks.
    pub fn joined_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The first tool result block, if any.
    pub fn result_block(&self) -> Option<(&str, &str, bool)> {
        self.content.iter().find_map(|b| match b {
            ContentBlock::ToolResult {
                call_id,
                output,
                is_error,
            } => Some((call_id.as_str(), output.as_str(), *is_error)),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    ToolResult,
}

/// A single content block within a message.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String, // normalized: lowercase
        input: Value,
    },
    ToolResult {
        call_id: String,
        output: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceTool {
    Claude,
    Opencode,
    Pi,
}

impl FromStr for SourceTool {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "claude" => Ok(SourceTool::Claude),
            "opencode" => Ok(SourceTool::Opencode),
            "pi" => Ok(SourceTool::Pi),
            _ => Err(format!(
                "unknown format '{}'; must be one of: claude, opencode, pi",
                s
            )),
        }
    }
}

impl std::fmt::Display for SourceTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceTool::Claude => write!(f, "claude"),
            SourceTool::Opencode => write!(f, "opencode"),
            SourceTool::Pi => write!(f, "pi"),
        }
    }
}

/// Stash item for a reasoning/thinking block. Signatures are dropped on purpose:
/// they only validate against the provider that issued them.
pub fn thinking_stash(timestamp_ms: u64, text: &str) -> Value {
    serde_json::json!({
        "kind": "thinking",
        "timestamp_ms": timestamp_ms,
        "text": text,
    })
}

/// Pick the live branch of a session tree: walk from `leaf` back to the root via
/// `parent_of`. Claude and pi keep rewound or edited branches in the file, and
/// converting those would replay prompts the user threw away.
///
/// A parent that isn't in the file (dropped line, damaged session) would cut
/// off everything before it, so the walk continues at `fallback(i, walked)`
/// instead, when the format has a rule for that. Returns the indices on the path,
/// root first, and whether that fallback was needed.
pub fn branch_path(
    leaf: Option<usize>,
    index_of: &std::collections::HashMap<&str, usize>,
    parent_of: impl Fn(usize) -> Option<String>,
    fallback: impl Fn(usize, &std::collections::HashSet<usize>) -> Option<usize>,
) -> (Vec<usize>, bool) {
    let mut path = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut repaired = false;
    let mut current = leaf;
    while let Some(i) = current {
        if !seen.insert(i) {
            repaired = true; // malformed file with a parent cycle
            break;
        }
        path.push(i);
        current = match parent_of(i) {
            None => None,
            Some(p) => index_of.get(p.as_str()).copied().or_else(|| {
                repaired = true;
                fallback(i, &seen)
            }),
        };
    }
    path.reverse();
    (path, repaired)
}

/// Title fallback: first 80 characters of the first user message.
pub fn title_from_messages(messages: &[ConvertedMessage]) -> String {
    messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .map(|m| m.joined_text().chars().take(80).collect::<String>())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Untitled session".to_string())
}

/// Make tool calls and results pair up one-to-one, which every target requires:
/// Claude's API rejects a resumed history with an unanswered tool_use, and
/// opencode can't decode a tool block that never finished.
///
/// Some sources reuse call IDs across turns (or leave them empty), so IDs are
/// made unique and each result answers the earliest open call with its ID.
pub fn pair_tool_results(messages: Vec<ConvertedMessage>) -> Vec<ConvertedMessage> {
    use std::collections::{HashMap, HashSet, VecDeque};

    let mut used: HashSet<String> = HashSet::new();
    let mut open: HashMap<String, VecDeque<String>> = HashMap::new();
    let mut paired = Vec::with_capacity(messages.len());

    for mut msg in messages {
        if msg.role == MessageRole::ToolResult {
            let original = msg.tool_call_id.clone().unwrap_or_default();
            // A result with no open call has nothing to attach to.
            let Some(id) = open.get_mut(&original).and_then(VecDeque::pop_front) else {
                continue;
            };
            for block in &mut msg.content {
                if let ContentBlock::ToolResult { call_id, .. } = block {
                    call_id.clone_from(&id);
                }
            }
            msg.tool_call_id = Some(id);
            paired.push(msg);
            continue;
        }
        for block in &mut msg.content {
            if let ContentBlock::ToolCall { id, .. } = block {
                let original = id.clone();
                let base = if original.is_empty() {
                    "call"
                } else {
                    original.as_str()
                };
                let mut unique = base.to_string();
                let mut n = 1;
                while !used.insert(unique.clone()) {
                    unique = format!("{}_{}", base, n);
                    n += 1;
                }
                open.entry(original).or_default().push_back(unique.clone());
                *id = unique;
            }
        }
        paired.push(msg);
    }

    // Every result goes directly after the assistant message holding its call,
    // in call order: the API rejects a tool_result that doesn't follow its
    // tool_use, and streamed parallel calls can interleave with results.
    let mut results: HashMap<String, ConvertedMessage> = HashMap::new();
    let mut rest = Vec::with_capacity(paired.len());
    for msg in paired {
        match msg.tool_call_id.clone() {
            Some(id) if msg.role == MessageRole::ToolResult => {
                results.insert(id, msg);
            }
            _ => rest.push(msg),
        }
    }
    let mut out = Vec::with_capacity(rest.len() + results.len());
    for msg in rest {
        let calls: Vec<(String, String)> = msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolCall { id, name, .. } => Some((id.clone(), name.clone())),
                _ => None,
            })
            .collect();
        let ts = msg.timestamp_ms;
        out.push(msg);
        for (id, name) in calls {
            let result = results.remove(&id).unwrap_or_else(|| {
                ConvertedMessage::tool_result(
                    ts,
                    id,
                    Some(name),
                    "No result recorded".to_string(),
                    true,
                )
            });
            out.push(result);
        }
    }
    out
}

/// Truncate to `max` characters, adding "..." when cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{}...", kept)
    }
}

#[cfg(test)]
#[path = "convert_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "convert_branch_tests.rs"]
mod branch_tests;
