---
clync: patch
---
Merged sessions keep their line order instead of being rewritten in tree order with metadata such as `last-prompt` moved to the end, so Claude Code resumes from the right turn. Rows both copies share stay put, the other machine's new rows follow, and this machine's own rows come last. A copy that adds nothing is replaced by the other one, which also moves sessions reordered by older clync versions back to Claude's order. ISO 8601 timestamps are now parsed, so the newer copy of an edited entry wins.
