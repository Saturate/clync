# SPEC: `clync convert` - Session format conversion

## Problem

Users work across multiple AI coding tools (Claude Code, opencode, pi). Each stores conversation sessions in its own format. There's no way to move sessions between tools, which locks conversation history into a single tool.

## Goal

Add `clync convert` to translate sessions between Claude Code, opencode (v2 only), and pi (all directions). The converted sessions should be browsable in the target tool and contain the full conversation with tool call history.

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

**Storage:** One JSONL file per session. Each line is a JSON object. Lines form a tree via `uuid`/`parentUuid` fields. Each assistant entry contains exactly **one** content block (one entry per thinking, text, or tool_use block; they are never combined).

**Entry types** (the `type` field):

| Type | Purpose | Convertible |
|------|---------|-------------|
| `user` | User message (plain text or tool_result blocks) | Yes |
| `assistant` | Assistant response (one of: text, tool_use, thinking) | Yes |
| `system` | Hook summaries, stop reasons | No |
| `attachment` | Hook context injections | No |
| `mode` | Agent mode (normal/plan/etc) | No |
| `permission-mode` | Permission level | No |
| `bridge-session` | Cloud session linking | No |
| `ai-title` | Auto-generated title | Yes (session title) |
| `last-prompt` | Cursor state | No |
| `file-history-snapshot` | File change tracking | No |
| `file-history-delta` | File change tracking | No |
| `queue-operation` | Queue management | No |
| `pr-link` | PR association | No |
| `cost-state` | Cost tracking | No |
| `atis-latch` | Internal state | No |

**User message (plain text):**
```json
{
  "parentUuid": "previous-uuid",
  "isSidechain": false,
  "type": "user",
  "uuid": "this-uuid",
  "timestamp": "2026-08-10T13:21:02.020Z",
  "message": {
    "role": "user",
    "content": "the user's text"
  },
  "sessionId": "session-uuid",
  "version": "2.1.226",
  "cwd": "/Users/alkj/code/github/clync"
}
```

Note: `message.content` can be a string or an array of content blocks. The `version`, `cwd`, `sessionId` fields appear on user entries.

**User message (tool result):** Tool results are `type: "user"` entries where `message.content` is an array of `tool_result` blocks:
```json
{
  "parentUuid": "assistant-uuid",
  "isSidechain": false,
  "type": "user",
  "uuid": "result-uuid",
  "timestamp": "2026-08-10T13:21:05.000Z",
  "message": {
    "role": "user",
    "content": [
      {
        "type": "tool_result",
        "tool_use_id": "toolu_01UHL3j13a9fpiXbmcK4dbzQ",
        "content": "command output here",
        "is_error": false
      }
    ]
  },
  "toolUseResult": { ... },
  "sourceToolAssistantUUID": "assistant-entry-uuid"
}
```

Note: There is **no** separate `type: "tool_result"` top-level entry type. Tool results are always `type: "user"` with `tool_result` content blocks.

