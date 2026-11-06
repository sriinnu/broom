//! A finished scan: focus, history, list order, picking and the cleanup plan.

use super::*;

/// Which way a history step moves.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Travel {
    Back,
    Forward,
}

pub(super) struct Results {
    /// Shared with background searches, so they can read the whole tree without a copy;
    /// the rare mutation (after a trash) goes through `Arc::make_mut`.
    pub(super) tree: Arc<Tree>,
    pub(super) focus: usize,
    pub(super) segments: Rc<Vec<Segment>>,
    pub(super) swatches: HashMap<usize, Hsla>,
    pub(super) chart_hover: Option<Hit>,
    pub(super) list_hover: Option<usize>,
    pub(super) anim_start: Instant,
    pub(super) chart_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Staged for the Trash (the "cleanup plan"): top-level nodes only, never
    /// one inside another.
    pub(super) collector: Vec<usize>,
    /// What every checkbox shows, worked out once per plan rather than per
    /// row: rows ask on every frame, and walking each staged node's ancestry
    /// for each of them made a big plan lag. See `tick`.
    pub(super) tick_index: std::cell::RefCell<Option<TickIndex>>,
    pub(super) list_scroll: UniformListScrollHandle,
    /// The focused folder's children in list order (`sort`); the chart keeps size order.
    pub(super) order: Vec<usize>,
    pub(super) sort: SortBy,
    pub(super) descending: bool,
    /// Modification times, read on demand when the list sorts by them; `None`
    /// where there's nothing to stat (the APFS remainder) or the stat failed.
    pub(super) mtimes: HashMap<usize, Option<SystemTime>>,
    /// Creation times, read alongside the modification times.
    pub(super) ctimes: HashMap<usize, Option<SystemTime>>,
    /// Rows the Modified sort still needs times for; Broom stats them in the
    /// background and re-sorts when they land.
    pub(super) mtimes_wanted: Vec<usize>,
    pub(super) mtimes_loading: bool,
    /// Compared with an earlier scan of the same root: when it was, and how
    /// much each folder grew since (see `history`). Filled in just after the
    /// scan finishes.
    pub(super) growth_since: Option<SystemTime>,
    pub(super) growth: HashMap<usize, i64>,
    /// Tells this scan's results from the next one's, so late background work
    /// can't land on a tree it wasn't asked about.
    pub(super) scan_id: u64,
    /// The keyboard's row in the navigator.
    pub(super) selected: Option<usize>,
    /// Rows the user picked with ⌃- or ⇧-click, on top of the cursor row.
    /// Empty means "just the cursor", which keeps plain clicking as it was.
    pub(super) picked: HashSet<usize>,
    /// Browser-style navigation history of focused folders (node indices).
    /// `back` holds where ‹Back goes, newest last; `fwd` holds where Forward›
    /// goes. A fresh `navigate` clears `fwd`. Detached nodes (trashed) are
    /// skipped when walking either stack.
    pub(super) back: Vec<usize>,
    pub(super) fwd: Vec<usize>,
    pub(super) elapsed: Duration,
    /// The user wants dates in the list whatever it's sorted by. Sorting by
    /// Modified shows them anyway; see `dates_shown`.
    pub(super) show_modified: bool,
    /// The user wants creation dates in the list.
    pub(super) show_created: bool,
    /// Some listed row has a change worth showing, so the growth column earns
    /// its width. An empty one used to squeeze names down to a letter or two.
    pub(super) growth_listed: bool,
    /// What the user asked to scan; for the startup disk this is `/`, while the tree
    /// itself is rooted at the Data volume.
    pub(super) requested_root: PathBuf,
    pub(super) findings: Vec<Finding>,
    /// What deleting the Collector's items frees (worked out in the background with
    /// `scan::frees_of`; `None` while calculating). `collector_version` guards against a
    /// slow result for an older selection.
    pub(super) collector_frees: Cell<Option<u64>>,
    pub(super) collector_version: u64,
    /// A banner over the chart: the scan-complete line, or `banner_text` when set
    /// (e.g. after a move to the Trash). Cleared by a timer (`show_banner`).
    pub(super) banner: Option<Instant>,
    pub(super) banner_text: Option<String>,
    pub(super) banner_version: u64,
    /// What the last move to the Trash sent where (original, now), for the
    /// banner's "Put Back". Cleared with the banner.
    pub(super) last_trashed: Vec<(PathBuf, PathBuf)>,
    /// When the banner is a move to the Trash: what it freed, for the count-up
    /// and the comparison. Cleared by any other banner.
    pub(super) freed: Option<Freed>,
    /// Dust drifting off where the trashed slices were, for a moment.
    pub(super) dust: Option<Rc<delight::Puff>>,
    pub(super) color_by: ColorBy,
    /// What each node is, for colouring by kind. Filled in as nodes appear in the chart;
    /// node indices stay valid for the life of the tree, so nothing is classified twice.
    pub(super) categories: HashMap<usize, Category>,
    /// The kind under the pointer in the chart's legend: the chart highlights just that.
    pub(super) legend_hover: Option<Category>,
    /// Folders that look like someone's home folder (`home_folders`).
    pub(super) homes: Vec<usize>,
    /// The query at the centre of the current ask state.
    pub(super) query: SharedString,
    pub(super) ask: AskState,
    /// Exact-match candidate groups for the current query text, from the background.
    pub(super) matches: Option<Arc<Vec<CandidateGroup>>>,
    /// Bumped whenever the query text changes, so a slow background search for older
    /// text is dropped when it lands.
    pub(super) search_version: u64,
    /// Shown from the last session's cache, or being refreshed in place.
    pub(super) refresh: Option<Refresh>,
    /// This tree was caught up from FSEvents rather than read in full.
    pub(super) caught_up: bool,
}

