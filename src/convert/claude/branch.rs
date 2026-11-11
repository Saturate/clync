//! Which entries of a Claude session make up the conversation Claude Code
//! resumes, in the order it replays them.
//!
//! Claude keeps rewound and edited branches in the file, writes parallel tool
//! calls as separate entries that fork the parent chain, and sometimes appends
//! a result before its call. This mirrors Claude Code 2.1's transcript loader:
//! pick the leaf, walk `parentUuid` back to the root, then pull in the rest of
//! every assistant turn on that path and the results that answer it.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::Value;

use super::super::{branch_path, jsonl};

fn str_of(e: &Value, key: &str) -> Option<String> {
    e.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn type_of(e: &Value) -> &str {
    e.get("type").and_then(|v| v.as_str()).unwrap_or("")
}

fn is_main(e: &Value) -> bool {
    e.get("isSidechain") != Some(&Value::Bool(true)) && e.get("uuid").is_some()
}

fn message_id(e: &Value) -> Option<String> {
    e.get("message").and_then(|m| str_of(m, "id"))
}

fn blocks(e: &Value) -> impl Iterator<Item = &Value> {
    e.get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
}

/// Rows Claude doesn't count as conversation when tracking the last row.
fn is_row(e: &Value) -> bool {
    is_main(e)
        && type_of(e) != "progress"
        && !(type_of(e) == "system" && str_of(e, "subtype").as_deref() == Some("file_snapshot"))
        && !(type_of(e) == "attachment"
            && e.get("attachment")
                .and_then(|a| str_of(a, "type"))
                .as_deref()
                == Some("fork_briefing"))
}

/// How far back Claude bridges a missing parent: the newest earlier entry
/// written at most this long before the orphan; past that the chain stops.
const BRIDGE_WINDOW_MS: u64 = 5000;

/// Indices of the conversation entries to convert, in replay order. Returns
/// every index for sessions written before Claude recorded uuids.
pub fn live_branch(entries: &[Value], path: &Path) -> Vec<usize> {
    let is_message = |e: &Value| matches!(type_of(e), "user" | "assistant");
    if !entries.iter().any(|e| is_main(e) && is_message(e)) {
        return (0..entries.len()).collect();
    }
    let index_of: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.get("uuid").and_then(|v| v.as_str()).map(|u| (u, i)))
        .collect();
    let parent_of = |i: usize| {
        let e = &entries[i];
        if let Some(parent) = str_of(e, "parentUuid") {
            return Some(parent);
        }
        let logical = str_of(e, "logicalParentUuid")?;
        // Some boundaries point at an entry that descends from the boundary
        // itself, a loop that would end the walk there. Continue from the last
        // row before the boundary instead.
        let target = index_of.get(logical.as_str()).copied();
        let loops_back = target.is_some_and(|t| {
            let mut seen = HashSet::new();
            let mut current = Some(t);
            while let Some(j) = current {
                if j == i {
                    return true;
                }
                if !seen.insert(j) {
                    return false;
                }
                current = str_of(&entries[j], "parentUuid")
                    .and_then(|p| index_of.get(p.as_str()).copied());
            }
            false
        });
        if loops_back {
            return (0..i)
                .rev()
                .find(|&j| is_row(&entries[j]))
                .and_then(|j| str_of(&entries[j], "uuid"));
        }
        Some(logical)
    };
    let ts = |i: usize| jsonl::entry_timestamp(&entries[i]);

    let bridge = |i: usize, walked: &HashSet<usize>| {
        let at = ts(i)?;
        (0..entries.len())
            .filter(|&j| !walked.contains(&j) && is_row(&entries[j]))
            .filter_map(|j| ts(j).map(|t| (t, j)))
            .filter(|&(t, _)| t <= at && at - t <= BRIDGE_WINDOW_MS)
            .max()
            .map(|(_, j)| j)
    };
    let walk = |leaf: usize| {
        let (path_idx, gap) = branch_path(Some(leaf), &index_of, parent_of, bridge);
        let completed = complete_turns(entries, &path_idx);
        (completed, path_idx, gap)
    };

    // Leaves newest first rather than the last line: sessions rewritten by
    // clync's own sync merge are in tree order, so the last line can be a
    // parallel sibling from an earlier turn.
    let leaves = newest_leaves(entries, &parent_of, &index_of);
    let (chosen, _, gap) = match pick_leaf(entries, &index_of) {
        Leaf::Empty => return Vec::new(),
        Leaf::Explicit(i) => walk(i),
        Leaf::Latest => match leaves.first() {
            Some(&leaf) => walk(leaf),
            None => return Vec::new(),
        },
        Leaf::Checkpoint {
            checkpoint,
            direct_only,
        } => {
            // The pointer is written mid-turn, so a conversation that went on
            // past it resumes from a later leaf. "Past it" includes the pointer
            // landing on a parallel-result sibling of the turn the leaf
            // continues from, unless the pointer was ever set explicitly.
            let since = ts(checkpoint).unwrap_or(0);
            leaves
                .iter()
                .filter(|&&leaf| ts(leaf).unwrap_or(0) >= since)
                .map(|&leaf| walk(leaf))
                .find(|(completed, path_idx, _)| {
                    if direct_only {
                        path_idx.contains(&checkpoint)
                    } else {
                        let on_branch: HashSet<usize> = completed.iter().copied().collect();
                        covered(checkpoint, &on_branch, entries, &index_of, &parent_of)
                    }
                })
                .unwrap_or_else(|| walk(checkpoint))
        }
    };
    if gap {
        eprintln!(
            "warning: {} references entries that aren't in the file; history across the gap may be incomplete",
            path.display()
        );
    }
    chosen
}