**Assistant message (one block per entry):**
```json
{
  "parentUuid": "previous-uuid",
  "isSidechain": false,
  "type": "assistant",
  "uuid": "this-uuid",
  "timestamp": "2026-08-10T13:21:04.000Z",
  "message": {
    "model": "claude-opus-4-8",
    "id": "msg_...",
    "role": "assistant",
    "content": [
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

Each assistant entry has `message.content` as an array with exactly one block. Block types: `thinking` (with `thinking` and `signature` fields), `text` (with `text` field), `tool_use` (with `id`, `name`, `input` fields). A sequence like thinking + text + tool_use + tool_use produces four separate assistant entries, each with a `parentUuid` pointing to the previous one.

**Claude tool names:** `Bash`, `Read`, `Write`, `Edit`, `Grep`, `Agent`, `WebFetch`, `WebSearch`, `AskUserQuestion`, plus MCP tool names (e.g. `mcp__chrome-devtools__click`).

### opencode sessions (v2 only)

**Location:** `~/.local/share/opencode/opencode.db` (SQLite). Respects `$XDG_DATA_HOME` if set.

We target opencode >= 2.0 only. The v1 format (`session` + `message` + `part` tables) is not supported; opencode 2.x no longer writes to those tables for new sessions.

**Tables used for v2:**

**`project`** - Project registry:
```sql
CREATE TABLE project (
  id text PRIMARY KEY,              -- SHA1 of worktree path
  worktree text NOT NULL,           -- absolute path
  vcs text,                         -- e.g. "git"
  name text,                        -- project name (basename of path)
  icon_url text,
  icon_url_override text,
  icon_color text,
  time_created integer NOT NULL,    -- epoch ms
  time_updated integer NOT NULL,
  time_initialized integer,
  sandboxes text NOT NULL,          -- JSON, default "[]"
  commands text,
  time_active integer DEFAULT 0 NOT NULL,
  time_archived integer
);
```

**`session_v2`** - Session metadata:
```sql
CREATE TABLE session_v2 (
  id text PRIMARY KEY,              -- e.g. "ses_ea2c57b5cffejQAqoGtwUPwxPk"
  project_id text NOT NULL,         -- FK -> project.id
  workspace_id text,
  parent_id text,
  fork_session_id text,
  fork_boundary text,
  slug text NOT NULL,
  directory text NOT NULL,          -- working directory
  path text,
  title text,
  version text NOT NULL,            -- opencode version
  share_url text,
  summary_additions integer,
  summary_deletions integer,
  summary_files integer,
  summary_diffs text,
  metadata text,                    -- nullable JSON, used for provenance
  cost real DEFAULT 0 NOT NULL,
  tokens_input integer DEFAULT 0 NOT NULL,
  tokens_output integer DEFAULT 0 NOT NULL,
  tokens_reasoning integer DEFAULT 0 NOT NULL,
  tokens_cache_read integer DEFAULT 0 NOT NULL,
  tokens_cache_write integer DEFAULT 0 NOT NULL,
  revert text,
  permission text,
  agent text,
  model text,                       -- JSON: {"id":"model-id","providerID":"provider","variant":"default"}
  time_created integer NOT NULL,    -- epoch ms
  time_updated integer NOT NULL,
  time_compacting integer,
  time_archived integer,
  time_suspended integer,
  resume_attempts integer DEFAULT 0 NOT NULL,
  time_idle integer,
  time_viewed integer,
  idle_outcome text,
  CONSTRAINT fk FOREIGN KEY (project_id) REFERENCES project(id) ON DELETE CASCADE
);
```

**`session_message`** - All messages in a session (replaces v1's separate message + part tables):
```sql
CREATE TABLE session_message (
  id text PRIMARY KEY,
  session_id text NOT NULL,         -- FK -> session_v2.id
  type text NOT NULL,               -- "user", "assistant", "idle", "synthetic", "system", "compaction", "model-switched"
  seq integer NOT NULL,             -- explicit ordering
  time_created integer NOT NULL,    -- epoch ms
  time_updated integer NOT NULL,
  data text NOT NULL,               -- JSON, structure varies by type
  CONSTRAINT fk FOREIGN KEY (session_id) REFERENCES session_v2(id) ON DELETE CASCADE
);
-- Unique index on (session_id, seq)
```

**session_message types:**

| type | Convertible | Notes |
|------|-------------|-------|
| `user` | Yes | User text input |
| `assistant` | Yes | Assistant response with inline tool calls |
| `idle` | No | Session idle events |
| `synthetic` | No | Shell output, subagent results, interrupts |
| `system` | No | Tool catalog updates, instructions |
| `compaction` | No | Context summarization |
| `model-switched` | No | Model change events |

**User message data (v2):**
```json
{
  "time": {"created": 1792565478670},
  "text": "the user's text",
  "files": [],
  "agents": []
}
```

Note: v2 user messages use `text` (string), not `role`/`content` like v1. No `role` field.

**Assistant message data (v2):**
```json
{
  "time": {
    "created": 1792565478688,
    "streamed": 1792565546451,
    "completed": 1792565546555
  },
  "agent": "build",
  "model": {"id": "model-id", "providerID": "provider", "variant": "default"},
  "content": [
    {"type": "reasoning", "text": "...", "native": {"reasoningField": "reasoning"}, "time": {...}},
    {"type": "text", "text": "response text"},
    {"type": "tool", "id": "call_1k30c600", "name": "shell", "executed": false,
     "state": {
       "status": "completed",
       "input": {"command": "ls"},
       "content": [{"type": "text", "text": "output here"}],
       "metadata": {"status": "completed", "truncated": false, "exit": 0}
     },
     "time": {"created": 1792565543541, "ran": 1792565546448, "completed": 1792565546490}
    }
  ],
  "snapshot": {"start": "git-sha", "end": "git-sha", "files": []},
  "finish": "tool-calls|stop|error",
  "rawFinish": "tool_calls|stop",
  "cost": 0,
  "tokens": {"input": 100, "output": 50, "reasoning": 0, "cache": {"write": 0, "read": 0}}
}
```

Key v2 differences from v1 spec:
- Tool results are **inline** in the assistant content block's `state.content` (array of `{type: "text", text}` blocks), not in `state.output` (string).
- Tool blocks use `name` field (not `tool` as in v1 parts) and `id` (not `callID`).
- `state.metadata` contains extra fields like `preview`, `display`, `truncated`, `exit`, `matches`.
- Reasoning blocks use `type: "reasoning"` (not `thinking`), with `native.reasoningField`.

**opencode v2 tool names:** `shell` (primary; replaces v1's `bash`), `read`, `write`, `edit`, `grep`, `glob`, `webfetch`, `websearch`, `subagent` (replaces v1's `task`), `question`, `skill`, `todowrite`.

### pi sessions

**Location:** `~/.pi/agent/sessions/<encoded-project-dir>/<timestamp>_<uuid>.jsonl`

**Storage:** One JSONL file per session. Each line is a JSON object. Lines form a chain via `id`/`parentId` fields. Project dirs are double-dash encoded: `--Users-alkj-code-github-clync--`.

**Entry types** (the `type` field):

| Type | Purpose | Convertible |
|------|---------|-------------|
| `session` | Session header (version, id, cwd) | Yes (metadata) |
| `message` | User, assistant, and tool result messages | Yes |
| `model_change` | Model switch event | No |
| `thinking_level_change` | Thinking level toggle | No |
| `compaction` | Context window summarization | No |

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

**Assistant message** (can contain text, toolCall, thinking in one entry):
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

Note: pi uses `arguments` (not `input`) for tool call parameters, and `toolCall` (camelCase) as the content block type.

**Tool result** (separate message entry):
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

**Thinking blocks in pi:** Use `thinkingSignature` field (not `signature` as in Claude):
```json
{"type": "thinking", "thinking": "...", "thinkingSignature": "reasoning"}
```

**pi tool names (observed):** `bash`, `read`, `write`, `edit`, `grep`, `glob`. No `fetch` or `task` observed, but listed in pi docs.

**Key differences from Claude:**
- Threading uses `id`/`parentId` (short hex IDs) instead of `uuid`/`parentUuid` (UUIDv4)
- Content is always an array (never a plain string)
- Tool calls use `arguments` field (not `input`)
- Tool calls use `toolCall` type (not `tool_use`)
- Tool results are separate messages with `role: "toolResult"`
- Tool names are lowercase
- Project dir encoding uses double-dashes: `--Users-alkj-code--` vs Claude's single-dash `-Users-alkj-code`
- Unlike Claude, pi can have multiple content blocks per assistant entry

## Intermediate Representation (IR)

All conversions go through a common IR. Adding a new tool means implementing `read(source) -> IR` and `write(IR) -> target`.

```rust
struct ConvertedSession {
    source_id: String,
    source_tool: SourceTool,  // Claude | Opencode | Pi
    title: String,
    project_dir: PathBuf,
    messages: Vec<ConvertedMessage>,
    model: Option<String>,
    provider: Option<String>,
    tokens: Option<TokenUsage>,
}

