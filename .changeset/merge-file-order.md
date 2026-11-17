---
clync: patch
---
Merged sessions keep their original line order instead of being rewritten in tree order with metadata such as `last-prompt` moved to the end, so Claude Code resumes from the right turn. Entries that only exist on the remote go right after the last entry both sides share. ISO 8601 timestamps are now parsed, so the newer copy of an edited entry wins.
