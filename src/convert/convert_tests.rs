use super::*;
use serde_json::json;

fn call(id: &str) -> ConvertedMessage {
    ConvertedMessage {
        role: MessageRole::Assistant,
        timestamp_ms: 1,
        content: vec![ContentBlock::ToolCall {
            id: id.to_string(),
            name: "bash".to_string(),
            input: json!({}),
        }],
        tool_call_id: None,
        tool_name: None,
    }
}

fn result(id: &str) -> ConvertedMessage {
    ConvertedMessage::tool_result(2, id.to_string(), None, "ok".to_string(), false)
}

#[test]
fn truncate_counts_characters_not_bytes() {
    let title = "æøå".repeat(30);
    let cut = truncate(&title, 10);
    assert_eq!(cut.chars().count(), 10);
    assert!(cut.ends_with("..."));
    assert_eq!(truncate("short", 10), "short");
}

#[test]
fn pi_project_dir_encoding_matches_pi() {
    let path = std::path::Path::new("/Users/alkj/code/github/clync");
    assert_eq!(
        pi::encode_project_dir(path),
        "--Users-alkj-code-github-clync--"
    );
    assert_eq!(
        pi::decode_project_dir("--Users-alkj-code-github-clync--"),
        path
    );
}

#[test]
fn unanswered_tool_call_gets_error_result() {
    let out = pair_tool_results(vec![call("a")]);
    assert_eq!(out.len(), 2);
    let (id, output, is_error) = out[1].result_block().unwrap();
    assert_eq!((id, output, is_error), ("a", "No result recorded", true));
}

#[test]
fn orphan_and_duplicate_results_are_dropped() {
    let out = pair_tool_results(vec![call("a"), result("a"), result("a"), result("zzz")]);
    assert_eq!(out.len(), 2);
}

#[test]
fn opencode_ids_match_format_and_sort_order() {
    let older = opencode::opencode_id("ses", 1_700_000_000_000, true);
    let newer = opencode::opencode_id("ses", 1_700_086_400_000, true);
    assert_eq!(older.len(), "ses_".len() + 12 + 14);
    // Sessions use descending IDs so newer ones sort first.
    assert!(newer < older);
    let first = opencode::opencode_id("msg", 1_700_000_000_000, false);
    let second = opencode::opencode_id("msg", 1_700_000_000_001, false);
    assert!(first < second);
}

