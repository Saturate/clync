use anyhow::Result;
use std::path::Path;
use walkdir::WalkDir;

use crate::config::Config;
use crate::crypto::Cipher;
use crate::manifest::normalize_project_path;
use crate::resolver::{build_remote_map, resolve_project_dir};

fn encrypted_name(name: &str, encrypted: bool) -> String {
    if encrypted {
        format!("{name}.age")
    } else {
        name.to_string()
    }
}

fn is_encrypted(config: &Config) -> bool {
    !matches!(config.encryption, crate::config::EncryptionConfig::None)
}

pub struct MemoriesPushResult {
    pub pushed: u32,
}

pub struct MemoriesPullResult {
    pub pulled: u32,
}

pub fn push_memories(config: &Config, cipher: &Cipher) -> Result<MemoriesPushResult> {
    let claude_dir = &config.sync.claude_dir;
    let memories_dir = config.sync.repo.join("memories");
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
    let claude_dir = &config.sync.claude_dir;
    let memories_dir = config.sync.repo.join("memories");
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
        let local_dir_name = resolve_project_dir(&normalized_name, &remote_map, &projects_dir)
            .unwrap_or_else(|| crate::manifest::denormalize_project_path(&normalized_name));
        let dst_dir = projects_dir.join(&local_dir_name).join("memory");
        pulled += restore_directory(&project_entry.path(), &dst_dir, cipher)?;
    }

    Ok(MemoriesPullResult { pulled })
}

/// Migrate memories from the old `extras/memories/` layout (raw paths) to
/// the new `memories/` layout (normalized paths).
/// Returns (migrated_projects, migrated_files).
pub fn migrate_from_extras(config: &Config, cipher: &Cipher) -> Result<(u32, u32)> {
    let extras_memories = config.sync.repo.join("extras").join("memories");
    if !extras_memories.exists() {
        return Ok((0, 0));
    }

    let new_memories_dir = config.sync.repo.join("memories");
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

fn sync_directory(src_dir: &Path, dst_dir: &Path, cipher: &Cipher, encrypted: bool) -> Result<u32> {
    if !src_dir.exists() {
        return Ok(0);
    }
    std::fs::create_dir_all(dst_dir)?;
    let mut count = 0u32;
    for entry in WalkDir::new(src_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(src_dir)?;
        let dst = dst_dir.join(encrypted_name(&rel.to_string_lossy(), encrypted));
        count += sync_file_if_changed(entry.path(), &dst, cipher)?;
    }
    Ok(count)
}

fn sync_file_if_changed(src: &Path, dst: &Path, cipher: &Cipher) -> Result<u32> {
    if !src.exists() {
        return Ok(0);
    }

    let src_mtime = std::fs::metadata(src)?
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if dst.exists() {
        let dst_mtime = std::fs::metadata(dst)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if src_mtime <= dst_mtime {
            return Ok(0);
        }
    }

    cipher.encrypt_file(src, dst)?;
    Ok(1)
}

fn restore_directory(src_dir: &Path, dst_dir: &Path, cipher: &Cipher) -> Result<u32> {
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

fn restore_file(src: &Path, dst: &Path, cipher: &Cipher) -> Result<u32> {
    if !src.exists() {
        return Ok(0);
    }

    if dst.exists() {
        let src_mtime = std::fs::metadata(src)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dst_mtime = std::fs::metadata(dst)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if src_mtime <= dst_mtime {
            return Ok(0);
        }
    }

    let plaintext = match cipher.decrypt_file(src) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: could not decrypt {}: {e}", src.display());
            return Ok(0);
        }
    };
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dst, plaintext)?;
    Ok(1)
}

fn merge_memory_index(remote_src: &Path, local_dst: &Path, cipher: &Cipher) -> Result<u32> {
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

/// Merge two MEMORY.md index files by unioning link entries.
/// Local entries are preserved in order; new remote entries are appended.
/// On conflict (same link target), the longer line wins.
fn merge_memory_md(local: &str, remote: &str) -> String {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut non_link_lines: Vec<String> = Vec::new();

    for line in local.lines() {
        if let Some(target) = extract_link_target(line) {
            entries.push((target.clone(), line.to_string()));
            seen.insert(target);
        } else {
            non_link_lines.push(line.to_string());
        }
    }

    for line in remote.lines() {
        if let Some(target) = extract_link_target(line) {
            if seen.contains(&target) {
                if let Some(existing) = entries.iter_mut().find(|(t, _)| t == &target)
                    && line.len() > existing.1.len()
                {
                    existing.1 = line.to_string();
                }
            } else {
                entries.push((target.clone(), line.to_string()));
                seen.insert(target);
            }
        }
    }

    let mut result = String::new();
    for line in &non_link_lines {
        result.push_str(line);
        result.push('\n');
    }
    for (_, line) in &entries {
        result.push_str(line);
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
    fn merge_preserves_non_link_lines() {
        let local = "# Header\n\n- [Note](note.md) - a note\n";
        let remote = "- [Other](other.md) - another\n";
        let merged = merge_memory_md(local, remote);
        assert!(merged.contains("# Header"));
        assert!(merged.contains("note.md"));
        assert!(merged.contains("other.md"));
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
}