/// A refresh of the tree on screen: the disk is read (or caught up from
/// FSEvents) in the background and the result swapped in when it lands.
pub(super) struct Refresh {
    /// When the tree on screen was taken, if it came from the cache rather
    /// than from this session's own scan.
    pub(super) from: Option<SystemTime>,
    /// The refresh is under way.
    pub(super) running: bool,
    /// Tells a full walk behind the refresh to stop when this one is dropped
    /// (another root picked, Start Over).
    pub(super) cancel: Arc<Progress>,
    pub(super) _task: Option<Task<()>>,
}

impl Drop for Refresh {
    fn drop(&mut self) {
        self.cancel.cancelled.store(true, Ordering::Relaxed);
    }
}

/// A move to the Trash, as the banner tells it.
pub(super) struct Freed {
    pub(super) bytes: u64,
    pub(super) items: usize,
    /// The lifetime milestone this cleanup carried the tally past, if any.
    pub(super) milestone: Option<u64>,
}

impl Results {
    /// The toolbar's word on where the tree on screen came from: "Scanned in
    /// 39.9s", "Updated in 0.4s" after a catch-up, "From 5m ago" for a cached
    /// tree, with "· refreshing…" while a refresh runs behind it.
    pub(super) fn status_line(&self) -> String {
        let seconds = self.elapsed.as_secs_f32();
        let base = match self.refresh.as_ref().and_then(|f| f.from) {
            Some(taken) => match format_age(taken).as_str() {
                "now" => "From just now".to_string(),
                age => format!("From {age} ago"),
            },
            None if self.caught_up => format!("Updated in {seconds:.1}s"),
            None => format!("Scanned in {seconds:.1}s"),
        };
        if self.refresh.as_ref().is_some_and(|f| f.running) { format!("{base} · refreshing…") } else { base }
    }

