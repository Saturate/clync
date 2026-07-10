# SPEC: `clync convert` - Session format conversion

## Problem

Users work across multiple AI coding tools (Claude Code, opencode, pi). Each stores conversation sessions in its own format. There's no way to move sessions between tools, which locks conversation history into a single tool.

## Goal

Add `clync convert` to translate sessions between Claude Code, opencode, and pi (all directions). The converted sessions should be browsable in the target tool and contain the full conversation with tool call history.

Architecture: each format has a reader (into an intermediate representation) and a writer (from IR to target format). Adding a new tool means implementing one reader + one writer, and it automatically converts to/from all other tools.

## CLI

```bash
# Claude Code -> opencode
clync convert --from claude --to opencode <session-uuid-or-path>
clync convert --from claude --to opencode --all          # convert all sessions
clync convert --from claude --to opencode --project <dir> # sessions for one project (real path, not encoded)

# opencode -> Claude Code  
clync convert --from opencode --to claude <session-id>
clync convert --from opencode --to claude --all

# pi -> Claude Code
clync convert --from pi --to claude <session-id-or-path>
clync convert --from pi --to claude --all

# Claude Code -> pi
clync convert --from claude --to pi <session-uuid-or-path>

# Any direction works
clync convert --from pi --to opencode --all
clync convert --from opencode --to pi --all

# List available sessions from a source
clync convert --from claude --list
clync convert --from opencode --list
clync convert --from pi --list

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

### pi sessions

**Location:** `~/.pi/agent/sessions/<encoded-project-dir>/<timestamp>_<uuid>.jsonl`

**Storage:** One JSONL file per session, similar to Claude. Each line is a JSON object. Lines form a chain via `id`/`parentId` fields. Project dirs are double-dash encoded: `--Users-alkj-code-github-clync--`.

**Entry types** (the `type` field):

| Type | Purpose | Convertible |
|------|---------|-------------|
| `session` | Session header (version, id, cwd) | Yes (metadata) |
| `message` | User, assistant, and tool result messages | Yes |
| `model_change` | Model switch event | No (metadata only) |
| `thinking_level_change` | Thinking level toggle | No |
| `compaction` | Context window summarization | No (internal optimization) |

**Session header:**
```json
{
  "type": "session",
  "version": 3,
  "id": "019f36bd-cb99-7fa8-aea8-214213eb872c",
  "timestamp": "2026-07-06T09:23:55.929Z",
  "cwd": "/Users/alkj/Downloads/web_reactoops"
}
```

**User message:**
```json
{
  "type": "message",
  "id": "df50631a",
  "parentId": "bb20cd79",
  "timestamp": "2026-07-06T09:24:06.289Z",
  "message": {
    "role": "user",
    "content": [
      {"type": "text", "text": "the user's text"}
    ],
    "timestamp": 1783329846287
  }
}
```

**Assistant message** (can contain text, toolCall, thinking):
```json
{
  "type": "message",
  "id": "d9b19377",
  "parentId": "d84c240d",
  "timestamp": "2026-07-06T09:24:14.929Z",
  "message": {
    "role": "assistant",
    "content": [
      {"type": "text", "text": "Let me search for it:"},
      {"type": "toolCall", "id": "call_m36yrj2r", "name": "bash", "arguments": {"command": "find / -name flag.txt"}}
    ],
    "api": "openai-completions",
    "provider": "ollama",
    "model": "allangpt:q8",
    "usage": {"input": 1800, "output": 21, "reasoning": 0, "totalTokens": 1821}
  }
}
```

**Tool result** (separate message, role is `toolResult`):
```json
{
  "type": "message",
  "id": "d84c240d",
  "parentId": "3af2dab4",
  "timestamp": "2026-07-06T09:24:12.722Z",
  "message": {
    "role": "toolResult",
    "toolCallId": "call_phrd9ke2",
    "toolName": "read",
    "content": [
      {"type": "text", "text": "file contents here"}
    ],
    "isError": true
  }
}
```

**pi tool names:** `bash`, `read`, `write`, `edit`, `grep`, `glob`, `fetch`, `task`.

**Key differences from Claude:**
- Threading uses `id`/`parentId` (short hex IDs) instead of `uuid`/`parentUuid` (UUIDv4)
- Content is always an array (never a plain string)
- Tool results are separate messages with `role: "toolResult"` (similar to Claude's separate `tool_result` entries)
- Tool names are lowercase (like opencode, unlike Claude's PascalCase)
- Has `compaction` entries for context window management (no equivalent in Claude/opencode)
- Project dir encoding uses double-dashes: `--Users-alkj-code--` vs Claude's single-dash `-Users-alkj-code`

## Intermediate Representation (IR)

All conversions go through a common IR. Adding a new tool means implementing `read(source) -> IR` and `write(IR) -> target`. The IR captures the common denominator across all three formats.

```rust
struct ConvertedSession {
    /// Original source identifier (UUID, session ID, or file path)
    source_id: String,
    /// Which tool this came from
    source_tool: SourceTool,  // Claude | Opencode | Pi
    /// Session title (from ai-title, session.title, or first message)
    title: String,
    /// Working directory / project path
    project_dir: PathBuf,
    /// Ordered list of conversation messages
    messages: Vec<ConvertedMessage>,
    /// Model used (if known)
    model: Option<String>,
    /// Provider (if known)
    provider: Option<String>,
    /// Aggregate token usage
    tokens: Option<TokenUsage>,
}

