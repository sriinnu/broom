//! Empty Trash, from inside Broom. Moving things to the Trash frees nothing
//! until the Trash is emptied; this closes that loop, with the number that
//! actually came back.
//!
//! A confirm card first, because this one can't be undone: what's in the
//! Trash, how big it is, and Cancel as the default. Then every Trash Finder
//! would empty (the home one and each other volume's) is emptied off the UI
//! thread, and the banner counts up what went and how much the disk gained.
//! Without Full Disk Access Broom can't see into the Trash, so the card
//! offers to have Finder empty it instead.

use super::*;

use crate::trashcan::{Contents, Unreadable};

/// The Empty Trash card, while it's open.
pub(super) struct EmptyTrashState {
    pub(super) dirs: Vec<PathBuf>,
    /// `None` while counting.
    pub(super) contents: Option<Result<Contents, Unreadable>>,
    pub(super) working: bool,
    /// Emptied with no results on screen for the banner: the card says it.
    pub(super) done: Option<trashcan::Emptied>,
    /// Finder said no (or Broom couldn't ask).
    pub(super) failed: Option<String>,
}

impl Broom {
    /// File › Empty Trash…, ⇧⌘⌫, or the button on the after-sweep banner.
    pub(super) fn open_empty_trash(&mut self, _: &EmptyTrash, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.review.is_some() || self.empty_trash.is_some() || self.merge.is_some() || self.space.is_some() {
            return;
        }
        self.modal = None;
        self.context_menu = None;
        let volumes: Vec<PathBuf> = self.volumes.iter().skip(1).map(|v| v.path.clone()).collect();
        // Scripted runs use a fake HOME, but other volumes' Trashes are real:
        // the recorder only ever empties the home one.
        #[cfg(feature = "snapshot")]
        let volumes: Vec<PathBuf> = { let _ = volumes; Vec::new() };
        let dirs = trashcan::trash_dirs(&home_dir(), &volumes);
        self.empty_trash = Some(EmptyTrashState { dirs: dirs.clone(), contents: None, working: false, done: None, failed: None });
        window.focus(&self.modal_focus, cx);
        cx.spawn(async move |this, cx| {
            let contents = cx.background_spawn(async move { trashcan::measure(&dirs) }).await;
            this.update(cx, |this, cx| {
                if let Some(state) = &mut this.empty_trash {
                    state.contents = Some(contents);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn close_empty_trash(&mut self, _: &CloseEmptyTrash, window: &mut Window, cx: &mut Context<Self>) {
        if self.empty_trash.as_ref().is_some_and(|s| s.working) {
            return;
        }
        if self.empty_trash.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    /// For the recorder: press the card's red button.
    #[cfg(feature = "snapshot")]
    pub fn demo_confirm_empty_trash(&mut self, cx: &mut Context<Self>) {
        let with_finder = matches!(self.empty_trash.as_ref().and_then(|s| s.contents.as_ref()), Some(Err(Unreadable::NoAccess)));
        self.confirm_empty_trash(with_finder, cx);
    }

    /// The card's "Empty Trash": the only place Broom deletes for good.
    fn confirm_empty_trash(&mut self, with_finder: bool, cx: &mut Context<Self>) {
        let Some(state) = &mut self.empty_trash else { return };
        if state.working {
            return;
        }
        state.working = true;
        state.failed = None;
        let dirs = state.dirs.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let before = trashcan::free_space(&dirs);
                    let result = if with_finder {
                        trashcan::empty_with_finder().map(|()| trashcan::Emptied::default())
                    } else {
                        trashcan::empty(&dirs).map_err(|e| match e {
                            Unreadable::NoAccess => "Broom can't see into the Trash without Full Disk Access.".to_string(),
                            Unreadable::Other(why) => why,
                        })
                    };
                    // APFS hands blocks back a moment after the files go; give
                    // the free count up to two seconds to catch up.
                    let mut gain = 0;
                    if let Ok(emptied) = &result {
                        let deadline = Instant::now() + Duration::from_secs(2);
                        loop {
                            gain = trashcan::free_space(&dirs).saturating_sub(before);
                            if gain + gain / 10 >= emptied.gone.bytes || Instant::now() >= deadline {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(150));
                        }
                    }
                    (result, gain)
                })
                .await;
            this.update(cx, |this, cx| this.empty_trash_finished(outcome, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    fn empty_trash_finished(&mut self, (result, gain): (Result<trashcan::Emptied, String>, u64), cx: &mut Context<Self>) {
        let emptied = match result {
            Ok(emptied) => emptied,
            Err(why) => {
                if let Some(state) = &mut self.empty_trash {
                    state.working = false;
                    state.failed = Some(why);
                }
                cx.notify();
                return;
            }
        };
        cx.play_haptic_feedback(HapticFeedbackStyle::LevelChange);
        // Finder doesn't say what went; the disk's gain is the honest number.
        let bytes = if emptied.gone.bytes > 0 { emptied.gone.bytes } else { gain };
        let note = EmptiedNote { gain, stuck: emptied.stuck.len() };
        if let Some(r) = self.results() {
            // Nothing is left to put back.
            r.last_trashed.clear();
            self.empty_trash = None;
            self.show_banner(Some(format!("Emptied the Trash ({})", format_size(bytes))), cx);
            if let Some(r) = self.results() {
                r.freed = Some(Freed { bytes, items: emptied.gone.items, milestone: None, emptied: Some(note) });
            }
        } else if let Some(state) = &mut self.empty_trash {
            state.working = false;
            state.done = Some(trashcan::Emptied { gone: Contents { items: emptied.gone.items, bytes }, stuck: emptied.stuck });
        }
        cx.notify();
    }

    pub(super) fn render_empty_trash(&self, state: &EmptyTrashState, cx: &mut Context<Self>) -> AnyElement {
        let muted = |text: String| div().text_xs().text_color(rgb(colors().muted)).child(text);
        let (title, body, action): (&str, Vec<String>, Option<(&'static str, bool)>) = match (&state.done, &state.contents) {
            (Some(done), _) => (
                "The Trash is empty",
                vec![
                    format!("{} gone for good, {} back on the disk.", items_phrase(done.gone.items), format_size(done.gone.bytes)),
                    stuck_line(done.stuck.len()),
                ],
                None,
            ),
            (None, None) => ("Empty the Trash?", vec!["Counting what's in it…".into()], None),
            (None, Some(Ok(contents))) if contents.items == 0 => ("The Trash is already empty", vec!["Nothing to delete.".into()], None),
            (None, Some(Ok(contents))) => (
                "Empty the Trash?",
                vec![
                    format!("{} ({}) will be deleted permanently.", items_phrase(contents.items), format_size(contents.bytes)),
                    "You can't undo this. Locked items stay in the Trash.".into(),
                ],
                Some(("Empty Trash", false)),
            ),
            (None, Some(Err(Unreadable::NoAccess))) => (
                "Empty the Trash?",
                vec![
                    "Broom can't see into the Trash without Full Disk Access, so it can't say what's there.".into(),
                    "Finder can empty it for you. macOS may ask once whether Broom may do that. You can't undo this.".into(),
                ],
                Some(("Empty with Finder", true)),
            ),
            (None, Some(Err(Unreadable::Other(why)))) => ("Couldn't read the Trash", vec![why.clone()], None),
        };
        let cancel_label = if action.is_some() { "Cancel" } else { "Done" };
        div()
            .id("empty-trash-card")
            .w(px(440.))
            .p_4()
            .rounded(theme::radius(12.))
            .bg(rgb(colors().panel))
            .border_1()
            .border_color(rgb(colors().border))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_lg().font_family(theme::current().display).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().flex().flex_col().gap_1().children(body.into_iter().filter(|l| !l.is_empty()).map(|l| div().text_sm().child(l))))
            .when_some(state.failed.clone(), |d, why| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded(theme::radius(8.))
                        .bg(theme::alpha(colors().danger, 0.12))
                        .text_xs()
                        .text_color(rgb(colors().danger))
                        .child(format!("Couldn't empty the Trash: {why}")),
                )
            })
            .when(state.working, |d| d.child(muted("Emptying…".into())))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        button("empty-cancel", cancel_label)
                            .when(!state.working, |b| b.on_click(cx.listener(|this, _, window, cx| this.close_empty_trash(&CloseEmptyTrash, window, cx)))),
                    )
                    .when_some(action, |d, (label, with_finder)| {
                        d.child(
                            primary_button("empty-confirm", if state.working { "Emptying…" } else { label }, colors().danger)
                                .when(state.working, |b| b.opacity(0.6))
                                .when(!state.working, |b| b.on_click(cx.listener(move |this, _, _, cx| this.confirm_empty_trash(with_finder, cx)))),
                        )
                    }),
            )
            .into_any_element()
    }
}

/// What the after-empty banner adds to `Freed`.
#[derive(Clone, Copy, Debug)]
pub(super) struct EmptiedNote {
    /// How much the disk's free space actually grew.
    pub(super) gain: u64,
    /// Items left in the Trash.
    pub(super) stuck: usize,
}

pub(super) fn items_phrase(n: usize) -> String {
    if n == 1 { "1 item".to_string() } else { format!("{} items", format_count(n as u64)) }
}

pub(super) fn stuck_line(stuck: usize) -> String {
    match stuck {
        0 => String::new(),
        1 => "1 item stayed in the Trash (locked or in use).".into(),
        n => format!("{n} items stayed in the Trash (locked or in use)."),
    }
}

/// The banner's second line after an empty: the disk's gain, said honestly.
/// APFS can hold blocks back (local Time Machine snapshots, clones shared
/// with files outside the Trash), so a gain well short of what went says so.
pub(super) fn emptied_detail(bytes: u64, note: EmptiedNote) -> String {
    let disk = if note.gain + note.gain / 10 >= bytes || bytes == 0 {
        "The space is back on the disk.".to_string()
    } else if note.gain > 0 {
        format!("The disk has {} more free so far; macOS releases the rest as it can.", format_size(note.gain))
    } else {
        "macOS releases the space as it can (local snapshots may hold it a while).".to_string()
    };
    let stuck = stuck_line(note.stuck);
    if stuck.is_empty() { disk } else { format!("{disk} {stuck}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_says_how_much_really_came_back() {
        let note = |gain, stuck| EmptiedNote { gain, stuck };
        assert_eq!(emptied_detail(1_000_000, note(1_000_000, 0)), "The space is back on the disk.");
        assert_eq!(emptied_detail(1_000_000, note(950_000, 0)), "The space is back on the disk.");
        assert!(emptied_detail(10_000_000, note(2_000_000, 0)).starts_with("The disk has 2"));
        assert!(emptied_detail(10_000_000, note(0, 0)).contains("snapshots"));
        assert!(emptied_detail(10, note(10, 2)).ends_with("2 items stayed in the Trash (locked or in use)."));
    }
}
