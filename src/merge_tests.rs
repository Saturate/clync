use super::*;
use crate::parser::parse_jsonl;

fn entry(uuid: &str, ts: u64, content: &str) -> ConversationEntry {
    serde_json::from_str(&format!(
        r#"{{"uuid":"{uuid}","type":"user","timestamp":{ts},"message":{{"content":"{content}"}}}}"#
    ))
    .unwrap()
}

fn entry_with_parent(uuid: &str, parent: &str, ts: u64) -> ConversationEntry {
    serde_json::from_str(&format!(
        r#"{{"uuid":"{uuid}","parentUuid":"{parent}","type":"user","timestamp":{ts}}}"#
    ))
    .unwrap()
}

#[test]
fn merge_identical() {
    let entries = vec![entry("a", 100, "hello"), entry("b", 200, "world")];
    let result = smart_merge(&entries, &entries);
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.edits_resolved, 0);
}

#[test]
fn merge_local_only() {
    let local = vec![entry("a", 100, "hello"), entry("b", 200, "world")];
    let remote = vec![entry("a", 100, "hello")];
    let result = smart_merge(&local, &remote);
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.local_only, 1);
}

#[test]
fn merge_remote_only() {
    let local = vec![entry("a", 100, "hello")];
    let remote = vec![entry("a", 100, "hello"), entry("c", 300, "new")];
    let result = smart_merge(&local, &remote);
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.remote_only, 1);
}

#[test]
fn merge_edit_newer_wins() {
    let local = vec![entry("a", 100, "old")];
    let remote = vec![entry("a", 200, "new")];
    let result = smart_merge(&local, &remote);
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.edits_resolved, 1);
    assert_eq!(result.entries[0].timestamp_millis(), 200);
}

#[test]
fn merge_preserves_tree_order() {
    let local = vec![
        entry("root", 100, "root"),
        entry_with_parent("child1", "root", 200),
    ];
    let remote = vec![
        entry("root", 100, "root"),
        entry_with_parent("child2", "root", 300),
    ];
    let result = smart_merge(&local, &remote);
    assert_eq!(result.entries.len(), 3);
    assert_eq!(result.entries[0].uuid.as_deref(), Some("root"));
}

#[test]
fn merge_diverged_branches() {
    let base = vec![
        entry("msg-1", 1000, "hello"),
        entry_with_parent("msg-2", "msg-1", 2000),
    ];
    let mut local = base.clone();
    local.push(entry_with_parent("msg-4", "msg-2", 3500));

    let mut remote = base;
    remote.push(entry_with_parent("msg-3", "msg-2", 3000));

    let result = smart_merge(&local, &remote);

    let uuids: Vec<_> = result
        .entries
        .iter()
        .filter_map(|e| e.uuid.as_deref())
        .collect();
    assert!(uuids.contains(&"msg-1"), "missing msg-1: {uuids:?}");
    assert!(uuids.contains(&"msg-2"), "missing msg-2: {uuids:?}");
    assert!(
        uuids.contains(&"msg-3"),
        "missing msg-3 (remote): {uuids:?}"
    );
    assert!(uuids.contains(&"msg-4"), "missing msg-4 (local): {uuids:?}");
    assert_eq!(result.entries.len(), 4, "expected 4 entries: {uuids:?}");
}

#[test]
fn merge_dedup_non_uuid_entries() {
    let meta = r#"{"type":"mode","mode":"normal"}"#;
    let local = parse_jsonl(meta.as_bytes()).unwrap();
    let remote = parse_jsonl(meta.as_bytes()).unwrap();
    let result = smart_merge(&local, &remote);
    assert_eq!(result.entries.len(), 1);
}

#[test]
fn merge_large_conversation() {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for i in 0..100 {
        let parent = if i == 0 {
            None
        } else {
            Some(format!("m{}", i - 1))
        };
        let e = entry_with_parent(
            &format!("m{i}"),
            parent.as_deref().unwrap_or(""),
            i as u64 * 100,
        );
        local.push(e.clone());
        remote.push(e);
    }
    // Local adds 50 more
    for i in 100..150 {
        local.push(entry_with_parent(
            &format!("m{i}"),
            &format!("m{}", i - 1),
            i as u64 * 100,
        ));
    }
    // Remote adds 50 different
    for i in 150..200 {
        remote.push(entry_with_parent(&format!("m{i}"), "m99", i as u64 * 100));
    }

    let result = smart_merge(&local, &remote);
    assert_eq!(result.entries.len(), 200, "all 200 entries preserved");
    assert_eq!(result.local_only, 50);
    assert_eq!(result.remote_only, 50);
}

