use anyhow::Result;

use crate::config::{Config, EncryptionConfig};
use crate::crypto::Cipher;
use crate::scanner::ScanFilter;
use crate::store::create_store;
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

pub fn do_push(do_sync: bool) -> Result<PushOutput> {
    let config = Config::load()?;
    let cipher = Cipher::from_config(&config.encryption)?;
    let store = create_store(&config)?;

    let (pushed, skipped, extras, mem) = {
        let _lock = store.try_lock()?;

        if let Some(path) = config.storage_path() {
            repo_meta::RepoMeta::from_config(&config).save(path)?;
        }
        ensure_repo_readme(&config)?;
        auto_migrate_memories(&config, &cipher);

        let filter = ScanFilter::default();
        let result = sync::push(&config, &cipher, &filter, store.as_ref())?;
        let extras_result = extras::push_extras(&config, &cipher)?;
        let mem_result = memories::push_memories(&config, &cipher)?;

        let mut log = synclog::SyncLogEntry::new("push");
        log.sessions_pushed = result.pushed;
        log.sessions_skipped = result.skipped;
        log.extras = extras_result.pushed + mem_result.pushed;
        if let Some(path) = config.storage_path() {
            synclog::append(path, &log).ok();
        }

        let total_extra = extras_result.pushed + mem_result.pushed;
        if do_sync && (result.pushed > 0 || total_extra > 0) {
            let machine = manifest::get_machine_id();
            let mut parts = Vec::new();
            if result.pushed > 0 {
                parts.push(format!("{} sessions", result.pushed));
            }
            if total_extra > 0 {
                parts.push(format!("{total_extra} extras"));
            }
            store.sync_up(&format!("clync push ({}) from {machine}", parts.join(", ")))?;
        }

        (
            result.pushed,
            result.skipped,
            extras_result.pushed,
            mem_result.pushed,
        )
    };

    Ok(PushOutput {
        sessions: pushed,
        skipped,
        extras,
        memories: mem,
    })
}

pub fn do_pull(do_sync: bool) -> Result<PullOutput> {
    let config = Config::load()?;
    let store = create_store(&config)?;

    if do_sync {
        store.sync_down()?;
    }

    let (pulled, merged, skipped, extras, mem) = {
        let _lock = store.try_lock()?;

        let cipher = Cipher::from_config(&config.encryption)?;
        auto_migrate_memories(&config, &cipher);

        let filter = ScanFilter::default();
        let result = sync::pull(&config, &cipher, &filter, store.as_ref())?;
        let extras_result = extras::pull_extras(&config, &cipher)?;
        let mem_result = memories::pull_memories(&config, &cipher)?;

        let mut log = synclog::SyncLogEntry::new("pull");
        log.sessions_pulled = result.pulled;
        log.sessions_merged = result.merged;
        log.sessions_skipped = result.skipped;
        log.extras = extras_result.pulled + mem_result.pulled;
        if let Some(path) = config.storage_path() {
            synclog::append(path, &log).ok();
        }

        if do_sync {
            let machine = manifest::get_machine_id();
            store.sync_up(&format!("clync pull from {machine}")).ok();
        }

        (
            result.pulled,
            result.merged,
            result.skipped,
            extras_result.pulled,
            mem_result.pulled,
        )
    };

    Ok(PullOutput {
        pulled,
        merged,
        skipped,
        extras,
        memories: mem,
    })
}

pub fn cmd_push(no_sync: bool, filter: ScanFilter) -> Result<()> {
    let config = Config::load()?;
    let cipher = Cipher::from_config(&config.encryption)?;
    let store = create_store(&config)?;
    let do_sync = config.sync.storage.auto_push() && !no_sync;

    {
        let _lock = store.lock()?;

        if let Some(path) = config.storage_path() {
            repo_meta::RepoMeta::from_config(&config).save(path)?;
        }
        ensure_repo_readme(&config)?;
        auto_migrate_memories(&config, &cipher);

        let result = sync::push(&config, &cipher, &filter, store.as_ref())?;
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
        if let Some(path) = config.storage_path() {
            synclog::append(path, &log).ok();
        }

        let total_extra = extras_result.pushed + mem_result.pushed;
        if do_sync && (result.pushed > 0 || total_extra > 0) {
            let machine = manifest::get_machine_id();
            let mut parts = Vec::new();
            if result.pushed > 0 {
                parts.push(format!("{} sessions", result.pushed));
            }
            if total_extra > 0 {
                parts.push(format!("{total_extra} extras"));
            }
            store.sync_up(&format!("clync push ({}) from {machine}", parts.join(", ")))?;
        }
    }

    Ok(())
}

pub fn cmd_pull(no_sync: bool, filter: ScanFilter) -> Result<()> {
    let config = Config::load()?;
    let store = create_store(&config)?;
    let do_sync = config.sync.storage.auto_push() && !no_sync;

    if do_sync {
        store.sync_down()?;
    }

    {
        let _lock = store.lock()?;

        let cipher = Cipher::from_config(&config.encryption)?;
        auto_migrate_memories(&config, &cipher);

        let result = sync::pull(&config, &cipher, &filter, store.as_ref())?;
        println!(
            "pull: {} new, {} merged, {} unchanged",
            result.pulled, result.merged, result.skipped
        );

        if !result.unmapped_with_remote.is_empty() {
            let n = result.unmapped_with_remote.len();
            println!(
                "note: {n} project{} with remote URLs not cloned locally. run `clync checkout` to clone.",
                if n == 1 { "" } else { "s" }
            );
        }

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
        if let Some(path) = config.storage_path() {
            synclog::append(path, &log).ok();
        }

        if do_sync {
            let machine = manifest::get_machine_id();
            store.sync_up(&format!("clync pull from {machine}")).ok();
        }
    }

    Ok(())
}

pub fn cmd_status(filter: ScanFilter) -> Result<()> {
    let config = Config::load()?;
    let cipher = Cipher::from_config(&config.encryption)?;
    let store = create_store(&config)?;
    let result = sync::status(&config, &cipher, &filter, store.as_ref())?;

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
    let store_path = match config.storage_path() {
        Some(p) => p,
        None => return,
    };
    let old_dir = store_path.join("extras").join("memories");
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
