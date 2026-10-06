use anyhow::Result;

use crate::config::Config;
use crate::crypto::Cipher;
use crate::fileutil::{
    is_encrypted, restore_directory, restore_file, storage_name, sync_directory,
    sync_file_if_changed,
};

pub fn push_extras(config: &Config, cipher: &Cipher) -> Result<ExtrasPushResult> {
    let store_path = match config.storage_path() {
        Some(p) => p,
        None => {
            let has_extras = config.targets.settings
                || config.targets.commands
                || config.targets.skills
                || config.targets.global_claude_md;
            if has_extras {
                eprintln!(
                    "warning: extras sync (settings, commands, skills) is not supported with S3 storage"
                );
            }
            return Ok(ExtrasPushResult { pushed: 0 });
        }
    };
    let claude_dir = &config.sync.claude_dir;
    let targets = &config.targets;
    let extras_dir = store_path.join("extras");
    let enc = is_encrypted(config);
    let comp = config.sync.compression;
    let level = config.sync.compression_level;

    let rename_old = |name: &str| {
        if comp {
            let new = extras_dir.join(storage_name(name, enc, true));
            let old = extras_dir.join(storage_name(name, enc, false));
            if !new.exists() && old.exists() {
                std::fs::rename(&old, &new).ok();
            }
        }
    };

    let mut pushed = 0u32;

    if targets.settings {
        rename_old("settings.json");
        pushed += sync_file_if_changed(
            &claude_dir.join("settings.json"),
            &extras_dir.join(storage_name("settings.json", enc, comp)),
            cipher,
            comp,
            level,
        )?;
        rename_old("settings.local.json");
        pushed += sync_file_if_changed(
            &claude_dir.join("settings.local.json"),
            &extras_dir.join(storage_name("settings.local.json", enc, comp)),
            cipher,
            comp,
            level,
        )?;
    }
    if targets.commands {
        pushed += sync_directory(
            &claude_dir.join("commands"),
            &extras_dir.join("commands"),
            cipher,
            enc,
            comp,
            level,
        )?;
    }
    if targets.skills {
        pushed += sync_directory(
            &claude_dir.join("skills"),
            &extras_dir.join("skills"),
            cipher,
            enc,
            comp,
            level,
        )?;
    }
    if targets.global_claude_md {
        rename_old("CLAUDE.md");
        pushed += sync_file_if_changed(
            &claude_dir.join("CLAUDE.md"),
            &extras_dir.join(storage_name("CLAUDE.md", enc, comp)),
            cipher,
            comp,
            level,
        )?;
    }

    Ok(ExtrasPushResult { pushed })
}

pub fn pull_extras(config: &Config, cipher: &Cipher) -> Result<ExtrasPullResult> {
    let store_path = match config.storage_path() {
        Some(p) => p,
        None => return Ok(ExtrasPullResult { pulled: 0 }),
    };
    let claude_dir = &config.sync.claude_dir;
    let targets = &config.targets;
    let extras_dir = store_path.join("extras");
    let enc = is_encrypted(config);

    if !extras_dir.exists() {
        return Ok(ExtrasPullResult { pulled: 0 });
    }

    let mut pulled = 0u32;

    let find = |name: &str| -> std::path::PathBuf {
        let compressed = extras_dir.join(storage_name(name, enc, true));
        if compressed.exists() {
            return compressed;
        }
        extras_dir.join(storage_name(name, enc, false))
    };

    if targets.settings {
        pulled += restore_file(
            &find("settings.json"),
            &claude_dir.join("settings.json"),
            cipher,
        )?;
        pulled += restore_file(
            &find("settings.local.json"),
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
        pulled += restore_file(&find("CLAUDE.md"), &claude_dir.join("CLAUDE.md"), cipher)?;
    }

    Ok(ExtrasPullResult { pulled })
}

pub struct ExtrasPushResult {
    pub pushed: u32,
}

pub struct ExtrasPullResult {
    pub pulled: u32,
}
