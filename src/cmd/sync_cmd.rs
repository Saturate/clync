use anyhow::Result;

use crate::config::{Config, EncryptionConfig};
use crate::crypto::Cipher;
use crate::scanner::ScanFilter;
use crate::storage::GitStorage;
use crate::{extras, manifest, memories, repo_meta, sync, synclog};

use super::init::ensure_repo_readme;

pub struct PushOutput {
    pub sessions: u32,
    pub skipped: u32,
    pub extras: u32,
    pub memories: u32,
}

pub struct PullOutput {
    pub pulled: u32,
    pub merged: u32,
    pub skipped: u32,
    pub extras: u32,
    pub memories: u32,
}

pub fn do_push(use_git: bool) -> Result<PushOutput> {
    let config = Config::load()?;
    let cipher = Cipher::from_config(&config.encryption)?;
    let storage = GitStorage::new(config.sync.repo.clone());

    let (pushed, skipped, extras, mem) = {
        let _lock = storage.try_lock()?;

        repo_meta::RepoMeta::from_config(&config).save(&config.sync.repo)?;
        ensure_repo_readme(&config)?;
        auto_migrate_memories(&config, &cipher);

        let filter = ScanFilter::default();
        let result = sync::push(&config, &cipher, &filter, &storage)?;
        let extras_result = extras::push_extras(&config, &cipher)?;
        let mem_result = memories::push_memories(&config, &cipher)?;

        let mut log = synclog::SyncLogEntry::new("push");
        log.sessions_pushed = result.pushed;
        log.sessions_skipped = result.skipped;
        log.extras = extras_result.pushed + mem_result.pushed;
        synclog::append(&config.sync.repo, &log).ok();

        let total_extra = extras_result.pushed + mem_result.pushed;
        if use_git && (result.pushed > 0 || total_extra > 0) {
            let machine = manifest::get_machine_id();
            let mut parts = Vec::new();
            if result.pushed > 0 {
                parts.push(format!("{} sessions", result.pushed));
            }
            if total_extra > 0 {
                parts.push(format!("{total_extra} extras"));
            }
            storage.commit(&format!("clync push ({}) from {machine}", parts.join(", ")))?;
        }

        (
            result.pushed,
            result.skipped,
            extras_result.pushed,
            mem_result.pushed,
        )
    };

    if use_git && (pushed > 0 || extras + mem > 0) {
        storage.push_remote()?;
    }

    Ok(PushOutput {
        sessions: pushed,
        skipped,
        extras,
        memories: mem,
    })
}

pub fn do_pull(use_git: bool) -> Result<PullOutput> {
    let config = Config::load()?;
    let storage = GitStorage::new(config.sync.repo.clone());

    if use_git {
        storage.pull_remote()?;
    }

    let (pulled, merged, skipped, extras, mem, should_commit) = {
        let _lock = storage.try_lock()?;

        let cipher = Cipher::from_config(&config.encryption)?;
        auto_migrate_memories(&config, &cipher);

        let filter = ScanFilter::default();
        let result = sync::pull(&config, &cipher, &filter, &storage)?;
        let extras_result = extras::pull_extras(&config, &cipher)?;
        let mem_result = memories::pull_memories(&config, &cipher)?;

        let mut log = synclog::SyncLogEntry::new("pull");
        log.sessions_pulled = result.pulled;
        log.sessions_merged = result.merged;
        log.sessions_skipped = result.skipped;
        log.extras = extras_result.pulled + mem_result.pulled;
        synclog::append(&config.sync.repo, &log).ok();

        let did_commit = if use_git {
            let machine = manifest::get_machine_id();
            storage
                .commit(&format!("clync pull from {machine}"))
                .is_ok()
        } else {
            false
        };

        (
            result.pulled,
            result.merged,
            result.skipped,
            extras_result.pulled,
            mem_result.pulled,
            did_commit,
        )
    };

    if use_git && should_commit {
        storage.push_remote().ok();
    }

    Ok(PullOutput {
        pulled,
        merged,
        skipped,
        extras,
        memories: mem,
    })
}

