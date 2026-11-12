---
clync: patch
---
Stop syncing `node_modules`, `.git`, `__pycache__` and `.venv` directories, Claude Code's own managed skills (`skills/synced/`) and deleted skills (`skills/.trash/`). Copies already in the sync repo are removed on the next push and are never restored on pull, which keeps Claude Code's managed skills from being overwritten with stale versions.
