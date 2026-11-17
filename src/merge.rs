use std::collections::{HashMap, HashSet};

use crate::parser::ConversationEntry;

#[cfg_attr(not(test), allow(dead_code))]
pub struct MergeResult {
    pub entries: Vec<ConversationEntry>,
    pub local_only: usize,
    pub remote_only: usize,
    pub edits_resolved: usize,
}

/// Lower ranks look more like Claude's own file. Older clync merges wrote
/// every uuid-less row after all uuid rows, while Claude interleaves them
/// (`mode`, `last-prompt`, ...) between turns. Those merges also put rows in
/// tree order (timestamps going backwards) and collapsed repeated metadata
/// rows (a shorter file). The hashes settle anything left, so both machines
/// agree.
fn claude_order_rank<'a>(
    entries: &[ConversationEntry],
    hashes: &'a [u64],
) -> (bool, u64, std::cmp::Reverse<usize>, &'a [u64]) {
    let metadata_at_end = entries.iter().any(|e| e.uuid.is_none())
        && entries
            .iter()
            .skip_while(|e| e.uuid.is_some())
            .all(|e| e.uuid.is_none());
    let times: Vec<u64> = entries
        .iter()
        .filter(|e| e.uuid.is_some())
        .map(ConversationEntry::timestamp_millis)
        .collect();
    let backwards: u64 = times.windows(2).map(|w| w[0].saturating_sub(w[1])).sum();
    (
        metadata_at_end,
        backwards,
        std::cmp::Reverse(entries.len()),
        hashes,
    )
}

/// Merges keep file order, because Claude Code resumes from the last row in
/// file order. Both copies are append-only logs of the same session, so they
/// share a common prefix; after that fork point the remote's new rows go
/// first and the local machine's own rows last, keeping its latest turn last.
pub fn smart_merge<'a>(
    local: &'a [ConversationEntry],
    remote: &'a [ConversationEntry],
) -> MergeResult {
    let local_hashes: Vec<u64> = local.iter().map(ConversationEntry::content_hash).collect();
    let remote_hashes: Vec<u64> = remote.iter().map(ConversationEntry::content_hash).collect();
    let fork = local_hashes
        .iter()
        .zip(&remote_hashes)
        .take_while(|(l, r)| l == r)
        .count();

    let local_uuids: HashSet<&str> = local.iter().filter_map(|e| e.uuid.as_deref()).collect();
    let remote_uuids: HashSet<&str> = remote.iter().filter_map(|e| e.uuid.as_deref()).collect();

    // A side whose rows past the fork are all in the other file adds nothing
    // (it is stale, possibly reordered by an older clync merge), so take the
    // other file unchanged. When both qualify they hold the same rows in a
    // different order, and `claude_order_rank` picks the same file on both
    // machines.
    let local_set: HashSet<u64> = local_hashes.iter().copied().collect();
    let remote_set: HashSet<u64> = remote_hashes.iter().copied().collect();
    let adds_nothing = |tail: &[u64], other: &HashSet<u64>| tail.iter().all(|h| other.contains(h));
    let local_stale = adds_nothing(&local_hashes[fork..], &remote_set);
    let remote_stale = adds_nothing(&remote_hashes[fork..], &local_set);
    let keep_local = !local_stale
        || claude_order_rank(local, &local_hashes) <= claude_order_rank(remote, &remote_hashes);
    if remote_stale && keep_local {
        return MergeResult {
            entries: local.to_vec(),
            local_only: local_uuids.difference(&remote_uuids).count(),
            remote_only: 0,
            edits_resolved: 0,
        };
    }
    if local_stale {
        return MergeResult {
            entries: remote.to_vec(),
            local_only: 0,
            remote_only: remote_uuids.difference(&local_uuids).count(),
            edits_resolved: 0,
        };
    }

    let remote_by_uuid: HashMap<&str, (&ConversationEntry, u64)> = remote
        .iter()
        .zip(&remote_hashes)
        .filter_map(|(e, h)| e.uuid.as_deref().map(|u| (u, (e, *h))))
        .collect();
    // Claude repeats identical metadata rows (`mode`, `permission-mode`, ...)
    // after every turn; a copy already in the local tail adds nothing.
    let local_tail_hashes: HashSet<u64> = local_hashes[fork..].iter().copied().collect();

    let mut local_only = 0usize;
    let mut edited: HashSet<&str> = HashSet::default();
    // A uuid in the shared prefix can still have an edited copy later in the
    // remote file, so prefix rows go through the same check as the local tail.
    let mut resolve = |entry: &'a ConversationEntry, hash: u64| -> &'a ConversationEntry {
        let Some(uuid) = entry.uuid.as_deref() else {
            return entry;
        };
        let Some(&(remote_entry, remote_hash)) = remote_by_uuid.get(uuid) else {
            local_only += 1;
            return entry;
        };
        if hash == remote_hash {
            return entry;
        }
        edited.insert(uuid);
        // Content hash breaks timestamp ties so both machines pick the same copy.
        let remote_newer =
            (remote_entry.timestamp_millis(), remote_hash) > (entry.timestamp_millis(), hash);
        if remote_newer { remote_entry } else { entry }
    };

    let mut entries: Vec<ConversationEntry> = Vec::with_capacity(local.len().max(remote.len()));
    for (entry, hash) in local[..fork].iter().zip(&local_hashes[..fork]) {
        entries.push(resolve(entry, *hash).clone());
    }

    let mut remote_only: HashSet<&str> = HashSet::default();
    for (entry, hash) in remote[fork..].iter().zip(&remote_hashes[fork..]) {
        let keep = match entry.uuid.as_deref() {
            Some(uuid) => !local_uuids.contains(uuid) && remote_only.insert(uuid),
            None => !local_tail_hashes.contains(hash),
        };
        if keep {
            entries.push(entry.clone());
        }
    }

    for (entry, hash) in local[fork..].iter().zip(&local_hashes[fork..]) {
        entries.push(resolve(entry, *hash).clone());
    }

    MergeResult {
        entries,
        local_only,
        remote_only: remote_only.len(),
        edits_resolved: edited.len(),
    }
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