    /// Pick up where the user was in `old`, a tree of the same root this one
    /// replaces: the focused folder (or the nearest one above it that's still
    /// there), the history, the cursor, the picked rows and the cleanup plan.
    /// Node numbers mean nothing across trees, so everything goes by path, and
    /// whatever no longer exists is dropped.
    pub(super) fn carry_over_from(&mut self, old: &Results) {
        let map = |ix: usize| -> Option<usize> {
            let attached = ix == Tree::ROOT || old.tree.nodes.get(ix)?.parent.is_some();
            if !attached {
                return None;
            }
            self.tree.find(&old.tree.path_of(ix))
        };
        let focus = old
            .tree
            .ancestry(old.focus)
            .into_iter()
            .rev()
            .find_map(|ix| map(ix).filter(|&n| self.tree.nodes[n].kind == Kind::Dir))
            .unwrap_or(Tree::ROOT);
        let back: Vec<usize> = old.back.iter().filter_map(|&ix| map(ix)).collect();
        let fwd: Vec<usize> = old.fwd.iter().filter_map(|&ix| map(ix)).collect();
        let selected = old.selected.and_then(map);
        let picked: HashSet<usize> = old.picked.iter().filter_map(|&ix| map(ix)).collect();
        let collector: Vec<usize> = old.collector.iter().filter_map(|&ix| map(ix)).collect();
        self.focus = focus;
        self.back = back;
        self.fwd = fwd;
        self.selected = selected;
        self.picked = picked;
        self.collector_frees.set(if collector.is_empty() { Some(0) } else { None });
        self.collector = collector;
        self.list_scroll = old.list_scroll.clone();
        self.banner = old.banner;
        self.banner_text = old.banner_text.clone();
        self.banner_version = old.banner_version;
        self.last_trashed = old.last_trashed.clone();
        self.legend_hover = old.legend_hover;
        self.relayout();
    }

    /// After the tree changed under the lists (nodes detached or grafted): drop
    /// everything that pointed into it and start the chart's settle animation.
    /// Returns whether the cleanup plan still has items, so its "frees" figure
    /// needs recomputing.
    pub(super) fn tree_changed(&mut self) -> bool {
        let tree = self.tree.clone();
        self.collector.retain(|&c| tree.nodes[c].parent.is_some());
        let recompute = !self.collector.is_empty();
        self.collector_frees.set(if recompute { None } else { Some(0) });
        // The tree changed under any in-flight search: bump the version so its
        // result (computed on the old tree) is dropped instead of re-installing
        // stale cards.
        self.search_version += 1;
        self.matches = None;
        self.ask = AskState::Idle;
        self.chart_hover = None;
        self.list_hover = None;
        self.anim_start = clock::now();
        self.relayout();
        recompute
    }

    pub(super) fn new(tree: Tree, findings: Vec<Finding>, elapsed: Duration, requested_root: PathBuf, color_by: ColorBy, sort: SortBy, descending: bool, show_modified: bool) -> Self {
        let mut results = Self {
            tree: Arc::new(tree),
            focus: Tree::ROOT,
            segments: Rc::default(),
            swatches: HashMap::new(),
            chart_hover: None,
            list_hover: None,
            anim_start: clock::now(),
            chart_bounds: Rc::default(),
            collector: Vec::new(),
            tick_index: Default::default(),
            list_scroll: UniformListScrollHandle::new(),
            order: Vec::new(),
            sort,
            descending,
            mtimes: HashMap::new(),
            ctimes: HashMap::new(),
            mtimes_wanted: Vec::new(),
            mtimes_loading: false,
            growth_since: None,
            growth: HashMap::new(),
            scan_id: NEXT_SCAN_ID.fetch_add(1, Ordering::Relaxed),
            selected: None,
            back: Vec::new(),
            fwd: Vec::new(),
            picked: HashSet::new(),
            elapsed,
            show_modified,
            show_created: false,
            growth_listed: false,
            requested_root,
            findings,
            collector_frees: Cell::new(None),
            collector_version: 0,
            banner: None,
            banner_text: None,
            banner_version: 0,
            last_trashed: Vec::new(),
            freed: None,
            dust: None,
            color_by,
            categories: HashMap::new(),
            legend_hover: None,
            homes: Vec::new(),
            query: SharedString::default(),
            ask: AskState::Idle,
            matches: None,
            search_version: 0,
            refresh: None,
            caught_up: false,
        };
        results.homes = home_folders(&results.tree);
        results.relayout();
        results
    }

