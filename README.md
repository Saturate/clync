# clync

Encrypted sync for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) across machines.

![Demo](demo.gif)

## What it does

Claude Code stores conversations, memories, settings, commands, and skills locally in `~/.claude/`. clync encrypts all of it with [age](https://age-encryption.org) and syncs it through a git repo. When the same session is edited on two machines, clync merges them using the conversation's UUID tree structure instead of overwriting.

Great for backup, and for people using more than one machine.

## Features

| Feature | Description |
|---------|-------------|
| **Encryption** | age-based encryption with key management options |
| **Smart merge** | UUID tree merge for diverged conversations |
| **Full sync** | Sessions, memories, settings, commands, skills, CLAUDE.md |
| **Parallel** | rayon-based parallel encrypt/decrypt |
| **MCP server** | 8 tools for Claude Code integration |
| **Multi-machine** | `join` command for second machine setup |
| **Key management** | Local key file, passphrase, 1Password, Bitwarden, pass, or none |
| **Auto git** | Commits and pushes automatically by default |

## Install

```bash
cargo install clync
```

Or from source:

```bash
git clone https://github.com/Saturate/clync
cd clync
cargo install --path .
```

## Quick start

### First machine

```bash
clync init
```

The interactive setup walks you through:
1. Sync repo path
2. Encryption method (key file, passphrase, 1Password, Bitwarden, pass, or none)
3. What to sync (everything by default)
4. Git remote (can create a private GitHub repo via `gh` cli)
5. First push

Or non-interactive for scripting:

```bash
# With 1Password
clync init --repo ~/clync-repo --onepassword "op://Personal/clync/age-secret-key"

# With no encryption (use a private repo, but I still do not recommend this as sessions have a lot of senstive data.)
clync init --repo ~/clync-repo --no-encrypt
```

### Second machine

```bash
clync join git@github.com:you/clync-data.git
```

This clones the repo, reads `clync.toml` to detect the encryption method, asks for the key, and pulls all sessions.

### Sync

```bash
clync sync       # pull + push (auto git by default)
clync push       # encrypt and push
clync pull       # pull and smart-merge
clync status     # see what's different
```

## Commands

| Command | Description |
|---------|-------------|
| `init` | Interactive setup (or flag-driven) |
| `join <url>` | Set up on a new machine from existing repo |
| `push` | Encrypt and push changes |
| `pull` | Pull and smart-merge |
| `sync` | Pull then push |
| `status` | Show diff between local and remote |
| `list [query]` | Search sessions by project, UUID, or content |
| `log` | Show sync history (machine, operation, counts) |
| `config` | `show`, `edit`, `path`, `set key value` |
| `mcp` | Run as stdio MCP server |

## Merge

When both machines edit the same session, clync merges using the conversation structure:

- Each message has a UUID and parent UUID forming a tree
- Messages unique to one side are included
- Same UUID with different content: newer timestamp wins
- Non-UUID entries (metadata) are deduplicated

No data is lost. Both machines' messages are preserved.

## Encryption

Six modes, pick during `clync init`:

| Mode | Key source | Dependencies |
|------|-----------|--------------|
| **Key file** | `~/.config/clync/key.txt` | None |
| **Passphrase** | Environment variable | None |
| **1Password** | `op read "op://..."` | `op` CLI |
| **Bitwarden** | `bw get` | `bw` CLI |
| **pass** | `pass show` | `pass` |
| **None** | No encryption | Use private repo |

The age secret key is never stored in the sync repo. With password managers, the key syncs automatically across machines.

## Configuration

Config lives at `~/.config/clync/config.toml`:

```toml
[sync]
repo = "~/clync-repo"
claude_dir = "~/.claude"
include_companion_dirs = false
auto_git = true

[encryption]
method = "onepassword"
reference = "op://Personal/clync/age-secret-key"

[targets]
sessions = true
memories = true
settings = true
commands = true
skills = true
global_claude_md = true
```

The sync repo also contains a plaintext `clync.toml` with the encryption method and targets, so `clync join` knows what to expect.

## MCP server

Add to your Claude Code MCP config:

```json
{
  "mcpServers": {
    "clync": {
      "command": "clync",
      "args": ["mcp"]
    }
  }
}
```

Tools: `list_sessions`, `session_detail`, `sync_status`, `sync_push`, `sync_pull`, `sync_log`, `config_show`, `help`

## Sync repo structure

```
clync.toml              # plaintext metadata (encryption method, targets)
README.md               # auto-generated
manifest.json.age       # encrypted session index (or .json if unencrypted)
sync.log.jsonl          # plaintext sync history
sessions/
  <uuid>.jsonl.age      # encrypted session files
extras/
  settings.json.age     # encrypted settings
  CLAUDE.md.age         # encrypted global instructions
  commands/             # encrypted custom commands
  skills/               # encrypted custom skills
  memories/
    <project>/          # encrypted project memories
```

## License

MIT
