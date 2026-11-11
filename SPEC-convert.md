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
    origin: Option<Origin>,   // set when the source is itself a clync conversion
    title: String,
    project_dir: PathBuf,
    messages: Vec<ConvertedMessage>,
    model: Option<String>,
    provider: Option<String>,
    tokens: Option<TokenUsage>,
    stash: Vec<serde_json::Value>,  // source content the IR can't express
}

struct Origin { tool: SourceTool, id: String }

struct ConvertedMessage {
    role: MessageRole,          // User, Assistant, ToolResult
    timestamp_ms: u64,
    content: Vec<ContentBlock>,
    // For tool results only:
    tool_call_id: Option<String>,
    tool_name: Option<String>,
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
    // Thinking goes to the stash as text; signatures only validate
    // against the provider that issued them
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
2. Keep only the live branch Claude resumes (see "Branches" below), in replay order rather than file order
3. Filter out non-conversation entries (all types except `user` and `assistant`), keeping branch order
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

**Project resolution:** Use the ID opencode itself computes (see "Writer requirements" below), inserting the `project` row if missing. The session `directory` stays the original working directory, which may be a subfolder of the repo.

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
- SQLite: `rusqlite` crate (bundled)
- UUID generation: `uuid` crate

### File structure
- `src/convert/mod.rs` - IR types, tool-call pairing, shared helpers
- `src/convert/cli.rs` - `clync convert`: listing, session lookup, dedupe, read -> write loop
- `src/convert/jsonl.rs` - JSONL helpers shared by Claude and pi
- `src/convert/{claude,opencode,pi}/` - per format: `mod.rs` (paths, listing, provenance index), `read.rs`, `write.rs`
- `src/convert/tools.rs` - tool name normalization
- `tests/convert_e2e.rs` - converts in an isolated HOME and opens the results with the real tools

### Provenance and dedupe

Every converted session records its **origin**: the session it ultimately came from. When the source is itself a clync conversion, its recorded origin is carried forward, so `claude -> pi -> opencode` still points at the original Claude session.

**In opencode v2:** `session_v2.metadata` JSON (and `version = 'clync'`):
```json
{"clync_source": "claude", "clync_source_id": "uuid", "clync_converted_at": "2026-07-10T14:30:00Z", "clync_stash": [...]}
```

**In Claude JSONL:** custom entries near the top (Claude ignores unknown types):
```json
{"type": "clync-provenance", "source": "opencode", "sourceId": "ses_...", "convertedAt": "2026-07-10T14:30:00Z"}
{"type": "clync-stash", "version": 1, "source": "opencode", "sourceId": "ses_...", "entries": [...]}
```

**In pi JSONL:** pi `custom` entries, which stay in the session tree but out of the model context. They come right after the header so the last message stays the leaf pi resumes from:
```json
{"type": "custom", "customType": "clync-provenance", "id": "1a2b3c4d", "parentId": null, "data": {"source": "claude", "sourceId": "uuid", "convertedAt": "..."}}
{"type": "custom", "customType": "clync-stash", "id": "5e6f7a8b", "parentId": "1a2b3c4d", "data": {"version": 1, "entries": [...]}}
```

Before writing, the target's existing conversions are indexed by origin. A session is skipped when:
- its origin is the target tool (converting it back would duplicate the original), or
- the target already has a conversion of that origin. `--force` deletes that conversion and writes a fresh one.

### Stash

The stash keeps source content the IR can't express, so a future reverse conversion can restore it. It deliberately leaves out bookkeeping that only matters to the source tool:

| Source | Stashed | Dropped |
|---|---|---|
| Claude | thinking text (no signature), `summary`, `system` compact_boundary / away_summary, non-text blocks (images) | attachments, file-history, api-request records, hooks, permission/mode, sidechains |
| opencode | reasoning text, compaction/synthetic/system messages, unknown blocks | idle markers |
| pi | thinking text (no signature), `compaction`, `branch_summary`, non-clync `custom` entries, other message roles | model and thinking-level changes |

When the source is a conversion, its existing stash is carried over unchanged instead.

### Writer requirements found by opening the output

- **Tool calls and results pair one-to-one.** Unanswered calls get a synthetic error result ("No result recorded"); orphan results are dropped. Claude's API rejects a resumed history with an unanswered `tool_use`, and opencode can't decode an unfinished tool block.
- **opencode tool state** is decoded strictly by status: `completed` carries `content: [{type: "text", text}]`; `error` carries `error: {type: "tool.execution", message}` and no `content` or `metadata`.
- **opencode IDs** follow opencode's format: prefix, 12 hex chars of `timestamp_ms * 0x1000 + counter` (bit-inverted for session IDs so newer sort first, low 48 bits only), 14 random base62 chars.
- **opencode project ID** must be the one opencode computes, or the session won't show up in that project. For a git repo: sha1 of `git-remote:<host>/<path>` for `origin` (host lowercased; leading `/`, trailing `.git` and `/` stripped; `file:` remotes ignored), else the ID cached in `<git common dir>/opencode`, else the root commit. Outside git: sha1 of `directory:<path>`, or an existing row for that path.
- **opencode `event_sequence`** gets a row at the session's last message `seq`. opencode allocates the next message seq from it, so without it continuing the session collides with the converted rows. `--force` deletes the session's `event`, `event_sequence`, `session_pending`, `session_inbox` and `session_message` rows explicitly.
- **opencode model reference** is required on every assistant message; sessions without a known model get `{id: "unknown", providerID: "unknown"}`. Claude's `<synthetic>` model label is not treated as a model.
- **Claude** entries form one `parentUuid` chain starting at `null`, carry `cwd`, and blocks of one assistant turn share `message.id`. The final `last-prompt` points at the last entry and carries `lastPrompt`; the stash sits between the messages and it, so the session picker finds the first prompt in the file's head and `lastPrompt` in its tail.
- **Claude folder names** replace every UTF-16 unit that isn't an ASCII letter or digit with `-` (`DCC.Frontends` -> `DCC-Frontends`); names over 200 characters are cut to 200 and suffixed with `-` plus the base36 of the absolute Java-style string hash of the path.
- **pi** folder names are `--<path without leading slash, / -> ->--`; entries need 8-hex `id`s with `parentId: null` on the first. Every assistant message carries a `usage` object (zeroed) and `model`/`provider`: pi's footer sums usage without a guard and crashes the TUI otherwise. A `session_info` entry carries the title for pi's session picker.
- **Branches.** Claude and pi keep rewound or edited branches in the file, and readers convert only the branch the tool itself resumes.
  - **pi:** the last entry walked back through `parentId`. v1 files without ids keep every entry.
  - **Claude** (mirrors Claude Code 2.1's loader, `src/convert/claude/branch.rs`), in one forward pass: a `last-prompt` with a non-empty `leafUuid` sets the checkpoint and `explicit` makes it binding; an explicit `null` means rewound to empty; any later conversation row clears both flags; a `compact_boundary` cancels the checkpoint. A binding checkpoint is the leaf. Otherwise leaves (entries nothing points at) are tried newest timestamp first, and the first whose branch covers the checkpoint wins, because the pointer is written mid-turn and often lands on a parallel-result sibling. Newest timestamp rather than last line, because sessions rewritten by clync's own sync merge are in tree order, so the last line can be a parallel sibling from an earlier turn. A checkpoint that was ever explicit only yields to direct descendants. With no checkpoint the newest leaf is used. The walk follows `parentUuid`, and `logicalParentUuid` across compaction boundaries.
  - **Result placement:** after pairing, every tool result is moved directly after the assistant message holding its call, for all targets.
  - **Compaction links** that point forward in the file (seen in real sessions, forming a cycle) continue from the last row before the boundary instead.
  - **Completing turns:** parallel tool calls are separate entries sharing `message.id` whose results fork the chain, so every block of a turn on the branch and every result answering one of its calls is added, ordered right after that turn. Entries are replayed in branch order, not file order, because Claude sometimes writes a result before its call and rewrites compacted sessions with the boundary first.
  - **Missing parents** bridge like Claude's chain builder: to the newest entry not already on the chain written at most 5 s before the orphan; past that the chain stops there. Either way a warning is printed.
  - **Compaction summaries** (`isCompactSummary`) go to the stash, since the full history is converted anyway.
- **Tool call IDs** are made unique per session and each result answers the earliest open call with its ID, since some sources reuse IDs across turns.
- **Project directory** comes from the session's own `cwd` (Claude entries, pi header), not from decoding the folder name, which is lossy for paths containing `-`.
- Writers refuse relative project paths and write atomically (temp file + rename, one SQLite transaction). `--force` writes the new copy first and removes the old one only after that succeeds.

### Error handling
- Skip unparseable lines (warn, don't fail)
- Subagent transcripts (`<session>/subagents/*.jsonl`) are rejected; convert the parent session
- Sessions with no conversation (only bookkeeping entries) are skipped
- Lines with invalid UTF-8 (a write cut off mid-character) are skipped like other unparseable lines
- `--all` keeps going past failures and exits non-zero if any session failed

### Testing

**Unit tests** (`src/convert/convert_tests.rs`, `tools.rs`): tool name mapping, pi folder encoding, character-safe truncation, tool-call pairing, opencode ID format and ordering, Claude reader on a fixture (titles, cwd, stash contents, sidechains).

**E2e test** (`tests/convert_e2e.rs`, ignored by default since it needs `opencode` and `pi` on PATH):
```bash
cargo test --test convert_e2e -- --ignored
CLYNC_E2E_CLAUDE=1 cargo test --test convert_e2e -- --ignored   # also resumes in Claude Code (one Haiku call)
```
It writes Claude and pi fixtures into a temp HOME and a git repo with an `origin` remote, runs a chain of conversions in every direction, asserts dedupe counts on each step and on a full re-run, checks `--force` replaces instead of duplicating, then opens the results: `opencode session export`/`list` (plus the project ID and `event_sequence` watermark), `pi --export` (asserting pi resolves the last message as the leaf), pi's TUI under a pseudo-terminal, and a structural check of the Claude file.

### What's intentionally lossy

| Lost in Claude -> opencode | Reason |
|---|---|
| Thinking signatures | Only valid for the provider that issued them |
| Hook/attachment/file-history/api-request entries | Claude bookkeeping |
| Permission mode, bridge-session | Claude-specific |
| Sidechain messages | Subagent transcripts, not the main conversation |

| Lost in opencode -> Claude | Reason |
|---|---|
| idle markers, model-switched events | opencode bookkeeping |
| Git snapshots (start/end SHA) | No equivalent |
| Workspace/agent metadata | Different agent model |
| Tool state.metadata (preview, display) | Claude uses different display model |

| Lost in pi -> Claude/opencode | Reason |
|---|---|
| Model and thinking-level change events | Session settings, not content |
| thinkingSignature field | Provider-specific |

### Clarifications

**opencode DB path:** `~/.local/share/opencode/opencode.db`. Respects `$XDG_DATA_HOME`. clync never creates it; run opencode once first.

**Session title fallback:** 1. Claude `custom-title`, 2. `ai-title`, 3. first user message truncated to 80 characters, 4. `"Untitled session"`.

**Claude assistant merging:** consecutive `type: "assistant"` entries between user entries are one logical turn and become one `ConvertedMessage`. When writing back, they're split into one entry per block with a shared `message.id`.

**opencode v2 seq numbering:** `seq` is unique per session. Writers assign sequential integers; readers order by `seq`.

**opencode shell vs bash:** v2 renamed the command tool from `bash` to `shell`. The IR normalizes to `bash`; the opencode writer maps `bash` -> `shell`.

### Future extensions
- Aider, Cursor, Windsurf session formats (add reader/writer per tool)
- `--watch` mode for auto-conversion of new sessions
- opencode v1 read support (for legacy sessions)
