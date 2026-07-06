use anyhow::{Context, Result, bail};
use std::path::PathBuf;

use crate::config::{self, Config, EncryptionConfig, SyncConfig};
use crate::crypto::Cipher;
use crate::io::InputSource;
use crate::scanner::ScanFilter;
use crate::storage::GitStorage;
use crate::{extras, manifest, memories, repo_meta, sync};

pub fn cmd_init(
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

    let interactive = repo.is_none() && op_ref.is_none() && !no_encrypt;

    if interactive {
        return cmd_init_interactive(input);
    }

    let repo = repo.unwrap_or_else(|| {
        config::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".clync")
            .join("data")
    });

    let enc_override = if no_encrypt {
        Some(EncryptionConfig::None)
    } else {
        None
    };

    init_with_options(repo, op_ref, enc_override, Default::default())
}

fn cmd_init_interactive(input: &dyn InputSource) -> Result<()> {
    println!("clync setup\n");

    let default_repo = config::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".clync")
        .join("data");
    let repo = input.prompt_with_default("sync repo path", &default_repo.to_string_lossy())?;
    let repo = config::expand_path(&PathBuf::from(&repo));

    println!("\nencryption:");
    println!("  1) local key file (default, no dependencies)");
    println!("  2) passphrase (from env var, no key file needed)");
    println!("  3) 1Password CLI (op://)");
    println!("  4) Bitwarden CLI (bw)");
    println!("  5) pass (Unix password manager)");
    println!("  6) none (plain text, use private repo)");
    let enc_choice = input.prompt_with_default("choice [1-6]", "1")?;

    let (op_ref, enc_override) = match enc_choice.trim() {
        "2" => {
            let env_var =
                input.prompt_with_default("env var name for passphrase", "CLYNC_PASSPHRASE")?;
            println!("set {env_var} in your shell before running clync");
            (None, Some(EncryptionConfig::Passphrase { env_var }))
        }
        "3" => {
            let reference = input
                .prompt_with_default("1Password reference", "op://Personal/clync/age-secret-key")?;
            (Some(reference), None)
        }
        "4" => {
            let item_id = input.prompt("Bitwarden item ID or name")?;
            let field = input.prompt_with_default("field name", "notes")?;
            (None, Some(EncryptionConfig::Bitwarden { item_id, field }))
        }
        "5" => {
            let entry = input.prompt_with_default("pass entry path", "clync/age-key")?;
            (None, Some(EncryptionConfig::Pass { entry }))
        }
        "6" => {
            println!("no encryption - make sure your sync repo is private");
            (None, Some(EncryptionConfig::None))
        }
        _ => (None, None),
    };

    println!("\nwhat to sync (all on by default, disable what you don't want):");
    let settings = input.prompt_yn("  sync settings.json?", true)?;
    let commands = input.prompt_yn("  sync custom commands?", true)?;
    let skills = input.prompt_yn("  sync custom skills?", true)?;
    let claude_md = input.prompt_yn("  sync global CLAUDE.md?", true)?;

    let targets = config::SyncTargets {
        sessions: true,
        memories: true,
        settings,
        commands,
        skills,
        global_claude_md: claude_md,
    };

    println!();
    init_with_options(repo.clone(), op_ref, enc_override, targets)?;

    println!();
    println!("git remote setup:");
    println!("  1) create a new private GitHub repo (needs gh CLI)");
    println!("  2) add an existing remote URL");
    println!("  3) skip (local only for now)");
    let remote_choice = input.prompt_with_default("choice [1-3]", "1")?;

    let git_storage = GitStorage::new(repo.clone());
    match remote_choice.trim() {
        "1" => {
            let repo_name = input.prompt_with_default("github repo name", "clync-data")?;
            let gh_result = std::process::Command::new("gh")
                .args([
                    "repo",
                    "create",
                    &repo_name,
                    "--private",
                    "--description",
                    "Encrypted Claude Code sync (managed by clync)",
                ])
                .output();
            match gh_result {
                Ok(output) if output.status.success() => {
                    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    println!("created: {url}");
                    let ssh_url = format!(
                        "git@github.com:{}.git",
                        url.trim_start_matches("https://github.com/")
                    );
                    git_storage.add_remote(&ssh_url)?;
                }
                Ok(output) => {
                    eprintln!(
                        "gh repo create failed: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                Err(_) => {
                    eprintln!("gh CLI not found. Install with: brew install gh");
                }
            }
        }
        "2" => {
            let remote_url = input.prompt("remote url (e.g. git@github.com:you/clync-data.git)")?;
            if !remote_url.is_empty() {
                git_storage.add_remote(&remote_url)?;
                println!("remote added: {remote_url}");
            }
        }
        _ => {
            println!("skipped. add a remote later with:");
            println!("  cd {} && git remote add origin <url>", repo.display());
        }
    }

    println!();
    let do_push = input.prompt_yn("do first push now?", true)?;
    if do_push {
        let config = Config::load()?;
        let keys = Cipher::from_config(&config.encryption)?;
        let filter = ScanFilter::default();

        repo_meta::RepoMeta::from_config(&config).save(&repo)?;

        let result = sync::push(&config, &keys, &filter, &git_storage)?;
        let extras = extras::push_extras(&config, &keys)?;
        let mem = memories::push_memories(&config, &keys)?;
        println!(
            "synced {} sessions, {} extras",
            result.pushed,
            extras.pushed + mem.pushed
        );

        let machine = manifest::get_machine_id();
        let total = result.pushed + extras.pushed + mem.pushed;
        git_storage.commit(&format!("clync init ({total} files) from {machine}"))?;

        if git_storage.has_remote() {
            let do_git_push = input.prompt_yn("git push to remote?", true)?;
            if do_git_push {
                git_storage.push_remote()?;
                println!("pushed to remote");
            }
        }
    }

    println!("\ndone. run `clync sync --git` to sync anytime.");
    println!("add to Claude Code MCP config for in-session access:");
    println!("  clync config path  # shows config location");
    println!("  see `clync mcp` for MCP server setup");

    Ok(())
}