    pub(super) fn relayout(&mut self) {
        let segments = sunburst::layout(&self.tree, self.focus);
        if self.color_by == ColorBy::Kind {
            for s in &segments {
                if let Target::Node(ix) = s.target {
                    let (tree, homes) = (&self.tree, &self.homes);
                    self.categories.entry(ix).or_insert_with(|| category_of(tree, homes, ix));
                }
            }
        }
        self.swatches = segments
            .iter()
            .filter_map(|s| match s.target {
                Target::Node(ix) if s.depth == 1 => Some((ix, self.color(s))),
                _ => None,
            })
            .collect();
        self.segments = Rc::new(segments);
        self.reorder();
    }

    /// The list order for the focused folder: by size, name or modification
    /// time, either way round. The APFS remainder slices stay last regardless.
    pub(super) fn reorder(&mut self) {
        let mut order = self.tree.nodes[self.focus].children.clone();
        let tree = &self.tree;
        let desc = self.descending;
        match self.sort {
            // The tree keeps children largest first already.
            SortBy::Size => {
                if !desc {
                    order.reverse();
                }
            }
            SortBy::Name => order.sort_by(|&a, &b| {
                // Folders first either way, then case-insensitive by name.
                let (na, nb) = (&tree.nodes[a], &tree.nodes[b]);
                let by_name = na.name.to_lowercase().cmp(&nb.name.to_lowercase());
                (na.kind != Kind::Dir).cmp(&(nb.kind != Kind::Dir)).then(if desc { by_name.reverse() } else { by_name })
            }),
            SortBy::Modified => {
                // Times were queued above, if the column is on: sorting by date
                // and reading dates are the same work.
                let mtimes = &self.mtimes;
                let time = |ix: usize| mtimes.get(&ix).copied().flatten();
                // Unknown times sink to the bottom either way.
                order.sort_by(|&a, &b| match (time(a), time(b)) {
                    (Some(x), Some(y)) => if desc { y.cmp(&x) } else { x.cmp(&y) },
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                });
            }
        }
        order.sort_by_key(|&ix| tree.nodes[ix].kind == Kind::Other);
        self.order = order;
        // Times are wanted whenever they'd be drawn or sorted by.
        self.mtimes_wanted = if self.dates_shown() || self.show_created {
            self.order.iter().copied().filter(|ix| !self.mtimes.contains_key(ix)).collect()
        } else {
            Vec::new()
        };
        self.growth_listed = self.order.iter().any(|ix| self.growth.get(ix).copied().and_then(format_delta).is_some());
        // A picked row that's gone (trashed, or the list changed) can't stay
        // picked: Space and ⌘C would act on something no longer on screen.
        if !self.picked.is_empty() {
            let listed: HashSet<usize> = self.order.iter().copied().collect();
            self.picked.retain(|ix| listed.contains(ix));
        }
        if let Some(sel) = self.selected {
            if !self.order.contains(&sel) {
                self.selected = None;
            }
        }
    }

    /// Dates are on screen: asked for, or the list is sorted by them.
    pub(super) fn dates_shown(&self) -> bool {
        self.show_modified || self.sort == SortBy::Modified
    }

    /// What Space and ⌘C act on, in the order rows are listed: every picked
    /// row, or the cursor row when only the cursor is lit.
    pub(super) fn selection(&self) -> Vec<usize> {
        if self.picked.is_empty() {
            return self.selected.into_iter().collect();
        }
        self.order.iter().copied().filter(|ix| self.picked.contains(ix)).collect()
    }