#[test]
fn merge_flat_no_parents() {
    let local = vec![
        entry("a", 100, "first"),
        entry("b", 200, "second"),
        entry("c", 300, "local only"),
    ];
    let remote = vec![
        entry("a", 100, "first"),
        entry("b", 200, "second"),
        entry("d", 400, "remote only"),
    ];
    let result = smart_merge(&local, &remote);
    let uuids: Vec<_> = result
        .entries
        .iter()
        .filter_map(|e| e.uuid.as_deref())
        .collect();
    assert_eq!(uuids.len(), 4);
    assert!(uuids.contains(&"a"));
    assert!(uuids.contains(&"b"));
    assert!(uuids.contains(&"c"));
    assert!(uuids.contains(&"d"));
}

#[test]
fn merge_idempotent() {
    let local = vec![
        entry("m1", 100, "hello"),
        entry_with_parent("m2", "m1", 200),
        entry_with_parent("m3", "m2", 300),
    ];
    let remote = vec![
        entry("m1", 100, "hello"),
        entry_with_parent("m2", "m1", 200),
        entry_with_parent("m4", "m2", 400),
    ];

    let first = smart_merge(&local, &remote);
    let second = smart_merge(&first.entries, &remote);
    let third = smart_merge(&second.entries, &first.entries);

    assert_eq!(
        first.entries.len(),
        second.entries.len(),
        "merge(merge(L,R), R) == merge(L,R)"
    );
    assert_eq!(
        first.entries.len(),
        third.entries.len(),
        "merge should be idempotent"
    );

    let first_uuids: Vec<_> = first
        .entries
        .iter()
        .filter_map(|e| e.uuid.as_deref())
        .collect();
    let third_uuids: Vec<_> = third
        .entries
        .iter()
        .filter_map(|e| e.uuid.as_deref())
        .collect();
    for uuid in &first_uuids {
        assert!(
            third_uuids.contains(uuid),
            "uuid {uuid} lost after re-merge"
        );
    }
}

#[test]
fn merge_with_interleaved_metadata() {
    let local_raw = r#"{"type":"mode","mode":"normal","sessionId":"s1"}
{"uuid":"m1","type":"user","timestamp":100,"message":{"content":"hi"}}
{"type":"summary","data":"local summary"}
{"uuid":"m2","parentUuid":"m1","type":"assistant","timestamp":200,"message":{"content":"hello"}}
{"type":"mode","mode":"plan","sessionId":"s1"}"#;

    let remote_raw = r#"{"type":"mode","mode":"normal","sessionId":"s1"}
{"uuid":"m1","type":"user","timestamp":100,"message":{"content":"hi"}}
{"uuid":"m2","parentUuid":"m1","type":"assistant","timestamp":200,"message":{"content":"hello"}}
{"uuid":"m3","parentUuid":"m2","type":"user","timestamp":300,"message":{"content":"remote msg"}}
{"type":"summary","data":"remote summary"}"#;

    let local = parse_jsonl(local_raw.as_bytes()).unwrap();
    let remote = parse_jsonl(remote_raw.as_bytes()).unwrap();
    let result = smart_merge(&local, &remote);

    let uuids: Vec<_> = result
        .entries
        .iter()
        .filter_map(|e| e.uuid.as_deref())
        .collect();
    assert_eq!(uuids.len(), 3, "should have m1, m2, m3: {uuids:?}");
    assert!(uuids.contains(&"m3"), "remote message missing");

    let non_uuid = result.entries.iter().filter(|e| e.uuid.is_none()).count();
    assert!(
        non_uuid >= 2,
        "should preserve metadata entries, got {non_uuid}"
    );
}