#[test]
fn claude_reader_keeps_conversation_and_stashes_only_content() {
    let dir = std::env::temp_dir().join(format!("clync-convert-unit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("11111111-2222-3333-4444-555555555555.jsonl");
    let lines = [
        json!({"type": "ai-title", "aiTitle": "Generated"}),
        json!({"type": "custom-title", "customTitle": "Mine"}),
        json!({"type": "attachment", "attachment": {"big": "noise"}}),
        json!({"type": "user", "uuid": "u1", "parentUuid": null, "cwd": "/work/my-repo",
               "timestamp": "2026-01-01T00:00:00Z",
               "message": {"role": "user", "content": "hello"}}),
        json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1", "timestamp": "2026-01-01T00:00:01Z",
               "message": {"role": "assistant", "model": "claude-x",
                           "content": [{"type": "thinking", "thinking": "hmm", "signature": "sig"}]}}),
        json!({"type": "assistant", "uuid": "a2", "parentUuid": "a1", "timestamp": "2026-01-01T00:00:02Z",
               "message": {"role": "assistant",
                           "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]}}),
        json!({"type": "user", "uuid": "u2", "parentUuid": "a2", "timestamp": "2026-01-01T00:00:03Z",
               "message": {"role": "user", "content": [
                   {"type": "tool_result", "tool_use_id": "t1", "content": "out"}]}}),
        json!({"type": "user", "isSidechain": true, "timestamp": "2026-01-01T00:00:04Z",
               "message": {"role": "user", "content": "subagent prompt"}}),
    ];
    let body: String = lines.iter().map(|l| format!("{}\n", l)).collect();
    std::fs::write(&path, body).unwrap();

    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(ir.title, "Mine");
    // cwd wins over decoding the folder name, which would mangle "my-repo".
    assert_eq!(ir.project_dir, std::path::Path::new("/work/my-repo"));
    assert_eq!(ir.model.as_deref(), Some("claude-x"));
    let roles: Vec<_> = ir.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(
        roles,
        [
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::ToolResult
        ]
    );
    assert_eq!(ir.stash.len(), 1);
    assert_eq!(ir.stash[0]["kind"], "thinking");
    assert!(ir.stash[0].get("signature").is_none());
}

#[test]
fn reused_tool_call_ids_pair_by_position() {
    // Two turns that both call "call_1": each result must answer its own call.
    let out = pair_tool_results(vec![
        call("call_1"),
        result("call_1"),
        call("call_1"),
        result("call_1"),
    ]);
    let mut calls = Vec::new();
    let mut results = Vec::new();
    for m in &out {
        for b in &m.content {
            match b {
                ContentBlock::ToolCall { id, .. } => calls.push(id.clone()),
                ContentBlock::ToolResult { call_id, .. } => results.push(call_id.clone()),
                ContentBlock::Text { .. } => {}
            }
        }
    }
    assert_eq!(calls, ["call_1", "call_1_1"]);
    assert_eq!(results, calls);
}

#[test]
fn claude_folder_names_match_claude_code() {
    let enc = |p: &str| claude::encode_project_dir(std::path::Path::new(p));
    assert_eq!(
        enc("/Users/alkj/code/DCC.Frontends"),
        "-Users-alkj-code-DCC-Frontends"
    );
    // One dash per UTF-16 unit: é is one unit, the crab emoji is two.
    assert_eq!(enc("/Users/alkj/café 🦀"), "-Users-alkj-caf----");
    // Expected values computed with Claude Code's own function.
    let long = format!("/Users/alkj{}", "/very-long-directory-name".repeat(9));
    assert!(enc(&long).ends_with("-very-long-dir-1klf9o"));
    assert_eq!(enc(&long).len(), 200 + "-1klf9o".len());
}

#[test]
fn opencode_project_id_matches_remote_hash() {
    use opencode::project::{normalize_remote, sha1_hex};
    for url in [
        "https://github.com/Saturate/clync.git",
        "git@github.com:Saturate/clync.git",
        "ssh://git@GitHub.com:22/Saturate/clync/",
    ] {
        assert_eq!(
            normalize_remote(url).as_deref(),
            Some("github.com/Saturate/clync"),
            "{url}"
        );
    }
    assert_eq!(normalize_remote("file:///srv/repo.git"), None);
    // The ID opencode cached for this repo in .git/opencode.
    assert_eq!(
        sha1_hex("git-remote:github.com/Saturate/clync"),
        "889fb3588fa2deeea19466749264ea9bf933fa71"
    );
}

pub(super) fn write_fixture(name: &str, lines: &[serde_json::Value]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "clync-convert-unit-{}-{}",
        name,
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("22222222-3333-4444-5555-666666666666.jsonl");
    let body: String = lines.iter().map(|l| format!("{}\n", l)).collect();
    std::fs::write(&path, body).unwrap();
    path
}

pub(super) fn user_texts(ir: &ConvertedSession) -> Vec<String> {
    ir.messages
        .iter()
        .filter(|m| m.role != MessageRole::ToolResult)
        .map(|m| m.joined_text())
        .collect()
}

#[test]
fn claude_reader_follows_the_live_branch_across_compaction() {
    let msg = |uuid: &str, parent: Option<&str>, ty: &str, text: &str| {
        json!({"type": ty, "uuid": uuid, "parentUuid": parent, "cwd": "/w",
               "message": {"role": ty, "content": [{"type": "text", "text": text}]}})
    };
    let path = write_fixture(
        "claude-branch",
        &[
            msg("u1", None, "user", "first"),
            msg("a1", Some("u1"), "assistant", "answer"),
            msg("u2", Some("a1"), "user", "abandoned"),
            msg("a2", Some("u2"), "assistant", "abandoned answer"),
            json!({"type": "system", "subtype": "compact_boundary", "uuid": "c1",
               "parentUuid": null, "logicalParentUuid": "a1"}),
            msg("u3", Some("c1"), "user", "live"),
            msg("a3", Some("u3"), "assistant", "live answer"),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(user_texts(&ir), ["first", "answer", "live", "live answer"]);
}

#[test]
fn pi_reader_follows_the_live_branch() {
    let msg = |id: &str, parent: Option<&str>, role: &str, text: &str| {
        json!({"type": "message", "id": id, "parentId": parent,
               "message": {"role": role, "content": [{"type": "text", "text": text}]}})
    };
    let path = write_fixture(
        "pi-branch",
        &[
            json!({"type": "session", "version": 3, "id": "s1", "cwd": "/w"}),
            msg("m1", None, "user", "first"),
            msg("m2", Some("m1"), "assistant", "answer"),
            msg("m3", Some("m2"), "user", "dead"),
            msg("m4", Some("m2"), "user", "live"),
            json!({"type": "session_info", "id": "i1", "parentId": "m4", "name": "Named"}),
        ],
    );
    let ir = pi::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(user_texts(&ir), ["first", "answer", "live"]);
    assert_eq!(ir.title, "Named");
}