    /// ⌘- or ⌃-click toggles one row without losing the rest; ⇧-click takes
    /// the range from the cursor. A plain click drops the picking and moves
    /// the cursor.
    pub(super) fn pick(&mut self, ix: usize, additive: bool, range: bool) {
        if additive {
            // The lit cursor row is part of what you're adding to, as in Finder.
            if self.picked.is_empty() {
                self.picked.extend(self.selected.filter(|s| self.order.contains(s)));
            }
            // The cursor only ever sits on a lit row: the next ⇧-click measures
            // from it, and Enter opens it, so it can't be somewhere you can't see.
            self.selected = if self.picked.remove(&ix) {
                None
            } else {
                self.picked.insert(ix);
                Some(ix)
            };
        } else if range {
            let to = self.row_of(ix).unwrap_or(0);
            let from = self.selected.and_then(|s| self.row_of(s)).unwrap_or(to);
            let (lo, hi) = (from.min(to), from.max(to));
            self.picked = self.order[lo..=hi].iter().copied().collect();
            self.selected.get_or_insert(ix);
        } else {
            self.picked.clear();
            self.selected = Some(ix);
        }
    }

    /// ⌘A: every real row in the list. The APFS remainder and the like aren't
    /// things you can act on, so they stay out.
    pub(super) fn pick_all(&mut self) {
        let tree = &self.tree;
        self.picked = self.order.iter().copied().filter(|&ix| tree.nodes[ix].kind != Kind::Other).collect();
    }

    /// What ⌘C copies: every lit row's path, one per line, or the open folder's
    /// when no real row is lit.
    pub(super) fn paths_to_copy(&self) -> String {
        let mut rows: Vec<usize> = self.selection().into_iter().filter(|&ix| self.tree.nodes[ix].kind != Kind::Other).collect();
        if rows.is_empty() {
            rows.push(self.focus);
        }
        let paths: Vec<String> = rows.into_iter().map(|ix| self.tree.path_of(ix).to_string_lossy().into_owned()).collect();
        paths.join("\n")
    }

    /// A row's place in the list, if it's listed at all.
    pub(super) fn row_of(&self, ix: usize) -> Option<usize> {
        self.order.iter().position(|&o| o == ix)
    }

    pub(super) fn set_sort(&mut self, sort: SortBy, descending: bool) {
        self.sort = sort;
        self.descending = descending;
        self.reorder();
    }

    pub(super) fn set_color_by(&mut self, color_by: ColorBy) {
        self.color_by = color_by;
        self.legend_hover = None;
        self.relayout();
    }

    pub(super) fn category(&self, target: Target) -> Option<Category> {
        match target {
            Target::Node(ix) => self.categories.get(&ix).copied(),
            Target::Small { .. } => None,
        }
    }

    pub(super) fn color(&self, segment: &Segment) -> Hsla {
        match (self.color_by, self.category(segment.target)) {
            (ColorBy::Kind, Some(category)) => {
                let base = classify::color(category);
                // A touch lighter per ring, so nested folders of the same kind stay apart.
                let lightness = (base.lightness + 0.015 * (segment.depth - 1) as f32).min(0.9);
                gpui::hsla(base.hue.into_positive_degrees() / 360.0, base.saturation, lightness, 1.0)
            }
            _ => sunburst::base_color(segment),
        }
    }

    pub(super) fn navigate(&mut self, ix: usize) {
        // Detached nodes (trashed while referenced by a finding) have no place in the chart.
        let detached = self.tree.nodes[ix].parent.is_none() && ix != Tree::ROOT;
        if detached || self.tree.nodes[ix].kind != Kind::Dir || ix == self.focus {
            return;
        }
        // Where we just were joins the back stack, and a fresh move invalidates
        // the forward one: you can't go forward after going somewhere new.
        self.back.push(self.focus);
        self.fwd.clear();
        self.move_focus(ix);
    }

