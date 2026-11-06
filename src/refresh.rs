//! A current tree for a root, as cheaply as the disk allows.
//!
//! A full walk of a home folder is bound by APFS metadata reads, not by
//! threads: half a minute whatever the machine. Most of that disk didn't change
//! since the last look, though. So every finished scan is kept (`cache`) with
//! the FSEvents id from just before it started, and the next refresh replays
//! the event history since then (`fsevents`) and re-reads only the folders it
//! names (`scan::catchup`). Anything that makes the history untrustworthy falls
//! back to the full walk; correctness comes first, speed second.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::cache::{self, Meta};
use crate::fsevents;
use crate::scan::{self, CatchUp, Progress, Tree};

/// Past this many changed folders the replay is no shortcut.
const MAX_CHANGES: usize = 200_000;
/// How long the FSEvents replay may take before it's given up on.
const REPLAY_TIMEOUT: Duration = Duration::from_secs(20);

/// How a tree came to be current.
#[derive(Clone, Copy, Debug)]
pub enum How {
    /// Read from scratch.
    Full,
    /// The cached tree, caught up with this many changed folders.
    CaughtUp { changes: usize, work: CatchUp },
}

pub struct Fresh {
    pub tree: Tree,
    pub meta: Meta,
    pub how: How,
    pub elapsed: Duration,
}

/// A full scan that remembers where the FSEvents history stood when it began,
/// so the next refresh can catch up from there.
pub fn full_scan(requested_root: &Path, progress: &Progress, excludes: &[PathBuf], outline_depth: usize) -> (Tree, Meta) {
    // Before reading anything: a change made while the walk runs is then either
    // in the tree already or in the history after this id, never lost.
    let event_id = fsevents::current_event_id();
    let tree = scan::scan(requested_root, progress, excludes, outline_depth);
    let meta = meta_for(requested_root, &tree, event_id, excludes);
    (tree, meta)
}

fn meta_for(requested_root: &Path, tree: &Tree, event_id: u64, excludes: &[PathBuf]) -> Meta {
    use std::os::unix::fs::MetadataExt;
    let (root_dev, root_ino) = std::fs::symlink_metadata(&tree.root_path).map(|m| (m.dev(), m.ino())).unwrap_or((0, 0));
    let volume_uuid = fsevents::volume_uuid(root_dev).unwrap_or_default();
    Meta {
        requested_root: requested_root.to_path_buf(),
        taken: SystemTime::now(),
        // No history on this volume: nothing to catch up from.
        event_id: if volume_uuid.is_empty() { 0 } else { event_id },
        volume_uuid,
        root_dev,
        root_ino,
        excludes: cache::excludes_key(excludes),
    }
}

/// Bring a cached tree up to date from the FSEvents history, or say why it
/// can't be and a full scan is needed.
pub fn catch_up(meta: &Meta, mut tree: Tree, excludes: &[PathBuf]) -> Result<Fresh, &'static str> {
    use std::os::unix::fs::MetadataExt;
    let start = Instant::now();
    if meta.excludes != cache::excludes_key(excludes) {
        return Err("the exclusions changed");
    }
    if meta.event_id == 0 || meta.volume_uuid.is_empty() {
        return Err("this volume keeps no event history");
    }
    let root = std::fs::symlink_metadata(&tree.root_path).map_err(|_| "the root is gone")?;
    if (root.dev(), root.ino()) != (meta.root_dev, meta.root_ino) {
        return Err("the root is a different folder now");
    }
    if fsevents::volume_uuid(root.dev()).as_deref() != Some(meta.volume_uuid.as_str()) {
        return Err("the volume's event history was reset");
    }
    let now_id = fsevents::current_event_id();
    let changes = fsevents::changes_since(&tree.root_path, meta.event_id, REPLAY_TIMEOUT).map_err(fsevents::Lost::reason)?;
    if changes.len() > MAX_CHANGES {
        return Err("too much changed");
    }
    // Whole folders FSEvents lost track in get walked again; if that is most of
    // the tree, the full walk is as quick and simpler.
    let rewalk: u64 = changes.iter().filter(|c| c.recursive).filter_map(|c| tree.find(&c.path)).map(|ix| tree.nodes[ix].items).sum();
    if rewalk > tree.nodes[Tree::ROOT].items / 2 {
        return Err("most of the tree changed");
    }
    let work = tree.catch_up(&changes, excludes);
    if meta.requested_root == Path::new("/") && !scan::remeasure_volume_slices(&mut tree) {
        return Err("the startup disk's volumes couldn't be measured");
    }
    // From the id noted before the replay: anything after it comes up next time.
    let meta = meta_for(&meta.requested_root, &tree, now_id, excludes);
    Ok(Fresh { tree, meta, how: How::CaughtUp { changes: changes.len(), work }, elapsed: start.elapsed() })
}

