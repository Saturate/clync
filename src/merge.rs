use std::collections::{HashMap, HashSet};

use crate::parser::ConversationEntry;

#[cfg_attr(not(test), allow(dead_code))]
pub struct MergeResult {
    pub entries: Vec<ConversationEntry>,
    pub local_only: usize,
    pub remote_only: usize,
    pub edits_resolved: usize,
}

pub fn smart_merge(local: &[ConversationEntry], remote: &[ConversationEntry]) -> MergeResult {
    let mut uuid_entries: HashMap<String, &ConversationEntry> = HashMap::new();
    let mut non_uuid_local: Vec<&ConversationEntry> = Vec::new();
    let mut non_uuid_remote: Vec<&ConversationEntry> = Vec::new();

    let mut local_only = 0usize;
    let mut remote_only = 0usize;
    let mut edits_resolved = 0usize;

    let local_uuids: HashMap<String, &ConversationEntry> = local
        .iter()
        .filter_map(|e| e.uuid.as_ref().map(|u| (u.clone(), e)))
        .collect();

    let remote_uuids: HashMap<String, &ConversationEntry> = remote
        .iter()
        .filter_map(|e| e.uuid.as_ref().map(|u| (u.clone(), e)))
        .collect();

    for (uuid, local_entry) in &local_uuids {
        if let Some(remote_entry) = remote_uuids.get(uuid) {
            let local_hash = local_entry.content_hash();
            let remote_hash = remote_entry.content_hash();
            if local_hash == remote_hash {
                uuid_entries.insert(uuid.clone(), local_entry);
            } else {
                edits_resolved += 1;
                if remote_entry.timestamp_millis() > local_entry.timestamp_millis() {
                    uuid_entries.insert(uuid.clone(), remote_entry);
                } else {
                    uuid_entries.insert(uuid.clone(), local_entry);
                }
            }
        } else {
            local_only += 1;
            uuid_entries.insert(uuid.clone(), local_entry);
        }
    }

    for (uuid, remote_entry) in &remote_uuids {
        if !local_uuids.contains_key(uuid) {
            remote_only += 1;
            uuid_entries.insert(uuid.clone(), remote_entry);
        }
    }

    for entry in local {
        if entry.uuid.is_none() {
            non_uuid_local.push(entry);
        }
    }
    for entry in remote {
        if entry.uuid.is_none() {
            non_uuid_remote.push(entry);
        }
    }

    let mut seen_hashes: HashSet<u64> = HashSet::new();
    let mut non_uuid_merged: Vec<&ConversationEntry> = Vec::new();
    for entry in non_uuid_local.iter().chain(non_uuid_remote.iter()) {
        let hash = entry.content_hash();
        if seen_hashes.insert(hash) {
            non_uuid_merged.push(entry);
        }
    }

    let mut result: Vec<ConversationEntry> = build_ordered_tree(&uuid_entries);
    let mut non_uuid_cloned: Vec<ConversationEntry> =
        non_uuid_merged.into_iter().cloned().collect();
    non_uuid_cloned.sort_by_key(|e| e.timestamp_millis());
    result.extend(non_uuid_cloned);

    MergeResult {
        entries: result,
        local_only,
        remote_only,
        edits_resolved,
    }
}

fn build_ordered_tree(entries: &HashMap<String, &ConversationEntry>) -> Vec<ConversationEntry> {
    let mut children: HashMap<Option<&str>, Vec<&str>> = HashMap::new();
    for (uuid, entry) in entries {
        children
            .entry(entry.parent_uuid.as_deref())
            .or_default()
            .push(uuid);
    }

    for siblings in children.values_mut() {
        siblings.sort_by_key(|uuid| {
            entries
                .get(*uuid)
                .map(|e| e.timestamp_millis())
                .unwrap_or(0)
        });
    }

    let mut result = Vec::with_capacity(entries.len());
    let mut stack: Vec<&str> = Vec::new();

    if let Some(roots) = children.get(&None) {
        for root in roots.iter().rev() {
            stack.push(root);
        }
    }

    let all_parent_uuids: HashSet<&str> = entries
        .values()
        .filter_map(|e| e.parent_uuid.as_deref())
        .collect();
    for uuid in entries.keys() {
        if !all_parent_uuids.contains(uuid.as_str())
            && entries
                .get(uuid)
                .and_then(|e| e.parent_uuid.as_ref())
                .is_some_and(|p| !entries.contains_key(p))
        {
            stack.push(uuid);
        }
    }

    let mut visited: HashSet<&str> = HashSet::new();
    while let Some(uuid) = stack.pop() {
        if !visited.insert(uuid) {
            continue;
        }
        if let Some(entry) = entries.get(uuid) {
            result.push((*entry).clone());
        }
        if let Some(kids) = children.get(&Some(uuid)) {
            for kid in kids.iter().rev() {
                stack.push(kid);
            }
        }
    }

    for (uuid, entry) in entries {
        if !visited.contains(uuid.as_str()) {
            result.push((*entry).clone());
        }
    }

    result
}

#[cfg(test)]
mod tests {
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
}
