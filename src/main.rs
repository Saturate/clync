mod config;
mod crypto;
mod extras;
pub(crate) mod io;
mod list;
mod manifest;
mod mcp;
mod merge;
mod parser;
mod repo_meta;
mod resolver;
mod scanner;
pub(crate) mod secret;
mod storage;
mod sync;
mod synclog;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use config::{Config, EncryptionConfig, SyncConfig};
use crypto::Cipher;
use io::{InputSource, StdioInput};
use scanner::ScanFilter;
use storage::GitStorage;

const BANNER: &str = concat!(
    "\n",
    "          ░██\n",
    "           ░██\n",
    " ░███████  ░██ ░██    ░██ ░████████   ░███████\n",
    "░██    ░██ ░██ ░██    ░██ ░██    ░██ ░██    ░██\n",
    "░██        ░██ ░██    ░██ ░██    ░██ ░██\n",
    "░██    ░██ ░██ ░██   ░███ ░██    ░██ ░██    ░██\n",
    " ░███████  ░██  ░█████░██ ░██    ░██  ░███████\n",
    "                      ░██\n",
    "                ░███████   v",
    env!("CARGO_PKG_VERSION"),
    "\n",
);

#[derive(Parser)]
#[command(
    name = "clync",
    about = "Encrypted sync for Claude Code across machines",
    version,
    before_help = BANNER
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Initialize config, generate encryption key, set up sync repo.
    /// Run without flags for interactive setup.
    Init {
        /// Path to the sync git repo
        #[arg(long)]
        repo: Option<PathBuf>,

        /// Use 1Password for key storage (pass an op:// reference)
        #[arg(long, value_name = "OP_REF")]
        onepassword: Option<String>,

        /// Skip encryption (store files in plain text)
        #[arg(long)]
        no_encrypt: bool,
    },
    /// Encrypt and commit changed data to the sync repo
    Push {
        /// Skip git commit/push (overrides auto_git config)
        #[arg(long)]
        no_git: bool,

        /// Only sync sessions modified within N days
        #[arg(long, value_name = "DAYS")]
        max_age: Option<u64>,

        /// Skip sessions larger than N bytes
        #[arg(long, value_name = "BYTES")]
        max_size: Option<u64>,
    },
    /// Decrypt and smart-merge remote data into local
    Pull {
        /// Skip git pull (overrides auto_git config)
        #[arg(long)]
        no_git: bool,

        /// Only sync sessions modified within N days
        #[arg(long, value_name = "DAYS")]
        max_age: Option<u64>,

        /// Skip sessions larger than N bytes
        #[arg(long, value_name = "BYTES")]
        max_size: Option<u64>,
    },
    /// Pull then push (bidirectional sync)
    Sync {
        /// Skip git operations (overrides auto_git config)
        #[arg(long)]
        no_git: bool,

        /// Only sync sessions modified within N days
        #[arg(long, value_name = "DAYS")]
        max_age: Option<u64>,

        /// Skip sessions larger than N bytes
        #[arg(long, value_name = "BYTES")]
        max_size: Option<u64>,
    },
    /// Show what differs between local and remote
    Status {
        /// Only check sessions modified within N days
        #[arg(long, value_name = "DAYS")]
        max_age: Option<u64>,
    },
    /// List local sessions with optional search
    List {
        /// Search by project name, UUID, or first message content
        #[arg(value_name = "QUERY")]
        query: Option<String>,

        /// Only show sessions modified within N days
        #[arg(long, value_name = "DAYS")]
        max_age: Option<u64>,

        /// Max results to show
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show or update configuration
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Show recent sync operations
    Log {
        /// Number of entries to show
        #[arg(short = 'n', default_value = "10")]
        limit: usize,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Set up clync on a new machine by cloning an existing sync repo
    Join {
        /// Git URL of the sync repo
        url: String,

        /// Local path for the cloned repo
        #[arg(long)]
        repo: Option<PathBuf>,

        /// Use 1Password for key storage
        #[arg(long, value_name = "OP_REF")]
        onepassword: Option<String>,

        /// Skip encryption (for repos with encryption=none)
        #[arg(long)]
        no_encrypt: bool,
    },
    /// Run as MCP server (stdio JSON-RPC)
    Mcp,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current config
    Show,
    /// Open config file in $EDITOR
    Edit,
    /// Show config file path
    Path,
    /// Set a config value (e.g. targets.skills true)
    Set {
        /// Key in dot notation (e.g. targets.skills, sync.include_companion_dirs)
        key: String,
        /// Value to set
        value: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let input = StdioInput;

    match cli.command {
        Cmd::Init {
            repo,
            onepassword,
            no_encrypt,
        } => cmd_init(repo, onepassword, no_encrypt, &input),
        Cmd::Push {
            no_git,
            max_age,
            max_size,
        } => cmd_push(no_git, build_filter(max_age, max_size)),
        Cmd::Pull {
            no_git,
            max_age,
            max_size,
        } => cmd_pull(no_git, build_filter(max_age, max_size)),
        Cmd::Sync {
            no_git,
            max_age,
            max_size,
        } => {
            let filter = build_filter(max_age, max_size);
            cmd_pull(no_git, filter.clone())?;
            cmd_push(no_git, filter)
        }
        Cmd::Status { max_age } => cmd_status(build_filter(max_age, None)),
        Cmd::List {
            query,
            max_age,
            limit,
            json,
        } => cmd_list(query, max_age, limit, json),
        Cmd::Log { limit, json } => cmd_log(limit, json),
        Cmd::Config { action } => cmd_config(action),
        Cmd::Join {
            url,
            repo,
            onepassword,
            no_encrypt,
        } => cmd_join(url, repo, onepassword, no_encrypt, &input),
        Cmd::Mcp => mcp::run_mcp_server(),
    }
}

fn build_filter(max_age: Option<u64>, max_size: Option<u64>) -> ScanFilter {
    ScanFilter {
        max_age_days: max_age,
        max_file_size: max_size,
    }
}

fn cmd_init(
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
        println!(
            "synced {} sessions, {} extras",
            result.pushed, extras.pushed
        );

        let machine = manifest::get_machine_id();
        let total = result.pushed + extras.pushed;
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
                use crypto::Keys;
                let keys = Keys::generate();
                eprintln!("generated age key pair (will not be shown again)");
                eprintln!("  public:  {}", keys.public_key());
                eprintln!("  secret:  {}", keys.secret_key());
                eprintln!();
                eprintln!("store the secret key in Bitwarden item: {item_id}");
            }
            EncryptionConfig::Pass { entry } => {
                use crypto::Keys;
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
        use crypto::Keys;
        let keys = Keys::generate();
        eprintln!("generated age key pair (will not be shown again)");
        eprintln!("  public:  {}", keys.public_key());
        eprintln!("  secret:  {}", keys.secret_key());
        eprintln!();
        eprintln!("store the secret key in 1Password at: {reference}");
        eprintln!("then verify with: op read \"{reference}\"");
        EncryptionConfig::OnePassword { reference }
    } else {
        use crypto::Keys;
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

pub struct PushOutput {
    pub sessions: u32,
    pub skipped: u32,
    pub extras: u32,
}

pub fn do_push(use_git: bool) -> Result<PushOutput> {
    let config = Config::load()?;
    let cipher = Cipher::from_config(&config.encryption)?;
    let storage = GitStorage::new(config.sync.repo.clone());

    let (pushed, skipped, extras) = {
        let _lock = storage.try_lock()?;

        repo_meta::RepoMeta::from_config(&config).save(&config.sync.repo)?;
        ensure_repo_readme(&config)?;

        let filter = ScanFilter::default();
        let result = sync::push(&config, &cipher, &filter, &storage)?;
        let extras_result = extras::push_extras(&config, &cipher)?;

        let mut log = synclog::SyncLogEntry::new("push");
        log.sessions_pushed = result.pushed;
        log.sessions_skipped = result.skipped;
        log.extras = extras_result.pushed;
        synclog::append(&config.sync.repo, &log).ok();

        (result.pushed, result.skipped, extras_result.pushed)
    };

    if use_git && (pushed > 0 || extras > 0) {
        let machine = manifest::get_machine_id();
        let mut parts = Vec::new();
        if pushed > 0 {
            parts.push(format!("{pushed} sessions"));
        }
        if extras > 0 {
            parts.push(format!("{extras} extras"));
        }
        let msg = format!("clync push ({}) from {machine}", parts.join(", "));
        storage.commit(&msg)?;
        storage.push_remote()?;
    }

    Ok(PushOutput {
        sessions: pushed,
        skipped,
        extras,
    })
}

pub struct PullOutput {
    pub pulled: u32,
    pub merged: u32,
    pub skipped: u32,
    pub extras: u32,
}

pub fn do_pull(use_git: bool) -> Result<PullOutput> {
    let config = Config::load()?;
    let storage = GitStorage::new(config.sync.repo.clone());

    if use_git {
        storage.pull_remote()?;
    }

    let (pulled, merged, skipped, extras) = {
        let _lock = storage.try_lock()?;

        let cipher = Cipher::from_config(&config.encryption)?;
        let filter = ScanFilter::default();
        let result = sync::pull(&config, &cipher, &filter, &storage)?;
        let extras_result = extras::pull_extras(&config, &cipher)?;

        let mut log = synclog::SyncLogEntry::new("pull");
        log.sessions_pulled = result.pulled;
        log.sessions_merged = result.merged;
        log.sessions_skipped = result.skipped;
        log.extras = extras_result.pulled;
        synclog::append(&config.sync.repo, &log).ok();

        (
            result.pulled,
            result.merged,
            result.skipped,
            extras_result.pulled,
        )
    };

    if use_git {
        let machine = manifest::get_machine_id();
        storage.commit(&format!("clync pull from {machine}")).ok();
        storage.push_remote().ok();
    }

    Ok(PullOutput {
        pulled,
        merged,
        skipped,
        extras,
    })
}

fn cmd_push(no_git: bool, filter: ScanFilter) -> Result<()> {
    let config = Config::load()?;
    let cipher = Cipher::from_config(&config.encryption)?;
    let storage = GitStorage::new(config.sync.repo.clone());
    let _lock = storage.lock()?;
    let use_git = config.sync.auto_git && !no_git;

    repo_meta::RepoMeta::from_config(&config).save(&config.sync.repo)?;
    ensure_repo_readme(&config)?;

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

    let mut log = synclog::SyncLogEntry::new("push");
    log.sessions_pushed = result.pushed;
    log.sessions_skipped = result.skipped;
    log.extras = extras_result.pushed;
    synclog::append(&config.sync.repo, &log).ok();

    if use_git && (result.pushed > 0 || extras_result.pushed > 0) {
        let machine = manifest::get_machine_id();
        let mut parts = Vec::new();
        if result.pushed > 0 {
            parts.push(format!("{} sessions", result.pushed));
        }
        if extras_result.pushed > 0 {
            parts.push(format!("{} extras", extras_result.pushed));
        }
        storage.commit(&format!("clync push ({}) from {machine}", parts.join(", ")))?;
        storage.push_remote()?;
    }

    Ok(())
}

fn cmd_pull(no_git: bool, filter: ScanFilter) -> Result<()> {
    let config = Config::load()?;
    let storage = GitStorage::new(config.sync.repo.clone());
    let _lock = storage.lock()?;
    let use_git = config.sync.auto_git && !no_git;

    if use_git {
        storage.pull_remote()?;
    }

    let cipher = Cipher::from_config(&config.encryption)?;
    let result = sync::pull(&config, &cipher, &filter, &storage)?;
    println!(
        "pull: {} new, {} merged, {} unchanged",
        result.pulled, result.merged, result.skipped
    );

    let extras_result = extras::pull_extras(&config, &cipher)?;
    if extras_result.pulled > 0 {
        println!("pull: {} extra files restored", extras_result.pulled);
    }

    let mut log = synclog::SyncLogEntry::new("pull");
    log.sessions_pulled = result.pulled;
    log.sessions_merged = result.merged;
    log.sessions_skipped = result.skipped;
    log.extras = extras_result.pulled;
    synclog::append(&config.sync.repo, &log).ok();

    if use_git {
        let machine = manifest::get_machine_id();
        storage.commit(&format!("clync pull from {machine}")).ok();
        storage.push_remote().ok();
    }

    Ok(())
}

fn cmd_status(filter: ScanFilter) -> Result<()> {
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
            println!("  + {} [{}] {}B", short_uuid(&s.uuid), s.project, s.size);
        }
    }
    if !result.remote_only.is_empty() {
        println!("remote only ({}, pull to sync):", result.remote_only.len());
        for s in &result.remote_only {
            println!("  - {} [{}] {}B", short_uuid(&s.uuid), s.project, s.size);
        }
    }
    if !result.diverged.is_empty() {
        println!(
            "diverged ({}, pull will smart-merge):",
            result.diverged.len()
        );
        for s in &result.diverged {
            println!("  ~ {} [{}] {}B", short_uuid(&s.uuid), s.project, s.size);
        }
    }
    if result.in_sync > 0 {
        println!("in sync: {}", result.in_sync);
    }

    Ok(())
}

fn cmd_list(query: Option<String>, max_age: Option<u64>, limit: usize, json: bool) -> Result<()> {
    let config = Config::load()?;
    let filter = ScanFilter {
        max_age_days: max_age,
        max_file_size: None,
    };

    let sessions = list::list_sessions(&config.claude_projects_dir(), query.as_deref(), &filter)?;
    let limited: Vec<_> = sessions.into_iter().take(limit).collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&limited)?);
        return Ok(());
    }

    if limited.is_empty() {
        println!("no sessions found");
        return Ok(());
    }

    for s in &limited {
        let age = format_age(s.mtime);
        let size = format_size(s.size_bytes);
        let preview = s
            .first_message
            .as_deref()
            .unwrap_or("(no user message)")
            .replace('\n', " ");
        let preview = truncate_safe(&preview, 80);

        println!(
            "{} [{}] {} | {} msgs | {}",
            short_uuid(&s.uuid),
            s.project,
            age,
            s.messages,
            size
        );
        println!("  {preview}");
    }

    Ok(())
}

