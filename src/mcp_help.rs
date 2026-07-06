pub fn help_text(topic: &str) -> String {
    match topic {
        "setup" => "\
# clync setup

1. Install: cargo install clync
2. Initialize: clync init --repo ~/.clync/data
   - Add --onepassword 'op://vault/clync/age-key' for 1Password key storage
3. Add remote: cd ~/.clync/data && git remote add origin <url>
4. First sync: clync push --git

For 1Password: store the printed secret key at the op:// path, then verify with `op read`."
            .into(),
        "sync" => "\
# clync sync commands

  clync push [--git] [--max-age DAYS] [--max-size BYTES]
    Encrypt changed sessions and extras, commit to sync repo.
    --git also runs git add/commit/push.

  clync pull [--git] [--max-age DAYS] [--max-size BYTES]
    Decrypt and smart-merge remote sessions into local.
    --git also runs git pull first.

  clync sync [--git]
    Bidirectional: pull then push.

  clync status [--max-age DAYS]
    Show what's different between local and remote.

Smart merge: when the same session was edited on two machines, clync merges
using UUID-based conversation trees. Same-UUID entries with different content
are resolved by keeping the newer timestamp."
            .into(),
        "list" => "\
# clync list

  clync list [QUERY] [--max-age DAYS] [-n LIMIT] [--json]

Search sessions by project name, UUID, or first message content.
Results are sorted by most recently modified.

Examples:
  clync list                    # show 20 most recent sessions
  clync list security           # search for 'security'
  clync list --max-age 7        # last week only
  clync list --json -n 5        # JSON output, 5 results"
            .into(),
        "mcp" => "\
# clync MCP server

Run as a stdio MCP server for Claude Code integration:

  clync mcp

Add to Claude Code's MCP config:
  {
    \"mcpServers\": {
      \"clync\": {
        \"command\": \"clync\",
        \"args\": [\"mcp\"]
      }
    }
  }

Available tools:
  list_sessions   - search sessions by project/UUID/content
  session_detail  - get details and recent messages for a session
  sync_status     - show local vs remote diff
  sync_push       - encrypt and push to sync repo
  sync_pull       - pull and decrypt from sync repo
  config_show     - show current configuration
  help            - this help (with optional topic)"
            .into(),
        "config" => "\
# clync configuration

Config location: ~/Library/Application Support/clync/config.toml (macOS)
                  ~/.config/clync/config.toml (Linux)

[sync]
repo = '~/.clync/data'           # path to the git sync repo
claude_dir = '~/.claude'         # claude code data directory
include_companion_dirs = false   # sync subagent/tool-result dirs

[sync.git]
lfs_threshold = 103809024        # auto-track sessions over 99MB with git-lfs (0 = disabled)

[encryption]
method = 'key_file'              # or 'onepassword'
path = '~/.config/clync/key.txt' # age secret key file
# reference = 'op://vault/item'  # for 1Password method

[targets]
sessions = true          # conversation JSONL files
memories = true          # project memory files
settings = false         # settings.json, settings.local.json
commands = false         # custom slash commands
skills = false           # custom skills
global_claude_md = false # ~/.claude/CLAUDE.md"
            .into(),
        _ => "\
# clync - encrypted sync for Claude Code

Commands:
  clync init     Initialize config, generate age key, set up sync repo
  clync push     Encrypt and push to sync repo
  clync pull     Decrypt and smart-merge from sync repo
  clync sync     Bidirectional sync (pull + push)
  clync status   Show what differs between local and remote
  clync list     Search and browse local sessions
  clync mcp      Run as MCP server (stdio JSON-RPC)

Help topics: setup, sync, list, mcp, config
  clync help           # or via MCP: help tool with topic param

Encryption: age (https://age-encryption.org)
Key storage: local file or 1Password CLI (op://)"
            .into(),
    }
}
