//! Session format conversion between Claude Code, opencode (v2), and pi.
//!
//! All conversions go through a common Intermediate Representation (IR).
//! Adding a new tool means implementing `read(source) -> IR` and `write(IR) -> target`.

// Provenance functions and the `raw` field on ConvertedMessage are
// written but not yet wired into the --force duplicate-check path.
#![allow(dead_code)]

pub mod claude;
pub mod opencode;
pub mod pi;
pub mod tools;

use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Session metadata after conversion.
#[derive(Debug, Clone)]
pub struct ConvertedSession {
    pub source_id: String,
    pub source_tool: SourceTool,
    pub title: String,
    pub project_dir: PathBuf,
    pub messages: Vec<ConvertedMessage>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub tokens: Option<TokenUsage>,
    /// Raw source entries that didn't map to the IR (thinking blocks,
    /// system entries, metadata, step markers, etc.). Carried through
    /// to the target format so a reverse conversion can reconstruct
    /// a more faithful original.
    pub stash: Vec<serde_json::Value>,
}

/// A single conversation message in the IR.
#[derive(Debug, Clone)]
pub struct ConvertedMessage {
    pub role: MessageRole,
    pub timestamp_ms: u64,
    pub content: Vec<ContentBlock>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub is_error: bool,
    /// The raw source JSON for this message, preserved so the target
    /// writer can stash fields that the IR doesn't model (e.g. thinking
    /// signatures, git snapshots, token breakdowns).
    pub raw: Option<serde_json::Value>,
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
        input: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        output: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
}

#[derive(Debug, Clone, PartialEq)]
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

/// Entry point for `clync convert`.
#[allow(clippy::too_many_arguments)]
pub fn convert_sessions(
    from: &str,
    to: &str,
    session_arg: Option<&str>,
    all: bool,
    list: bool,
    dry_run: bool,
    project: Option<&str>,
    force: bool,
) -> Result<(), anyhow::Error> {
    let source: SourceTool = from.parse().map_err(|e: String| anyhow::anyhow!("{}", e))?;

    if list {
        return list_sessions_cmd(&source, project);
    }

    let target: SourceTool = to.parse().map_err(|e: String| anyhow::anyhow!("{}", e))?;

    if source == target {
        anyhow::bail!("source and target format are the same");
    }

    let project_path = project.map(|p| {
        let expanded = shellexpand::tilde(p);
        PathBuf::from(expanded.as_ref())
    });

    if all {
        convert_all(&source, &target, project_path.as_deref(), dry_run, force)
    } else if let Some(session) = session_arg {
        convert_one(&source, &target, session, dry_run, force)
    } else {
        anyhow::bail!("specify a session ID/path, --all, or --list")
    }
}

fn list_sessions_cmd(source: &SourceTool, project: Option<&str>) -> Result<(), anyhow::Error> {
    let project_path = project.map(|p| {
        let expanded = shellexpand::tilde(p);
        PathBuf::from(expanded.as_ref())
    });

    match source {
        SourceTool::Claude => {
            let sessions = claude::list_sessions(project_path.as_deref())?;
            if sessions.is_empty() {
                println!("No Claude sessions found.");
                return Ok(());
            }
            println!("{:<40} {:<60} Project", "UUID", "Title");
            println!("{}", "-".repeat(120));
            for s in &sessions {
                println!(
                    "{:<40} {:<60} {}",
                    s.uuid,
                    truncate(&s.title, 58),
                    s.project_dir.display()
                );
            }
            println!("\n{} session(s)", sessions.len());
        }
        SourceTool::Opencode => {
            let sessions = opencode::list_sessions(project_path.as_deref())?;
            if sessions.is_empty() {
                println!("No opencode v2 sessions found.");
                return Ok(());
            }
            println!("{:<32} {:<60} Directory", "ID", "Title");
            println!("{}", "-".repeat(120));
            for s in &sessions {
                let converted = if s.provenance_source_id.is_some() {
                    " [converted]"
                } else {
                    ""
                };
                println!(
                    "{:<32} {:<60} {}{}",
                    s.id,
                    truncate(&s.title, 58),
                    s.directory,
                    converted,
                );
            }
            println!("\n{} session(s)", sessions.len());
        }
        SourceTool::Pi => {
            let sessions = pi::list_sessions(project_path.as_deref())?;
            if sessions.is_empty() {
                println!("No pi sessions found.");
                return Ok(());
            }
            println!("{:<40} {:<60} Project", "ID", "Title");
            println!("{}", "-".repeat(120));
            for s in &sessions {
                println!(
                    "{:<40} {:<60} {}",
                    truncate(&s.id, 38),
                    truncate(&s.title, 58),
                    s.project_dir.display(),
                );
            }
            println!("\n{} session(s)", sessions.len());
        }
    }

    Ok(())
}