#[test]
fn merge_three_way() {
    let base = vec![entry("m1", 100, "base"), entry_with_parent("m2", "m1", 200)];

    let mut from_a = base.clone();
    from_a.push(entry_with_parent("a1", "m2", 300));

    let mut from_b = base.clone();
    from_b.push(entry_with_parent("b1", "m2", 400));

    let mut from_c = base;
    from_c.push(entry_with_parent("c1", "m2", 500));

    // A merges B's changes
    let ab = smart_merge(&from_a, &from_b);
    // Then merges C's changes
    let abc = smart_merge(&ab.entries, &from_c);

    let uuids: Vec<_> = abc
        .entries
        .iter()
        .filter_map(|e| e.uuid.as_deref())
        .collect();
    assert!(uuids.contains(&"m1"), "base m1 missing");
    assert!(uuids.contains(&"m2"), "base m2 missing");
    assert!(uuids.contains(&"a1"), "A's message missing");
    assert!(uuids.contains(&"b1"), "B's message missing");
    assert!(uuids.contains(&"c1"), "C's message missing");
    assert_eq!(uuids.len(), 5, "should have all 5: {uuids:?}");
}

#[test]
fn merge_empty_local() {
    let local: Vec<ConversationEntry> = Vec::new();
    let remote = vec![entry("m1", 100, "only remote")];
    let result = smart_merge(&local, &remote);
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.remote_only, 1);
}

#[test]
fn merge_empty_remote() {
    let local = vec![entry("m1", 100, "only local")];
    let remote: Vec<ConversationEntry> = Vec::new();
    let result = smart_merge(&local, &remote);
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.local_only, 1);
}

#[test]
fn merge_both_empty() {
    let result = smart_merge(&[], &[]);
    assert_eq!(result.entries.len(), 0);
}

#[test]
fn merge_deep_chain() {
    let mut local = Vec::new();
    for i in 0..500 {
        let parent = if i == 0 {
            None
        } else {
            Some(format!("m{}", i - 1))
        };
        local.push(entry_with_parent(
            &format!("m{i}"),
            parent.as_deref().unwrap_or(""),
            i as u64,
        ));
    }
    let mut remote = local.clone();
    remote.push(entry_with_parent("extra", "m499", 1000));

    let result = smart_merge(&local, &remote);
    assert_eq!(result.entries.len(), 501);
    assert!(
        result
            .entries
            .iter()
            .any(|e| e.uuid.as_deref() == Some("extra")),
        "extra message at end of deep chain should be included"
    );
}

#[test]
fn merge_branching_conversation() {
    // Simulate a conversation where the user edited a message creating a branch
    let local = vec![
        entry("root", 100, "start"),
        entry_with_parent("reply1", "root", 200),
        entry_with_parent("edit1", "root", 300), // user edited, new branch from root
        entry_with_parent("reply2", "edit1", 400),
    ];
    let remote = vec![
        entry("root", 100, "start"),
        entry_with_parent("reply1", "root", 200),
        entry_with_parent("remote-reply", "reply1", 500),
    ];
    let result = smart_merge(&local, &remote);
    let uuids: Vec<_> = result
        .entries
        .iter()
        .filter_map(|e| e.uuid.as_deref())
        .collect();
    assert_eq!(uuids.len(), 5, "all branches preserved: {uuids:?}");
    assert!(uuids.contains(&"edit1"));
    assert!(uuids.contains(&"reply2"));
    assert!(uuids.contains(&"remote-reply"));
}

fn uuids(entries: &[ConversationEntry]) -> Vec<&str> {
    entries
        .iter()
        .map(|e| e.uuid.as_deref().unwrap_or("-"))
        .collect()
}

#[test]
fn merge_keeps_local_file_order_and_last_prompt_position() {
    // Parallel tool results: t2 is a sibling of t1 written later, and
    // last-prompt is the final row. Tree order would put t2 under t1's
    // subtree and move last-prompt to the end with other metadata.
    let raw = r#"{"uuid":"u1","type":"user","timestamp":"2026-01-01T00:00:01.000Z"}
{"uuid":"a1","parentUuid":"u1","type":"assistant","timestamp":"2026-01-01T00:00:02.000Z"}
{"uuid":"t2","parentUuid":"u1","type":"user","timestamp":"2026-01-01T00:00:04.000Z"}
{"uuid":"t1","parentUuid":"a1","type":"user","timestamp":"2026-01-01T00:00:03.000Z"}
{"type":"last-prompt","lastPrompt":"hi","leafUuid":"t1"}"#;
    let entries = parse_jsonl(raw.as_bytes()).unwrap();
    let result = smart_merge(&entries, &entries);
    assert_eq!(uuids(&result.entries), ["u1", "a1", "t2", "t1", "-"]);
}

