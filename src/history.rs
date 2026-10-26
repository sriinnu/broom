//! What grew since the last scan. After every finished scan Broom writes a
//! small record of folder sizes for that root (the folders worth a megabyte,
//! a few levels down) and compares the new tree against an earlier record.
//!
//! One JSON file per scanned root in `<state dir>/history/`, holding the most
//! recent record plus one per day for the last week. The comparison uses the
//! newest record that's at least an hour old, so a quick rescan doesn't wipe
//! out "since Tuesday"; with nothing that old it falls back to the newest.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::scan::{Kind, Tree};

/// How deep the record goes, and the smallest folder it keeps. Together they
/// keep a whole-disk record to a few tens of thousands of lines.
const DEPTH: usize = 5;
const MIN_SIZE: u64 = 1 << 20;
const DAYS_KEPT: u64 = 7;
const MIN_AGE: Duration = Duration::from_secs(3600);

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    /// Seconds since 1970.
    taken: u64,
    /// Folder path relative to the root ("" for the root itself) → bytes.
    sizes: HashMap<String, u64>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    root: String,
    records: Vec<Record>,
}

/// How the tree compares with an earlier scan of the same root.
pub struct Growth {
    /// When that earlier scan finished.
    pub since: SystemTime,
    /// Node → bytes gained (negative: shrank). Only folders the earlier record
    /// knew about; something new has nothing to compare with.
    pub by_node: HashMap<usize, i64>,
}

/// Compare `tree` with the right earlier record, then add a record of it.
/// File errors just mean no comparison; this never fails a scan.
pub fn compare_and_record(tree: &Tree) -> Option<Growth> {
    let path = file_for(&tree.root_path)?;
    compare_and_record_at(tree, &path, SystemTime::now())
}

fn compare_and_record_at(tree: &Tree, path: &Path, now: SystemTime) -> Option<Growth> {
    let now_secs = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let mut file: File = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let root = tree.root_path.to_string_lossy().into_owned();
    if file.root != root {
        file = File { root, records: Vec::new() };
    }
    let growth = baseline(&file.records, now_secs).map(|old| Growth {
        since: UNIX_EPOCH + Duration::from_secs(old.taken),
        by_node: compare(tree, &old.sizes),
    });

    file.records.push(Record { taken: now_secs, sizes: record(tree) });
    prune(&mut file.records, now_secs);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec(&file) {
        // Whole and atomic, like settings: a half-written record is worse than none.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
    growth
}

/// The newest record at least `MIN_AGE` old, else the newest of all.
fn baseline(records: &[Record], now: u64) -> Option<&Record> {
    let old_enough = records.iter().filter(|r| now.saturating_sub(r.taken) >= MIN_AGE.as_secs()).max_by_key(|r| r.taken);
    old_enough.or_else(|| records.iter().max_by_key(|r| r.taken))
}

/// Keep the newest record and the newest one of each day in the last week.
fn prune(records: &mut Vec<Record>, now: u64) {
    records.sort_by_key(|r| std::cmp::Reverse(r.taken));
    let mut days_seen = Vec::new();
    let mut first = true;
    records.retain(|r| {
        let day = r.taken / 86_400;
        let keep = first || (now.saturating_sub(r.taken) <= DAYS_KEPT * 86_400 && !days_seen.contains(&day));
        first = false;
        if keep {
            days_seen.push(day);
        }
        keep
    });
}

/// The folders worth recording, with their paths relative to the root.
fn walk(tree: &Tree, mut visit: impl FnMut(usize, &str)) {
    let mut stack: Vec<(usize, String, usize)> = vec![(Tree::ROOT, String::new(), 0)];
    while let Some((ix, rel, depth)) = stack.pop() {
        visit(ix, &rel);
        if depth == DEPTH {
            continue;
        }
        for &c in &tree.nodes[ix].children {
            let child = &tree.nodes[c];
            if child.kind == Kind::Dir && child.size >= MIN_SIZE {
                let rel = if rel.is_empty() { child.name.to_string() } else { format!("{rel}/{}", child.name) };
                stack.push((c, rel, depth + 1));
            }
        }
    }
}

fn record(tree: &Tree) -> HashMap<String, u64> {
    let mut sizes = HashMap::new();
    walk(tree, |ix, rel| {
        sizes.insert(rel.to_string(), tree.nodes[ix].size);
    });
    sizes
}

fn compare(tree: &Tree, old: &HashMap<String, u64>) -> HashMap<usize, i64> {
    let mut by_node = HashMap::new();
    walk(tree, |ix, rel| {
        if let Some(&before) = old.get(rel) {
            by_node.insert(ix, tree.nodes[ix].size as i64 - before as i64);
        }
    });
    by_node
}

/// `<state dir>/history/<hash of the root>.json`. FNV-1a, written out here so
/// the name doesn't change with the Rust version the way `DefaultHasher` may.
fn file_for(root: &Path) -> Option<PathBuf> {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in root.to_string_lossy().bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
    }
    Some(crate::settings::state_dir()?.join("history").join(format!("{hash:016x}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Node, NameIndex};

    fn tree(root: &str, app_size: u64) -> Tree {
        let mb = MIN_SIZE;
        let node = |name: &str, size: u64, parent: Option<usize>, children: Vec<usize>| Node {
            name: name.to_string().into(),
            size,
            kind: Kind::Dir,
            parent,
            children,
            items: 1,
        };
        Tree {
            root_path: PathBuf::from(root),
            nodes: vec![
                node("root", 10 * mb + app_size, None, vec![1, 2]),
                node("docs", 10 * mb, Some(0), vec![]),
                node("app", app_size, Some(0), vec![]),
            ],
            errors: 0,
            cloud_only: 0,
            index: std::sync::Arc::new(std::sync::OnceLock::<NameIndex>::new()),
        }
    }

    #[test]
    fn a_second_scan_sees_what_grew() {
        let dir = std::env::temp_dir().join(format!("broom-history-{}", std::process::id()));
        let path = dir.join("h.json");
        let t0 = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert!(compare_and_record_at(&tree("/x", 2 * MIN_SIZE), &path, t0).is_none());
        let later = tree("/x", 5 * MIN_SIZE);
        let growth = compare_and_record_at(&later, &path, t0 + Duration::from_secs(7200)).unwrap();
        assert_eq!(growth.since, t0);
        assert_eq!(growth.by_node[&2], 3 * MIN_SIZE as i64);
        assert_eq!(growth.by_node[&1], 0);
        assert_eq!(growth.by_node[&Tree::ROOT], 3 * MIN_SIZE as i64);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_quick_rescan_keeps_the_older_baseline() {
        let now = 1_800_000_000;
        let records = vec![
            Record { taken: now - 3 * 86_400, sizes: HashMap::new() },
            Record { taken: now - 60, sizes: HashMap::new() },
        ];
        assert_eq!(baseline(&records, now).unwrap().taken, now - 3 * 86_400);
    }

    #[test]
    fn pruning_keeps_one_a_day_for_a_week() {
        let now = 1_800_000_000;
        let mut records: Vec<Record> =
            (0..40).map(|i| Record { taken: now - i * 6 * 3600, sizes: HashMap::new() }).collect();
        prune(&mut records, now);
        assert!(records.len() <= DAYS_KEPT as usize + 2, "{}", records.len());
        assert_eq!(records[0].taken, now);
    }
}