fn convert_all(
    source: &SourceTool,
    target: &SourceTool,
    project: Option<&Path>,
    dry_run: bool,
    force: bool,
) -> Result<(), anyhow::Error> {
    let mut converted = 0u64;
    let mut skipped = 0u64;
    let mut errors = 0u64;

    match source {
        SourceTool::Claude => {
            let sessions = claude::list_sessions(project)?;
            println!("Found {} Claude session(s)", sessions.len());
            for s in &sessions {
                match convert_one_inner(source, target, &s.path, &s.uuid, dry_run, force) {
                    Ok(true) => converted += 1,
                    Ok(false) => skipped += 1,
                    Err(e) => {
                        eprintln!("  error converting {}: {}", s.uuid, e);
                        errors += 1;
                    }
                }
            }
        }
        SourceTool::Opencode => {
            let sessions = opencode::list_sessions(project)?;
            println!("Found {} opencode v2 session(s)", sessions.len());
            for s in &sessions {
                match convert_one_inner(source, target, &PathBuf::new(), &s.id, dry_run, force) {
                    Ok(true) => converted += 1,
                    Ok(false) => skipped += 1,
                    Err(e) => {
                        eprintln!("  error converting {}: {}", s.id, e);
                        errors += 1;
                    }
                }
            }
        }
        SourceTool::Pi => {
            let sessions = pi::list_sessions(project)?;
            println!("Found {} pi session(s)", sessions.len());
            for s in &sessions {
                match convert_one_inner(source, target, &s.path, &s.id, dry_run, force) {
                    Ok(true) => converted += 1,
                    Ok(false) => skipped += 1,
                    Err(e) => {
                        eprintln!("  error converting {}: {}", s.id, e);
                        errors += 1;
                    }
                }
            }
        }
    }

    println!(
        "\nDone: {} converted, {} skipped, {} errors",
        converted, skipped, errors
    );
    Ok(())
}

fn convert_one(
    source: &SourceTool,
    target: &SourceTool,
    session_arg: &str,
    dry_run: bool,
    force: bool,
) -> Result<(), anyhow::Error> {
    let path = PathBuf::from(session_arg);
    match convert_one_inner(source, target, &path, session_arg, dry_run, force) {
        Ok(true) => Ok(()),
        Ok(false) => {
            println!("Skipped (already converted). Use --force to overwrite.");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Returns Ok(true) if converted, Ok(false) if skipped.
fn convert_one_inner(
    source: &SourceTool,
    target: &SourceTool,
    path: &Path,
    id: &str,
    dry_run: bool,
    _force: bool,
) -> Result<bool, anyhow::Error> {
    // Read source into IR
    let ir = match source {
        SourceTool::Claude => claude::read_session(path)?,
        SourceTool::Opencode => opencode::read_session(id)?,
        SourceTool::Pi => pi::read_session(path)?,
    };

    let msg_count = ir.messages.len();
    let tool_count = ir
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, ContentBlock::ToolCall { .. }))
        .count();

    let stash_count = ir.stash.len();

    if dry_run {
        println!(
            "  [dry-run] {} -> {}: \"{}\" ({} messages, {} tool calls, {} stashed)",
            source, target, ir.title, msg_count, tool_count, stash_count
        );
        return Ok(true);
    }

    // TODO: Check provenance for duplicates (skip unless force)
    // This is best done per-target, but for now we just proceed

    // Write to target
    let output_desc = match target {
        SourceTool::Claude => {
            let path = claude::write_session(&ir, None)?;
            format!("{}", path.display())
        }
        SourceTool::Opencode => {
            
            opencode::write_session(&ir)?
        }
        SourceTool::Pi => {
            let path = pi::write_session(&ir, None)?;
            format!("{}", path.display())
        }
    };

    println!(
        "  {} -> {}: \"{}\" ({} messages, {} tool calls, {} stashed) -> {}",
        source, target, ir.title, msg_count, tool_count, stash_count, output_desc
    );

    Ok(true)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