fn cmd_log(limit: usize, json: bool) -> Result<()> {
    let config = Config::load()?;
    let entries = synclog::read_recent(&config.sync.repo, limit)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("no sync history yet");
        return Ok(());
    }

    for entry in &entries {
        let age = format_age(entry.timestamp);
        let mut parts = Vec::new();
        if entry.sessions_pushed > 0 {
            parts.push(format!("{} pushed", entry.sessions_pushed));
        }
        if entry.sessions_pulled > 0 {
            parts.push(format!("{} pulled", entry.sessions_pulled));
        }
        if entry.sessions_merged > 0 {
            parts.push(format!("{} merged", entry.sessions_merged));
        }
        if entry.extras > 0 {
            parts.push(format!("{} extras", entry.extras));
        }
        let summary = if parts.is_empty() {
            "no changes".into()
        } else {
            parts.join(", ")
        };
        println!(
            "{} {} [{}] {}",
            age, entry.operation, entry.machine, summary
        );
    }

    Ok(())
}

fn cmd_config(action: Option<ConfigAction>) -> Result<()> {
    let action = action.unwrap_or(ConfigAction::Show);
    match action {
        ConfigAction::Show => {
            let path = Config::config_path()?;
            if !path.exists() {
                bail!("no config found. Run `clync init` first.");
            }
            let config = Config::load()?;
            let enc_method = match &config.encryption {
                EncryptionConfig::KeyFile { path } => format!("key_file ({})", path.display()),
                EncryptionConfig::Passphrase { env_var } => format!("passphrase (${env_var})"),
                EncryptionConfig::OnePassword { reference } => format!("1password ({reference})"),
                EncryptionConfig::Bitwarden { item_id, .. } => format!("bitwarden ({item_id})"),
                EncryptionConfig::Pass { entry } => format!("pass ({entry})"),
                EncryptionConfig::None => "none (plain text)".into(),
            };
            let t = &config.targets;
            println!("sync repo:       {}", config.sync.repo.display());
            println!("claude dir:      {}", config.sync.claude_dir.display());
            println!("encryption:      {enc_method}");
            println!("auto git:        {}", config.sync.auto_git);
            println!("companion dirs:  {}", config.sync.include_companion_dirs);
            println!();
            println!("targets:");
            println!("  sessions:        {}", t.sessions);
            println!("  memories:        {}", t.memories);
            println!("  settings:        {}", t.settings);
            println!("  commands:        {}", t.commands);
            println!("  skills:          {}", t.skills);
            println!("  global CLAUDE.md: {}", t.global_claude_md);
        }
        ConfigAction::Edit => {
            let path = Config::config_path()?;
            if !path.exists() {
                bail!("no config found. Run `clync init` first.");
            }
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
            std::process::Command::new(&editor)
                .arg(&path)
                .status()
                .with_context(|| format!("failed to open {editor}"))?;
        }
        ConfigAction::Path => {
            let path = Config::config_path()?;
            println!("{}", path.display());
        }
        ConfigAction::Set { key, value } => {
            let path = Config::config_path()?;
            if !path.exists() {
                bail!("no config found. Run `clync init` first.");
            }
            let contents = std::fs::read_to_string(&path)?;
            let mut doc: toml::Table = toml::from_str(&contents).context("invalid config")?;

            set_toml_value(&mut doc, &key, &value)?;

            let new_contents = toml::to_string_pretty(&doc)?;

            let _: Config = toml::from_str(&new_contents)
                .context("invalid config after set. Check key and value types.")?;

            std::fs::write(&path, new_contents)?;
            println!("set {key} = {value}");
        }
    }
    Ok(())
}

