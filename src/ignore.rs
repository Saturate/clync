//! Which files and dirs directory sync (skills, commands, memories) skips.

use anyhow::Result;
use std::path::Path;
use walkdir::{DirEntry, WalkDir};

/// Dependency, VCS and cache dirs: regenerable, often huge, never worth syncing.
const DEFAULT_IGNORED: &[&str] = &["node_modules", ".git", "__pycache__", ".venv"];

/// Names skipped at any depth, plus names skipped only as direct children
/// of the synced root (e.g. `skills/synced`).
#[derive(Debug, Clone)]
pub struct IgnoreRules {
    any_depth: Vec<String>,
    top_level: Vec<String>,
}

impl IgnoreRules {
    /// Built-in defaults plus `top_level` for this root, adjusted by the
    /// user's `sync.ignore`: `name` adds a rule, `!name` removes one.
    pub fn new(user: &[String], top_level: &[&str]) -> Self {
        let mut rules = Self {
            any_depth: DEFAULT_IGNORED.iter().map(|s| s.to_string()).collect(),
            top_level: top_level.iter().map(|s| s.to_string()).collect(),
        };
        for rule in user.iter().map(|r| r.trim()) {
            if let Some(name) = rule.strip_prefix('!') {
                rules.any_depth.retain(|n| n != name);
                rules.top_level.retain(|n| n != name);
            } else if !rule.is_empty() && !rules.any_depth.iter().any(|n| n == rule) {
                rules.any_depth.push(rule.to_string());
            }
        }
        rules
    }

    fn matches(&self, name: &str, depth: usize) -> bool {
        self.any_depth.iter().any(|n| n == name)
            || (depth == 1 && self.top_level.iter().any(|n| n == name))
    }

    /// `stored` entries live in the sync store, where files carry
    /// `.zst`/`.age` suffixes that must be stripped before matching.
    fn is_ignored(&self, entry: &DirEntry, stored: bool) -> bool {
        let name = entry.file_name().to_string_lossy();
        let name = if stored && entry.file_type().is_file() {
            strip_storage_suffix(&name)
        } else {
            &name
        };
        self.matches(name, entry.depth())
    }

    /// Files under `root` that aren't ignored. Ignored dirs aren't descended.
    pub fn walk_files<'a>(
        &'a self,
        root: &Path,
        stored: bool,
    ) -> impl Iterator<Item = DirEntry> + 'a {
        WalkDir::new(root)
            .into_iter()
            .filter_entry(move |e| e.depth() == 0 || !self.is_ignored(e, stored))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
    }

    /// Delete ignored entries from the store, e.g. copies an older version
    /// (or an earlier, looser `sync.ignore`) pushed.
    pub fn prune(&self, store_dir: &Path) -> Result<()> {
        let mut stale = Vec::new();
        let mut walker = WalkDir::new(store_dir).into_iter();
        while let Some(entry) = walker.next() {
            let Ok(entry) = entry else { continue };
            if entry.depth() > 0 && self.is_ignored(&entry, true) {
                if entry.file_type().is_dir() {
                    walker.skip_current_dir();
                }
                stale.push(entry);
            }
        }
        for entry in stale {
            if entry.file_type().is_dir() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
}

pub fn strip_storage_suffix(name: &str) -> &str {
    name.strip_suffix(".zst.age")
        .or_else(|| name.strip_suffix(".age"))
        .or_else(|| name.strip_suffix(".zst"))
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(rules: &[&str]) -> Vec<String> {
        rules.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_apply_at_any_depth_top_level_only_at_root() {
        let r = IgnoreRules::new(&[], &["synced"]);
        assert!(r.matches("node_modules", 1));
        assert!(r.matches("node_modules", 5));
        assert!(r.matches("synced", 1));
        assert!(!r.matches("synced", 2));
        assert!(!r.matches("SKILL.md", 1));
    }

    #[test]
    fn user_rules_add_names() {
        let r = IgnoreRules::new(&user(&["dist", " .DS_Store "]), &[]);
        assert!(r.matches("dist", 3));
        assert!(r.matches(".DS_Store", 1));
        assert!(r.matches("node_modules", 1), "defaults are kept");
    }

    #[test]
    fn negation_removes_defaults_and_top_level() {
        let r = IgnoreRules::new(&user(&["!node_modules", "!synced"]), &["synced", ".trash"]);
        assert!(!r.matches("node_modules", 2));
        assert!(!r.matches("synced", 1));
        assert!(r.matches(".trash", 1));
        assert!(r.matches(".git", 1));
    }

    #[test]
    fn stored_suffixes_are_stripped() {
        assert_eq!(strip_storage_suffix("notes.md.zst.age"), "notes.md");
        assert_eq!(strip_storage_suffix("notes.md.age"), "notes.md");
        assert_eq!(strip_storage_suffix("notes.md.zst"), "notes.md");
        assert_eq!(strip_storage_suffix("notes.md"), "notes.md");
    }
}
