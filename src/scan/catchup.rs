//! Bringing an older tree up to date from a list of folders that changed,
//! instead of reading the whole disk again.
//!
//! FSEvents (see `fsevents`) names the folders whose *own* listing changed
//! since some moment: a file created, deleted, renamed or written to inside
//! it. For those a shallow re-read is enough. The folder's entries are listed
//! again, files take their new sizes, folders that are still there keep the
//! subtree already in the tree, new folders are walked in full, and whatever is
//! gone is detached. Only when FSEvents says it lost track below a folder
//! (`MustScanSubDirs`) is that folder walked again from scratch, and that goes
//! through `Tree::graft`.
//!
//! The rules are the walk's own (`Walker::new`): the same devices, the same
//! skipped and excluded folders, the same allocated sizes. A tree caught up
//! this way equals a fresh full scan of the same disk; the tests in `scan`
//! check exactly that.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{Kind, LIVE_DEPTH, Node, Progress, Tree, Walker, dirlist, disk_size, display_name, flatten, sort_children};

/// A folder whose contents changed since the tree was taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    /// Something below it changed too and FSEvents can't say what: read it all.
    pub recursive: bool,
}

/// What a catch-up had to do, for the bench line and the logs.
#[derive(Clone, Copy, Debug, Default)]
pub struct CatchUp {
    /// Folders listed again, one level.
    pub relisted: usize,
    /// Folders walked in full: new ones, and the ones FSEvents lost track in.
    pub walked: usize,
}

/// Deeper than the live chart goes, so walking a subtree never touches it.
const NOT_LIVE: usize = LIVE_DEPTH + 1;

impl Tree {
    /// Apply `changes` to this tree, reading only those folders (and anything new
    /// inside them). `excludes` must be what the tree was scanned with. Changes
    /// outside the root or inside an excluded folder are ignored.
    pub fn catch_up(&mut self, changes: &[Change], excludes: &[PathBuf]) -> CatchUp {
        let progress = Progress::default();
        let walker = Walker::new(&self.root_path, &progress, excludes);
        dirlist::raise_fd_limit();
        dirlist::disable_cloud_downloads();
        let mut stats = CatchUp::default();

        let wanted: Vec<&Change> = changes.iter().filter(|c| c.path.starts_with(&self.root_path) && !skipped(&walker, &self.root_path, &c.path)).collect();
        let mut recursive: Vec<PathBuf> = wanted.iter().filter(|c| c.recursive).map(|c| c.path.clone()).collect();
        recursive.sort();
        recursive.dedup();
        // Sorted, so an outer folder comes before anything inside it.
        let mut outer: Vec<PathBuf> = Vec::new();
        for path in recursive {
            if !outer.last().is_some_and(|o| path.starts_with(o)) {
                outer.push(path);
            }
        }
        let mut shallow: Vec<PathBuf> =
            wanted.iter().filter(|c| !c.recursive && !outer.iter().any(|o| c.path.starts_with(o))).map(|c| c.path.clone()).collect();

        for path in outer {
            match self.find(&path).filter(|&ix| self.nodes[ix].kind == Kind::Dir && is_real_dir(&path)) {
                Some(ix) => {
                    let own = if ix == Tree::ROOT { root_own(&path) } else { dirlist::dir_alloc(&path).unwrap_or(0) };
                    let known = |file: &Path| self.find(file).filter(|&f| self.nodes[f].kind == Kind::File).map(|f| self.nodes[f].size);
                    let mut rewalker = Walker::new(&self.root_path, &progress, excludes);
                    rewalker.prior = Some(&known);
                    let raw = rewalker.walk_dir(None, &path, display_name(&path), own, &progress.live, NOT_LIVE);
                    drop(rewalker);
                    let mut nodes = Vec::new();
                    flatten(raw, None, &mut nodes);
                    let fresh = Tree { root_path: path.clone(), nodes, errors: 0, cloud_only: 0, index: Default::default() };
                    self.graft(ix, &fresh);
                    stats.walked += 1;
                }
                // New, or gone, or no longer a folder: that is a change to the
                // listing of the nearest folder above it that the tree has.
                None => shallow.push(path),
            }
        }

        // Each changed path, lifted to the nearest folder that both the tree and the
        // disk still have, deepest first: a parent re-read after its children sees
        // their settled sizes.
        let mut targets: Vec<usize> = shallow.iter().filter_map(|path| self.nearest_dir(path)).collect();
        targets.sort_unstable();
        targets.dedup();
        let mut by_depth: Vec<(usize, usize)> = targets.into_iter().map(|ix| (self.depth(ix), ix)).collect();
        by_depth.sort_unstable_by(|a, b| b.cmp(a));
        for (_, ix) in by_depth {
            // An earlier re-read may have replaced this folder (it became a file and
            // back, say); then that re-read covered it.
            if ix != Tree::ROOT && self.nodes[ix].parent.is_none() {
                continue;
            }
            stats.walked += self.relist(ix, &walker, &progress);
            stats.relisted += 1;
        }

        self.errors += progress.errors.load(Ordering::Relaxed);
        self.cloud_only += progress.cloud_only.load(Ordering::Relaxed);
        self.index = Arc::new(std::sync::OnceLock::new());
        stats
    }

