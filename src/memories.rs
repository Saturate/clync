use anyhow::Result;
use std::path::Path;
use walkdir::WalkDir;

use crate::config::Config;
use crate::crypto::Cipher;
use crate::fileutil::{
    encrypted_name, is_encrypted, is_safe_path_component, mtime_secs, restore_file, sync_directory,
};
use crate::manifest::normalize_project_path;
use crate::resolver::{build_remote_map, resolve_project_dir};

pub struct MemoriesPushResult {
    pub pushed: u32,
}

pub struct MemoriesPullResult {
    pub pulled: u32,
}

pub fn push_memories(config: &Config, cipher: &Cipher) -> Result<MemoriesPushResult> {
    if !config.targets.memories {
        return Ok(MemoriesPushResult { pushed: 0 });
    }

    let claude_dir = &config.sync.claude_dir;
    let store_path = match config.storage_path() {
        Some(p) => p,
        None => {
            eprintln!("warning: memories sync is not supported with S3 storage");
            return Ok(MemoriesPushResult { pushed: 0 });
        }
    };
    let memories_dir = store_path.join("memories");
    let enc = is_encrypted(config);

    let projects_dir = claude_dir.join("projects");
    if !projects_dir.exists() {
        return Ok(MemoriesPushResult { pushed: 0 });
    }

    let mut pushed = 0u32;
    for project_entry in std::fs::read_dir(&projects_dir)? {
        let project_entry = project_entry?;
        if !project_entry.path().is_dir() {
            continue;
        }
        let memory_dir = project_entry.path().join("memory");
        if !memory_dir.exists() {
            continue;
        }
        let raw_name = project_entry.file_name().to_string_lossy().to_string();
        let normalized = normalize_project_path(&raw_name);
        let dst_dir = memories_dir.join(&normalized);
        pushed += sync_directory(&memory_dir, &dst_dir, cipher, enc)?;
    }

    Ok(MemoriesPushResult { pushed })
}

pub fn pull_memories(config: &Config, cipher: &Cipher) -> Result<MemoriesPullResult> {
    if !config.targets.memories {
        return Ok(MemoriesPullResult { pulled: 0 });
    }

    let claude_dir = &config.sync.claude_dir;
    let store_path = match config.storage_path() {
        Some(p) => p,
        None => return Ok(MemoriesPullResult { pulled: 0 }),
    };
    let memories_dir = store_path.join("memories");
    if !memories_dir.exists() {
        return Ok(MemoriesPullResult { pulled: 0 });
    }

    let projects_dir = claude_dir.join("projects");
    let remote_map = build_remote_map(&projects_dir);

    let mut pulled = 0u32;
    for project_entry in std::fs::read_dir(&memories_dir)? {
        let project_entry = project_entry?;
        if !project_entry.path().is_dir() {
            continue;
        }
        let normalized_name = project_entry.file_name().to_string_lossy().to_string();
        if !is_safe_path_component(&normalized_name) {
            eprintln!("warning: skipping memory dir with unsafe path: {normalized_name}");
            continue;
        }
        let local_dir_name = resolve_project_dir(&normalized_name, &remote_map, &projects_dir)
            .unwrap_or_else(|| crate::manifest::denormalize_project_path(&normalized_name));
        let dst_dir = projects_dir.join(&local_dir_name).join("memory");
        pulled += restore_memory_directory(&project_entry.path(), &dst_dir, cipher)?;
    }

    Ok(MemoriesPullResult { pulled })
}

