use anyhow::Result;
use std::path::Path;
use walkdir::WalkDir;

use crate::config::Config;
use crate::crypto::Cipher;

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

pub fn push_extras(config: &Config, cipher: &Cipher) -> Result<ExtrasPushResult> {
    let claude_dir = &config.sync.claude_dir;
    let targets = &config.targets;
    let extras_dir = config.sync.repo.join("extras");
    let enc = is_encrypted(config);

    let mut pushed = 0u32;

    if targets.memories {
        pushed += sync_project_memories(claude_dir, &extras_dir, cipher, enc)?;
    }
    if targets.settings {
        pushed += sync_file_if_changed(
            &claude_dir.join("settings.json"),
            &extras_dir.join(encrypted_name("settings.json", enc)),
            cipher,
        )?;
        pushed += sync_file_if_changed(
            &claude_dir.join("settings.local.json"),
            &extras_dir.join(encrypted_name("settings.local.json", enc)),
            cipher,
        )?;
    }
    if targets.commands {
        pushed += sync_directory(
            &claude_dir.join("commands"),
            &extras_dir.join("commands"),
            cipher,
            enc,
        )?;
    }
    if targets.skills {
        pushed += sync_directory(
            &claude_dir.join("skills"),
            &extras_dir.join("skills"),
            cipher,
            enc,
        )?;
    }
    if targets.global_claude_md {
        pushed += sync_file_if_changed(
            &claude_dir.join("CLAUDE.md"),
            &extras_dir.join(encrypted_name("CLAUDE.md", enc)),
            cipher,
        )?;
    }

    Ok(ExtrasPushResult { pushed })
}

pub fn pull_extras(config: &Config, cipher: &Cipher) -> Result<ExtrasPullResult> {
    let claude_dir = &config.sync.claude_dir;
    let targets = &config.targets;
    let extras_dir = config.sync.repo.join("extras");
    let enc = is_encrypted(config);

    if !extras_dir.exists() {
        return Ok(ExtrasPullResult { pulled: 0 });
    }

    let mut pulled = 0u32;

    if targets.memories {
        pulled += restore_project_memories(claude_dir, &extras_dir, cipher)?;
    }
    if targets.settings {
        pulled += restore_file(
            &extras_dir.join(encrypted_name("settings.json", enc)),
            &claude_dir.join("settings.json"),
            cipher,
        )?;
        pulled += restore_file(
            &extras_dir.join(encrypted_name("settings.local.json", enc)),
            &claude_dir.join("settings.local.json"),
            cipher,
        )?;
    }
    if targets.commands {
        pulled += restore_directory(
            &extras_dir.join("commands"),
            &claude_dir.join("commands"),
            cipher,
        )?;
    }
    if targets.skills {
        pulled += restore_directory(
            &extras_dir.join("skills"),
            &claude_dir.join("skills"),
            cipher,
        )?;
    }
    if targets.global_claude_md {
        pulled += restore_file(
            &extras_dir.join(encrypted_name("CLAUDE.md", enc)),
            &claude_dir.join("CLAUDE.md"),
            cipher,
        )?;
    }

    Ok(ExtrasPullResult { pulled })
}

pub struct ExtrasPushResult {
    pub pushed: u32,
}

pub struct ExtrasPullResult {
    pub pulled: u32,
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
        count += restore_file(entry.path(), &dst, cipher)?;
    }
    Ok(count)
}

fn sync_project_memories(
    claude_dir: &Path,
    extras_dir: &Path,
    cipher: &Cipher,
    encrypted: bool,
) -> Result<u32> {
    let projects_dir = claude_dir.join("projects");
    if !projects_dir.exists() {
        return Ok(0);
    }
    let mut count = 0u32;
    for project_entry in std::fs::read_dir(&projects_dir)? {
        let project_entry = project_entry?;
        if !project_entry.path().is_dir() {
            continue;
        }
        let memory_dir = project_entry.path().join("memory");
        if !memory_dir.exists() {
            continue;
        }
        let project_name = project_entry.file_name().to_string_lossy().to_string();
        let dst_dir = extras_dir.join("memories").join(&project_name);
        count += sync_directory(&memory_dir, &dst_dir, cipher, encrypted)?;
    }
    Ok(count)
}

fn restore_project_memories(claude_dir: &Path, extras_dir: &Path, cipher: &Cipher) -> Result<u32> {
    let memories_dir = extras_dir.join("memories");
    if !memories_dir.exists() {
        return Ok(0);
    }
    let mut count = 0u32;
    for project_entry in std::fs::read_dir(&memories_dir)? {
        let project_entry = project_entry?;
        if !project_entry.path().is_dir() {
            continue;
        }
        let project_name = project_entry.file_name().to_string_lossy().to_string();
        let dst_dir = claude_dir
            .join("projects")
            .join(&project_name)
            .join("memory");
        count += restore_directory(&project_entry.path(), &dst_dir, cipher)?;
    }
    Ok(count)
}
