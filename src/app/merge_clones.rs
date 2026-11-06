//! Merge as Clones: the "Duplicate files" finding's other way out. Instead of
//! sending extra copies to the Trash, each is replaced in place by an APFS
//! clone of the copy that stays (see `duplicates::merge`). Nothing is deleted
//! and nothing moves; the space comes back at once, no Trash to empty.

use super::*;

use crate::duplicates::{Pair, Skip};

/// The confirm card, while it's open.
pub(super) struct MergeState {
    pub(super) pairs: Vec<Pair>,
    pub(super) working: bool,
    /// Copies done so far, for the button while it runs.
    pub(super) done: Arc<std::sync::atomic::AtomicUsize>,
}

/// The finding the merge belongs to.
pub(super) const DUPLICATES: &str = "Duplicate files";

/// What a run came to.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Merged {
    pub(super) merged: Vec<PathBuf>,
    pub(super) freed: u64,
    pub(super) changed: usize,
    pub(super) no_clone: usize,
    pub(super) locked: usize,
    pub(super) failed: Vec<String>,
}

impl Merged {
    pub(super) fn tally(&mut self, pair: &Pair, outcome: Result<u64, Skip>) {
        match outcome {
            Ok(freed) => {
                self.merged.push(pair.extra.clone());
                self.freed += freed;
            }
            Err(Skip::Changed) => self.changed += 1,
            Err(Skip::NoClone) => self.no_clone += 1,
            Err(Skip::Locked) => self.locked += 1,
            Err(Skip::Failed(why)) => self.failed.push(why),
        }
    }

    /// The banner: a headline, then a line on anything left alone.
    pub(super) fn banner(&self) -> String {
        let n = self.merged.len();
        let head = match n {
            0 => "No duplicates merged".to_string(),
            1 => format!("Merged 1 duplicate into a clone ({} freed)", format_size(self.freed)),
            n => format!("Merged {n} duplicates into clones ({} freed)", format_size(self.freed)),
        };
        let mut left = Vec::new();
        let count = |n: usize, what: &str| (n > 0).then(|| format!("{n} {what}"));
        left.extend(count(self.changed, "changed since the scan"));
        left.extend(count(self.no_clone, "on a disk without clones"));
        left.extend(count(self.locked, "locked or hard-linked"));
        left.extend(count(self.failed.len(), "failed"));
        let detail = if left.is_empty() {
            "Every file is where it was; the bytes are stored once.".to_string()
        } else {
            format!("Left as they were: {}.", left.join(", "))
        };
        format!("{head}\n{detail}")
    }
}