enum Leaf {
    /// The user explicitly rewound to an empty conversation.
    Empty,
    /// No usable checkpoint: resume from the newest leaf.
    Latest,
    /// Set with `/rewind` and not continued since.
    Explicit(usize),
    /// A `last-prompt` checkpoint the conversation may have continued past.
    Checkpoint {
        checkpoint: usize,
        direct_only: bool,
    },
}

/// One forward pass mirroring Claude Code's transcript loader. A `last-prompt`
/// with a `leafUuid` sets the checkpoint; `explicit` makes it binding (and an
/// explicit null means rewound to empty) until any later conversation row
/// clears that; a compaction cancels the checkpoint. A checkpoint that was ever
/// explicit only yields to leaves that descend from it directly.
fn pick_leaf(entries: &[Value], index_of: &HashMap<&str, usize>) -> Leaf {
    let mut checkpoint: Option<String> = None;
    let mut explicit = false;
    let mut ever_explicit = false;
    let mut empty = false;
    for e in entries {
        if type_of(e) == "system" && str_of(e, "subtype").as_deref() == Some("compact_boundary") {
            checkpoint = None;
            explicit = false;
            ever_explicit = false;
        }
        if is_row(e) {
            explicit = false;
            empty = false;
            continue;
        }
        if type_of(e) != "last-prompt" {
            continue;
        }
        let is_explicit = e.get("explicit") == Some(&Value::Bool(true));
        match e.get("leafUuid") {
            Some(Value::Null) if is_explicit => {
                empty = true;
                checkpoint = None;
                explicit = false;
                ever_explicit = false;
            }
            Some(Value::String(leaf)) if !leaf.is_empty() => {
                let same = checkpoint.as_deref() == Some(leaf.as_str());
                explicit = is_explicit || (explicit && same);
                ever_explicit = is_explicit || (ever_explicit && same);
                checkpoint = Some(leaf.clone());
                empty = false;
            }
            _ => {}
        }
    }
    if empty {
        return Leaf::Empty;
    }
    match checkpoint.and_then(|c| index_of.get(c.as_str()).copied()) {
        Some(c) if explicit => Leaf::Explicit(c),
        Some(c) => Leaf::Checkpoint {
            checkpoint: c,
            direct_only: ever_explicit,
        },
        None => Leaf::Latest,
    }
}