fn set_toml_value(table: &mut toml::Table, key: &str, value: &str) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() == 2 {
        let section = table
            .get_mut(parts[0])
            .and_then(|v| v.as_table_mut())
            .with_context(|| format!("section '{}' not found", parts[0]))?;

        let parsed: toml::Value = if value == "true" {
            toml::Value::Boolean(true)
        } else if value == "false" {
            toml::Value::Boolean(false)
        } else if let Ok(n) = value.parse::<i64>() {
            toml::Value::Integer(n)
        } else {
            toml::Value::String(value.to_string())
        };

        section.insert(parts[1].to_string(), parsed);
        Ok(())
    } else {
        bail!("key must be in section.field format (e.g. targets.skills)")
    }
}

fn cmd_join(
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

    println!("cloning sync repo...");
    let git_storage = GitStorage::clone_repo(&url, &repo)?;

    let has_files = repo.join("clync.toml").exists() || repo.join("manifest.json").exists();
    if !has_files {
        git_storage.checkout_first_branch()?;
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
    } else {
        println!("this repo requires an age key to decrypt");
        println!("provide the same key used on the other machine");
        let key = input.prompt("paste age secret key (AGE-SECRET-KEY-...)")?;
        let key_path = config_dir.join("key.txt");
        write_secret_file(&key_path, &format!("{key}\n"))?;
        EncryptionConfig::KeyFile { path: key_path }
    };

    let claude_dir = config::home_dir()
        .context("cannot determine home directory")?
        .join(".claude");

    let config = Config {
        sync: SyncConfig {
            repo: repo.clone(),
            claude_dir,
            include_companion_dirs: false,
            auto_git: true,
        },
        encryption,
        targets: Default::default(),
    };

    config.save(&config_path)?;
    println!("config saved to {}", config_path.display());

    let do_pull = input.prompt_yn("pull sessions now?", true)?;
    println!();
    if do_pull {
        let cipher = Cipher::from_config(&config.encryption)?;
        let filter = ScanFilter::default();
        let result = sync::pull(&config, &cipher, &filter, &git_storage)?;
        let extras = extras::pull_extras(&config, &cipher)?;
        println!(
            "pulled {} sessions, {} merged, {} extras",
            result.pulled, result.merged, extras.pulled
        );
    }

    println!("\ndone. run `clync sync --git` to sync anytime.");
    Ok(())
}

fn short_uuid(uuid: &str) -> &str {
    let mut end = uuid.len().min(8);
    while end > 0 && !uuid.is_char_boundary(end) {
        end -= 1;
    }
    &uuid[..end]
}

fn ensure_repo_readme(config: &Config) -> Result<()> {
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

fn write_secret_file(path: &std::path::Path, content: &str) -> Result<()> {
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

fn truncate_safe(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max.saturating_sub(3);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

fn format_age(mtime: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let diff = now.saturating_sub(mtime);
    if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

fn ssh_to_https(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("git@") {
        let converted = rest.replacen(':', "/", 1);
        return format!("https://{converted}");
    }
    url.to_string()
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
