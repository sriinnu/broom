//! Starting, finishing and keeping scans up to date, and the background work around them.

use super::*;

/// Show kept trees and catch up from FSEvents. Off for the recorder, whose
/// scripts expect a live scan, and with `BROOM_NO_CACHE` for a forced full read.
pub(super) fn use_cache() -> bool {
    !cfg!(feature = "snapshot") && std::env::var_os("BROOM_NO_CACHE").is_none()
}

impl Broom {
    /// The banner's "Put back": move the last batch out of the Trash, then
    /// re-read just the folders they went back into.
    pub(super) fn put_back(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.results() else { return };
        let landed = std::mem::take(&mut r.last_trashed);
        if landed.is_empty() {
            return;
        }
        r.banner = None;
        // What the lifetime tally counted for this batch, to take back for
        // whatever actually comes out of the Trash.
        let swept = r.freed.take().map(|f| f.bytes).unwrap_or(0);
        let sent = landed.len();
        let scan_id = r.scan_id;
        let (excludes, outline_depth) = (self.settings.exclude_paths(), self.settings.outline_depth);
        cx.spawn(async move |this, cx| {
            let (restored, folders) = cx
                .background_spawn(async move {
                    let restored = trashcan::put_back(&landed);
                    let mut parents: Vec<PathBuf> = restored.iter().filter_map(|p| p.parent().map(PathBuf::from)).collect();
                    parents.sort();
                    parents.dedup();
                    // A parent inside another parent is covered by the outer scan.
                    let outer: Vec<PathBuf> = parents.iter().filter(|p| !parents.iter().any(|q| q != *p && p.starts_with(q))).cloned().collect();
                    let folders: Vec<(PathBuf, Tree)> =
                        outer.into_iter().map(|p| { let t = scan::scan(&p, &scan::Progress::default(), &excludes, outline_depth); (p, t) }).collect();
                    (restored, folders)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let mut recompute = false;
                if let Some(r) = this.results().filter(|r| r.scan_id == scan_id) {
                    for (folder, fresh) in &folders {
                        if let Some(ix) = r.tree.find(folder) {
                            // The graft detaches what was inside; don't be left standing in it.
                            if r.tree.is_ancestor_or_self(ix, r.focus) {
                                r.focus = ix;
                            }
                            Arc::make_mut(&mut r.tree).graft(ix, fresh);
                        }
                    }
                    r.mtimes.clear();
                    r.ctimes.clear();
                    recompute = r.tree_changed();
                }
                if recompute {
                    this.collector_changed(cx);
                }
                if sent > 0 && !restored.is_empty() {
                    delight::record_put_back((swept as u128 * restored.len() as u128 / sent as u128) as u64);
                }
                let what = if restored.len() == 1 { "1 item".to_string() } else { format!("{} items", restored.len()) };
                this.show_banner(Some(format!("Put {what} back where it was.")), cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// After `trash::delete_all` succeeded for these nodes (of the current tree):
    /// take them out of the tree and every list that pointed at them.
    pub fn remove_trashed(&mut self, items: &[usize], cx: &mut Context<Self>) {
        // Before the results borrow: the menu may point at a node that's
        // about to leave the tree.
        self.context_menu = None;
        let mut recompute = false;
        if let Some(r) = self.results() {
            for &ix in items {
                if r.tree.nodes[ix].parent.is_none() {
                    continue;
                }
                if r.tree.is_ancestor_or_self(ix, r.focus) {
                    r.focus = r.tree.nodes[ix].parent.unwrap_or(Tree::ROOT);
                }
                Arc::make_mut(&mut r.tree).remove(ix);
            }
            // The folders they left have new modification times.
            r.mtimes.clear();
            r.ctimes.clear();
            findings::prune_after_removal(&mut r.findings, &r.tree);
            recompute = r.tree_changed();
        }
        if recompute {
            self.collector_changed(cx);
        }
        cx.notify();
    }

    /// Finder changed the Trash (emptied it, or something landed there, including
    /// what Broom itself moved): re-scan just that folder off the UI thread, graft
    /// it into the tree, refresh the Trash finding and the disks' free space. A
    /// cheap stat of the folder every 2 s decides whether any of that is needed.
    pub(super) fn watch_trash(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let trash_dir = home_dir().join(".Trash");
            let mut seen: Option<(std::time::SystemTime, usize)> = None;
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                // Only while a tree that holds the Trash is on screen; otherwise the
                // next scan reads it fresh anyway.
                let Ok(Some((root, excludes, outline_depth))) = this.update(cx, |this, _| {
                    let settings = (this.settings.exclude_paths(), this.settings.outline_depth);
                    match &this.screen {
                        Screen::Results(r) if r.tree.find(&trash_dir).is_some() => Some((r.tree.root_path.clone(), settings.0, settings.1)),
                        _ => None,
                    }
                }) else {
                    seen = None;
                    continue;
                };
                let fingerprint = cx
                    .background_spawn({
                        let trash_dir = trash_dir.clone();
                        async move {
                            let modified = std::fs::metadata(&trash_dir).and_then(|m| m.modified()).ok()?;
                            let entries = std::fs::read_dir(&trash_dir).ok()?.count();
                            Some((modified, entries))
                        }
                    })
                    .await;
                let Some(fingerprint) = fingerprint else { continue };
                if seen == Some(fingerprint) {
                    continue;
                }
                let first_look = seen.is_none();
                seen = Some(fingerprint);
                if first_look {
                    // The scan that just finished already read it.
                    continue;
                }
                let (fresh, volumes) = cx
                    .background_spawn({
                        let trash_dir = trash_dir.clone();
                        async move {
                            let fresh = scan::scan(&trash_dir, &scan::Progress::default(), &excludes, outline_depth);
                            (fresh, scan::volumes())
                        }
                    })
                    .await;
                let bases = findings::Bases::for_root(&root);
                let recompute = this
                    .update(cx, |this, cx| {
                        this.context_menu = None;
                        if let Some(bar) = &this.menubar {
                            bar.update(startup_fraction(&volumes), &volume_lines(&volumes));
                        }
                        this.volumes = volumes;
                        let mut recompute = false;
                        if let Some(r) = this.results() {
                            if let Some(ix) = r.tree.find(&trash_dir) {
                                if r.tree.is_ancestor_or_self(ix, r.focus) {
                                    r.focus = ix;
                                }
                                Arc::make_mut(&mut r.tree).graft(ix, &fresh);
                                // The Trash changed under us; its times did too.
                                r.mtimes.clear();
                                r.ctimes.clear();
                                findings::prune_after_removal(&mut r.findings, &r.tree);
                                findings::refresh_location(&mut r.findings, &r.tree, &bases, "Trash");
                                recompute = r.tree_changed();
                            }
                        }
                        this.apply_protection();
                        cx.notify();
                        recompute
                    })
                    .unwrap_or(false);
                if recompute {
                    this.update(cx, |this, cx| this.collector_changed(cx)).ok();
                }
            }
        })
    }

    /// Keep the disks' numbers honest wherever they're shown: the rail always,
    /// the menu-bar extra when it's on — emptying the Trash or mounting a
    /// volume should move them without a relaunch. Recomputed off the UI
    /// thread; 5s while nothing is scanned (the rail is the whole show), 30s
    /// otherwise.
    pub(super) fn watch_menubar(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let (volumes, clouds) = cx.background_spawn(async move { (scan::volumes(), scan::cloud_roots()) }).await;
                let idle = this
                    .update(cx, |this, cx| {
                        if let Some(bar) = &this.menubar {
                            bar.update(startup_fraction(&volumes), &volume_lines(&volumes));
                        }
                        this.volumes = volumes;
                        this.clouds = clouds;
                        cx.notify();
                        matches!(this.screen, Screen::Start)
                    })
                    .unwrap_or(false);
                let pace = if idle { 5 } else { 30 };
                cx.background_executor().timer(Duration::from_secs(pace)).await;
            }
        })
    }