/// The current tree for `requested_root`: the cached one caught up if it can be,
/// else a full scan.
pub fn refresh(requested_root: &Path, excludes: &[PathBuf], outline_depth: usize, cancelled: &Progress) -> Fresh {
    let start = Instant::now();
    if let Some((meta, tree)) = cache::load(requested_root) {
        match catch_up(&meta, tree, excludes) {
            Ok(mut fresh) => {
                fresh.elapsed = start.elapsed();
                return fresh;
            }
            Err(why) => log(&format!("full scan of {}: {why}", requested_root.display())),
        }
    }
    let (tree, meta) = full_scan(requested_root, cancelled, excludes, outline_depth);
    Fresh { tree, meta, how: How::Full, elapsed: start.elapsed() }
}

/// Keep `tree` for next time, if that pays: a catch-up always replaces the file
/// it came from, a full scan only when it was slow enough to be worth skipping.
/// A quick one drops any older file instead, so it can't be shown later.
pub fn store(meta: &Meta, tree: &Tree, how: How, elapsed: Duration) {
    if matches!(how, How::Full) && elapsed < cache::WORTH_CACHING {
        cache::forget(&meta.requested_root);
        return;
    }
    let start = Instant::now();
    match cache::save(meta, tree) {
        Ok(bytes) => log(&format!("cached {} nodes, {} in {:.2}s", tree.nodes.len(), scan::format_size(bytes), start.elapsed().as_secs_f64())),
        Err(error) => log(&format!("couldn't cache the tree: {error}")),
    }
}

/// One line on stderr with `BROOM_TIMING` (or `BROOM_PHASES`) set.
pub fn log(line: &str) {
    if std::env::var_os("BROOM_TIMING").is_some() || std::env::var_os("BROOM_PHASES").is_some() {
        eprintln!("refresh: {line}");
    }
}

/// `broom --bench-catchup <path> [runs]`: a full scan, then the cache written
/// and read back, then `runs` catch-ups from it, each timed, so the three can
/// be compared on a real disk. The state goes to a temp folder unless
/// `BROOM_STATE_DIR` says otherwise, so the app's own cache isn't touched.
pub fn bench(root: &Path, runs: usize) {
    if std::env::var_os("BROOM_STATE_DIR").is_none() {
        let dir = std::env::temp_dir().join(format!("broom-bench-{}", std::process::id()));
        // SAFETY: single-threaded here; nothing else reads the environment yet.
        unsafe { std::env::set_var("BROOM_STATE_DIR", &dir) };
    }
    let fingerprint = |tree: &Tree| {
        let root = &tree.nodes[Tree::ROOT];
        let attached = tree.nodes.iter().enumerate().filter(|(ix, n)| n.parent.is_some() || *ix == Tree::ROOT).count();
        format!("size={} files={} nodes={attached}", root.size, root.items)
    };

    let start = Instant::now();
    let (tree, meta) = full_scan(root, &Progress::default(), &[], 2);
    eprintln!("full scan     {:>8.3}s  {}", start.elapsed().as_secs_f64(), fingerprint(&tree));

    let path = cache::file_for(root).expect("state dir");
    let start = Instant::now();
    let bytes = cache::save_at(&path, &meta, &tree).expect("write cache");
    eprintln!("cache write   {:>8.3}s  {} ({:.1} bytes/node)", start.elapsed().as_secs_f64(), scan::format_size(bytes), bytes as f64 / tree.nodes.len() as f64);
    let start = Instant::now();
    let (_, loaded) = cache::load(root).expect("read cache");
    eprintln!("cache load    {:>8.3}s  {}", start.elapsed().as_secs_f64(), fingerprint(&loaded));
    drop(loaded);
    drop(tree);

    for run in 0..runs {
        let start = Instant::now();
        let (meta, tree) = cache::load(root).expect("read cache");
        let loaded = start.elapsed();
        match catch_up(&meta, tree, &[]) {
            Ok(fresh) => {
                let How::CaughtUp { changes, work } = fresh.how else { unreachable!() };
                eprintln!(
                    "catch-up {run}    {:>8.3}s  (load {:.3}s + replay & re-read {:.3}s)  {changes} changed folders, {} re-listed, {} walked  {}",
                    start.elapsed().as_secs_f64(),
                    loaded.as_secs_f64(),
                    fresh.elapsed.as_secs_f64(),
                    work.relisted,
                    work.walked,
                    fingerprint(&fresh.tree)
                );
                cache::save_at(&path, &fresh.meta, &fresh.tree).expect("write cache");
            }
            Err(why) => eprintln!("catch-up {run}: fell back ({why})"),
        }
    }
    let start = Instant::now();
    let (tree, _) = full_scan(root, &Progress::default(), &[], 2);
    eprintln!("full again    {:>8.3}s  {}", start.elapsed().as_secs_f64(), fingerprint(&tree));
}
