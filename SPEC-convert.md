# SPEC: `clync convert` - Session format conversion

## Problem

Users work across multiple AI coding tools (Claude Code, opencode, pi). Each stores conversation sessions in its own format. There's no way to move sessions between tools, which locks conversation history into a single tool.

## Goal

Add `clync convert` to translate sessions between Claude Code and opencode (bidirectional). The converted sessions should be browsable in the target tool and contain the full conversation with tool call history.

## CLI

```bash
# Claude Code -> opencode
clync convert --from claude --to opencode <session-uuid-or-path>
clync convert --from claude --to opencode --all          # convert all sessions
clync convert --from claude --to opencode --project <dir> # sessions for one project

# opencode -> Claude Code  
clync convert --from opencode --to claude <session-id>
clync convert --from opencode --to claude --all

# List available sessions from a source
clync convert --from claude --list
clync convert --from opencode --list

# Dry run (show what would be converted, don't write)
clync convert --from claude --to opencode --dry-run <session>
```

## Format Details

### Claude Code sessions

**Location:** `~/.claude/projects/<encoded-project-dir>/<uuid>.jsonl`

**Storage:** One JSONL file per session. Each line is a JSON object. Lines form a tree via `uuid`/`parentUuid` fields.

**Entry types** (the `type` field):

| Type | Purpose | Convertible |
|------|---------|-------------|
| `user` | User message | Yes |
| `assistant` | Assistant response (text, tool_use, thinking) | Yes |
| `system` | Hook summaries, stop reasons | No (Claude-specific) |
| `attachment` | Hook context injections | No |
| `mode` | Agent mode (normal/plan/etc) | No |
| `permission-mode` | Permission level | No |
| `bridge-session` | Cloud session linking | No |
| `ai-title` | Auto-generated title | Yes (as session title) |
| `last-prompt` | Cursor state | No |

**User message structure:**
```json
{
  "parentUuid": "previous-uuid",
  "isSidechain": false,
  "type": "user",
  "uuid": "this-uuid",
  "message": {
    "role": "user",
    "content": "the user's text"
  }
}
```

Note: `content` can be a string or an array of content blocks.

**Assistant message structure:**
```json
{
  "parentUuid": "user-uuid",
  "isSidechain": false,
  "uuid": "this-uuid",
  "message": {
    "model": "claude-opus-4-8",
    "id": "msg_...",
    "role": "assistant",
    "content": [
      {"type": "thinking", "thinking": "...", "signature": "..."},
      {"type": "text", "text": "response text"},
      {"type": "tool_use", "id": "toolu_...", "name": "Bash", "input": {"command": "ls"}}
    ],
    "stop_reason": "end_turn|tool_use",
    "usage": {
      "input_tokens": 14275,
      "output_tokens": 277,
      "cache_read_input_tokens": 16773,
      "cache_creation_input_tokens": 5507
    }
  }
}
```

**Tool result** (separate entry, follows the assistant tool_use):
```json
{
  "parentUuid": "assistant-uuid",
  "type": "tool_result",
  "uuid": "result-uuid",
  "tool_use_id": "toolu_...",
  "content": "command output here"
}
```

Note: Tool results don't have a `type` field at the top level sometimes; identify them by presence of `tool_use_id`. The `content` can be a string or array of content blocks.

**Claude tool names:** `Bash`, `Read`, `Write`, `Edit`, `Grep`, `Agent`, `WebFetch`, `WebSearch`, `AskUserQuestion`, plus MCP tool names.

### opencode sessions

**Location:** `~/.local/share/opencode/opencode.db` (SQLite)

**Tables:**

**`project`** - Project registry:
- `id` (text PK) - SHA1 hash of worktree path
- `worktree` (text) - absolute path, e.g. `/Users/alkj/code/github/clync`
- `vcs` (text) - e.g. `"git"`
- `name` (text) - project name

**`session`** - Session metadata:
- `id` (text PK) - e.g. `ses_0b42fb309ffe1Q1xG9O7qyeDjK`
- `project_id` (text FK -> project.id)
- `parent_id` (text, nullable) - for branched sessions
- `directory` (text) - working directory
- `title` (text) - session title
- `agent` (text) - e.g. `"build"`
- `model` (text) - JSON: `{"id":"model-id","providerID":"provider"}`
- `cost` (real) - total cost
- `tokens_input`, `tokens_output`, `tokens_reasoning`, `tokens_cache_read`, `tokens_cache_write` (integer)
- `time_created`, `time_updated` (integer, milliseconds)
- `slug` (text) - URL-friendly identifier
- `version` (text) - opencode version