/// Main entries nothing else points at, newest timestamp first (later lines
/// break ties).
fn newest_leaves(
    entries: &[Value],
    parent_of: &impl Fn(usize) -> Option<String>,
    index_of: &HashMap<&str, usize>,
) -> Vec<usize> {
    let has_child: HashSet<usize> = (0..entries.len())
        .filter(|&i| is_main(&entries[i]))
        .filter_map(|i| parent_of(i).and_then(|p| index_of.get(p.as_str()).copied()))
        .collect();
    let mut leaves: Vec<usize> = (0..entries.len())
        .filter(|&i| is_row(&entries[i]) && !has_child.contains(&i))
        .collect();
    leaves.sort_unstable_by_key(|&i| {
        std::cmp::Reverse((jsonl::entry_timestamp(&entries[i]).unwrap_or(0), i))
    });
    leaves
}

/// Whether `checkpoint` is part of the branch already chosen: walking up from
/// it reaches that branch before reaching any other message.
fn covered(
    checkpoint: usize,
    branch: &HashSet<usize>,
    entries: &[Value],
    index_of: &HashMap<&str, usize>,
    parent_of: &impl Fn(usize) -> Option<String>,
) -> bool {
    let mut seen = HashSet::new();
    let mut current = Some(checkpoint);
    while let Some(i) = current {
        if branch.contains(&i) {
            return true;
        }
        if matches!(type_of(&entries[i]), "user" | "assistant") || !seen.insert(i) {
            return false;
        }
        current = parent_of(i).and_then(|p| index_of.get(p.as_str()).copied());
    }
    false
}

/// Add what the walk misses and order it. Parallel tool calls are separate
/// entries sharing `message.id`, and their results fork the chain, so a single
/// parent walk only sees one of them. Each extra block goes right after the
/// last path entry of its turn, and each extra result right after the turn
/// holding its call, which keeps tool_use/tool_result adjacency for the API.
fn complete_turns(entries: &[Value], path: &[usize]) -> Vec<usize> {
    let mut chosen: HashSet<usize> = path.iter().copied().collect();
    // Sort key: (anchor position on the path, rank, file index).
    let mut keyed: Vec<((usize, u8, usize), usize)> = path
        .iter()
        .enumerate()
        .map(|(p, &i)| ((p, 0, i), i))
        .collect();

    let mut turn_anchor: HashMap<String, usize> = HashMap::new();
    for (p, &i) in path.iter().enumerate() {
        if type_of(&entries[i]) == "assistant"
            && let Some(id) = message_id(&entries[i])
        {
            turn_anchor.insert(id, p);
        }
    }

    let mut call_anchor: HashMap<String, usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if type_of(e) != "assistant" || !is_main(e) {
            continue;
        }
        let Some(anchor) = message_id(e).and_then(|id| turn_anchor.get(&id).copied()) else {
            continue;
        };
        if chosen.insert(i) {
            keyed.push(((anchor, 1, i), i));
        }
        for block in blocks(e).filter(|b| type_of(b) == "tool_use") {
            if let Some(id) = str_of(block, "id") {
                call_anchor.insert(id, anchor);
            }
        }
    }
    // Calls on path entries that carry no message id.
    for (p, &i) in path.iter().enumerate() {
        for block in blocks(&entries[i]).filter(|b| type_of(b) == "tool_use") {
            if let Some(id) = str_of(block, "id") {
                call_anchor.entry(id).or_insert(p);
            }
        }
    }

    for (i, e) in entries.iter().enumerate() {
        if type_of(e) != "user" || !is_main(e) || chosen.contains(&i) {
            continue;
        }
        let anchor = blocks(e)
            .filter_map(|b| str_of(b, "tool_use_id"))
            .filter_map(|id| call_anchor.get(&id).copied())
            .min();
        if let Some(anchor) = anchor {
            chosen.insert(i);
            keyed.push(((anchor, 2, i), i));
        }
    }

    keyed.sort_unstable_by_key(|(key, _)| *key);
    keyed.into_iter().map(|(_, i)| i).collect()
}