    /// Stand in `ix`. Going up lands the keyboard on the folder just left; going
    /// down starts afresh. A picking is about the rows on screen, so it stays behind.
    pub(super) fn move_focus(&mut self, ix: usize) {
        let came_from = (self.tree.nodes[self.focus].parent == Some(ix)).then_some(self.focus);
        self.focus = ix;
        self.chart_hover = None;
        self.list_hover = None;
        self.anim_start = clock::now();
        self.selected = came_from;
        self.picked.clear();
        self.relayout();
        match came_from.and_then(|c| self.order.iter().position(|&o| o == c)) {
            Some(pos) => self.list_scroll.scroll_to_item(pos, ScrollStrategy::Center),
            None => self.list_scroll.scroll_to_item(0, ScrollStrategy::Top),
        }
    }

    /// A folder we could still stand in: not trashed, and so not detached.
    pub(super) fn can_stand_in(&self, ix: usize) -> bool {
        self.tree.nodes.get(ix).is_some_and(|n| n.kind == Kind::Dir && (n.parent.is_some() || ix == Tree::ROOT))
    }

    /// The button lights only if a step would really go somewhere.
    pub(super) fn can_travel(&self, way: Travel) -> bool {
        let stack = match way {
            Travel::Back => &self.back,
            Travel::Forward => &self.fwd,
        };
        stack.iter().any(|&ix| ix != self.focus && self.can_stand_in(ix))
    }

    /// Take one step through the history, remembering where we were so the
    /// opposite direction can undo it. Entries that stopped being folders we can
    /// stand in (trashed, so detached) are dropped on the way past.
    pub(super) fn travel(&mut self, way: Travel) -> bool {
        loop {
            let step = match way {
                Travel::Back => self.back.pop(),
                Travel::Forward => self.fwd.pop(),
            };
            let Some(ix) = step else { return false };
            if !self.can_stand_in(ix) || ix == self.focus {
                continue;
            }
            match way {
                Travel::Back => self.fwd.push(self.focus),
                Travel::Forward => self.back.push(self.focus),
            }
            self.move_focus(ix);
            return true;
        }
    }

    pub(super) fn hovered(&self) -> Option<Target> {
        match self.chart_hover {
            Some(Hit::Segment(i)) => self.segments.get(i).map(|s| s.target),
            Some(Hit::Center) => None,
            None => self.list_hover.map(Target::Node),
        }
    }

    pub(super) fn collect(&mut self, ix: usize) {
        // Same rules the ask path enforces (see `ask::collectable`): never the
        // root, detached nodes, volume slices — and never a whole home folder,
        // whichever way the user collects.
        if ix == Tree::ROOT
            || self.tree.nodes[ix].parent.is_none()
            || self.tree.nodes[ix].kind == Kind::Other
            || self.homes.contains(&ix)
        {
            return;
        }
        if self.collector.iter().any(|&c| self.tree.is_ancestor_or_self(c, ix)) {
            return;
        }
        let tree = &self.tree;
        self.collector.retain(|&c| !tree.is_ancestor_or_self(ix, c));
        self.collector.push(ix);
        self.collector_frees.set(None);
    }

    pub(super) fn collected_size(&self) -> u64 {
        self.collector.iter().map(|&c| self.tree.nodes[c].size).sum()
    }

    /// Whether `ix` may be staged at all (see `collect`).
    pub(super) fn stageable(&self, ix: usize) -> bool {
        ix != Tree::ROOT
            && self.tree.nodes[ix].parent.is_some()
            && self.tree.nodes[ix].kind != Kind::Other
            && !self.homes.contains(&ix)
    }

