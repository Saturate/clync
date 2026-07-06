use anyhow::Result;

use crate::config::Config;
use crate::crypto::Cipher;
use crate::fileutil::{
    encrypted_name, is_encrypted, restore_directory, restore_file, sync_directory,
    sync_file_if_changed,
};

pub fn push_extras(config: &Config, cipher: &Cipher) -> Result<ExtrasPushResult> {
    let claude_dir = &config.sync.claude_dir;
    let targets = &config.targets;
    let extras_dir = config.sync.repo.join("extras");
    let enc = is_encrypted(config);

    let mut pushed = 0u32;

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