**`message`** - One row per conversation turn:
- `id` (text PK) - e.g. `msg_f4bd04d0a001syHG0qwZJWM5ww`
- `session_id` (text FK)
- `data` (text JSON) - message metadata
- `time_created`, `time_updated` (integer ms)

User message data:
```json
{
  "role": "user",
  "time": {"created": 1783683370250},
  "agent": "build",
  "model": {"providerID": "ollama", "modelID": "allangpt:q8"}
}
```

Assistant message data:
```json
{
  "parentID": "msg_user_id",
  "role": "assistant",
  "mode": "build",
  "agent": "build",
  "path": {"cwd": "/path", "root": "/path"},
  "cost": 0,
  "tokens": {"total": 16868, "input": 16662, "output": 206, "reasoning": 0, "cache": {"write": 0, "read": 0}},
  "modelID": "allangpt:q8",
  "providerID": "ollama",
  "time": {"created": 1783683370265, "completed": 1783683550911},
  "finish": "tool-calls|stop"
}
```

**`part`** - Message content (multiple parts per message):
- `id` (text PK)
- `message_id` (text FK -> message.id)
- `session_id` (text FK)
- `data` (text JSON)
- `time_created`, `time_updated` (integer ms)

Part types:

Text:
```json
{"type": "text", "text": "the content"}
```

Tool call (includes both input AND result in one part):
```json
{
  "type": "tool",
  "tool": "read",
  "callID": "call_d9rjs2ij",
  "state": {
    "status": "completed",
    "input": {"filePath": "/path/to/file"},
    "output": "file contents...",
    "title": "README.md",
    "time": {"start": 1783683371000, "end": 1783683372000}
  }
}
```

Step markers (wrap each assistant turn):
```json
{"type": "step-start", "snapshot": "git-sha"}
{"type": "step-finish", "reason": "tool-calls|stop", "snapshot": "git-sha", "tokens": {...}, "cost": 0}
```

**opencode tool names:** `bash`, `read`, `write`, `edit`, `grep`, `glob`, `fetch`, `task` (subagent).

## Conversion Mapping

### Claude -> opencode

| Claude | opencode | Notes |
|--------|----------|-------|
| Session file `<uuid>.jsonl` | `session` row + `message` + `part` rows | Create project if needed |
| `type: "user"` entry | `message` (role:user) + `part` (type:text) | |
| `type: "assistant"` entry | `message` (role:assistant) + parts for each content block | |
| `content[type: "text"]` | `part` (type:text) | |
| `content[type: "tool_use"]` + following tool_result | `part` (type:tool) with state.input + state.output | Merge tool call and result into single part |
| `content[type: "thinking"]` | Dropped | opencode doesn't store extended thinking |
| `type: "ai-title"` | `session.title` | |
| `message.usage` | `session.tokens_*` fields (aggregated) | Sum across messages |
| `message.model` | `session.model` JSON | Map to opencode format |
| `uuid`/`parentUuid` tree | `message.data.parentID` | Flatten tree to linear sequence |
| Metadata entries (mode, system, etc.) | Dropped | |

**Tool name mapping (Claude -> opencode):**
| Claude | opencode |
|--------|----------|
| `Bash` | `bash` |
| `Read` | `read` |
| `Write` | `write` |
| `Edit` | `edit` |
| `Grep` | `grep` |
| `WebFetch` | `fetch` |
| `Agent` | `task` |
| Other | Keep original name |

**ID generation:** opencode uses prefixed IDs. Generate them as:
- Session: `ses_<random-hex>` (use a UUID or timestamp-based scheme)
- Message: `msg_<random-hex>`
- Part: `prt_<random-hex>`

**Project resolution:** Look up the project by worktree path in the `project` table. The encoded Claude project dir (e.g. `-Users-alkj-code-github-clync`) decodes to `/Users/alkj/code/github/clync`. If no project row exists, create one with `id = sha1(worktree)`.