    /// What the checkbox on `ix` shows: on if staged, implied if a folder
    /// above it is, mixed if something below it is.
    pub(super) fn tick(&self, ix: usize) -> Tick {
        let mut cache = self.tick_index.borrow_mut();
        // The plan is changed in place from several places, so the cache
        // checks it hasn't moved on rather than trusting to be told.
        let fresh = cache.as_ref().is_some_and(|t| t.matches(&self.tree, &self.collector));
        if !fresh {
            *cache = Some(TickIndex::new(&self.tree, &self.collector));
        }
        let index = cache.as_ref().expect("just built");
        if index.staged.contains(&ix) {
            return Tick::On;
        }
        let mut up = self.tree.nodes[ix].parent;
        while let Some(p) = up {
            if index.staged.contains(&p) {
                return Tick::Implied;
            }
            up = self.tree.nodes[p].parent;
        }
        if index.above.contains(&ix) {
            return Tick::Mixed;
        }
        Tick::Off
    }

    /// The checkbox on `ix` was clicked. A folder staged through its parent
    /// stays as it is: the parent's own box is where that decision lives.
    pub(super) fn toggle_staged(&mut self, ix: usize) {
        match self.tick(ix) {
            Tick::On => {
                self.collector.retain(|&c| c != ix);
                self.collector_frees.set(None);
            }
            Tick::Implied => {}
            Tick::Off | Tick::Mixed => self.collect(ix),
        }
    }

    /// The checkbox on a group of nodes (a finding): on when every node is
    /// staged or implied, mixed when some are, off otherwise.
    pub(super) fn group_tick(&self, nodes: &[usize]) -> Tick {
        let states: Vec<Tick> = nodes.iter().map(|&n| self.tick(n)).collect();
        if states.is_empty() {
            Tick::Off
        } else if states.iter().all(|t| matches!(t, Tick::On | Tick::Implied)) {
            Tick::On
        } else if states.iter().all(|t| *t == Tick::Off) {
            Tick::Off
        } else {
            Tick::Mixed
        }
    }

    /// Stage or unstage a whole group at once.
    pub(super) fn toggle_group(&mut self, nodes: &[usize]) {
        match self.group_tick(nodes) {
            Tick::On | Tick::Implied => {
                self.collector.retain(|c| !nodes.contains(c));
                self.collector_frees.set(None);
            }
            Tick::Off | Tick::Mixed => {
                for &n in nodes {
                    self.collect(n);
                }
            }
        }
    }

    /// Stage these rows together. One pass over the plan rather than one per row:
    /// ⌘A in a big folder can be a hundred thousand of them.
    pub(super) fn stage_rows(&mut self, rows: &[usize]) {
        let tree = &self.tree;
        let mut wanted: HashSet<usize> = self.collector.iter().copied().collect();
        let start = wanted.len();
        wanted.extend(rows.iter().copied().filter(|&ix| ix != Tree::ROOT && tree.nodes[ix].parent.is_some() && tree.nodes[ix].kind != Kind::Other && !self.homes.contains(&ix)));
        if wanted.len() == start {
            return;
        }
        // Only the topmost of anything nested stays, as with one at a time.
        let covered = |mut ix: usize| {
            while let Some(parent) = tree.nodes[ix].parent {
                if wanted.contains(&parent) {
                    return true;
                }
                ix = parent;
            }
            false
        };
        let mut plan: Vec<usize> = self.collector.iter().copied().filter(|&c| !covered(c)).collect();
        let already: HashSet<usize> = plan.iter().copied().collect();
        plan.extend(rows.iter().copied().filter(|ix| wanted.contains(ix) && !already.contains(ix) && !covered(*ix)));
        self.collector = plan;
        self.collector_frees.set(None);
    }

    /// Space on the lit rows: all of them are staged already, so let them go;
    /// otherwise stage the rest. Never a flip per row, which would undo a plan
    /// the moment one new row joined the picking.
    pub(super) fn toggle_rows(&mut self, rows: &[usize]) {
        let staged: HashSet<usize> = self.collector.iter().copied().collect();
        let on = |mut ix: usize| loop {
            if staged.contains(&ix) {
                return true;
            }
            match self.tree.nodes[ix].parent {
                Some(parent) => ix = parent,
                None => return false,
            }
        };
        let rows: Vec<usize> = rows.iter().copied().filter(|&ix| self.stageable(ix)).collect();
        if rows.iter().all(|&ix| on(ix)) {
            let rows: HashSet<usize> = rows.into_iter().collect();
            self.collector.retain(|c| !rows.contains(c));
            self.collector_frees.set(None);
        } else {
            self.stage_rows(&rows);
        }
    }

