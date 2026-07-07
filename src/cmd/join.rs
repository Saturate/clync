use anyhow::{Context, Result, bail};
use std::path::PathBuf;

use crate::config::{self, Config, EncryptionConfig, StorageConfig, SyncConfig};
use crate::crypto::Cipher;
use crate::io::InputSource;
use crate::scanner::ScanFilter;
use crate::store::git::GitStore;
use crate::{extras, memories, repo_meta, resolver, sync};

use super::init::prompt_manual_key;
use super::sync_cmd::auto_migrate_memories;

pub fn cmd_join(
    url: String,
    repo: Option<PathBuf>,
    op_ref: Option<String>,
    no_encrypt: bool,
    input: &dyn InputSource,
) -> Result<()> {
    let config_path = Config::config_path()?;
    if config_path.exists() {
        bail!(
            "config already exists at {}. Remove it to reinitialize.",
            config_path.display()
        );
    }

    let repo = repo.unwrap_or_else(|| {
        config::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".clync")
            .join("data")
    });

    let git_store = if repo.join(".git").exists() {
        let store = GitStore::new(repo.clone());
        let existing_remote = store.get_remote_url().unwrap_or_default();
        let normalized_existing = resolver::normalize_remote(&existing_remote);
        let normalized_new = resolver::normalize_remote(&url);
        if !normalized_existing.is_empty() && normalized_existing != normalized_new {
            bail!(
                "directory {} already contains a different repo ({}). Use --repo to specify a different path.",
                repo.display(),
                existing_remote
            );
        }
        println!("sync repo already exists, pulling latest...");
        store.pull_remote()?;
        store
    } else {
        println!("cloning sync repo...");
        GitStore::clone_repo(&url, &repo)?
    };

    let has_files = repo.join("clync.toml").exists() || repo.join("manifest.json").exists();
    if !has_files {
        git_store.checkout_first_branch()?;
    }

    let meta = repo_meta::RepoMeta::load(&repo)?;
    if let Some(ref meta) = meta {
        println!("repo encryption: {}", meta.encryption.method);
        if let Some(ref hint) = meta.encryption.hint {
            println!("  hint: {hint}");
        }
    }

    let config_dir = Config::config_dir()?;
    std::fs::create_dir_all(&config_dir)?;

    let encryption = if no_encrypt {
        EncryptionConfig::None
    } else if let Some(reference) = op_ref {
        EncryptionConfig::OnePassword { reference }
    } else if meta.as_ref().is_some_and(|m| m.encryption.method == "none") {
        println!("repo uses no encryption");
        EncryptionConfig::None
    } else if meta
        .as_ref()
        .is_some_and(|m| m.encryption.method == "passphrase")
    {
        let env_var =
            input.prompt_with_default("env var name for passphrase", "CLYNC_PASSPHRASE")?;
        EncryptionConfig::Passphrase { env_var }
    } else if let Some(ref hint) = meta.as_ref().and_then(|m| m.encryption.hint.clone()) {
        let method = meta
            .as_ref()
            .map(|m| m.encryption.method.as_str())
            .unwrap_or("");
        match method {
            "onepassword" => {
                let use_op = input.prompt_yn(&format!("use 1Password ({hint})?"), true)?;
                if use_op {
                    let reference = input.prompt_with_default("1Password reference", hint)?;
                    EncryptionConfig::OnePassword { reference }
                } else {
                    prompt_manual_key(input, &config_dir)?
                }
            }
            "bitwarden" => {
                let field = input.prompt_with_default("bitwarden field name", "notes")?;
                EncryptionConfig::Bitwarden {
                    item_id: hint.clone(),
                    field,
                }
            }
            "pass" => EncryptionConfig::Pass {
                entry: hint.clone(),
            },
            _ => prompt_manual_key(input, &config_dir)?,
        }
    } else {
        prompt_manual_key(input, &config_dir)?
    };

    let claude_dir = config::home_dir()
        .context("cannot determine home directory")?
        .join(".claude");

    let config = Config {
        sync: SyncConfig {
            claude_dir,
            include_companion_dirs: false,
            storage: StorageConfig::Git {
                path: repo.clone(),
                auto_push: true,
                lfs_threshold: config::default_lfs_threshold(),
            },
        },
        encryption,
        targets: Default::default(),
    };

    config.save(&config_path)?;
    println!("config saved to {}", config_path.display());

    let do_pull = input.prompt_yn("pull sessions now?", true)?;
    println!();
    if do_pull {
        let pull_result = (|| -> Result<()> {
            let cipher = Cipher::from_config(&config.encryption)?;
            auto_migrate_memories(&config, &cipher);
            let filter = ScanFilter::default();
            let result = sync::pull(&config, &cipher, &filter, &git_store)?;
            let extras = extras::pull_extras(&config, &cipher)?;
            let mem = memories::pull_memories(&config, &cipher)?;
            println!(
                "pulled {} sessions, {} merged, {} extras",
                result.pulled,
                result.merged,
                extras.pulled + mem.pulled
            );
            Ok(())
        })();

        if let Err(e) = pull_result {
            eprintln!("join failed: {e}");
            eprintln!("cleaning up...");
            std::fs::remove_dir_all(&config_dir).ok();
            std::fs::remove_dir_all(&repo).ok();
            bail!("join failed. check your encryption settings and try again.");
        }
    }

    println!("\ndone. run `clync push` to sync anytime.");
    Ok(())
}
