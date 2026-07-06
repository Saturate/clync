use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

use crate::cmd::short_uuid;
use crate::config::Config;
use crate::list::list_sessions;
use crate::mcp_help::help_text;
use crate::scanner::ScanFilter;

#[derive(Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

pub fn run_mcp_server() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: Value::Null,
                    result: None,
                    error: Some(json!({"code": -32700, "message": format!("parse error: {e}")})),
                };
                write_response(&mut stdout, &resp)?;
                continue;
            }
        };

        if request.method.starts_with("notifications/") {
            continue;
        }
        let response = handle_request(&request);
        write_response(&mut stdout, &response)?;
    }

    Ok(())
}

fn write_response(stdout: &mut io::Stdout, resp: &JsonRpcResponse) -> Result<()> {
    let json = serde_json::to_string(resp)?;
    writeln!(stdout, "{json}")?;
    stdout.flush()?;
    Ok(())
}

fn handle_request(req: &JsonRpcRequest) -> JsonRpcResponse {
    let id = req.id.clone().unwrap_or(Value::Null);

    match req.method.as_str() {
        "initialize" => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "clync",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
            error: None,
        },
        "tools/list" => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(json!({
                "tools": tool_definitions()
            })),
            error: None,
        },
        "tools/call" => {
            let tool_name = req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args = req.params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(tool_name, &args) {
                Ok(result) => JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id,
                    result: Some(json!({
                        "content": [{"type": "text", "text": result}]
                    })),
                    error: None,
                },
                Err(e) => JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id,
                    result: Some(json!({
                        "content": [{"type": "text", "text": format!("error: {e}")}],
                        "isError": true
                    })),
                    error: None,
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(
                json!({"code": -32601, "message": format!("unknown method: {}", req.method)}),
            ),
        },
    }
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_sessions",
            "description": "List Claude Code sessions with optional search. Returns UUID, project, message count, first message preview, size, and modification time for each session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search sessions by project name, UUID, or first message content"
                    },
                    "max_age_days": {
                        "type": "integer",
                        "description": "Only show sessions modified within N days"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max number of results (default: 20)"
                    }
                }
            }
        },
        {
            "name": "session_detail",
            "description": "Get details for a specific session by UUID (or prefix). Returns message count, participants, timestamps, project, and the last N messages.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "uuid": {
                        "type": "string",
                        "description": "Full or partial UUID of the session"
                    },
                    "tail": {
                        "type": "integer",
                        "description": "Number of recent messages to include (default: 10)"
                    }
                },
                "required": ["uuid"]
            }
        },
        {
            "name": "sync_status",
            "description": "Show what differs between local sessions and the encrypted sync repo. Lists sessions that are local-only, remote-only, diverged, or in sync.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "sync_push",
            "description": "Encrypt and push changed sessions and extras (memories, settings, etc.) to the sync repo.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "git": {
                        "type": "boolean",
                        "description": "Also git add, commit, and push to remote (default: auto_git config, usually true)"
                    }
                }
            }
        },
        {
            "name": "sync_pull",
            "description": "Pull and decrypt sessions from sync repo, smart-merging any diverged sessions using UUID-based conversation trees.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "git": {
                        "type": "boolean",
                        "description": "Also git pull from remote first (default: auto_git config, usually true)"
                    }
                }
            }
        },
        {
            "name": "sync_log",
            "description": "Show recent sync operations with machine name, timestamps, and what was synced/merged.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "Number of recent entries to show (default: 10)"
                    }
                }
            }
        },
        {
            "name": "config_show",
            "description": "Show current clync configuration: sync repo path, encryption method, and which targets (sessions, memories, settings, etc.) are enabled.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "help",
            "description": "Show available clync commands and usage information. Call with a specific topic for detailed help.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Help topic: 'setup', 'sync', 'list', 'mcp', 'config', or 'all'"
                    }
                }
            }
        }
    ])
}