fn init_with_options(
    repo: PathBuf,
    op_ref: Option<String>,
    enc_override: Option<EncryptionConfig>,
    targets: config::SyncTargets,
) -> Result<()> {
    let claude_dir = config::home_dir()
        .context("cannot determine home directory")?
        .join(".claude");

    let config_dir = Config::config_dir()?;
    std::fs::create_dir_all(&config_dir)?;
    let config_path = Config::config_path()?;

    let encryption = if let Some(enc) = enc_override {
        match &enc {
            EncryptionConfig::None => println!("encryption: disabled"),
            EncryptionConfig::Passphrase { env_var } => {
                println!("encryption: passphrase from ${env_var}")
            }
            EncryptionConfig::Bitwarden { item_id, .. } => {
                use crate::crypto::Keys;
                let keys = Keys::generate();
                eprintln!("generated age key pair (will not be shown again)");
                eprintln!("  public:  {}", keys.public_key());
                eprintln!("  secret:  {}", keys.secret_key());
                eprintln!();
                eprintln!("store the secret key in Bitwarden item: {item_id}");
            }
            EncryptionConfig::Pass { entry } => {
                use crate::crypto::Keys;
                let keys = Keys::generate();
                let secret = keys.secret_key();
                eprintln!("generated age key pair (will not be shown again)");
                eprintln!("  secret:  {secret}");
                eprintln!();
                eprintln!("store with: echo '{secret}' | pass insert -m {entry}");
            }
            _ => {}
        }
        enc
    } else if let Some(reference) = op_ref {
        use crate::crypto::Keys;
        let keys = Keys::generate();
        eprintln!("generated age key pair (will not be shown again)");
        eprintln!("  public:  {}", keys.public_key());
        eprintln!("  secret:  {}", keys.secret_key());
        eprintln!();
        eprintln!("store the secret key in 1Password at: {reference}");
        eprintln!("then verify with: op read \"{reference}\"");
        EncryptionConfig::OnePassword { reference }
    } else {
        use crate::crypto::Keys;
        let keys = Keys::generate();
        let key_path = config_dir.join("key.txt");
        write_secret_file(&key_path, &format!("{}\n", keys.secret_key()))?;

        println!("age key saved to {}", key_path.display());
        println!("  public key: {}", keys.public_key());
        EncryptionConfig::KeyFile { path: key_path }
    };

    let config = Config {
        sync: SyncConfig {
            repo: repo.clone(),
            claude_dir,
            include_companion_dirs: false,
            auto_git: true,
            git: Default::default(),
        },
        encryption,
        targets,
    };

    config.save(&config_path)?;
    println!("config saved to {}", config_path.display());

    std::fs::create_dir_all(repo.join("sessions"))?;

    if !repo.join(".git").exists() {
        GitStorage::init_repo(&repo)?;
    }

    ensure_repo_readme(&config)?;

    println!("sync repo ready at {}", repo.display());
    Ok(())
}