    /// The deepest folder on `path`'s way down from the root that is in the tree
    /// and still a folder on disk.
    fn nearest_dir(&self, path: &Path) -> Option<usize> {
        let mut at = path;
        loop {
            if let Some(ix) = self.find(at) {
                if self.nodes[ix].kind == Kind::Dir && is_real_dir(at) {
                    return Some(ix);
                }
            }
            if at == self.root_path {
                return None;
            }
            at = at.parent()?;
        }
    }

    fn depth(&self, mut ix: usize) -> usize {
        let mut depth = 0;
        while let Some(parent) = self.nodes[ix].parent {
            depth += 1;
            ix = parent;
        }
        depth
    }

    /// Detach `ix` and everything below it, without touching any sizes: the
    /// caller is rebuilding the parent's totals anyway.
    fn detach(&mut self, ix: usize) {
        let mut stack = vec![ix];
        while let Some(n) = stack.pop() {
            self.nodes[n].parent = None;
            stack.extend(self.nodes[n].children.iter().copied());
        }
    }

    /// List folder `ix` again, one level, and rebuild its children from what is
    /// there now. Returns how many new folders had to be walked in full.
    fn relist(&mut self, ix: usize, walker: &Walker, progress: &Progress) -> usize {
        let path = self.path_of(ix);
        let listing = dirlist::Dir::open(&path).and_then(|dir| dir.list(&path, false));
        let Ok(listing) = listing else {
            // Unreadable now (permissions changed): keep what we knew rather than
            // pretend it emptied.
            self.errors += 1;
            return 0;
        };
        self.errors += listing.errors;

        let (old_size, old_items) = (self.nodes[ix].size, self.nodes[ix].items);
        let old_children = std::mem::take(&mut self.nodes[ix].children);
        let old_sum: u64 = old_children.iter().map(|&c| self.nodes[c].size).sum();
        // A folder's own allocation isn't stored; it's what its total holds beyond
        // its children.
        let old_own = old_size.saturating_sub(old_sum);
        let own = if ix == Tree::ROOT { root_own(&path) } else { dirlist::dir_alloc(&path).unwrap_or(old_own) };
        let mut by_name: HashMap<String, usize> = HashMap::with_capacity(old_children.len());
        let mut children = Vec::with_capacity(listing.entries.len());
        for c in old_children {
            // Volume slices aren't on disk; the caller owns them.
            if self.nodes[c].kind == Kind::Other {
                children.push(c);
            } else {
                by_name.insert(self.nodes[c].name.to_string(), c);
            }
        }

        let mut walked = 0;
        for entry in listing.entries {
            let old = by_name.remove(&entry.name);
            if !entry.is_dir {
                let size = match old {
                    // A hard-linked file was counted once, wherever the full walk met
                    // it first; keep that decision rather than count it twice.
                    Some(o) if entry.nlink > 1 && self.nodes[o].kind == Kind::File => self.nodes[o].size,
                    _ => entry.size,
                };
                match old.filter(|&o| self.nodes[o].kind == Kind::File) {
                    Some(o) => {
                        self.nodes[o].size = size;
                        children.push(o);
                    }
                    None => {
                        if let Some(o) = old {
                            self.detach(o);
                        }
                        children.push(self.push(Node { name: entry.name.into(), size, kind: Kind::File, parent: Some(ix), children: Vec::new(), items: 1 }));
                    }
                }
                continue;
            }
            let child_path = path.join(&entry.name);
            if walker.skip.contains(&child_path) || !walker.allowed_devices.contains(&entry.dev) {
                if let Some(o) = old {
                    self.detach(o);
                }
                continue;
            }
            if entry.dataless {
                // Its contents live in the cloud; as in the walk, only its own size counts.
                if let Some(o) = old {
                    self.detach(o);
                }
                progress.cloud_only.fetch_add(1, Ordering::Relaxed);
                children.push(self.push(Node { name: entry.name.into(), size: entry.size, kind: Kind::Dir, parent: Some(ix), children: Vec::new(), items: 0 }));
                continue;
            }
            match old.filter(|&o| self.nodes[o].kind == Kind::Dir) {
                Some(o) => {
                    // Still there: its subtree stands (FSEvents names it separately if
                    // its own contents changed). Only its own allocation is re-read.
                    let node = &self.nodes[o];
                    let sum: u64 = node.children.iter().map(|&c| self.nodes[c].size).sum();
                    let kept_own = node.size.saturating_sub(sum);
                    self.nodes[o].size = node_size_with_own(self.nodes[o].size, kept_own, entry.size);
                    children.push(o);
                }
                None => {
                    if let Some(o) = old {
                        self.detach(o);
                    }
                    let raw = walker.walk_dir(None, &child_path, entry.name, entry.size, &progress.live, NOT_LIVE);
                    children.push(flatten(raw, Some(ix), &mut self.nodes));
                    walked += 1;
                }
            }
        }
        // Whatever is left wasn't listed: deleted or renamed away.
        for (_, gone) in by_name {
            self.detach(gone);
        }

        let size = own + children.iter().map(|&c| self.nodes[c].size).sum::<u64>();
        let items = children.iter().map(|&c| self.nodes[c].items).sum();
        self.nodes[ix].children = children;
        self.nodes[ix].size = size;
        self.nodes[ix].items = items;
        sort_children(&mut self.nodes, ix);
        // The ancestors move by the difference, and may change rank among their siblings.
        let mut cur = self.nodes[ix].parent;
        while let Some(p) = cur {
            let node = &mut self.nodes[p];
            node.size = (node.size + size).saturating_sub(old_size);
            node.items = (node.items + items).saturating_sub(old_items);
            cur = node.parent;
        }
        let mut cur = self.nodes[ix].parent;
        while let Some(p) = cur {
            sort_children(&mut self.nodes, p);
            cur = self.nodes[p].parent;
        }
        walked
    }

    fn push(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }
}

fn node_size_with_own(size: u64, old_own: u64, new_own: u64) -> u64 {
    (size + new_own).saturating_sub(old_own)
}

/// The root's own allocation, measured the way the full scan does.
fn root_own(path: &Path) -> u64 {
    fs::symlink_metadata(path).as_ref().map(disk_size).unwrap_or(0)
}

/// A folder on disk, not a symlink to one.
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// Inside something the walk leaves out (an exclusion, /Volumes…), below the root.
fn skipped(walker: &Walker, root: &Path, path: &Path) -> bool {
    path.ancestors().take_while(|a| *a != root).any(|a| walker.skip.contains(a))
}