struct ConvertedMessage {
    role: MessageRole,          // User, Assistant, ToolResult
    timestamp_ms: u64,
    content: Vec<ContentBlock>,
    // For tool results only:
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    is_error: bool,
}

enum MessageRole { User, Assistant, ToolResult }

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
    // it's tool-specific and not universally displayable
}

struct TokenUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
}

enum SourceTool { Claude, Opencode, Pi }
```

**Linearization:** The IR `messages` Vec is a flat chronological list.

Claude sessions are a tree (branching via `parentUuid`, sidechains via `isSidechain`). Additionally, each content block is a separate entry. When reading Claude into the IR:

1. Filter out entries with `isSidechain: true`
2. Filter out non-conversation entries (all types except `user` and `assistant`)
3. Sort remaining by timestamp
4. **Merge consecutive assistant entries** into a single IR message when they share the same logical turn (consecutive assistant entries between user entries). This reconstructs multi-block messages from Claude's one-block-per-entry storage.
5. Convert `type: "user"` entries with `tool_result` content blocks into `MessageRole::ToolResult` IR messages

When writing to target format:
- **opencode v2:** Messages ordered by `seq`. Each assistant message gets one `session_message` row with all content blocks in the `data.content` array.
- **pi:** Each entry's `parentId` points to the previous entry's `id`
- **Claude:** Each content block becomes its own entry, linked via `uuid`/`parentUuid`

**Tool name normalization:** The IR stores tool names in lowercase. Readers normalize on ingest; writers map per target conventions.

| Canonical (IR) | Claude | opencode v2 | pi |
|----------------|--------|-------------|----|
| `bash` | `Bash` | `shell` | `bash` |
| `read` | `Read` | `read` | `read` |
| `write` | `Write` | `write` | `write` |
| `edit` | `Edit` | `edit` | `edit` |
| `grep` | `Grep` | `grep` | `grep` |
| `glob` | `Grep` | `glob` | `glob` |
| `webfetch` | `WebFetch` | `webfetch` | `fetch` |
| `websearch` | `WebSearch` | `websearch` | - |
| `agent` | `Agent` | `subagent` | `task` |
| `question` | `AskUserQuestion` | `question` | - |
| Other | Original case | lowercase | lowercase |

## Conversion Mapping

### Claude -> opencode v2

| Claude | opencode v2 | Notes |
|--------|-------------|-------|
| Session file `<uuid>.jsonl` | `session_v2` row + `session_message` rows | Create project if needed |
| `type: "user"` (plain text) | `session_message` (type:user) | `data.text` field |
| `type: "user"` (tool_result blocks) | Merged into preceding assistant's tool state | Tool output goes into `state.content` |
| Consecutive `type: "assistant"` entries | Single `session_message` (type:assistant) | Merge blocks into `data.content` array |
| `content[type: "text"]` | Content block `{type: "text", text}` | |
| `content[type: "tool_use"]` + tool_result | Content block `{type: "tool", name, id, state: {input, content, status}}` | Merge call + result |
| `content[type: "thinking"]` | Dropped | opencode v2 uses "reasoning" but from its own provider |
| `type: "ai-title"` | `session_v2.title` | |
| `message.usage` | `session_v2.tokens_*` (aggregated) | Sum across all assistant entries |
| `message.model` | `session_v2.model` JSON | Map to `{id, providerID, variant}` |
| All other entry types | Dropped | |

**ID generation:** opencode v2 uses prefixed IDs:
- Session: `ses_<random>` (base62-like characters after prefix)
- Message: `msg_<random>` (same pattern)
- Session slug: adjective-noun combo (e.g. `clever-otter`). Generate from a small word list.

**Project resolution:** Look up project by worktree path in the `project` table. Claude's `-Users-alkj-code-github-clync` decodes to `/Users/alkj/code/github/clync`. If no project row exists, create one with `id` as random 40-char hex (not SHA1 of path; opencode uses random IDs), `sandboxes = "[]"`, `time_created`/`time_updated` = now.

**Message ordering:** `session_message` has an explicit `seq` column (unique per session). Assign sequential integers starting from 1.

### opencode v2 -> Claude

| opencode v2 | Claude | Notes |
|-------------|--------|-------|
| `session_v2` row | Session JSONL file | Create in `~/.claude/projects/<encoded-dir>/` |
| `session_message` (type:user) | `type: "user"` entry | `data.text` -> `message.content` string |
| `session_message` (type:assistant) text blocks | `type: "assistant"` entry (one per block) | Split content array |
| Assistant tool block | `type: "assistant"` (tool_use) + `type: "user"` (tool_result) | Split state into call + result entries |
| `session_v2.title` | `type: "ai-title"` entry | |
| `session_v2.model` | `message.model` on each assistant entry | |
| idle, synthetic, system, compaction, model-switched | Dropped | |

**UUID generation:** Generate UUIDv4 for each entry's `uuid`, maintaining the `parentUuid` chain.

**Project dir encoding:** `/Users/alkj/code/github/clync` -> `-Users-alkj-code-github-clync`

### pi conversions

**pi -> IR:** Parse JSONL. `session` entry -> metadata. `message` entries -> IR messages by `message.role` (user, assistant, toolResult). Tool calls: `toolCall` blocks with `arguments` field map to `ToolCall` with `input`. Content is always an array. `model_change`, `thinking_level_change`, `compaction` entries are dropped.

**IR -> pi:** Write JSONL. Session header first, then messages in order. Generate short hex IDs for `id`/`parentId` chain. Content always as arrays. Tool calls use `toolCall` type with `arguments` field. Tool results as separate messages with `role: "toolResult"`.

**pi project dir encoding:** Double-dash wrapped: `--Users-alkj-code-github-clync--`. Decode: strip leading/trailing `--`, replace `-` with `/`, prepend `/`.

### Writing to Claude (any source -> Claude)

When writing the IR to a Claude JSONL file, prepend these metadata entries:
1. `{"type": "mode", "mode": "normal", "sessionId": "<uuid>"}`
2. `{"type": "ai-title", "aiTitle": "<session.title>", "sessionId": "<uuid>"}`
3. `{"type": "clync-provenance", "source": "<source-tool>", "sourceId": "<original-id>", "convertedAt": "<iso-timestamp>"}`

Then for each IR message:
- **User text:** One `type: "user"` entry with `message.content` as string
- **Assistant:** One `type: "assistant"` entry **per content block** (text, tool_use). Each entry's `message.content` is an array with exactly one block.
- **ToolResult:** One `type: "user"` entry with `message.content` as array of `{type: "tool_result", tool_use_id, content, is_error}`

All entries linked via `uuid`/`parentUuid` chain.

## Implementation Notes

### Dependencies
- SQLite: `rusqlite` crate
- UUID generation: `uuid` crate
- SHA1 for project ID: `sha1` crate

### File structure
- `src/convert/mod.rs` - IR types, CLI dispatch
- `src/convert/claude.rs` - Claude reader/writer (JSONL <-> IR)
- `src/convert/opencode.rs` - opencode v2 reader/writer (SQLite <-> IR)
- `src/convert/pi.rs` - pi reader/writer (JSONL <-> IR)
- `src/convert/tools.rs` - tool name normalization

### Provenance tracking

**In opencode v2:** `session_v2.metadata` JSON:
```json
{"clync_source": "claude", "clync_source_id": "uuid", "clync_converted_at": "2026-07-10T14:30:00Z"}
```

**In Claude JSONL:** Custom entry (Claude ignores unknown types):
```json
{"type": "clync-provenance", "source": "opencode", "sourceId": "ses_...", "convertedAt": "2026-07-10T14:30:00Z"}
```

**In pi JSONL:** Custom entry (pi ignores unknown types):
```json
{"type": "clync-provenance", "source": "claude", "sourceId": "uuid", "convertedAt": "2026-07-10T14:30:00Z"}
```

Before converting, check target for existing provenance. Skip unless `--force`.

### Error handling
- Skip unparseable entries (warn, don't fail)
- Skip if target session already exists (unless `--force`)
- Report: N messages converted, M tool calls mapped, K entries skipped

### Testing

**Unit tests:**
- Claude JSONL parsing (one-block-per-entry, tool_result-in-user)
- opencode v2 session_message construction
- Tool name mapping (all directions)
- Project dir encoding/decoding (Claude and pi styles)
- Assistant entry merging (Claude's split entries -> single IR message)
- Assistant entry splitting (IR multi-block -> Claude one-per-entry)

**E2e tests:**
- Claude -> opencode v2: write JSONL, convert, verify SQLite
- opencode v2 -> Claude: insert SQLite rows, convert, verify JSONL
- Roundtrip: Claude -> opencode -> Claude, verify message content matches
- pi -> Claude: write JSONL, convert, verify JSONL

**Manual testing:**
- Convert a real Claude session, open in opencode, verify browsable
- Convert an opencode session, verify Claude Code can read it
- Sessions with tool calls (Bash/shell, Read/read, Write/write)

### What's intentionally lossy

| Lost in Claude -> opencode | Reason |
|---|---|
| Extended thinking content | opencode uses its own reasoning format |
| Hook/system/attachment entries | Claude-specific |
| Permission mode, bridge-session | Claude-specific |
| Thinking signatures | Cryptographic verification, Claude-specific |
| Sidechain messages | No equivalent |
| file-history, pr-link, queue-operation | Claude-specific tracking |

| Lost in opencode -> Claude | Reason |
|---|---|
| idle/synthetic/system/compaction messages | opencode-specific |
| model-switched events | opencode-specific |
| Git snapshots (start/end SHA) | No equivalent |
| Workspace/agent metadata | Different agent model |
| Session-level cost aggregation | Claude doesn't aggregate |
| Tool state.metadata (preview, display) | Claude uses different display model |

| Lost in pi -> Claude/opencode | Reason |
|---|---|
| Compaction summaries | Internal optimization |
| Model change events | Metadata only |
| Thinking level changes | pi-specific |
| thinkingSignature field | pi-specific format |

| Lost in Claude/opencode -> pi | Reason |
|---|---|
| All Claude metadata entries | Claude-specific |
| opencode idle/synthetic/system | opencode-specific |
| Extended thinking content | pi thinking only from its own provider |

### Clarifications

**opencode DB path:** `~/.local/share/opencode/opencode.db`. Respects `$XDG_DATA_HOME`.

**Tool call without result:** When a Claude `tool_use` has no corresponding `tool_result` (interrupted session), write the opencode tool block with `state.status: "pending"` and empty content.

**Session title fallback:** 1. `ai-title` entry, 2. First user message truncated to 80 chars, 3. `"Untitled session"`.

**opencode ID generation:** Use random IDs. The `ses_` prefix pattern with base62-like characters. `session_message` IDs appear to be plain UUIDs or random strings.

**Claude assistant merging:** When reading Claude into the IR, consecutive `type: "assistant"` entries (between user entries) belong to the same logical turn. Merge them into one `ConvertedMessage` with multiple `ContentBlock`s. When writing back, split them out again.

**opencode v2 seq numbering:** The `seq` column on `session_message` is unique per session. When writing, assign sequential integers. When reading, order by `seq`.

**opencode reasoning blocks:** v2 uses `type: "reasoning"` (not `thinking`). The field is `text` with optional `native.reasoningField`. When converting from Claude thinking to opencode reasoning, drop the content (reasoning content is provider-specific).

**opencode shell vs bash:** v2 renamed the command tool from `bash` to `shell`. The IR normalizes to `bash`; the opencode writer maps `bash` -> `shell`.

### Future extensions
- Aider, Cursor, Windsurf session formats (add reader/writer per tool)
- `--watch` mode for auto-conversion of new sessions
- opencode v1 read support (for legacy sessions)
