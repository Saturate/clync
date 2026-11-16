//! Reader fixtures for session shapes seen in real Claude and pi files:
//! branches, parallel tool calls, compaction, gaps, rewinds.

use super::tests::{user_texts, write_fixture};
use super::*;
use serde_json::json;

fn claude_entry(
    uuid: &str,
    parent: Option<&str>,
    ty: &str,
    content: serde_json::Value,
) -> serde_json::Value {
    json!({"type": ty, "uuid": uuid, "parentUuid": parent, "cwd": "/w",
           "timestamp": "2026-01-01T00:00:00Z", "message": {"role": ty, "content": content}})
}

fn tool_results(ir: &ConvertedSession) -> Vec<(String, String, bool)> {
    ir.messages
        .iter()
        .filter_map(|m| m.result_block())
        .map(|(id, out, err)| (id.to_string(), out.to_string(), err))
        .collect()
}

#[test]
fn claude_reader_uses_last_prompt_leaf_and_keeps_parallel_results() {
    let text = |t: &str| json!([{"type": "text", "text": t}]);
    let tool_use = |id: &str| json!([{"type": "tool_use", "id": id, "name": "Bash", "input": {}}]);
    let result =
        |id: &str, out: &str| json!([{"type": "tool_result", "tool_use_id": id, "content": out}]);
    let path = write_fixture(
        "claude-parallel",
        &[
            claude_entry("u1", None, "user", text("go")),
            // Parallel calls: A's result hangs off A as a sibling of call B.
            claude_entry("a1", Some("u1"), "assistant", tool_use("tA")),
            claude_entry("r1", Some("a1"), "user", result("tA", "out A")),
            claude_entry("a2", Some("a1"), "assistant", tool_use("tB")),
            claude_entry("r2", Some("a2"), "user", result("tB", "out B")),
            claude_entry("a3", Some("r2"), "assistant", text("done")),
            json!({"type": "last-prompt", "leafUuid": "a3"}),
            // Claude sometimes appends a result late, after the real leaf.
            claude_entry("late", Some("u1"), "user", result("tX", "stray")),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(user_texts(&ir).last().map(String::as_str), Some("done"));
    let results = tool_results(&ir);
    assert!(
        results.contains(&("tA".into(), "out A".into(), false)),
        "{results:?}"
    );
    assert!(
        results.contains(&("tB".into(), "out B".into(), false)),
        "{results:?}"
    );
}

#[test]
fn missing_parent_bridges_the_gap_instead_of_truncating() {
    let text = |t: &str| json!([{"type": "text", "text": t}]);
    let path = write_fixture(
        "claude-gap",
        &[
            claude_entry("u1", None, "user", text("first")),
            claude_entry("a1", Some("u1"), "assistant", text("answer")),
            claude_entry("u2", Some("lost-line"), "user", text("second")),
            claude_entry("a2", Some("u2"), "assistant", text("answer two")),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(user_texts(&ir), ["first", "answer", "second", "answer two"]);
}

#[test]
fn pi_v1_sessions_without_ids_keep_every_message() {
    let msg = |role: &str, text: &str| json!({"type": "message", "message": {"role": role, "content": [{"type": "text", "text": text}]}});
    let path = write_fixture(
        "pi-v1",
        &[
            json!({"type": "session", "id": "s1", "cwd": "/w"}),
            msg("user", "hello"),
            msg("assistant", "hi"),
        ],
    );
    let ir = pi::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(user_texts(&ir), ["hello", "hi"]);
}

#[test]
fn pi_folder_encoding_matches_pi_for_colons() {
    let enc = pi::encode_project_dir(std::path::Path::new("/srv/a:b/c"));
    assert_eq!(enc, "--srv-a-b-c--");
}

fn claude_turn(
    uuid: &str,
    parent: Option<&str>,
    msg_id: &str,
    block: serde_json::Value,
) -> serde_json::Value {
    json!({"type": "assistant", "uuid": uuid, "parentUuid": parent, "cwd": "/w",
           "timestamp": "2026-01-01T00:00:00Z",
           "message": {"id": msg_id, "role": "assistant", "content": [block]}})
}

fn call_ids(ir: &ConvertedSession) -> Vec<String> {
    ir.messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolCall { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn claude_pointer_written_mid_turn_still_reaches_the_end() {
    let text = |t: &str| json!([{"type": "text", "text": t}]);
    let path = write_fixture(
        "claude-midturn",
        &[
            claude_entry("u1", None, "user", text("go")),
            claude_entry("a1", Some("u1"), "assistant", text("working")),
            json!({"type": "last-prompt", "leafUuid": "a1"}),
            claude_entry("a2", Some("a1"), "assistant", text("still working")),
            claude_entry("u2", Some("a2"), "user", text("next prompt")),
            claude_entry("a3", Some("u2"), "assistant", text("final answer")),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(
        user_texts(&ir).last().map(String::as_str),
        Some("final answer")
    );
}

#[test]
fn claude_interleaved_parallel_calls_keep_every_call_and_result() {
    let tool_use = |id: &str| json!({"type": "tool_use", "id": id, "name": "Bash", "input": {}});
    let result = |id: &str| json!([{"type": "tool_result", "tool_use_id": id, "content": format!("out {id}")}]);
    // One API message with three calls, streamed as three entries; results
    // fork the chain so a parent walk only follows one of them.
    let path = write_fixture(
        "claude-interleaved",
        &[
            claude_entry("u1", None, "user", json!("go")),
            claude_turn("c1", Some("u1"), "m1", tool_use("t1")),
            claude_turn("c2", Some("c1"), "m1", tool_use("t2")),
            claude_turn("c3", Some("c2"), "m1", tool_use("t3")),
            claude_entry("r1", Some("c1"), "user", result("t1")),
            claude_entry("r2", Some("c2"), "user", result("t2")),
            claude_entry("r3", Some("c3"), "user", result("t3")),
            claude_turn(
                "a2",
                Some("r3"),
                "m2",
                json!({"type": "text", "text": "done"}),
            ),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(call_ids(&ir), ["t1", "t2", "t3"]);
    let paired = pair_tool_results(ir.messages.clone());
    let fake = paired
        .iter()
        .filter_map(|m| m.result_block())
        .filter(|(_, out, _)| *out == "No result recorded")
        .count();
    assert_eq!(fake, 0, "a real result was replaced by a placeholder");
    assert_eq!(user_texts(&ir).last().map(String::as_str), Some("done"));
}

#[test]
fn claude_result_written_before_its_call_stays_paired() {
    let path = write_fixture(
        "claude-early-result",
        &[
            claude_entry("u1", None, "user", json!("go")),
            claude_entry(
                "r1",
                Some("c1"),
                "user",
                json!([{"type": "tool_result", "tool_use_id": "t1", "content": "early"}]),
            ),
            claude_turn(
                "c1",
                Some("u1"),
                "m1",
                json!({"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}),
            ),
            claude_turn(
                "a2",
                Some("r1"),
                "m2",
                json!({"type": "text", "text": "done"}),
            ),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    let paired = pair_tool_results(ir.messages);
    let results: Vec<_> = paired.iter().filter_map(|m| m.result_block()).collect();
    assert_eq!(results, [("t1", "early", false)]);
}

#[test]
fn claude_compaction_at_file_head_keeps_chronological_order() {
    let text = |t: &str| json!([{"type": "text", "text": t}]);
    // Claude rewrites compacted sessions with the boundary first; its logical
    // parent sits further down the file.
    let path = write_fixture(
        "claude-compact-head",
        &[
            json!({"type": "system", "subtype": "compact_boundary", "uuid": "b1",
               "parentUuid": null, "logicalParentUuid": "a1"}),
            json!({"type": "user", "uuid": "s1", "parentUuid": "b1", "isCompactSummary": true, "cwd": "/w",
               "message": {"role": "user", "content": "summary of earlier"}}),
            claude_entry("u1", None, "user", text("before compaction")),
            claude_entry("a1", Some("u1"), "assistant", text("old answer")),
            claude_entry("u2", Some("s1"), "user", text("after compaction")),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(
        user_texts(&ir),
        ["before compaction", "old answer", "after compaction"]
    );
    assert!(ir.stash.iter().any(|s| s.get("isCompactSummary").is_some()));
}

#[test]
fn claude_pointer_on_parallel_result_sibling_yields_to_the_continuation() {
    let tool_use = |id: &str| json!({"type": "tool_use", "id": id, "name": "Bash", "input": {}});
    let result = |id: &str| json!([{"type": "tool_result", "tool_use_id": id, "content": "ok"}]);
    let path = write_fixture(
        "claude-sibling-pointer",
        &[
            claude_entry("u1", None, "user", json!("go")),
            claude_turn("c1", Some("u1"), "m1", tool_use("t1")),
            claude_turn("c2", Some("c1"), "m1", tool_use("t2")),
            claude_entry("r1", Some("c1"), "user", result("t1")),
            json!({"type": "attachment", "uuid": "x1", "parentUuid": "r1"}),
            // Written mid-turn, pointing at the first result's branch.
            json!({"type": "last-prompt", "leafUuid": "x1"}),
            claude_entry("r2", Some("c2"), "user", result("t2")),
            claude_turn(
                "a2",
                Some("r2"),
                "m2",
                json!({"type": "text", "text": "both done"}),
            ),
            claude_entry("u2", Some("a2"), "user", json!("next question")),
            claude_turn(
                "a3",
                Some("u2"),
                "m3",
                json!({"type": "text", "text": "final"}),
            ),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    let texts = user_texts(&ir);
    assert_eq!(texts[texts.len() - 2..], ["next question", "final"]);
}

fn at(mut entry: serde_json::Value, seconds: u32) -> serde_json::Value {
    entry["timestamp"] = json!(format!(
        "2026-01-01T00:{:02}:{:02}Z",
        seconds / 60,
        seconds % 60
    ));
    entry
}

fn last_text(ir: &ConvertedSession) -> String {
    user_texts(ir).last().cloned().unwrap_or_default()
}

#[test]
fn claude_stale_parallel_tail_does_not_hide_the_final_answer() {
    let tool_use = |id: &str| json!({"type": "tool_use", "id": id, "name": "Bash", "input": {}});
    let result = |id: &str| json!([{"type": "tool_result", "tool_use_id": id, "content": "ok"}]);
    let path = write_fixture(
        "claude-stale-tail",
        &[
            at(claude_entry("u1", None, "user", json!("go")), 0),
            at(claude_turn("a1", Some("u1"), "A", tool_use("t1")), 1),
            at(claude_entry("r1", Some("a1"), "user", result("t1")), 2),
            at(claude_turn("b1", Some("r1"), "B", tool_use("t2")), 3),
            at(claude_entry("r2", Some("b1"), "user", result("t2")), 4),
            json!({"type": "last-prompt", "leafUuid": "r2"}),
            at(
                claude_turn(
                    "c1",
                    Some("r2"),
                    "C",
                    json!({"type": "text", "text": "FINAL ANSWER"}),
                ),
                9,
            ),
            // Claude appends a stale sibling of the first turn after the fact.
            at(claude_turn("a2", Some("a1"), "A", tool_use("t3")), 2),
            at(claude_entry("r3", Some("a2"), "user", result("t3")), 2),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(last_text(&ir), "FINAL ANSWER");
}

#[test]
fn claude_explicit_rewind_is_cleared_by_later_rows() {
    let text = |t: &str| json!([{"type": "text", "text": t}]);
    let path = write_fixture(
        "claude-explicit-then-more",
        &[
            at(claude_entry("a", None, "user", text("first")), 0),
            at(claude_entry("b", Some("a"), "assistant", text("answer")), 1),
            at(claude_entry("c", Some("b"), "user", text("discarded")), 2),
            json!({"type": "last-prompt", "leafUuid": "b", "explicit": true}),
            at(
                claude_entry("e", Some("b"), "user", text("after rewind")),
                5,
            ),
            at(
                claude_entry("f", Some("e"), "assistant", text("new answer")),
                6,
            ),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(
        user_texts(&ir),
        ["first", "answer", "after rewind", "new answer"]
    );
}

#[test]
fn claude_rewind_to_empty_then_new_prompt_keeps_the_new_conversation() {
    let text = |t: &str| json!([{"type": "text", "text": t}]);
    let path = write_fixture(
        "claude-empty-then-more",
        &[
            at(claude_entry("a", None, "user", text("old")), 0),
            json!({"type": "last-prompt", "leafUuid": null, "explicit": true}),
            at(claude_entry("e", None, "user", text("fresh start")), 5),
            at(claude_entry("f", Some("e"), "assistant", text("reply")), 6),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(user_texts(&ir), ["fresh start", "reply"]);
}

#[test]
fn claude_explicit_rewind_to_empty_converts_nothing() {
    let path = write_fixture(
        "claude-empty",
        &[
            at(claude_entry("a", None, "user", json!("old")), 0),
            json!({"type": "last-prompt", "leafUuid": null, "explicit": true}),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert!(ir.messages.is_empty());
}

#[test]
fn claude_bridges_missing_parents_only_within_five_seconds() {
    let text = |t: &str| json!([{"type": "text", "text": t}]);
    let near = write_fixture(
        "claude-gap-near",
        &[
            at(claude_entry("u1", None, "user", text("first")), 0),
            at(
                claude_entry("a1", Some("u1"), "assistant", text("answer")),
                1,
            ),
            at(claude_entry("u2", Some("lost"), "user", text("second")), 4),
        ],
    );
    let far = write_fixture(
        "claude-gap-far",
        &[
            at(claude_entry("u1", None, "user", text("first")), 0),
            at(
                claude_entry("a1", Some("u1"), "assistant", text("answer")),
                1,
            ),
            at(claude_entry("u2", Some("lost"), "user", text("second")), 30),
        ],
    );
    let near_ir = claude::read_session(&near).unwrap();
    let far_ir = claude::read_session(&far).unwrap();
    std::fs::remove_dir_all(near.parent().unwrap()).ok();
    std::fs::remove_dir_all(far.parent().unwrap()).ok();
    assert_eq!(user_texts(&near_ir), ["first", "answer", "second"]);
    // Claude stops the chain at a gap wider than its bridging window.
    assert_eq!(user_texts(&far_ir), ["second"]);
}

#[test]
fn every_result_follows_the_message_holding_its_call() {
    let tool_use = |id: &str| json!({"type": "tool_use", "id": id, "name": "Bash", "input": {}});
    let result = |id: &str| json!([{"type": "tool_result", "tool_use_id": id, "content": "ok"}]);
    // Streamed tool execution: a call is written after an earlier result of
    // the same turn, and the first call's result arrives much later.
    let path = write_fixture(
        "claude-streamed",
        &[
            at(claude_entry("u1", None, "user", json!("go")), 0),
            at(claude_turn("c1", Some("u1"), "M", tool_use("t1")), 1),
            at(claude_turn("c2", Some("c1"), "M", tool_use("t2")), 1),
            at(claude_entry("r2", Some("c2"), "user", result("t2")), 2),
            at(claude_turn("c3", Some("r2"), "M", tool_use("t3")), 3),
            at(claude_entry("r3", Some("c3"), "user", result("t3")), 4),
            at(
                claude_turn(
                    "a2",
                    Some("r3"),
                    "N",
                    json!({"type": "text", "text": "done"}),
                ),
                6,
            ),
            at(claude_entry("r1", Some("c1"), "user", result("t1")), 5),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    let paired = pair_tool_results(ir.messages);
    let mut expected: Vec<String> = Vec::new();
    for msg in &paired {
        if let Some((id, _, _)) = msg.result_block() {
            assert_eq!(
                expected.first().map(String::as_str),
                Some(id),
                "result out of place"
            );
            expected.remove(0);
        } else {
            assert!(
                expected.is_empty(),
                "unanswered calls before {:?}",
                msg.role
            );
            for b in &msg.content {
                if let ContentBlock::ToolCall { id, .. } = b {
                    expected.push(id.clone());
                }
            }
        }
    }
}

#[test]
fn compaction_link_pointing_forward_keeps_earlier_history() {
    let text = |t: &str| json!([{"type": "text", "text": t}]);
    let path = write_fixture(
        "claude-compact-cycle",
        &[
            at(claude_entry("u1", None, "user", text("before")), 0),
            at(
                claude_entry("a1", Some("u1"), "assistant", text("old answer")),
                1,
            ),
            at(
                json!({"type": "system", "subtype": "compact_boundary", "uuid": "b1",
                  "parentUuid": null, "logicalParentUuid": "x1"}),
                2,
            ),
            at(
                json!({"type": "user", "uuid": "s1", "parentUuid": "b1", "isCompactSummary": true,
                  "cwd": "/w", "message": {"role": "user", "content": "summary"}}),
                3,
            ),
            // The boundary's logical parent is written after it and loops back.
            at(
                json!({"type": "attachment", "uuid": "x1", "parentUuid": "s1"}),
                3,
            ),
            at(claude_entry("u2", Some("x1"), "user", text("after")), 4),
        ],
    );
    let ir = claude::read_session(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
    assert_eq!(user_texts(&ir), ["before", "old answer", "after"]);
}
