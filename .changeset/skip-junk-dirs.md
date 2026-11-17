---
clync: minor
---
Stop syncing `node_modules`, `.git`, `__pycache__` and `.venv` directories, Claude Code's own managed skills (`skills/synced/`) and deleted skills (`skills/.trash/`). Copies already in the sync repo are removed on the next push and are never restored on pull. Customize with the new `sync.ignore` setting: add names to skip, or `!name` to re-enable a default. `clync join` picks up the repo's rules, and `clync config set` now accepts arrays.