struct ConvertedMessage {
    /// Role: User, Assistant, ToolResult
    role: MessageRole,
    /// Timestamp (epoch milliseconds)
    timestamp_ms: u64,
    /// Content blocks in order
    content: Vec<ContentBlock>,
    /// For tool results: which tool call this responds to
    tool_call_id: Option<String>,
    /// For tool results: tool name
    tool_name: Option<String>,
    /// For tool results: whether it errored
    is_error: bool,
}

enum MessageRole {
    User,
    Assistant,
    ToolResult,
}

enum ContentBlock {
    Text { text: String },
    ToolCall {
        id: String,
        name: String,          // normalized: lowercase
        input: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        output: String,
        is_error: bool,
    },
    // Thinking is intentionally omitted from the IR;
    // it's Claude-specific and not displayable in other tools
}

struct TokenUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
}

enum SourceTool {
    Claude,
    Opencode,
    Pi,
}
```

**Linearization:** The IR `messages` Vec is a flat chronological list. Claude sessions can have tree structures (branching via `parentUuid`) and sidechains (`isSidechain: true`). When reading Claude sessions into the IR:

1. Filter out entries with `isSidechain: true` (branch experiments, not the main conversation)
2. Filter out non-conversation entries (mode, system, attachment, bridge-session, permission-mode, last-prompt)
3. Sort remaining entries by timestamp
4. The result is a linear sequence of user/assistant/tool-result messages

When writing from the IR to a target format:
- **opencode:** Each assistant message's `data.parentID` points to the immediately preceding user message's ID (simple linear chain)
- **pi:** Each entry's `parentId` points to the previous entry's `id` (linear chain)
- **Claude:** Generate `uuid`/`parentUuid` as a linear chain (each entry points to the previous one)

The tree structure is a Claude internal detail for conversation branching. The IR intentionally does not model it; all targets expect linear conversations.

**Tool name normalization:** The IR stores tool names in lowercase. Readers normalize on ingest; writers capitalize per target conventions.

| Canonical (IR) | Claude | opencode | pi |
|----------------|--------|----------|----|
| `bash` | `Bash` | `bash` | `bash` |
| `read` | `Read` | `read` | `read` |
| `write` | `Write` | `write` | `write` |
| `edit` | `Edit` | `edit` | `edit` |
| `grep` | `Grep` | `grep` | `grep` |
| `webfetch` | `WebFetch` | `fetch` | `fetch` |
| `agent` | `Agent` | `task` | `task` |
| Other | Original case | lowercase | lowercase |

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

**Tool name mapping:** Handled by the IR normalization table above.

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

**Tool name mapping:** Handled by the IR normalization table above.

**UUID generation:** Claude uses UUIDv4 for `uuid` fields. Generate fresh ones, maintaining the parent chain.

**Project dir encoding:** Convert the opencode `session.directory` path to Claude's encoded format: `/Users/alkj/code/github/clync` -> `-Users-alkj-code-github-clync` (replace `/` with `-`).

### pi conversions

Since pi and Claude are both JSONL with similar structure, pi conversions are the simplest.

**pi -> IR:** Parse JSONL. `session` entry -> session metadata. `message` entries -> IR messages based on `message.role` (user, assistant, toolResult). Content is always an array. `model_change`, `thinking_level_change`, `compaction` entries are dropped.

**IR -> pi:** Write JSONL. Session header first, then messages in order. Generate short hex IDs for `id`/`parentId` chain. Content always as arrays. Tool results as separate `role: "toolResult"` messages.

**pi project dir encoding:** Double-dash wrapped: `--Users-alkj-code-github-clync--`. Decode: strip leading/trailing `--`, replace `-` with `/`, prepend `/`.

### Writing to Claude (any source -> Claude)

When writing the IR to a Claude JSONL file, prepend these metadata entries before the conversation messages:
1. `{"type": "mode", "mode": "normal", "sessionId": "<uuid>"}`
2. `{"type": "ai-title", "aiTitle": "<session.title>", "sessionId": "<uuid>"}`
3. `{"type": "clync-provenance", "source": "<source-tool>", "sourceId": "<original-id>", "convertedAt": "<iso-timestamp>"}`

This applies regardless of whether the source is opencode or pi.

## Implementation Notes

### Dependencies
- SQLite access: use the `rusqlite` crate (well-maintained, ~30M downloads)
- UUID generation: use the `uuid` crate (already a transitive dep via ratatui on the checkout branch)
- SHA1 for project ID: use `sha1` or derive from the `age` crate's deps

### File structure
- `src/convert/mod.rs` - public API, CLI dispatch, IR types (`ConvertedSession`, `ConvertedMessage`, etc.)
- `src/convert/claude.rs` - Claude session reader/writer (JSONL <-> IR)
- `src/convert/opencode.rs` - opencode session reader/writer (SQLite <-> IR)
- `src/convert/pi.rs` - pi session reader/writer (JSONL <-> IR)
- `src/convert/tools.rs` - tool name normalization and mapping between formats

### Provenance tracking

Converted sessions include a provenance marker for debugging and to prevent double-conversion.

**In opencode (Claude -> opencode):** Store in the `session.metadata` JSON column (already exists, nullable text):
```json
{
  "clync_source": "claude",
  "clync_source_uuid": "81155ba9-0ba0-422f-9ae1-868de49f162d",
  "clync_converted_at": "2026-07-10T14:30:00Z"
}
```

**In Claude JSONL (opencode -> Claude):** Add a custom entry at the start of the file (Claude Code ignores unknown types):
```json
{"type": "clync-provenance", "source": "opencode", "sourceId": "ses_0b42fb309ffe1Q1xG9O7qyeDjK", "convertedAt": "2026-07-10T14:30:00Z"}
```

**Usage:**
- Before converting, check if the target already has a provenance marker for this source session. If so, skip unless `--force` is passed.
- The `--list` command should show `[converted]` next to sessions that came from conversion.

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

| Lost in pi -> Claude/opencode | Reason |
|---|---|
| Compaction summaries | Internal context optimization, not conversation content |
| Model change events | Metadata, not conversation |
| Thinking level changes | pi-specific setting |
| API/provider metadata per message | Not standardized across tools |

| Lost in Claude/opencode -> pi | Reason |
|---|---|
| All Claude metadata (hooks, system, permissions) | Claude-specific |
| opencode step markers and git snapshots | opencode-specific |
| Extended thinking content | pi supports it but only from its own provider |

## Clarifications

### opencode DB path
`~/.local/share/opencode/opencode.db` is correct on macOS. opencode uses XDG conventions (`$XDG_DATA_HOME/opencode/`), not the macOS `~/Library/Application Support/` pattern. Respect `$XDG_DATA_HOME` if set, fall back to `~/.local/share/opencode/`.

### Tool call without tool_result
When a Claude `tool_use` has no corresponding `tool_result` (session interrupted mid-execution), create the opencode `part` with `state.status: "pending"` and empty output. Don't skip it; the call itself is useful history.

### Session title fallback
The `ai-title` entry is not always present in Claude sessions. Fallback chain:
1. `ai-title` entry if present
2. First user message content, truncated to 80 chars
3. `"Untitled session"`

### ID collision avoidance
When generating opencode IDs (`ses_`, `msg_`, `prt_` prefixed), collision risk is negligible with 128-bit random values. Still, do a `SELECT EXISTS` check before insert. One query is cheap insurance.

### --project flag
Accepts the real filesystem path (e.g. `~/code/github/clync` or `/Users/alkj/code/github/clync`). The tool resolves it to the Claude-encoded form (`-Users-alkj-code-github-clync`) internally. Users should never need to know about dash-encoding.

### session.metadata column
The `session.metadata` column already exists in the opencode DB (added via ALTER TABLE after initial creation, which is why it appears at the end of the column list). It's a nullable text column. No migration needed; just write JSON to it.

### Timestamps (Claude -> opencode)
- `session.time_created`: oldest entry timestamp in the JSONL
- `session.time_updated`: newest entry timestamp in the JSONL
- `message.time_created` / `time_updated`: use the entry's own timestamp for both
- Claude timestamps vary: some entries have `message.timestamp` (epoch ms), some have ISO strings in a `timestamp` field. Normalize to epoch milliseconds for opencode.

### Message ordering in opencode
opencode orders messages by `(session_id, time_created, id)`, confirmed by the index `message_session_time_created_id_idx`. There is no explicit sequence column on `message`. Insert in chronological order.

### Project name
When creating a `project` row, use the basename of the worktree path as `name` (e.g. `clync` from `/Users/alkj/code/github/clync`). No need to store the Claude-encoded dir name; the worktree path is sufficient to re-derive it.

### Thinking-only assistant entries
If a Claude assistant entry contains only `thinking` content (no `text` or `tool_use` blocks), skip it entirely. It would produce zero visible parts in opencode and show as an empty turn. If thinking preceded a `tool_use` in the same content array, the tool_use parts carry the message forward.

### MCP and custom tool names
Lowercase all tool names when converting Claude -> opencode. opencode uses lowercase (`bash`, `read`, `grep`). MCP tool names like `mcp__chrome-devtools__click` should be lowercased as-is. When converting opencode -> Claude, capitalize the first letter for known tools (`bash` -> `Bash`), pass through unknown names unchanged.

### Future extensions
- Aider, Cursor, Windsurf session formats (add reader/writer per tool, all get every-direction conversion for free)
- Batch conversion for migration workflows
- MCP tool for converting sessions on demand
- `--watch` mode that auto-converts new sessions as they appear