pub fn cmd_push(no_git: bool, filter: ScanFilter) -> Result<()> {
    let config = Config::load()?;
    let cipher = Cipher::from_config(&config.encryption)?;
    let storage = GitStorage::new(config.sync.repo.clone());
    let use_git = config.sync.auto_git && !no_git;

    let should_push = {
        let _lock = storage.lock()?;

        repo_meta::RepoMeta::from_config(&config).save(&config.sync.repo)?;
        ensure_repo_readme(&config)?;
        auto_migrate_memories(&config, &cipher);

        let result = sync::push(&config, &cipher, &filter, &storage)?;
        let verb = if matches!(config.encryption, EncryptionConfig::None) {
            "synced"
        } else {
            "encrypted"
        };
        println!(
            "push: {} sessions {verb}, {} unchanged",
            result.pushed, result.skipped
        );

        let extras_result = extras::push_extras(&config, &cipher)?;
        if extras_result.pushed > 0 {
            println!("push: {} extra files synced", extras_result.pushed);
        }

        let mem_result = memories::push_memories(&config, &cipher)?;
        if mem_result.pushed > 0 {
            println!("push: {} memory files synced", mem_result.pushed);
        }

        let mut log = synclog::SyncLogEntry::new("push");
        log.sessions_pushed = result.pushed;
        log.sessions_skipped = result.skipped;
        log.extras = extras_result.pushed + mem_result.pushed;
        synclog::append(&config.sync.repo, &log).ok();

        let total_extra = extras_result.pushed + mem_result.pushed;
        if use_git && (result.pushed > 0 || total_extra > 0) {
            let machine = manifest::get_machine_id();
            let mut parts = Vec::new();
            if result.pushed > 0 {
                parts.push(format!("{} sessions", result.pushed));
            }
            if total_extra > 0 {
                parts.push(format!("{total_extra} extras"));
            }
            storage.commit(&format!("clync push ({}) from {machine}", parts.join(", ")))?;
            true
        } else {
            false
        }
    };

    if should_push {
        storage.push_remote()?;
    }

    Ok(())
}

pub fn cmd_pull(no_git: bool, filter: ScanFilter) -> Result<()> {
    let config = Config::load()?;
    let storage = GitStorage::new(config.sync.repo.clone());
    let use_git = config.sync.auto_git && !no_git;

    if use_git {
        storage.pull_remote()?;
    }

    let should_push = {
        let _lock = storage.lock()?;

        let cipher = Cipher::from_config(&config.encryption)?;
        auto_migrate_memories(&config, &cipher);

        let result = sync::pull(&config, &cipher, &filter, &storage)?;
        println!(
            "pull: {} new, {} merged, {} unchanged",
            result.pulled, result.merged, result.skipped
        );

        let extras_result = extras::pull_extras(&config, &cipher)?;
        if extras_result.pulled > 0 {
            println!("pull: {} extra files restored", extras_result.pulled);
        }

        let mem_result = memories::pull_memories(&config, &cipher)?;
        if mem_result.pulled > 0 {
            println!("pull: {} memory files restored", mem_result.pulled);
        }

        let mut log = synclog::SyncLogEntry::new("pull");
        log.sessions_pulled = result.pulled;
        log.sessions_merged = result.merged;
        log.sessions_skipped = result.skipped;
        log.extras = extras_result.pulled + mem_result.pulled;
        synclog::append(&config.sync.repo, &log).ok();

        if use_git {
            let machine = manifest::get_machine_id();
            storage
                .commit(&format!("clync pull from {machine}"))
                .is_ok()
        } else {
            false
        }
    };

    if should_push {
        storage.push_remote().ok();
    }

    Ok(())
}

pub fn cmd_status(filter: ScanFilter) -> Result<()> {
    let config = Config::load()?;
    let cipher = Cipher::from_config(&config.encryption)?;
    let storage = GitStorage::new(config.sync.repo.clone());
    let result = sync::status(&config, &cipher, &filter, &storage)?;

    let total_diff = result.local_only.len() + result.remote_only.len() + result.diverged.len();
    if total_diff == 0 {
        println!("all {} sessions in sync", result.in_sync);
        return Ok(());
    }

    if !result.local_only.is_empty() {
        println!("local only ({}, push to sync):", result.local_only.len());
        for s in &result.local_only {
            println!(
                "  + {} [{}] {}B",
                super::short_uuid(&s.uuid),
                s.project,
                s.size
            );
        }
    }
    if !result.remote_only.is_empty() {
        println!("remote only ({}, pull to sync):", result.remote_only.len());
        for s in &result.remote_only {
            println!(
                "  - {} [{}] {}B",
                super::short_uuid(&s.uuid),
                s.project,
                s.size
            );
        }
    }
    if !result.diverged.is_empty() {
        println!(
            "diverged ({}, pull will smart-merge):",
            result.diverged.len()
        );
        for s in &result.diverged {
            println!(
                "  ~ {} [{}] {}B",
                super::short_uuid(&s.uuid),
                s.project,
                s.size
            );
        }
    }
    if result.in_sync > 0 {
        println!("in sync: {}", result.in_sync);
    }

    Ok(())
}

pub(crate) fn auto_migrate_memories(config: &Config, cipher: &Cipher) {
    let old_dir = config.sync.repo.join("extras").join("memories");
    if !old_dir.exists() {
        return;
    }
    match memories::migrate_from_extras(config, cipher) {
        Ok((projects, files)) => {
            if projects > 0 {
                eprintln!(
                    "migrated {files} memory files across {projects} projects \
                     from extras/memories/ to memories/"
                );
            }
        }
        Err(e) => eprintln!("warning: memory migration failed: {e}"),
    }
}
