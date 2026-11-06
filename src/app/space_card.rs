//! "Where is my space?": the startup disk's ledger (see `crate::space`) in a
//! card. Every number says how it was found, the lines add up, and snapshots
//! are named for what they are. Time Machine's local snapshots can be
//! deleted from here, by a person, with an admin password; nothing else on
//! the card changes anything.

use super::*;

use crate::space::{self, How, Inputs, SnapshotKind};

pub(super) struct SpaceState {
    /// `None` while measuring; `Some(None)` when the startup disk isn't the
    /// APFS layout Broom understands.
    pub(super) inputs: Option<Option<Inputs>>,
    /// The second step before deleting snapshots is showing.
    pub(super) confirm: bool,
    pub(super) working: bool,
    /// What the last action did, or why it didn't.
    pub(super) note: Option<String>,
}

impl Broom {
    /// The tree on screen, if it's a whole-disk scan: bytes read and folders
    /// that couldn't be.
    fn whole_disk_scan(&mut self) -> (Option<u64>, u64) {
        let Some(r) = self.results() else { return (None, 0) };
        let root = &r.tree.nodes[Tree::ROOT];
        let whole = root.children.iter().any(|&c| r.tree.nodes[c].kind == Kind::Other && r.tree.nodes[c].name.as_ref() == "macOS");
        if !whole {
            return (None, 0);
        }
        let read: u64 = root.children.iter().filter(|&&c| r.tree.nodes[c].kind != Kind::Other).map(|&c| r.tree.nodes[c].size).sum();
        (Some(read), r.tree.errors)
    }

    /// View › Where Is My Space…, or the link under the startup disk.
    pub(crate) fn show_space(&mut self, _: &ShowSpace, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.review.is_some() || self.empty_trash.is_some() || self.merge.is_some() {
            return;
        }
        self.modal = None;
        self.context_menu = None;
        self.space = Some(SpaceState { inputs: None, confirm: false, working: false, note: None });
        window.focus(&self.modal_focus, cx);
        self.measure_space(None, cx);
        cx.notify();
    }