pub fn cmd_reset(keep_repo: bool, yes: bool, input: &dyn InputSource) -> Result<()> {
    let config_path = Config::config_path()?;
    if !config_path.exists() {
        println!("no clync config found, nothing to reset");
        return Ok(());
    }

    let config = Config::load()?;
    let config_dir = Config::config_dir()?;
    let repo_path = &config.sync.repo;

    println!("this will remove:");
    println!("  config: {}", config_dir.display());
    if !keep_repo && repo_path.exists() {
        println!("  sync repo: {}", repo_path.display());
    }
    println!();
    println!("sessions in ~/.claude will NOT be touched");

    if !yes {
        let confirm = input.prompt_yn("continue?", false)?;
        if !confirm {
            println!("cancelled");
            return Ok(());
        }
    }

    if !keep_repo && repo_path.exists() {
        std::fs::remove_dir_all(repo_path)
            .with_context(|| format!("could not remove {}", repo_path.display()))?;
        println!("removed {}", repo_path.display());
    }

    std::fs::remove_dir_all(&config_dir)
        .with_context(|| format!("could not remove {}", config_dir.display()))?;
    println!("removed {}", config_dir.display());
    println!("reset complete. run `clync init` or `clync join` to set up again.");

    Ok(())
}

pub(crate) fn prompt_manual_key(
    input: &dyn InputSource,
    config_dir: &std::path::Path,
) -> Result<EncryptionConfig> {
    println!("this repo requires an age key to decrypt");
    println!("provide the same key used on the other machine");
    let key = input.prompt("paste age secret key (AGE-SECRET-KEY-...)")?;
    let key_path = config_dir.join("key.txt");
    write_secret_file(&key_path, &format!("{key}\n"))?;
    Ok(EncryptionConfig::KeyFile { path: key_path })
}

pub(crate) fn write_secret_file(path: &std::path::Path, content: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(content.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, content)?;
    }
    Ok(())
}

pub(crate) fn ensure_repo_readme(config: &Config) -> Result<()> {
    let path = config.sync.repo.join("README.md");
    if path.exists() {
        return Ok(());
    }
    let enc_note = match &config.encryption {
        EncryptionConfig::None => "Files are stored in plain text.",
        EncryptionConfig::Passphrase { .. } => "Files are encrypted with age (passphrase-based).",
        _ => "Files are encrypted with age (key-based).",
    };
    let storage = GitStorage::new(config.sync.repo.clone());
    let ssh_url = storage
        .get_remote_url()
        .unwrap_or_else(|| "<this-repo-url>".to_string());
    let https_url = ssh_to_https(&ssh_url);
    std::fs::write(
        &path,
        format!(
            "# clync sync repo\n\n\
             This repo is managed by [clync](https://github.com/Saturate/clync) \
             and contains synced Claude Code data.\n\n\
             {enc_note}\n\n\
             ## Setup on another machine\n\n\
             ```bash\n\
             cargo install clync\n\n\
             # SSH\n\
             clync join {ssh_url}\n\n\
             # HTTPS\n\
             clync join {https_url}\n\
             ```\n\n\
             See `clync.toml` for sync configuration.\n"
        ),
    )?;
    Ok(())
}

fn ssh_to_https(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("git@") {
        let converted = rest.replacen(':', "/", 1);
        return format!("https://{converted}");
    }
    url.to_string()
}