### opencode -> Claude

| opencode | Claude | Notes |
|----------|--------|-------|
| `session` row | Session JSONL file | Create in `~/.claude/projects/<encoded-dir>/` |
| `message` (role:user) + `part` (type:text) | `type: "user"` entry | |
| `message` (role:assistant) + `part` (type:text) | `type: "assistant"` entry with content array | |
| `part` (type:tool) | `content[type: "tool_use"]` + separate `tool_result` entry | Split state into call + result |
| `part` (type:step-start/step-finish) | Dropped | |
| `session.title` | `type: "ai-title"` entry | |
| `session.model` | `message.model` on each assistant entry | |
| `session.tokens_*` | Dropped (Claude doesn't aggregate) | |
| `message.data.parentID` | `uuid`/`parentUuid` chain | Generate UUIDs for each entry |

**Tool name mapping (opencode -> Claude):**
| opencode | Claude |
|----------|--------|
| `bash` | `Bash` |
| `read` | `Read` |
| `write` | `Write` |
| `edit` | `Edit` |
| `grep` | `Grep` |
| `fetch` | `WebFetch` |
| `task` | `Agent` |
| Other | Keep original name |

**UUID generation:** Claude uses UUIDv4 for `uuid` fields. Generate fresh ones, maintaining the parent chain.

**Project dir encoding:** Convert the opencode `session.directory` path to Claude's encoded format: `/Users/alkj/code/github/clync` -> `-Users-alkj-code-github-clync` (replace `/` with `-`).

**Required metadata entries:** Add these to the start of the JSONL to make Claude recognize the session:
1. `{"type": "mode", "mode": "normal", "sessionId": "<uuid>"}`
2. `{"type": "ai-title", "aiTitle": "<session.title>", "sessionId": "<uuid>"}`

## Implementation Notes

### Dependencies
- SQLite access: use the `rusqlite` crate (well-maintained, ~30M downloads)
- UUID generation: use the `uuid` crate (already a transitive dep via ratatui on the checkout branch)
- SHA1 for project ID: use `sha1` or derive from the `age` crate's deps

### File structure
- `src/convert/mod.rs` - public API, CLI dispatch
- `src/convert/claude.rs` - Claude session reader/writer
- `src/convert/opencode.rs` - opencode session reader/writer
- `src/convert/mapping.rs` - tool name mapping, ID generation, format translation

### Error handling
- Skip entries that can't be parsed (warn, don't fail)
- If the target session already exists, skip unless `--force` is passed
- Report: N messages converted, M tool calls mapped, K entries skipped

### Testing

**Unit tests:**
- Claude JSONL parsing roundtrip
- opencode message/part construction
- Tool name mapping
- UUID/ID generation
- Project dir encoding/decoding

**E2e tests:**
- Write a Claude session JSONL manually, convert to opencode, verify SQLite content
- Write opencode SQLite rows, convert to Claude, verify JSONL content
- Roundtrip: Claude -> opencode -> Claude, verify messages match

**Manual testing:**
- Convert a real Claude session, open opencode, verify it shows up and is browsable
- Convert an opencode session, verify Claude Code can read it
- Test with sessions containing tool calls (Bash, Read, Write)

### What's intentionally lossy

| Lost in Claude -> opencode | Reason |
|---|---|
| Extended thinking content | opencode doesn't display it |
| Hook/system entries | Claude-specific infrastructure |
| Permission mode | Claude-specific |
| Bridge session linking | Claude cloud feature |
| Thinking signatures | Cryptographic verification, Claude-specific |
| Sidechain messages | opencode has no equivalent |

| Lost in opencode -> Claude | Reason |
|---|---|
| Step-start/step-finish markers | Claude has no equivalent |
| Per-step token counts | Claude tracks differently |
| Session-level cost aggregation | Claude doesn't aggregate |
| Git snapshots in steps | Claude doesn't snapshot |
| Workspace/agent metadata | Claude uses different agent model |

### Future extensions
- pi support (JSONL format, similar to Claude but with `id`/`parentId` instead of `uuid`/`parentUuid`)
- Aider, Cursor, Windsurf session formats
- Batch conversion for migration workflows
- MCP tool for converting sessions on demand
