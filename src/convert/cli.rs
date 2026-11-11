//! `clync convert` command: listing, dedupe, and the read -> write loop.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;

use super::{
    ContentBlock, ConvertedSession, Origin, SourceTool, claude, opencode, pair_tool_results, pi,
    truncate,
};

pub struct ConvertArgs<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub session: Option<&'a str>,
    pub all: bool,
    pub list: bool,
    pub dry_run: bool,
    pub project: Option<&'a str>,
    pub force: bool,
}

/// Entry point for `clync convert`.
pub fn run(args: &ConvertArgs) -> Result<()> {
    let source: SourceTool = args.from.parse().map_err(anyhow::Error::msg)?;
    let project = args.project.map(normalize_project).transpose()?;

    if args.list {
        return list_sessions(&source, project.as_deref());
    }

    anyhow::ensure!(
        !args.to.is_empty(),
        "--to is required unless --list is given"
    );
    let target: SourceTool = args.to.parse().map_err(anyhow::Error::msg)?;
    anyhow::ensure!(source != target, "source and target format are the same");

    let refs = if args.all {
        all_sessions(&source, project.as_deref())?
    } else if let Some(arg) = args.session {
        vec![resolve_one(&source, arg)?]
    } else {
        anyhow::bail!("specify a session ID/path, --all, or --list");
    };

    let mut run = Run {
        target: target.clone(),
        existing: existing_conversions(&target)?,
        dry_run: args.dry_run,
        force: args.force,
        stats: Stats::default(),
    };
    println!("Found {} {} session(s)", refs.len(), source);
    for session_ref in &refs {
        run.convert(&source, session_ref);
    }

    let s = &run.stats;
    println!(
        "\nDone: {} converted, {} skipped, {} errors",
        s.converted, s.skipped, s.errors
    );
    anyhow::ensure!(s.errors == 0, "{} session(s) failed to convert", s.errors);
    Ok(())
}

/// How a source session is addressed: a file for Claude and pi, a row ID for opencode.
enum SessionRef {
    File(PathBuf),
    Opencode(String),
}

impl SessionRef {
    fn read(&self, source: &SourceTool) -> Result<ConvertedSession> {
        match (self, source) {
            (SessionRef::File(path), SourceTool::Claude) => claude::read_session(path),
            (SessionRef::File(path), SourceTool::Pi) => pi::read_session(path),
            (SessionRef::Opencode(id), SourceTool::Opencode) => opencode::read_session(id),
            _ => anyhow::bail!("session reference doesn't match source format {}", source),
        }
    }

    fn label(&self) -> String {
        match self {
            SessionRef::File(p) => p.display().to_string(),
            SessionRef::Opencode(id) => id.clone(),
        }
    }
}

/// Where an earlier conversion of the same origin lives in the target.
enum Existing {
    File(PathBuf),
    Opencode(String),
}

#[derive(Default)]
struct Stats {
    converted: u64,
    skipped: u64,
    errors: u64,
}

struct Run {
    target: SourceTool,
    existing: HashMap<Origin, Existing>,
    dry_run: bool,
    force: bool,
    stats: Stats,
}

impl Run {
    fn convert(&mut self, source: &SourceTool, session_ref: &SessionRef) {
        match self.convert_inner(source, session_ref) {
            Ok(true) => self.stats.converted += 1,
            Ok(false) => self.stats.skipped += 1,
            Err(e) => {
                eprintln!("  error converting {}: {:#}", session_ref.label(), e);
                self.stats.errors += 1;
            }
        }
    }