pub fn migrate_from_extras(config: &Config, cipher: &Cipher) -> Result<(u32, u32)> {
    let store_path = match config.storage_path() {
        Some(p) => p,
        None => return Ok((0, 0)),
    };
    let extras_memories = store_path.join("extras").join("memories");
    if !extras_memories.exists() {
        return Ok((0, 0));
    }

    let new_memories_dir = store_path.join("memories");
    let enc = is_encrypted(config);
    let mut projects = 0u32;
    let mut files = 0u32;

    for project_entry in std::fs::read_dir(&extras_memories)? {
        let project_entry = project_entry?;
        if !project_entry.path().is_dir() {
            continue;
        }
        let raw_name = project_entry.file_name().to_string_lossy().to_string();
        let normalized = normalize_project_path(&raw_name);
        let dst_dir = new_memories_dir.join(&normalized);
        std::fs::create_dir_all(&dst_dir)?;

        for entry in WalkDir::new(project_entry.path())
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(project_entry.path())?
                .to_string_lossy()
                .to_string();

            let data = std::fs::read(entry.path())?;
            let plain_name = rel.strip_suffix(".age").unwrap_or(&rel);

            let is_age = rel.ends_with(".age");
            let is_memory_index = plain_name == "MEMORY.md";

            if is_memory_index {
                let plain = if is_age {
                    cipher.decrypt(&data)?
                } else {
                    data.clone()
                };
                let dst_name = encrypted_name("MEMORY.md", enc);
                let dst_path = dst_dir.join(&dst_name);
                if dst_path.exists() {
                    let existing_data = std::fs::read(&dst_path)?;
                    let existing_plain = if enc {
                        cipher.decrypt(&existing_data)?
                    } else {
                        existing_data
                    };
                    let existing_text = String::from_utf8_lossy(&existing_plain);
                    let new_text = String::from_utf8_lossy(&plain);
                    let merged = merge_memory_md(&existing_text, &new_text);
                    let out = if enc {
                        cipher.encrypt(merged.as_bytes())?
                    } else {
                        merged.into_bytes()
                    };
                    std::fs::write(&dst_path, out)?;
                } else {
                    let out = if enc { cipher.encrypt(&plain)? } else { plain };
                    std::fs::write(&dst_path, out)?;
                }
            } else if is_age && !enc {
                let plain = cipher.decrypt(&data)?;
                std::fs::write(dst_dir.join(plain_name), plain)?;
            } else if !is_age && enc {
                let encrypted = cipher.encrypt(&data)?;
                std::fs::write(dst_dir.join(encrypted_name(&rel, true)), encrypted)?;
            } else {
                std::fs::write(dst_dir.join(&rel), data)?;
            }
            files += 1;
        }
        projects += 1;
    }

    std::fs::remove_dir_all(&extras_memories)?;

    Ok((projects, files))
}

fn restore_memory_directory(src_dir: &Path, dst_dir: &Path, cipher: &Cipher) -> Result<u32> {
    if !src_dir.exists() {
        return Ok(0);
    }
    std::fs::create_dir_all(dst_dir)?;
    let mut count = 0u32;
    for entry in WalkDir::new(src_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(src_dir)?
            .to_string_lossy()
            .to_string();
        let original_name = rel.strip_suffix(".age").unwrap_or(&rel);
        let dst = dst_dir.join(original_name);

        if original_name == "MEMORY.md" && dst.exists() {
            count += merge_memory_index(entry.path(), &dst, cipher)?;
        } else {
            count += restore_file(entry.path(), &dst, cipher)?;
        }
    }
    Ok(count)
}

fn merge_memory_index(remote_src: &Path, local_dst: &Path, cipher: &Cipher) -> Result<u32> {
    if local_dst.exists() && mtime_secs(remote_src)? < mtime_secs(local_dst)? {
        return Ok(0);
    }

    let remote_plain = match cipher.decrypt_file(remote_src) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: could not decrypt {}: {e}", remote_src.display());
            return Ok(0);
        }
    };
    let remote_text = String::from_utf8_lossy(&remote_plain);
    let local_text = std::fs::read_to_string(local_dst)?;

    let merged = merge_memory_md(&local_text, &remote_text);
    if merged == local_text {
        return Ok(0);
    }
    std::fs::write(local_dst, merged)?;
    Ok(1)
}

fn merge_memory_md(local: &str, remote: &str) -> String {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Track link targets and their positions in local lines
    let local_lines: Vec<&str> = local.lines().collect();
    let mut link_targets: Vec<(usize, String)> = Vec::new();
    for (i, line) in local_lines.iter().enumerate() {
        if let Some(target) = extract_link_target(line) {
            link_targets.push((i, target.clone()));
            seen.insert(target);
        }
    }

    // Collect remote entries: upgrades for existing targets + new entries
    let mut upgrades: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut new_entries: Vec<String> = Vec::new();
    for line in remote.lines() {
        if let Some(target) = extract_link_target(line) {
            if seen.contains(&target) {
                if let Some((idx, _)) = link_targets.iter().find(|(_, t)| t == &target)
                    && line.len() > local_lines[*idx].len()
                {
                    upgrades.insert(target, line.to_string());
                }
            } else {
                new_entries.push(line.to_string());
                seen.insert(target);
            }
        }
    }

    let mut result = String::new();
    for (i, line) in local_lines.iter().enumerate() {
        if let Some(target) = extract_link_target(line) {
            if let Some(upgraded) = upgrades.get(&target) {
                result.push_str(upgraded);
            } else {
                result.push_str(line);
            }
        } else {
            // Preserve non-link lines (headers, blank lines) in place
            let _ = i;
            result.push_str(line);
        }
        result.push('\n');
    }
    for entry in &new_entries {
        result.push_str(entry);
        result.push('\n');
    }
    result
}

