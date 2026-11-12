# Contributing

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings -W clippy::too_many_lines
```

CI runs the tests on Linux, macOS and Windows. Build paths with `Path::join` rather than string concatenation, and don't assume `/` is the only separator.

## Code guidelines

- File length limit: 600 lines per `.rs` file (CI enforced)
- Function length limit: 200 lines (clippy enforced)
- Use conventional commits: `feat:`, `fix:`, `refactor:`, `chore:`
- Comments explain WHY, not what

## Changesets

This project uses [changesetter](https://crates.io/crates/changesetter) for release management. Every PR needs a changeset file in `.changeset/` (CI runs `changesetter check`):

```markdown
---
clync: patch
---
Fixed memories not syncing between machines
```

**Bump types:**
- `patch` - bug fixes, small improvements
- `minor` - new features
- `major` - breaking changes
- `none` - no release (CI, docs, internal changes)

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
  gitcmd.rs        `git` command builder (long paths on Windows)
  store/
    mod.rs         Store trait, LocalFs helper, factory
    git.rs         Git storage backend
    folder.rs      Local folder storage backend
    s3.rs          S3-compatible storage backend (feature-gated)
  mcp.rs           MCP server (stdio JSON-RPC)
  mcp_help.rs      MCP help text
  config.rs        Config types and loading
  crypto.rs        age encryption/decryption
  lfs.rs           Git LFS support
  manifest.rs      Session manifest and path normalization
  resolver.rs      Cross-machine project path resolution
  scanner.rs       Local session discovery
  merge.rs         UUID-tree session merge
  parser.rs        JSONL session parser
```