    /// Ok(true) when written (or would be, in a dry run), Ok(false) when skipped.
    fn convert_inner(&mut self, source: &SourceTool, session_ref: &SessionRef) -> Result<bool> {
        let mut ir = session_ref.read(source)?;
        let origin = ir.origin_key();

        // Converting back into the tool it came from would duplicate the original.
        if origin.tool == self.target {
            println!("  skip \"{}\": originally from {}", ir.title, self.target);
            return Ok(false);
        }
        if self.existing.contains_key(&origin) && !self.force {
            println!(
                "  skip \"{}\": already converted ({}); --force replaces it",
                ir.title, origin
            );
            return Ok(false);
        }
        if ir.messages.is_empty() {
            println!("  skip {}: no conversation to convert", session_ref.label());
            return Ok(false);
        }

        ir.messages = pair_tool_results(std::mem::take(&mut ir.messages));
        let tool_calls = ir
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::ToolCall { .. }))
            .count();
        let summary = format!(
            "{} -> {}: \"{}\" ({} messages, {} tool calls, {} stashed)",
            source,
            self.target,
            truncate(&ir.title, 60),
            ir.messages.len(),
            tool_calls,
            ir.stash.len()
        );
        if self.dry_run {
            println!("  [dry-run] {}", summary);
            return Ok(true);
        }

        let written = match self.target {
            SourceTool::Claude => Existing::File(claude::write_session(&ir)?),
            SourceTool::Pi => Existing::File(pi::write_session(&ir)?),
            SourceTool::Opencode => Existing::Opencode(opencode::write_session(&ir)?),
        };
        let location = match &written {
            Existing::File(p) => p.display().to_string(),
            Existing::Opencode(id) => id.clone(),
        };
        println!("  {} -> {}", summary, location);
        // Replace only after the new copy is safely written, so a failed
        // write never loses the earlier conversion.
        if let Some(old) = self.existing.insert(origin, written)
            && let Err(e) = remove_existing(&old)
        {
            eprintln!(
                "  warning: couldn't remove the replaced conversion: {:#}",
                e
            );
        }
        Ok(true)
    }
}

fn remove_existing(existing: &Existing) -> Result<()> {
    match existing {
        Existing::File(path) => std::fs::remove_file(path)?,
        Existing::Opencode(id) => opencode::delete_session(id)?,
    }
    Ok(())
}

/// `--project` is compared against absolute session directories, so resolve
/// `.`, `~` and trailing slashes the same way.
fn normalize_project(arg: &str) -> Result<PathBuf> {
    let expanded = PathBuf::from(shellexpand::tilde(arg).as_ref());
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()?.join(expanded)
    };
    Ok(absolute
        .canonicalize()
        .unwrap_or_else(|_| absolute.components().collect()))
}

fn existing_conversions(target: &SourceTool) -> Result<HashMap<Origin, Existing>> {
    Ok(match target {
        SourceTool::Claude => claude::provenance_index()?
            .into_iter()
            .map(|(k, v)| (k, Existing::File(v)))
            .collect(),
        SourceTool::Pi => pi::provenance_index()?
            .into_iter()
            .map(|(k, v)| (k, Existing::File(v)))
            .collect(),
        SourceTool::Opencode => opencode::provenance_index()?
            .into_iter()
            .map(|(k, v)| (k, Existing::Opencode(v)))
            .collect(),
    })
}

fn resolve_one(source: &SourceTool, arg: &str) -> Result<SessionRef> {
    Ok(match source {
        SourceTool::Claude => SessionRef::File(claude::resolve_session(arg)?),
        SourceTool::Pi => SessionRef::File(pi::resolve_session(arg)?),
        SourceTool::Opencode => SessionRef::Opencode(opencode::resolve_session(arg)?),
    })
}

fn all_sessions(source: &SourceTool, project: Option<&std::path::Path>) -> Result<Vec<SessionRef>> {
    Ok(match source {
        SourceTool::Claude => claude::list_sessions(project)?
            .into_iter()
            .map(|s| SessionRef::File(s.path))
            .collect(),
        SourceTool::Pi => pi::list_sessions(project)?
            .into_iter()
            .map(|s| SessionRef::File(s.path))
            .collect(),
        SourceTool::Opencode => opencode::list_sessions(project)?
            .into_iter()
            .map(|s| SessionRef::Opencode(s.id))
            .collect(),
    })
}

fn list_sessions(source: &SourceTool, project: Option<&std::path::Path>) -> Result<()> {
    let rows: Vec<(String, String, String)> = match source {
        SourceTool::Claude => claude::list_sessions(project)?
            .into_iter()
            .map(|s| (s.uuid, s.title, s.project_dir.display().to_string()))
            .collect(),
        SourceTool::Pi => pi::list_sessions(project)?
            .into_iter()
            .map(|s| (s.id, s.title, s.project_dir.display().to_string()))
            .collect(),
        SourceTool::Opencode => opencode::list_sessions(project)?
            .into_iter()
            .map(|s| {
                let mark = if s.converted { " [converted]" } else { "" };
                (s.id, s.title, format!("{}{}", s.directory, mark))
            })
            .collect(),
    };
    if rows.is_empty() {
        println!("No {} sessions found.", source);
        return Ok(());
    }
    println!("{:<40} {:<60} Project", "ID", "Title");
    println!("{}", "-".repeat(120));
    for (id, title, project) in &rows {
        println!(
            "{:<40} {:<60} {}",
            truncate(id, 38),
            truncate(title, 58),
            project
        );
    }
    println!("\n{} session(s)", rows.len());
    Ok(())
}
