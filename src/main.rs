mod cmd;
mod config;
mod crypto;
mod extras;
pub(crate) mod fileutil;
pub(crate) mod io;
mod lfs;
mod list;
mod manifest;
mod mcp;
mod mcp_help;
mod memories;
mod merge;
mod parser;
mod repo_meta;
mod resolver;
mod scanner;
pub(crate) mod secret;
mod storage;
mod sync;
mod synclog;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use io::StdioInput;

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
    /// Remove clync config and optionally the sync repo. Sessions in ~/.claude are untouched.
    Reset {
        /// Keep the local sync repo (only remove config and key)
        #[arg(long)]
        keep_repo: bool,

        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Run as MCP server (stdio JSON-RPC)
    Mcp,
}

#[derive(Subcommand)]
pub(crate) enum ConfigAction {
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
        } => cmd::init::cmd_init(repo, onepassword, no_encrypt, &input),
        Cmd::Push {
            no_git,
            max_age,
            max_size,
        } => cmd::sync_cmd::cmd_push(no_git, cmd::build_filter(max_age, max_size)),
        Cmd::Pull {
            no_git,
            max_age,
            max_size,
        } => cmd::sync_cmd::cmd_pull(no_git, cmd::build_filter(max_age, max_size)),
        Cmd::Sync {
            no_git,
            max_age,
            max_size,
        } => {
            let filter = cmd::build_filter(max_age, max_size);
            cmd::sync_cmd::cmd_pull(no_git, filter.clone())?;
            cmd::sync_cmd::cmd_push(no_git, filter)
        }
        Cmd::Status { max_age } => cmd::sync_cmd::cmd_status(cmd::build_filter(max_age, None)),
        Cmd::List {
            query,
            max_age,
            limit,
            json,
        } => cmd::cmd_list(query, max_age, limit, json),
        Cmd::Log { limit, json } => cmd::cmd_log(limit, json),
        Cmd::Config { action } => cmd::cmd_config(action),
        Cmd::Join {
            url,
            repo,
            onepassword,
            no_encrypt,
        } => cmd::join::cmd_join(url, repo, onepassword, no_encrypt, &input),
        Cmd::Reset { keep_repo, yes } => cmd::init::cmd_reset(keep_repo, yes, &input),
        Cmd::Mcp => mcp::run_mcp_server(),
    }
}