fn call_tool(name: &str, args: &Value) -> Result<String> {
    match name {
        "list_sessions" => {
            let config = Config::load()?;
            let query = args.get("query").and_then(|v| v.as_str());
            let max_age = args.get("max_age_days").and_then(|v| v.as_u64());
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

            let filter = ScanFilter {
                max_age_days: max_age,
                max_file_size: None,
            };

            let sessions = list_sessions(&config.claude_projects_dir(), query, &filter)?;
            let limited: Vec<_> = sessions.into_iter().take(limit).collect();

            Ok(serde_json::to_string_pretty(&limited)?)
        }
        "session_detail" => {
            let config = Config::load()?;
            let uuid_prefix = args
                .get("uuid")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("uuid is required"))?;
            let tail = args.get("tail").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

            let filter = ScanFilter::default();
            let sessions = crate::scanner::scan_sessions(&config.claude_projects_dir(), &filter)?;

            let session = sessions
                .iter()
                .find(|s| s.uuid.starts_with(uuid_prefix))
                .ok_or_else(|| anyhow::anyhow!("no session matching '{uuid_prefix}'"))?;

            let entries = crate::parser::parse_jsonl_file(&session.jsonl_path)?;

            let user_msgs = entries
                .iter()
                .filter(|e| e.entry_type.as_deref() == Some("user"))
                .count();
            let assistant_msgs = entries
                .iter()
                .filter(|e| e.entry_type.as_deref() == Some("assistant"))
                .count();

            let first_ts = entries.first().map(|e| e.timestamp_millis()).unwrap_or(0);
            let last_ts = entries.last().map(|e| e.timestamp_millis()).unwrap_or(0);

            let recent: Vec<Value> = entries
                .iter()
                .filter(|e| matches!(e.entry_type.as_deref(), Some("user") | Some("assistant")))
                .rev()
                .take(tail)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|e| {
                    let role = e.entry_type.as_deref().unwrap_or("unknown");
                    let content = e
                        .extra
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| {
                            c.as_str().map(|s| s.to_string()).or_else(|| {
                                c.as_array().map(|arr| {
                                    arr.iter()
                                        .filter_map(|item| {
                                            item.get("text").and_then(|t| t.as_str())
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                })
                            })
                        })
                        .unwrap_or_default();
                    let truncated = truncate_str(&content, 500);
                    json!({"role": role, "content": truncated})
                })
                .collect();

            let detail = json!({
                "uuid": session.uuid,
                "project": session.entry.project_path,
                "size_bytes": session.entry.size,
                "user_messages": user_msgs,
                "assistant_messages": assistant_msgs,
                "total_entries": entries.len(),
                "first_timestamp": first_ts,
                "last_timestamp": last_ts,
                "recent_messages": recent
            });

            Ok(serde_json::to_string_pretty(&detail)?)
        }
        "sync_status" => {
            let config = Config::load()?;
            let cipher = crate::crypto::Cipher::from_config(&config.encryption)?;
            let storage = crate::storage::GitStorage::new(config.sync.repo.clone());
            let filter = ScanFilter::default();
            let result = crate::sync::status(&config, &cipher, &filter, &storage)?;

            let mut output = String::new();
            if result.local_only.is_empty()
                && result.remote_only.is_empty()
                && result.diverged.is_empty()
            {
                output.push_str(&format!("all {} sessions in sync", result.in_sync));
            } else {
                for s in &result.local_only {
                    output.push_str(&format!(
                        "+ {} [{}] (local only)\n",
                        short_uuid(&s.uuid),
                        s.project
                    ));
                }
                for s in &result.remote_only {
                    output.push_str(&format!(
                        "- {} [{}] (remote only)\n",
                        short_uuid(&s.uuid),
                        s.project
                    ));
                }
                for s in &result.diverged {
                    output.push_str(&format!(
                        "~ {} [{}] (diverged)\n",
                        short_uuid(&s.uuid),
                        s.project
                    ));
                }
                if result.in_sync > 0 {
                    output.push_str(&format!("in sync: {}", result.in_sync));
                }
            }
            Ok(output)
        }
        "sync_push" => {
            let config = Config::load()?;
            let git = args
                .get("git")
                .and_then(|v| v.as_bool())
                .unwrap_or(config.sync.auto_git);
            let r = crate::cmd::do_push(git)?;
            Ok(format!(
                "pushed {} sessions ({} unchanged), {} extras, {} memories",
                r.sessions, r.skipped, r.extras, r.memories
            ))
        }
        "sync_pull" => {
            let config = Config::load()?;
            let git = args
                .get("git")
                .and_then(|v| v.as_bool())
                .unwrap_or(config.sync.auto_git);
            let r = crate::cmd::do_pull(git)?;
            Ok(format!(
                "pulled {} new, {} merged, {} unchanged, {} extras, {} memories",
                r.pulled, r.merged, r.skipped, r.extras, r.memories
            ))
        }
        "sync_log" => {
            let config = Config::load()?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let entries = crate::synclog::read_recent(&config.sync.repo, limit)?;
            Ok(serde_json::to_string_pretty(&entries)?)
        }
        "config_show" => tool_config_show(),
        "help" => {
            let topic = args.get("topic").and_then(|v| v.as_str()).unwrap_or("all");
            Ok(help_text(topic))
        }
        _ => anyhow::bail!("unknown tool: {name}"),
    }
}

fn tool_config_show() -> Result<String> {
    let config = Config::load()?;
    let enc_method = match &config.encryption {
        crate::config::EncryptionConfig::KeyFile { path } => {
            format!("key_file ({})", path.display())
        }
        crate::config::EncryptionConfig::Passphrase { env_var } => {
            format!("passphrase (${env_var})")
        }
        crate::config::EncryptionConfig::OnePassword { reference } => {
            format!("1password ({reference})")
        }
        crate::config::EncryptionConfig::Bitwarden { item_id, .. } => {
            format!("bitwarden ({item_id})")
        }
        crate::config::EncryptionConfig::Pass { entry } => {
            format!("pass ({entry})")
        }
        crate::config::EncryptionConfig::None => "none (plain text)".into(),
    };
    let lfs = if config.sync.git.lfs_threshold == 0 {
        "disabled".to_string()
    } else {
        format!(
            "{}MB threshold",
            config.sync.git.lfs_threshold / (1024 * 1024)
        )
    };
    let t = &config.targets;
    Ok(format!(
        "sync repo: {}\nclaude dir: {}\nencryption: {}\ncompanion dirs: {}\ngit lfs: {lfs}\n\ntargets:\n  sessions: {}\n  memories: {}\n  settings: {}\n  commands: {}\n  skills: {}\n  global CLAUDE.md: {}",
        config.sync.repo.display(),
        config.sync.claude_dir.display(),
        enc_method,
        config.sync.include_companion_dirs,
        t.sessions,
        t.memories,
        t.settings,
        t.commands,
        t.skills,
        t.global_claude_md
    ))
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max.saturating_sub(3);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}