    /// Whether `ix` (a chart slice) is staged, itself or through a parent.
    pub(super) fn is_staged(&self, ix: usize) -> bool {
        self.collector.iter().any(|&c| self.tree.is_ancestor_or_self(c, ix))
    }
}

/// What a node is. The APFS volume slices are the system's; the unscanned or unreadable
/// remainder could be anything.
pub(super) fn category_of(tree: &Tree, homes: &[usize], ix: usize) -> Category {
    let node = &tree.nodes[ix];
    let path = match node.kind {
        Kind::Other if [disk::NOT_SCANNED, disk::NOT_READABLE].contains(&node.name.as_ref()) => return Category::Mixed,
        Kind::Other => return Category::System,
        _ => {
            // Inside a home folder, describe the path from it (`~/…`) wherever the home
            // folder lives, as the classifier expects.
            let chain = tree.ancestry(ix);
            match chain.iter().rposition(|a| homes.contains(a)) {
                Some(home) => chain[home + 1..].iter().fold(PathBuf::from("~"), |path, &a| path.join(tree.nodes[a].name.as_ref())),
                None => tree.path_of(ix),
            }
        }
    };
    classify::classify(&path, node.kind == Kind::File)
}

/// Folders near the top of the tree that look like a home folder: a Library and at least
/// two of the usual Desktop, Documents, Downloads, Movies, Music, Pictures. That finds
/// the user's own (`/Users/<name>`) and also one scanned on a backup disk or a copy.
pub(super) fn home_folders(tree: &Tree) -> Vec<usize> {
    const USUAL: [&str; 6] = ["Desktop", "Documents", "Downloads", "Movies", "Music", "Pictures"];
    let mut homes = Vec::new();
    let mut level = vec![Tree::ROOT];
    for _ in 0..4 {
        let mut next = Vec::new();
        for &ix in &level {
            let names: Vec<&str> = tree.nodes[ix].children.iter().map(|&c| tree.nodes[c].name.as_ref()).collect();
            if names.contains(&"Library") && USUAL.iter().filter(|u| names.contains(u)).count() >= 2 {
                homes.push(ix);
                continue;
            }
            next.extend(tree.nodes[ix].children.iter().copied().filter(|&c| tree.nodes[c].kind == Kind::Dir));
        }
        level = next;
    }
    homes
}

/// The plan, indexed for `Results::tick`: the staged nodes, and every folder
/// above one of them.
pub(super) struct TickIndex {
    tree: *const Tree,
    nodes: usize,
    plan: Vec<usize>,
    staged: HashSet<usize>,
    above: HashSet<usize>,
}

impl TickIndex {
    fn new(tree: &Arc<Tree>, plan: &[usize]) -> Self {
        let staged: HashSet<usize> = plan.iter().copied().collect();
        let mut above = HashSet::new();
        for &c in plan {
            let mut up = tree.nodes[c].parent;
            while let Some(p) = up {
                // Already walked from here up by an earlier sibling.
                if !above.insert(p) {
                    break;
                }
                up = tree.nodes[p].parent;
            }
        }
        TickIndex { tree: Arc::as_ptr(tree), nodes: tree.nodes.len(), plan: plan.to_vec(), staged, above }
    }

    /// Built from this tree and this plan. A plan compare is a memcmp, far
    /// cheaper than the ancestry walks it saves.
    fn matches(&self, tree: &Arc<Tree>, plan: &[usize]) -> bool {
        self.tree == Arc::as_ptr(tree) && self.nodes == tree.nodes.len() && self.plan == plan
    }
}