fn extract_link_target(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with("- [") {
        return None;
    }
    let open = trimmed.find("](")?;
    let close = trimmed[open + 2..].find(')')? + open + 2;
    Some(trimmed[open + 2..close].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_disjoint() {
        let local = "- [Alpha](alpha.md) - first\n";
        let remote = "- [Beta](beta.md) - second\n";
        let merged = merge_memory_md(local, remote);
        assert!(merged.contains("- [Alpha](alpha.md) - first"));
        assert!(merged.contains("- [Beta](beta.md) - second"));
    }

    #[test]
    fn merge_overlap_keeps_longer() {
        let local = "- [Note](note.md) - short\n";
        let remote = "- [Note](note.md) - a much longer description here\n";
        let merged = merge_memory_md(local, remote);
        assert!(merged.contains("a much longer description"));
        assert_eq!(merged.matches("note.md").count(), 1);
    }

    #[test]
    fn merge_preserves_local_order() {
        let local = "- [C](c.md) - third\n- [A](a.md) - first\n";
        let remote = "- [B](b.md) - second\n";
        let merged = merge_memory_md(local, remote);
        let lines: Vec<&str> = merged.lines().collect();
        let c_pos = lines.iter().position(|l| l.contains("c.md")).unwrap();
        let a_pos = lines.iter().position(|l| l.contains("a.md")).unwrap();
        let b_pos = lines.iter().position(|l| l.contains("b.md")).unwrap();
        assert!(c_pos < a_pos, "local order preserved");
        assert!(a_pos < b_pos, "remote appended after local");
    }

    #[test]
    fn merge_identical() {
        let text = "- [Note](note.md) - same\n";
        let merged = merge_memory_md(text, text);
        assert_eq!(merged.matches("note.md").count(), 1);
    }

    #[test]
    fn merge_preserves_non_link_lines_in_place() {
        let local = "# Header\n\n- [Note](note.md) - a note\n";
        let remote = "- [Other](other.md) - another\n";
        let merged = merge_memory_md(local, remote);
        let lines: Vec<&str> = merged.lines().collect();
        assert_eq!(lines[0], "# Header");
        assert_eq!(lines[1], "");
        assert!(lines[2].contains("note.md"));
        assert!(lines[3].contains("other.md"));
    }

    #[test]
    fn merge_preserves_interleaved_blank_lines() {
        let local = "- [A](a.md) - first\n\n- [B](b.md) - second\n";
        let remote = "- [C](c.md) - third\n";
        let merged = merge_memory_md(local, remote);
        let lines: Vec<&str> = merged.lines().collect();
        assert_eq!(lines[0], "- [A](a.md) - first");
        assert_eq!(lines[1], "");
        assert_eq!(lines[2], "- [B](b.md) - second");
        assert_eq!(lines[3], "- [C](c.md) - third");
    }

    #[test]
    fn extract_link_target_valid() {
        assert_eq!(
            extract_link_target("- [Title](file.md) - desc"),
            Some("file.md".to_string())
        );
    }

    #[test]
    fn extract_link_target_no_match() {
        assert_eq!(extract_link_target("just a line"), None);
        assert_eq!(extract_link_target("# Header"), None);
    }

    #[test]
    fn merge_empty_local() {
        let merged = merge_memory_md("", "- [A](a.md) - first\n");
        assert!(merged.contains("- [A](a.md) - first"));
    }

    #[test]
    fn merge_empty_remote() {
        let merged = merge_memory_md("- [A](a.md) - first\n", "");
        assert!(merged.contains("- [A](a.md) - first"));
    }

    #[test]
    fn merge_both_empty() {
        let merged = merge_memory_md("", "");
        assert!(merged.is_empty());
    }

    #[test]
    fn merge_keeps_local_when_longer() {
        let local = "- [Note](note.md) - a long local description with details\n";
        let remote = "- [Note](note.md) - short\n";
        let merged = merge_memory_md(local, remote);
        assert!(merged.contains("a long local description"));
        assert!(!merged.contains("- short"));
        assert_eq!(merged.matches("note.md").count(), 1);
    }

    #[test]
    fn merge_deduplicates_remote_targets() {
        let local = "- [A](a.md) - first\n";
        let remote = "- [B](b.md) - second\n- [B](b.md) - also second\n";
        let merged = merge_memory_md(local, remote);
        assert_eq!(merged.matches("b.md").count(), 1);
    }

    #[test]
    fn extract_link_target_no_closing_paren() {
        assert_eq!(extract_link_target("- [Title](file.md"), None);
    }

    #[test]
    fn extract_link_target_indented() {
        assert_eq!(
            extract_link_target("  - [Title](file.md) - desc"),
            Some("file.md".to_string())
        );
    }

    #[test]
    fn extract_link_target_not_a_list_item() {
        assert_eq!(extract_link_target("[Title](file.md) - desc"), None);
    }

    #[test]
    fn extract_link_target_with_path() {
        assert_eq!(
            extract_link_target("- [Title](sub/dir/file.md) - desc"),
            Some("sub/dir/file.md".to_string())
        );
    }
}
