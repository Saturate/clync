//! Session format conversion between Claude Code, opencode, and pi.
//!
//! All conversions go through a common Intermediate Representation (IR).
//! Adding a new tool means implementing `read(source) -> IR` and `write(IR) -> target`.

pub mod tools;

use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Clone)]
/// Session metadata after conversion
pub struct ConvertedSession {
    /// Original source identifier (UUID, session ID, or file path)
    pub source_id: String,
    /// Which tool this came from
    pub source_tool: SourceTool,
    /// Session title (from ai-title, session.title, or first message)
    pub title: String,
    /// Working directory / project path
    pub project_dir: PathBuf,
    /// Ordered list of conversation messages
    pub messages: Vec<ConvertedMessage>,
    /// Model used (if known)
    pub model: Option<String>,
    /// Provider (if known)
    pub provider: Option<String>,
    /// Aggregate token usage
    pub tokens: Option<TokenUsage>,
}

#[derive(Debug, Clone)]
/// A single conversation message in the IR
pub struct ConvertedMessage {
    /// Role: User, Assistant, ToolResult
    pub role: MessageRole,
    /// Timestamp (epoch milliseconds)
    pub timestamp_ms: u64,
    /// Content blocks in order
    pub content: Vec<ContentBlock>,
    /// For tool results: which tool call this responds to
    pub tool_call_id: Option<String>,
    /// For tool results: tool name
    pub tool_name: Option<String>,
    /// For tool results: whether it errored
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
/// Message role in the conversation
pub enum MessageRole {
    User,
    Assistant,
    ToolResult,
}

#[derive(Debug, Clone)]
/// A single content block within a message
pub enum ContentBlock {
    /// Plain text content
    Text { text: String },
    /// An LLM tool call
    ToolCall {
        id: String,
        name: String,          // normalized: lowercase
        input: serde_json::Value,
    },
    /// Result from a tool execution
    ToolResult {
        call_id: String,
        output: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone)]
/// Token usage statistics
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
}

#[derive(Debug, Clone, PartialEq)]
/// Source tool identifier
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
            "opencode" | "opencoded" => Ok(SourceTool::Opencode),
            "pi" => Ok(SourceTool::Pi),
            _ => Err(format!(
                "Invalid source tool '{}'. Must be one of: claude, opencode, pi",
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

/// Convert a session from one format to another.
pub fn convert_sessions(
    from: &str,
    to: &str,
    session_arg: Option<&str>,
    all: bool,
    list: bool,
    _dry_run: bool,
) -> Result<(), anyhow::Error> {
    // Parse source and target tools
    let source_tool: SourceTool = from.parse().map_err(|_| {
        anyhow::anyhow!(
            "Invalid source format '{}'. Must be one of: claude, opencode, pi",
            from
        )
    })?;

    let target_tool: SourceTool = to.parse().map_err(|_| {
        anyhow::anyhow!(
            "Invalid target format '{}'. Must be one of: claude, opencode, pi",
            to
        )
    })?;

    // TODO: Implement actual conversion logic
    println!("Converting from {} to {}", source_tool, target_tool);

    if list {
        println!("Listing available sessions...");
        // TODO: List sessions based on source tool
    } else if all {
        println!("Converting all sessions...");
        // TODO: Convert all sessions
    } else if let Some(session) = session_arg {
        println!(" Converting session: {}", session);
        // TODO: Convert single session
    } else {
        return Err(anyhow::anyhow!(
            "Must specify a session ID/path, --all, or --list"
        ));
    }

    Ok(())
}
