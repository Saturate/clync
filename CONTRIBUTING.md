# Contributing

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings -W clippy::too_many_lines
```

## Code guidelines

- File length limit: 600 lines per `.rs` file (CI enforced)
- Function length limit: 200 lines (clippy enforced)
- Use conventional commits: `feat:`, `fix:`, `refactor:`, `chore:`
- Comments explain WHY, not what

## Changesets

This project uses [cargo-changeset](https://crates.io/crates/cargo-changeset) for release management. Every PR that changes user-facing behavior needs a changeset file.

```bash
cargo install cargo-changeset

# Add a changeset (interactive)
cargo changeset add

# Or non-interactive
cargo changeset add -p clync -b patch -c fixed -m "Description of what changed for users"
```

**Bump types:**
- `patch` - bug fixes, small improvements
- `minor` - new features (maps to patch in pre-1.0)
- `major` - breaking changes (maps to minor in pre-1.0)

**Categories:** `added`, `changed`, `deprecated`, `removed`, `fixed`, `security`

Write descriptions for users, not developers. "Fixed memories not syncing between machines" is better than "fix: normalize project paths in extras module".

## Releasing

Releases are automated. When changesets accumulate on main:

1. Run the Release workflow from GitHub Actions (no inputs needed)
2. It consumes pending changesets, bumps the version, updates CHANGELOG.md
3. A tag push triggers binary builds and a GitHub release with the changelog entry
4. Homebrew tap and crates.io are updated automatically

## Project structure

```
src/
  main.rs          CLI definition and dispatch
  cmd/
    init.rs        init and reset commands
    join.rs        join command
    sync_cmd.rs    push, pull, status commands
    mod.rs         list, log, config commands + formatting helpers
  sync.rs          Session sync engine (push/pull/merge)
  memories.rs      Memory sync with path normalization
  extras.rs        Settings, commands, skills sync
  fileutil.rs      Shared file helpers (encrypt, restore, sync)
  mcp.rs           MCP server (stdio JSON-RPC)
  mcp_help.rs      MCP help text
  config.rs        Config types and loading
  crypto.rs        age encryption/decryption
  lfs.rs           Git LFS support
  manifest.rs      Session manifest and path normalization
  resolver.rs      Cross-machine project path resolution
  scanner.rs       Local session discovery
  storage.rs       Git storage provider
  merge.rs         UUID-tree session merge
  parser.rs        JSONL session parser
```