    /// Re-check every couple of seconds while access is missing, so the card can react as
    /// soon as the user flips the switch in System Settings.
    pub(super) fn watch_access(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let granted = onboarding::has_full_disk_access();
                let keep_watching = this
                    .update(cx, |this, cx| {
                        if granted && this.access != Access::Granted {
                            this.access = Access::JustGranted;
                            cx.notify();
                        }
                        !granted
                    })
                    .unwrap_or(false);
                if !keep_watching {
                    break;
                }
            }
        })
    }

    pub(super) fn start_scan(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.error = None;
        self.context_menu = None;
        onboarding::mark_first_run_done();
        let progress = Arc::new(Progress::default());
        let started = clock::now();

        // Capture settings that the spawned task needs, before the borrow of
        // self expires.
        let excludes = self.settings.exclude_paths();
        let outline_depth = self.settings.outline_depth;
        let live_refresh_ms = self.settings.live_refresh_ms;

        let scan_task = cx.spawn({
            let progress = progress.clone();
            let root = root.clone();
            let root_for_results = root.clone();
            let excludes = excludes.clone();
            async move |this, cx| {
                // The last scan of this root, kept on disk, goes on screen at once; the
                // disk is read (or caught up from FSEvents) behind it.
                if use_cache() {
                    let cached = cx
                        .background_spawn({
                            let (root, excludes) = (root.clone(), excludes.clone());
                            async move {
                                let (meta, tree) = cache::load(&root)?;
                                if meta.excludes != cache::excludes_key(&excludes) {
                                    return None;
                                }
                                let findings = findings::from_tree(&tree, &findings::Bases::for_root(&tree.root_path));
                                Some((meta.taken, tree, findings))
                            }
                        })
                        .await;
                    if let Some((taken, tree, findings)) = cached {
                        this.update(cx, |this, cx| {
                            if !progress.cancelled.load(Ordering::Relaxed) {
                                this.show_cached(tree, findings, taken, root_for_results.clone(), cx);
                            }
                        })
                        .ok();
                        return;
                    }
                }
                let (tree, meta, findings) = cx
                    .background_spawn({
                        let progress = progress.clone();
                        let excludes = excludes.clone();
                        async move {
                            let (tree, meta) = refresh::full_scan(&root, &progress, &excludes, outline_depth);
                            // May survey clone sharing (e.g. pnpm's cloned node_modules), so
                            // keep it off the UI thread.
                            let findings = findings::from_tree(&tree, &findings::Bases::for_root(&tree.root_path));
                            (tree, meta, findings)
                        }
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if !progress.cancelled.load(Ordering::Relaxed) {
                        // Open the results where the user was looking during the scan.
                        let focus = match &this.screen {
                            Screen::Scanning(scanning) => scanning.focus.clone(),
                            _ => Vec::new(),
                        };
                        let root_str = root_for_results.to_string_lossy().into_owned();
                        let mut results = Results::new(tree, findings, clock::since(started), root_for_results, this.color_by, this.sort, this.sort_descending, this.show_modified);
                        results.show_created = this.settings.show_created;
                        let ix = resolve_path(&results.tree, &focus);
                        if ix != Tree::ROOT {
                            results.navigate(ix);
                            // Restoring where you were is a replacement, not a
                            // step: the old trail pointed into the old tree, so
                            // this scan starts with no history to walk.
                            results.back.clear();
                            results.fwd.clear();
                        }
                        // The chart is already on screen from the scan; don't replay the sweep.
                        results.anim_start = clock::now() - ZOOM_DURATION;
                        let (tree, scan_id, elapsed) = (results.tree.clone(), results.scan_id, results.elapsed);
                        this.screen = Screen::Results(results);
                        this.apply_protection();
                        let kept = tree.clone();
                        cx.background_spawn(async move { refresh::store(&meta, &kept, refresh::How::Full, elapsed) }).detach();
                        this.resolve_pending_findings(cx);
                        this.compare_with_history(tree.clone(), scan_id, cx);
                        this.find_duplicates(tree, scan_id, cx);
                        // Remember what was scanned so a relaunch picks up here.
                        this.settings.last_root = Some(root_str);
                        let _ = settings::save(&this.settings);
                        // A light tap (felt only with a finger on a Force Touch trackpad).
                        cx.play_haptic_feedback(HapticFeedbackStyle::LevelChange);
                        this.show_banner(None, cx);
                        cx.notify();
                    }
                })
                .ok();
            }
        });

        // Keep the counters fresh, and re-snapshot the live chart a few times a second.
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                let scanning = this.update(cx, |this, cx| {
                    cx.notify();
                    let Screen::Scanning(scanning) = &mut this.screen else { return false };
                    let items = scanning.progress.files.load(Ordering::Relaxed) + scanning.progress.dirs.load(Ordering::Relaxed);
                    scanning.eta.update(clock::since(scanning.started).as_secs_f64(), items);
                    if scanning.live.is_none() || clock::since(scanning.last_snapshot) >= ms(live_refresh_ms) {
                        let layout = scanning.progress.layout.get().map(|l| &**l);
                        let root = layout.map(|l| l.data_root.clone()).unwrap_or_else(|| scanning.root.clone());
                        let snapshot = live::snapshot(&scanning.progress.live, &root, layout);
                        scanning.live = Some(Rc::new(LiveView::new(snapshot, &scanning.focus, &mut scanning.finals, ms(700))));
                        scanning.last_snapshot = clock::now();
                    }
                    true
                });
                if !matches!(scanning, Ok(true)) {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(50)).await;
            }
        });

        self.screen = Screen::Scanning(Scanning {
            expected: if root == std::path::Path::new("/") { None } else { scan::volume_used(&root) },
            root,
            progress,
            started,
            live: None,
            last_snapshot: started,
            focus: Vec::new(),
            hover: None,
            chart_bounds: Rc::default(),
            eta: eta::Eta::default(),
            finals: HashMap::new(),
            motion: Rc::default(),
            _tasks: [scan_task, ticker],
        });
        cx.notify();
    }

    pub(super) fn cancel_scan(&mut self, cx: &mut Context<Self>) {
        if let Screen::Scanning(scanning) = &self.screen {
            scanning.progress.cancelled.store(true, Ordering::Relaxed);
        }
        self.review = None;
        self.screen = Screen::Start;
        cx.notify();
    }

    /// Put a banner over the chart for `banner_seconds`: the scan-complete line
    /// when `text` is `None`, else that text. A later banner outlives an
    /// earlier one's timer.
    pub(super) fn show_banner(&mut self, text: Option<String>, cx: &mut Context<Self>) {
        let seconds = self.settings.banner_seconds;
        let Some(r) = self.results() else { return };
        r.banner = Some(clock::now());
        r.banner_text = text;
        // A Trash banner sets this again right after; every other banner is plain.
        r.freed = None;
        r.banner_version += 1;
        let version = r.banner_version;
        cx.spawn(async move |this, cx| {
            let shown = clock::now();
            while clock::since(shown) < Duration::from_secs(seconds) {
                cx.background_executor().timer(Duration::from_millis(250)).await;
            }
            this.update(cx, |this, cx| {
                if let Some(r) = this.results() {
                    if r.banner_version == version {
                        r.banner = None;
                        r.banner_text = None;
                        r.freed = None;
                        r.last_trashed.clear();
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn open_folder(&mut self, _: &OpenFolder, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Scan".into()),
        });
        self.busy = true;
        cx.spawn(async move |this, cx| {
            this.update(cx, |this, _cx| this.busy = false).ok();
            if let Ok(Ok(Some(mut paths))) = paths.await {
                if let Some(path) = paths.pop() {
                    this.update(cx, |this, cx| this.start_scan(path, cx)).ok();
                }
            }
        })
        .detach();
    }

    pub(super) fn rescan(&mut self, _: &Rescan, _: &mut Window, cx: &mut Context<Self>) {
        // With a kept tree to catch up from, the results stay on screen and refresh
        // in place: a second or so instead of a full walk.
        if let Screen::Results(r) = &self.screen {
            let excludes = cache::excludes_key(&self.settings.exclude_paths());
            if use_cache() && cache::peek(&r.requested_root).is_some_and(|m| m.event_id != 0 && m.excludes == excludes) {
                self.refresh_in_place(cx);
                return;
            }
        }
        let root = match &self.screen {
            Screen::Results(r) => r.requested_root.clone(),
            Screen::Scanning(s) => s.root.clone(),
            Screen::Start => return,
        };
        self.cancel_scan(cx);
        self.start_scan(root, cx);
    }

    /// ⇧⌘R: read everything again from the disk, ignoring the kept tree, for
    /// when the quick catch-up is in doubt.
    pub(super) fn full_rescan(&mut self, _: &FullRescan, _: &mut Window, cx: &mut Context<Self>) {
        let root = match &self.screen {
            Screen::Results(r) => r.requested_root.clone(),
            Screen::Scanning(s) => s.root.clone(),
            Screen::Start => return,
        };
        self.cancel_scan(cx);
        self.start_scan(root, cx);
    }

    /// Put a tree kept from an earlier scan on screen, labelled with its age, and
    /// start bringing it up to date.
    pub(super) fn show_cached(&mut self, tree: Tree, findings: Vec<Finding>, taken: SystemTime, root: PathBuf, cx: &mut Context<Self>) {
        let root_str = root.to_string_lossy().into_owned();
        let mut results = Results::new(tree, findings, Duration::ZERO, root, self.color_by, self.sort, self.sort_descending, self.show_modified);
        results.show_created = self.settings.show_created;
        results.refresh = Some(Refresh { from: Some(taken), running: false, cancel: Arc::default(), _task: None });
        refresh::log(&format!("showing the kept tree ({} nodes), {}", results.tree.nodes.len(), results.status_line()));
        // No clone surveys, history record or duplicate search for a tree that is
        // about to be replaced: those run on the fresh one.
        self.screen = Screen::Results(results);
        self.apply_protection();
        self.settings.last_root = Some(root_str);
        let _ = settings::save(&self.settings);
        self.refresh_in_place(cx);
        cx.notify();
    }

    /// Read the disk again for the tree on screen, in the background: caught up
    /// from FSEvents when the kept tree allows, else a full walk. The result
    /// replaces the tree on screen without losing the user's place.
    pub(super) fn refresh_in_place(&mut self, cx: &mut Context<Self>) {
        let (excludes, outline_depth) = (self.settings.exclude_paths(), self.settings.outline_depth);
        let Some(r) = self.results() else { return };
        if r.refresh.as_ref().is_some_and(|f| f.running) {
            return;
        }
        let (root, scan_id) = (r.requested_root.clone(), r.scan_id);
        let cancel = Arc::new(Progress::default());
        let task = cx.spawn({
            let cancel = cancel.clone();
            async move |this, cx| {
                let (fresh, findings) = cx
                    .background_spawn(async move {
                        let fresh = refresh::refresh(&root, &excludes, outline_depth, &cancel);
                        let findings = findings::from_tree(&fresh.tree, &findings::Bases::for_root(&fresh.tree.root_path));
                        (fresh, findings)
                    })
                    .await;
                // A review card or a move to the Trash in flight holds node numbers of
                // the tree on screen; swap only once it's done with them.
                loop {
                    match this.update(cx, |this, _| this.review.is_some()) {
                        Ok(false) => break,
                        Ok(true) => cx.background_executor().timer(Duration::from_millis(300)).await,
                        Err(_) => return,
                    }
                }
                this.update(cx, |this, cx| this.swap_in(scan_id, fresh, findings, cx)).ok();
            }
        });
        let from = r.refresh.as_ref().and_then(|f| f.from);
        r.refresh = Some(Refresh { from, running: true, cancel, _task: Some(task) });
        cx.notify();
    }

    /// The refreshed tree replaces the one on screen (if it's still the one the
    /// refresh was for), keeping the user's place by path.
    fn swap_in(&mut self, scan_id: u64, fresh: refresh::Fresh, findings: Vec<Finding>, cx: &mut Context<Self>) {
        if !matches!(&self.screen, Screen::Results(r) if r.scan_id == scan_id) {
            return;
        }
        let Screen::Results(old) = std::mem::replace(&mut self.screen, Screen::Start) else { return };
        let refresh::Fresh { tree, meta, how, elapsed } = fresh;
        let mut results = Results::new(tree, findings, elapsed, old.requested_root.clone(), self.color_by, old.sort, old.descending, old.show_modified);
        results.show_created = old.show_created;
        results.caught_up = matches!(how, refresh::How::CaughtUp { .. });
        results.carry_over_from(&old);
        // The chart is already on screen; no sweep for a refresh.
        results.anim_start = clock::now() - ZOOM_DURATION;
        refresh::log(&format!(
            "swapped in ({:?}) after {:.2}s, focus {} -> {}",
            how,
            elapsed.as_secs_f64(),
            old.tree.path_of(old.focus).display(),
            results.tree.path_of(results.focus).display()
        ));
        drop(old);
        self.context_menu = None;
        let (tree, scan_id, staged) = (results.tree.clone(), results.scan_id, !results.collector.is_empty());
        self.screen = Screen::Results(results);
        self.apply_protection();
        let kept = tree.clone();
        cx.background_spawn(async move { refresh::store(&meta, &kept, how, elapsed) }).detach();
        self.resolve_pending_findings(cx);
        self.compare_with_history(tree.clone(), scan_id, cx);
        self.find_duplicates(tree, scan_id, cx);
        if staged {
            self.collector_changed(cx);
        }
        // Search results pointed into the old tree; ask again on the new one.
        self.query_changed(cx);
        cx.notify();
    }

    pub(super) fn start_over(&mut self, _: &StartOver, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel_scan(cx);
    }

    pub(super) fn scanning(&mut self) -> Option<&mut Scanning> {
        match &mut self.screen {
            Screen::Scanning(s) => Some(s),
            _ => None,
        }
    }

    pub(super) fn live_focus(&mut self, path: Vec<SharedString>, cx: &mut Context<Self>) {
        if let Some(scanning) = self.scanning() {
            scanning.focus = path;
            scanning.hover = None;
            // Re-lay out the current snapshot straight away rather than waiting for the next one.
            if let Some(view) = scanning.live.take() {
                let mut view = Rc::try_unwrap(view).unwrap_or_else(|rc| LiveView {
                    tree: Tree { root_path: rc.tree.root_path.clone(), nodes: rc.tree.nodes.clone(), errors: 0, cloud_only: 0, index: Default::default() },
                    done: rc.done.clone(),
                    focus: rc.focus,
                    segments: rc.segments.clone(),
                    swatches: rc.swatches.clone(),
                    pending: rc.pending,
                    settled_at: rc.settled_at.clone(),
                    keys: rc.keys.clone(),
                    id: rc.id,
                });
                view.refocus(&scanning.focus);
                scanning.live = Some(Rc::new(view));
            }
            cx.notify();
        }
    }

    pub(super) fn live_hover(&mut self, path: Option<Vec<SharedString>>, cx: &mut Context<Self>) {
        if let Some(scanning) = self.scanning() {
            if scanning.hover != path {
                scanning.hover = path;
                cx.notify();
            }
        }
    }

    /// Findings whose real saving needs a clone survey (e.g. node_modules).
    pub(super) fn resolve_pending_findings(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.results() else { return };
        let pending: Vec<(u64, Vec<PathBuf>)> = r
            .findings
            .iter()
            .filter(|f| f.pending)
            .map(|finding| (finding.id, finding.nodes.iter().map(|&ix| r.tree.path_of(ix)).collect()))
            .collect();
        if pending.is_empty() {
            return;
        }
        // One task, one survey at a time, on a small pool of its own: these walks
        // can take a minute on a big home folder, and on the shared rayon pool
        // they starved every search until they were done.
        cx.spawn(async move |this, cx| {
            for (id, paths) in pending {
                let frees = cx.background_spawn(async move { survey_pool().install(|| scan::frees_of(&paths)) }).await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        if let Some(r) = this.results() {
                            // By id, not index: the list re-sorts (and can be pruned by a
                            // trash) while the clone survey runs.
                            if let Some(finding) = r.findings.iter_mut().find(|f| f.id == id) {
                                finding.size = frees;
                                finding.pending = false;
                            }
                            r.findings.sort_by(|a, b| b.size.cmp(&a.size));
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    /// Hold the results on screen to the protect list (Settings › Protected):
    /// nothing on it is offered or can be staged. Run whenever findings land
    /// and whenever the list changes.
    pub(super) fn apply_protection(&mut self) {
        let list = self.settings.protect_paths();
        if let Some(r) = self.results() {
            r.protect(&list);
        }
    }

    /// When each project folder was last built, in the background: the ones
    /// built this week stay off a ticked finding (see `Results::recent`).
    fn survey_projects(&mut self, tree: Arc<Tree>, scan_id: u64, cx: &mut Context<Self>) {
        let Some(r) = self.results().filter(|r| r.scan_id == scan_id) else { return };
        let nodes: Vec<usize> = r.findings.iter().filter(|f| findings::PROJECT_FINDINGS.contains(&f.title)).flat_map(|f| f.nodes.iter().copied()).collect();
        if nodes.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let recent = cx
                .background_spawn(async move {
                    let since = SystemTime::now() - findings::RECENT;
                    nodes.into_iter().filter_map(|ix| findings::last_built(&tree.path_of(ix)).filter(|t| *t >= since).map(|t| (ix, t))).collect::<HashMap<_, _>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(r) = this.results().filter(|r| r.scan_id == scan_id) {
                    r.recent = recent;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Look for duplicate files in the background, on the survey pool so it
    /// queues behind the clone surveys instead of starving searches; the
    /// finding joins the list when it's done.
    pub(super) fn find_duplicates(&mut self, tree: Arc<Tree>, scan_id: u64, cx: &mut Context<Self>) {
        self.survey_projects(tree.clone(), scan_id, cx);
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { survey_pool().install(|| duplicates::find(&tree)) }).await;
            let Some((finding, pairs)) = found else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(r) = this.results().filter(|r| r.scan_id == scan_id) {
                    // Only what's still in the tree: a move to the Trash may have
                    // landed while the files were being read.
                    let mut finding = finding;
                    finding.nodes.retain(|&ix| r.tree.nodes[ix].parent.is_some());
                    if !finding.nodes.is_empty() {
                        r.dupe_pairs = pairs;
                        r.findings.push(finding);
                        r.findings.sort_by(|a, b| b.size.cmp(&a.size));
                    }
                }
                if this.results().is_some_and(|r| r.scan_id == scan_id) {
                    this.apply_protection();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Record this scan and compare it with an earlier one of the same root,
    /// off the main thread; the list's growth column appears when it lands.
    pub(super) fn compare_with_history(&mut self, tree: Arc<Tree>, scan_id: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let growth = cx.background_spawn(async move { history::compare_and_record(&tree) }).await;
            let Some(growth) = growth else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(r) = this.results().filter(|r| r.scan_id == scan_id) {
                    r.growth_since = Some(growth.since);
                    r.growth = growth.by_node;
                    r.reorder();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Stat whatever the date columns and the Modified sort are waiting on, off the main thread: a
    /// folder can hold a hundred thousand entries, or sit on a slow disk.
    pub(super) fn fetch_mtimes(&mut self, cx: &mut Context<Self>) {
        let Screen::Results(r) = &mut self.screen else { return };
        if r.mtimes_loading || r.mtimes_wanted.is_empty() {
            return;
        }
        let wanted = std::mem::take(&mut r.mtimes_wanted);
        let mut dirs: HashMap<usize, PathBuf> = HashMap::new();
        let jobs: Vec<(usize, Option<PathBuf>)> = wanted
            .into_iter()
            .map(|ix| {
                let node = &r.tree.nodes[ix];
                let path = match (node.kind, node.parent) {
                    (Kind::Other, _) | (_, None) => None,
                    (_, Some(parent)) => {
                        Some(dirs.entry(parent).or_insert_with(|| r.tree.path_of(parent)).join(node.name.as_ref()))
                    }
                };
                (ix, path)
            })
            .collect();
        r.mtimes_loading = true;
        let scan_id = r.scan_id;
        cx.spawn(async move |this, cx| {
            let times: Vec<(usize, Option<SystemTime>, Option<SystemTime>)> = cx
                .background_spawn(async move {
                    jobs.into_iter()
                        .map(|(ix, path)| {
                            let meta = path.and_then(|p| std::fs::symlink_metadata(p).ok());
                            let modified = meta.as_ref().and_then(|m| m.modified().ok());
                            let created = meta.as_ref().and_then(|m| m.created().ok());
                            (ix, modified, created)
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(r) = this.results().filter(|r| r.scan_id == scan_id) else { return };
                r.mtimes_loading = false;
                for (ix, modified, created) in times {
                    r.mtimes.insert(ix, modified);
                    r.ctimes.insert(ix, created);
                }
                if r.sort == SortBy::Modified {
                    r.reorder();
                }
                cx.notify();
            });
        })
        .detach();
    }
}