    /// Measure off the UI thread; `before`, when set, is the free space before
    /// an action, so the note can say what the action really gained.
    fn measure_space(&mut self, before: Option<(u64, String)>, cx: &mut Context<Self>) {
        let (scanned, unreadable) = self.whole_disk_scan();
        let name = self.volumes.first().map(|v| v.name.clone()).unwrap_or_else(|| "Startup Disk".into());
        cx.spawn(async move |this, cx| {
            let inputs = cx.background_spawn(async move { space::gather(name, scanned, unreadable) }).await;
            this.update(cx, |this, cx| {
                if let Some(state) = &mut this.space {
                    if let (Some((free_before, what)), Some(now)) = (&before, &inputs) {
                        let gained = now.free.saturating_sub(*free_before);
                        state.note = Some(format!("{what} Free space went from {} to {} (measured).", format_size(*free_before), format_size(now.free))
                            + if gained == 0 { " macOS may take a moment to release it." } else { "" });
                    }
                    state.inputs = Some(inputs);
                    state.working = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn close_space(&mut self, _: &CloseSpace, window: &mut Window, cx: &mut Context<Self>) {
        if self.space.as_ref().is_some_and(|s| s.working) {
            return;
        }
        if self.space.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    /// Delete the local Time Machine snapshots listed, after the second
    /// confirm. macOS asks for an administrator's password; Broom never sees
    /// it. Dates were checked to be digits and dashes when they were read.
    fn delete_snapshots(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.space else { return };
        let Some(Some(inputs)) = &state.inputs else { return };
        let dates: Vec<String> = inputs
            .snapshots
            .iter()
            .filter_map(|s| match &s.kind {
                SnapshotKind::TimeMachine { date } => Some(date.clone()),
                _ => None,
            })
            .collect();
        if dates.is_empty() || state.working {
            return;
        }
        let free_before = inputs.free;
        state.working = true;
        state.confirm = false;
        state.note = None;
        cx.spawn(async move |this, cx| {
            let n = dates.len();
            let result = cx.background_spawn(async move { crate::space::delete_time_machine_snapshots(&dates) }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        let what = format!("Deleted {} local Time Machine {}.", n, if n == 1 { "snapshot" } else { "snapshots" });
                        this.measure_space(Some((free_before, what)), cx);
                    }
                    Err(why) => {
                        if let Some(state) = &mut this.space {
                            state.working = false;
                            state.note = Some(format!("Nothing was deleted: {why}"));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_space(&self, state: &SpaceState, cx: &mut Context<Self>) -> AnyElement {
        let card = div()
            .id("space-card")
            .w(px(600.))
            .max_h(px(680.))
            .overflow_y_scroll()
            .p_4()
            .rounded(theme::radius(12.))
            .bg(rgb(colors().panel))
            .border_1()
            .border_color(rgb(colors().border))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_lg().font_family(theme::current().display).font_weight(FontWeight::SEMIBOLD).child("Where is my space?"))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors().muted))
                    .child("Every number says how Broom got it. Measured lines come from APFS or the scan; computed ones are arithmetic on those; macOS's estimates are its own and can't be checked."),
            );
        let inputs = match &state.inputs {
            None => return card.child(div().text_sm().child("Measuring the disk…")).child(self.space_buttons(state, cx)).into_any_element(),
            Some(None) => {
                return card
                    .child(div().text_sm().child("The startup disk isn't the APFS layout Broom can account for (a system volume and a data volume in one container)."))
                    .child(self.space_buttons(state, cx))
                    .into_any_element();
            }
            Some(Some(inputs)) => inputs,
        };
        let mut table = div().flex().flex_col().gap_1();
        for line in space::ledger(inputs) {
            let how_color = match line.how {
                How::Measured => colors().safe,
                How::Computed => colors().muted,
                How::Estimate | How::Unknown => colors().review,
            };
            table = table.child(
                div()
                    .flex()
                    .flex_col()
                    .when(line.depth == 1, |d| d.pl_4())
                    .when(line.depth == 2, |d| d.pl_8())
                    .when(line.depth == 0, |d| d.pt_1())
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .when(line.depth == 0, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .when(line.depth == 2, |d| d.text_xs().text_color(rgb(colors().muted)))
                                    .child(line.label),
                            )
                            .child(div().flex_none().text_xs().text_color(rgb(how_color)).child(line.how.label()))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(84.))
                                    .text_right()
                                    .font_family(theme::current().mono)
                                    .child(line.bytes.map(format_size).unwrap_or_else(|| "—".into())),
                            ),
                    )
                    .child(div().text_xs().text_color(rgb(colors().muted)).child(line.note)),
            );
        }
        let tm: Vec<&space::Snapshot> = inputs.snapshots.iter().filter(|s| matches!(s.kind, SnapshotKind::TimeMachine { .. })).collect();
        let macos = inputs.snapshots.iter().filter(|s| s.kind == SnapshotKind::MacOs).count();
        let other = inputs.snapshots.len() - tm.len() - macos;
        let mut snaps = div().flex().flex_col().gap_1().child(div().font_weight(FontWeight::SEMIBOLD).pt_1().child("Snapshots"));
        let size_unknown = format!("What each holds: {} (APFS only tells an administrator).", How::Unknown.label());
        if inputs.snapshots.is_empty() {
            snaps = snaps.child(div().text_xs().text_color(rgb(colors().muted)).child("None on this disk."));
        }
        if !tm.is_empty() {
            let dates: Vec<String> = tm
                .iter()
                .filter_map(|s| match &s.kind {
                    SnapshotKind::TimeMachine { date } => Some(date.clone()),
                    _ => None,
                })
                .collect();
            snaps = snaps
                .child(div().text_sm().child(format!(
                    "{} local Time Machine {}: {}",
                    tm.len(),
                    if tm.len() == 1 { "snapshot" } else { "snapshots" },
                    dates.join(", ")
                )))
                .child(div().text_xs().text_color(rgb(colors().muted)).child(format!(
                    "Hourly copies Time Machine keeps on this disk. macOS deletes them on its own when space runs low; they're part of \u{201c}purgeable\u{201d}. {size_unknown}"
                )));
        }
        if macos > 0 {
            snaps = snaps.child(div().text_xs().text_color(rgb(colors().muted)).child(format!(
                "{} macOS {} (software update and the sealed system). macOS manages these; Broom names them and leaves them alone.",
                macos,
                if macos == 1 { "snapshot" } else { "snapshots" }
            )));
        }
        if other > 0 {
            snaps = snaps.child(div().text_xs().text_color(rgb(colors().muted)).child(format!(
                "{other} other {} (another backup app's, say). Broom leaves them alone.",
                if other == 1 { "snapshot" } else { "snapshots" }
            )));
        }
        if state.confirm {
            snaps = snaps.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(theme::alpha(colors().danger, 0.10))
                    .border_1()
                    .border_color(theme::alpha(colors().danger, 0.35))
                    .text_xs()
                    .child(format!(
                        "Delete {} local Time Machine {}? Backups on your Time Machine disk are not touched, but you can no longer restore from these local copies. macOS will ask for an administrator password. This can't be undone.",
                        tm.len(),
                        if tm.len() == 1 { "snapshot" } else { "snapshots" }
                    )),
            );
        }
        card.child(table)
            .child(snaps)
            .when_some(state.note.clone(), |d, note| d.child(div().text_xs().font_weight(FontWeight::MEDIUM).child(note)))
            .child(self.space_buttons(state, cx))
            .into_any_element()
    }

    fn space_buttons(&self, state: &SpaceState, cx: &mut Context<Self>) -> impl IntoElement {
        let has_tm = matches!(&state.inputs, Some(Some(i)) if i.snapshots.iter().any(|s| matches!(s.kind, SnapshotKind::TimeMachine { .. })));
        div()
            .flex()
            .gap_2()
            .justify_end()
            .when(has_tm && !state.confirm && !state.working, |d| {
                d.child(button("space-delete", "Delete Time Machine Snapshots…").on_click(cx.listener(|this, _, _, cx| {
                    if let Some(s) = &mut this.space {
                        s.confirm = true;
                        cx.notify();
                    }
                })))
            })
            .when(state.confirm, |d| {
                d.child(button("space-keep", "Keep Them").on_click(cx.listener(|this, _, _, cx| {
                    if let Some(s) = &mut this.space {
                        s.confirm = false;
                        cx.notify();
                    }
                })))
                .child(primary_button("space-confirm", "Delete Snapshots", colors().danger).on_click(cx.listener(|this, _, _, cx| this.delete_snapshots(cx))))
            })
            .when(!state.confirm, |d| {
                d.child(
                    primary_button("space-done", if state.working { "Working…" } else { "Done" }, colors().accent)
                        .text_color(rgb(colors().on_accent))
                        .when(!state.working, |b| b.on_click(cx.listener(|this, _, window, cx| this.close_space(&CloseSpace, window, cx)))),
                )
            })
    }
}