impl Broom {
    /// The finding's "Merge as Clones…".
    pub(super) fn open_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.review.is_some() || self.empty_trash.is_some() || self.merge.is_some() || self.space.is_some() {
            return;
        }
        let Some(r) = self.results() else { return };
        // Only the copies the finding still lists: some may have been trashed.
        let listed: HashSet<PathBuf> = r
            .findings
            .iter()
            .filter(|f| f.title == DUPLICATES)
            .flat_map(|f| f.nodes.iter().filter(|&&ix| r.tree.nodes[ix].parent.is_some()).map(|&ix| r.tree.path_of(ix)))
            .collect();
        let pairs: Vec<Pair> = r.dupe_pairs.iter().filter(|p| listed.contains(&p.extra)).cloned().collect();
        if pairs.is_empty() {
            return;
        }
        self.modal = None;
        self.context_menu = None;
        self.merge = Some(MergeState { pairs, working: false, done: Arc::default() });
        window.focus(&self.modal_focus, cx);
        cx.notify();
    }

    pub(super) fn close_merge(&mut self, _: &CloseMerge, window: &mut Window, cx: &mut Context<Self>) {
        if self.merge.as_ref().is_some_and(|m| m.working) {
            return;
        }
        if self.merge.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    fn confirm_merge(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.merge else { return };
        if state.working {
            return;
        }
        state.working = true;
        let (pairs, done) = (state.pairs.clone(), state.done.clone());
        // Repaint as the count climbs.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let open = this.update(cx, |this, cx| {
                    cx.notify();
                    this.merge.as_ref().is_some_and(|m| m.working)
                });
                if !open.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let (merged, volumes) = cx
                .background_spawn(async move {
                    let mut merged = Merged::default();
                    for pair in &pairs {
                        merged.tally(pair, duplicates::merge(pair));
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                    (merged, scan::volumes())
                })
                .await;
            this.update(cx, |this, cx| this.merge_finished(merged, volumes, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    fn merge_finished(&mut self, merged: Merged, volumes: Vec<Volume>, cx: &mut Context<Self>) {
        self.merge = None;
        if let Some(bar) = &self.menubar {
            bar.update(startup_fraction(&volumes), &volume_lines(&volumes));
        }
        self.volumes = volumes;
        if !merged.merged.is_empty() {
            cx.play_haptic_feedback(HapticFeedbackStyle::LevelChange);
        }
        if let Some(r) = self.results() {
            // Merged copies aren't duplicates any more: off the finding, out
            // of the plan, and the finding goes once it's empty.
            let gone: HashSet<&PathBuf> = merged.merged.iter().collect();
            let nodes: HashSet<usize> = gone.iter().filter_map(|p| r.tree.find(p)).collect();
            let freed_by: HashMap<&PathBuf, u64> = r.dupe_pairs.iter().map(|p| (&p.extra, p.frees)).collect();
            let shrink: u64 = gone.iter().filter_map(|p| freed_by.get(p)).sum();
            for f in r.findings.iter_mut().filter(|f| f.title == DUPLICATES) {
                f.nodes.retain(|ix| !nodes.contains(ix));
                f.size = f.size.saturating_sub(shrink);
                f.blurb = format!("{} extra {}; one of each stays", f.nodes.len(), if f.nodes.len() == 1 { "copy" } else { "copies" });
            }
            r.findings.retain(|f| f.title != DUPLICATES || !f.nodes.is_empty());
            r.dupe_pairs.retain(|p| !gone.contains(&p.extra));
            r.collector.retain(|ix| !nodes.contains(ix));
        }
        self.collector_changed(cx);
        self.show_banner(Some(merged.banner()), cx);
        cx.notify();
    }

    pub(super) fn render_merge(&self, state: &MergeState, cx: &mut Context<Self>) -> AnyElement {
        let n = state.pairs.len();
        let frees: u64 = state.pairs.iter().map(|p| p.frees).sum();
        let copies = if n == 1 { "1 extra copy".to_string() } else { format!("{} extra copies", format_count(n as u64)) };
        let done = state.done.load(Ordering::Relaxed);
        div()
            .id("merge-card")
            .w(px(460.))
            .p_4()
            .rounded(theme::radius(12.))
            .bg(rgb(colors().panel))
            .border_1()
            .border_color(rgb(colors().border))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_lg().font_family(theme::current().display).font_weight(FontWeight::SEMIBOLD).child("Merge duplicates as clones?"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_sm()
                    .child(format!("{copies} will become APFS clones of the copy that stays, freeing about {}.", format_size(frees)))
                    .child("Every file keeps its place, name, dates and tags. Nothing is deleted or moved."),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors().muted))
                    .child("The bytes are stored once. If you later edit one of the files, only what changes takes new space."),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_end()
                    .child(button("merge-cancel", "Cancel").when(!state.working, |b| b.on_click(cx.listener(|this, _, window, cx| this.close_merge(&CloseMerge, window, cx)))))
                    .child(
                        primary_button("merge-confirm", if state.working { format!("Merging {done} of {n}…") } else { "Merge".to_string() }, colors().accent)
                            .text_color(rgb(colors().on_accent))
                            .when(state.working, |b| b.opacity(0.7))
                            .when(!state.working, |b| b.on_click(cx.listener(|this, _, _, cx| this.confirm_merge(cx)))),
                    ),
            )
            .into_any_element()
    }

    /// For the recorder: press Merge on the open card.
    #[cfg(feature = "snapshot")]
    pub fn demo_confirm_merge(&mut self, cx: &mut Context<Self>) {
        self.confirm_merge(cx);
    }

    /// For the recorder: open the card from the finding.
    #[cfg(feature = "snapshot")]
    pub fn demo_open_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_merge(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(name: &str) -> Pair {
        Pair { extra: PathBuf::from(name), keep: PathBuf::from("/keep"), frees: 0 }
    }

    #[test]
    fn the_banner_counts_what_merged_and_what_was_left() {
        let mut m = Merged::default();
        m.tally(&pair("/a"), Ok(3_000_000));
        m.tally(&pair("/b"), Ok(2_000_000));
        m.tally(&pair("/c"), Err(Skip::Changed));
        m.tally(&pair("/d"), Err(Skip::Locked));
        let text = m.banner();
        assert!(text.starts_with("Merged 2 duplicates into clones (5"), "{text}");
        assert!(text.ends_with("Left as they were: 1 changed since the scan, 1 locked or hard-linked."), "{text}");
        let mut clean = Merged::default();
        clean.tally(&pair("/a"), Ok(1));
        assert!(clean.banner().ends_with("the bytes are stored once."));
    }
}