#[test]
fn merge_inserts_remote_turn_after_last_shared_entry() {
    let local = vec![
        entry("m1", 100, "hi"),
        entry_with_parent("m2", "m1", 200),
        entry_with_parent("l1", "m2", 300),
    ];
    let remote = vec![
        entry("m1", 100, "hi"),
        entry_with_parent("m2", "m1", 200),
        entry_with_parent("r1", "m2", 250),
        entry_with_parent("r2", "r1", 260),
    ];
    let result = smart_merge(&local, &remote);
    assert_eq!(uuids(&result.entries), ["m1", "m2", "r1", "r2", "l1"]);
    assert_eq!((result.local_only, result.remote_only), (1, 2));

    let again = smart_merge(&result.entries, &remote);
    assert_eq!(uuids(&again.entries), uuids(&result.entries));
}

#[test]
fn merge_newer_iso_timestamp_wins_edit() {
    let local =
        parse_jsonl(br#"{"uuid":"a","type":"user","timestamp":"2026-01-01T00:00:01.000Z","v":1}"#)
            .unwrap();
    let remote =
        parse_jsonl(br#"{"uuid":"a","type":"user","timestamp":"2026-01-01T00:00:02.000Z","v":2}"#)
            .unwrap();
    let result = smart_merge(&local, &remote);
    assert_eq!(result.edits_resolved, 1);
    assert_eq!(result.entries[0].extra["v"], 2);
}

#[test]
fn merge_local_prefix_of_remote_with_repeated_metadata_equals_remote() {
    let raw = r#"{"type":"mode","mode":"normal"}
{"uuid":"u1","type":"user","timestamp":"2026-01-01T00:00:01.000Z"}
{"uuid":"a1","parentUuid":"u1","type":"assistant","timestamp":"2026-01-01T00:00:02.000Z"}
{"type":"mode","mode":"normal"}
{"type":"last-prompt","lastPrompt":"one","leafUuid":"a1"}
{"uuid":"u2","parentUuid":"a1","type":"user","timestamp":"2026-01-01T00:00:03.000Z"}
{"uuid":"a2","parentUuid":"u2","type":"assistant","timestamp":"2026-01-01T00:00:04.000Z"}
{"type":"mode","mode":"normal"}
{"type":"last-prompt","lastPrompt":"two","leafUuid":"a2"}
{"uuid":"u3","parentUuid":"a2","type":"user","timestamp":"2026-01-01T00:00:05.000Z"}
{"type":"mode","mode":"normal"}"#;
    let remote = parse_jsonl(raw.as_bytes()).unwrap();
    let hashes = |es: &[ConversationEntry]| -> Vec<u64> {
        es.iter().map(ConversationEntry::content_hash).collect()
    };
    for cut in 0..=remote.len() {
        let result = smart_merge(&remote[..cut], &remote);
        assert_eq!(hashes(&result.entries), hashes(&remote), "cut at {cut}");
    }
}

#[test]
fn merge_edit_with_equal_timestamps_converges() {
    let a = parse_jsonl(br#"{"uuid":"x","type":"user","timestamp":1,"v":1}"#).unwrap();
    let b = parse_jsonl(br#"{"uuid":"x","type":"user","timestamp":1,"v":2}"#).unwrap();
    let ab = smart_merge(&a, &b);
    let ba = smart_merge(&b, &a);
    assert_eq!(ab.entries[0].content_hash(), ba.entries[0].content_hash());
}

fn session(prefix: &str, turns: usize, start: usize) -> Vec<ConversationEntry> {
    let mut raw = String::default();
    for i in start..start + turns {
        let parent = if i == 0 {
            "null".to_string()
        } else {
            format!("\"{prefix}{}\"", i - 1)
        };
        raw.push_str(&format!(
            "{{\"uuid\":\"{prefix}{i}\",\"parentUuid\":{parent},\"type\":\"user\",\"timestamp\":{i}}}\n\
             {{\"type\":\"mode\",\"mode\":\"normal\"}}\n\
             {{\"type\":\"last-prompt\",\"leafUuid\":\"{prefix}{i}\"}}\n"
        ));
    }
    parse_jsonl(raw.as_bytes()).unwrap()
}

#[test]
fn merge_diverged_sessions_keep_each_branch_together_and_local_last() {
    let base = session("b", 3, 0);
    let mut local = base.clone();
    local.extend(session("l", 2, 3));
    let mut remote = base.clone();
    remote.extend(session("r", 5, 3));

    let result = smart_merge(&local, &remote);
    let order: Vec<&str> = result
        .entries
        .iter()
        .filter_map(|e| e.uuid.as_deref())
        .collect();
    assert_eq!(
        order,
        ["b0", "b1", "b2", "r3", "r4", "r5", "r6", "r7", "l3", "l4"]
    );
    let last = result.entries.last().unwrap();
    assert_eq!(last.extra["leafUuid"], "l4");

    let again = smart_merge(&result.entries, &remote);
    let hashes = |es: &[ConversationEntry]| -> Vec<u64> {
        es.iter().map(ConversationEntry::content_hash).collect()
    };
    assert_eq!(hashes(&again.entries), hashes(&result.entries));
    assert_eq!(
        hashes(&smart_merge(&result.entries, &local).entries),
        hashes(&result.entries)
    );
}

#[test]
fn merge_stale_reordered_local_takes_remote() {
    // An older clync merge left local tree-ordered and behind; remote kept
    // appending in Claude's order. Remote already has everything local has.
    let remote = session("s", 6, 0);
    let mut local: Vec<ConversationEntry> = remote[..9].to_vec();
    local.sort_by_key(|e| e.uuid.is_none());
    let hashes = |es: &[ConversationEntry]| -> Vec<u64> {
        es.iter().map(ConversationEntry::content_hash).collect()
    };
    assert_eq!(
        hashes(&smart_merge(&local, &remote).entries),
        hashes(&remote)
    );
    assert_eq!(
        hashes(&smart_merge(&remote, &local).entries),
        hashes(&remote)
    );
}

#[test]
fn merge_same_rows_different_order_keeps_longer_file() {
    // Claude's original keeps repeated metadata rows; an older clync merge
    // collapsed them and moved metadata to the end.
    let claude = session("s", 4, 0);
    let mut seen: HashSet<u64> = HashSet::default();
    let mut old: Vec<ConversationEntry> = claude
        .iter()
        .filter(|e| seen.insert(e.content_hash()))
        .cloned()
        .collect();
    old.sort_by_key(|e| e.uuid.is_none());
    assert!(old.len() < claude.len());
    let hashes = |es: &[ConversationEntry]| -> Vec<u64> {
        es.iter().map(ConversationEntry::content_hash).collect()
    };
    assert_eq!(hashes(&smart_merge(&old, &claude).entries), hashes(&claude));
    assert_eq!(hashes(&smart_merge(&claude, &old).entries), hashes(&claude));
}

#[test]
fn merge_same_length_reordered_copy_converges_on_time_order() {
    // No repeated metadata, so the reordered copy is the same length.
    let raw = r#"{"uuid":"u1","type":"user","timestamp":1}
{"uuid":"a1","parentUuid":"u1","type":"assistant","timestamp":2}
{"uuid":"t2","parentUuid":"u1","type":"user","timestamp":3}
{"uuid":"t1","parentUuid":"a1","type":"user","timestamp":4}
{"uuid":"a2","parentUuid":"t2","type":"assistant","timestamp":5}"#;
    let claude = parse_jsonl(raw.as_bytes()).unwrap();
    let tree: Vec<ConversationEntry> = [0, 1, 3, 2, 4].iter().map(|&i| claude[i].clone()).collect();
    let hashes = |es: &[ConversationEntry]| -> Vec<u64> {
        es.iter().map(ConversationEntry::content_hash).collect()
    };
    assert_eq!(
        hashes(&smart_merge(&tree, &claude).entries),
        hashes(&claude)
    );
    assert_eq!(
        hashes(&smart_merge(&claude, &tree).entries),
        hashes(&claude)
    );
}

#[test]
fn merge_remote_edit_of_prefix_uuid_keeps_newer_copy_in_place() {
    let shared = parse_jsonl(
        br#"{"uuid":"a","type":"user","timestamp":1,"v":1}
{"uuid":"b","parentUuid":"a","type":"user","timestamp":2}"#,
    )
    .unwrap();
    let mut local = shared.clone();
    local.push(entry_with_parent("c", "b", 4));
    let mut remote = shared;
    remote.extend(parse_jsonl(br#"{"uuid":"a","type":"user","timestamp":3,"v":2}"#).unwrap());
    let result = smart_merge(&local, &remote);
    assert_eq!(uuids(&result.entries), ["a", "b", "c"]);
    assert_eq!(result.entries[0].extra["v"], 2);
    assert_eq!(result.edits_resolved, 1);
}
