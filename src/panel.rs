//! The menu-bar panel: click the 🧹, type what you want gone ("node_modules",
//! "dist folders", "git worktrees", "old dmg files"), tick the matches, move
//! them to the Trash. A small pop-up window under the status item; it reads the
//! main view's scan (or starts a home-folder scan when there is none) and hands
//! deletions back to it so every total stays right.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, FontWeight, MouseButton, ScrollHandle, SharedString, Task, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, actions, div, point, prelude::*,
    px, rgb, size,
};
use gpui_ce_elements::editable_text::{EditableTextState, StringStorage, TextChanged, text_input};

use crate::app::{self, ScanState, Snapshot};
use crate::comic;
use crate::trashcan;
use crate::ask::{self, CandidateGroup, Intent, ProviderError, Scope};
use crate::findings::Safety;
use crate::scan::{self, Kind, format_count, format_size};

actions!(panel, [ClosePanel]);

const WIDTH: f32 = 460.;
const HEIGHT: f32 = 540.;
/// Rows shown per expanded group before "+ N more".
const MAX_ROWS: usize = 40;

const BG: u32 = 0x1c1d21;
const CARD: u32 = 0x25272d;
const CARD_HOVER: u32 = 0x33363c;
const BORDER: u32 = 0x34363c;
const TEXT: u32 = 0xe8e9ec;
const MUTED: u32 = 0x8d919a;
const DANGER: u32 = 0xe5484d;
const SAFE: u32 = 0x3fb950;
const REVIEW: u32 = 0xd29922;

thread_local! {
    static PANEL: std::cell::RefCell<Option<WindowHandle<Panel>>> = const { std::cell::RefCell::new(None) };
    /// When the panel last closed itself on losing focus. A click on the 🧹
    /// takes focus first, so the toggle that follows must not reopen it.
    static CLOSED_AT: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// Open the panel under the 🧹, or close it if it is up. Called on the app
/// thread from the menu-bar request channel.
pub fn toggle(cx: &mut App) {
    let open = PANEL.with(|p| *p.borrow());
    if let Some(handle) = open {
        let alive = gpui::AnyWindowHandle::from(handle).update(cx, |_, window, _| window.remove_window()).is_ok();
        PANEL.with(|p| *p.borrow_mut() = None);
        if alive {
            return;
        }
    }
    if CLOSED_AT.with(|c| c.get()).is_some_and(|t| t.elapsed() < Duration::from_millis(400)) {
        CLOSED_AT.with(|c| c.set(None));
        return;
    }
    let Some(broom) = app::broom() else { return };
    let anchor = broom.read(cx).menubar_anchor();
    let screen = cx.primary_display().map(|d| d.bounds());
    let (x, y) = match anchor {
        Some((cx_, bottom, _)) => (cx_ - WIDTH / 2.0, bottom + 6.0),
        None => (screen.map(|b| f32::from(b.size.width) - WIDTH - 16.0).unwrap_or(400.0), 30.0),
    };
    let x = match screen {
        Some(b) => x.clamp(8.0, (f32::from(b.size.width) - WIDTH - 8.0).max(8.0)),
        None => x.max(8.0),
    };
    let bounds = Bounds::new(point(px(x), px(y)), size(px(WIDTH), px(HEIGHT)));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let debug = std::env::var_os("BROOM_MB_DEBUG").is_some();
    if debug {
        eprintln!("panel: anchor {anchor:?} → bounds {bounds:?}");
    }
    let handle = cx.open_window(options, |window, cx| cx.new(|cx| Panel::new(window, cx)));
    match handle {
        Ok(handle) => {
            PANEL.with(|p| *p.borrow_mut() = Some(handle));
            // Take the field's entity out of the lease before touching it: an
            // edit emits, and the subscriber updates the panel.
            let query = handle.read(cx).ok().map(|panel| panel.query.clone());
            if let Some(query) = query {
                let input = query.read(cx).focus_handle(cx);
                let _ = handle.update(cx, |_, window, cx| window.focus(&input, cx));
                // Dev aid: `BROOM_PANEL_QUERY="node_modules"` types for the screenshot.
                if let Ok(text) = std::env::var("BROOM_PANEL_QUERY") {
                    query.update(cx, |state, cx| state.emplace(&text, cx));
                }
            }
        }
        Err(error) => eprintln!("panel: could not open: {error:#}"),
    }
}

/// Dev aid (`snapshot` feature): fill the panel's field as if typed.
#[cfg(feature = "snapshot")]
pub fn type_query(cx: &mut App, text: &str) {
    if let Some(handle) = PANEL.with(|p| *p.borrow()) {
        let query = handle.read(cx).ok().map(|panel| panel.query.clone());
        if let Some(query) = query {
            query.update(cx, |state, cx| state.emplace(text, cx));
        }
    }
}

/// Dev aid (`snapshot` feature): run something against the panel's window.
#[cfg(feature = "snapshot")]
pub fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App)) {
    if let Some(handle) = PANEL.with(|p| *p.borrow()) {
        let _ = gpui::AnyWindowHandle::from(handle).update(cx, |_, window, cx| f(window, cx));
    }
}

/// Dev aid (`snapshot` feature): the panel's pixels, if it is open.
#[cfg(feature = "snapshot")]
pub fn capture(cx: &mut App) -> Option<Result<image::RgbaImage, String>> {
    let handle = PANEL.with(|p| *p.borrow())?;
    // The window-level handle: drawing runs the panel's render, which must not
    // happen inside the panel's own update lease.
    gpui::AnyWindowHandle::from(handle)
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            window.render_to_image().map_err(|e| format!("{e:#}"))
        })
        .ok()
}

fn close(cx: &mut App) {
    if let Some(handle) = PANEL.with(|p| p.borrow_mut().take()) {
        let _ = gpui::AnyWindowHandle::from(handle).update(cx, |_, window, _| window.remove_window());
    }
}

/// What the panel is doing, for the status line.
enum Phase {
    /// Ready: results shown (possibly none).
    Idle,
    /// Enter was pressed and the providers are reading the words.
    Thinking,
    /// A provider's reading of the request, or why there was none.
    Note(SharedString),
    /// Items are on their way to the Trash.
    Trashing,
    /// The last deletion, summarised.
    Done(SharedString),
}

pub struct Panel {
    focus: FocusHandle,
    query: Entity<EditableTextState>,
    snapshot: Option<Snapshot>,
    scan: ScanState,
    groups: Arc<Vec<CandidateGroup>>,
    /// Node indices ticked for deletion.
    checked: HashSet<usize>,
    /// Group indices whose items are listed.
    expanded: HashSet<usize>,
    /// What deleting the ticked items really frees (clones counted once);
    /// `None` while being worked out.
    frees: Option<u64>,
    frees_version: u64,
    search_version: u64,
    phase: Phase,
    /// The inline "really?" step before a deletion.
    confirming: bool,
    /// The items tumbling into the bin while a move runs.
    drop: Option<Rc<trashcan::Drop>>,
    /// A search is in flight: the list below is for the previous words.
    searching: bool,
    was_active: bool,
    last_active: bool,
    opened: std::time::Instant,
    scroll: ScrollHandle,
    _watch: Task<()>,
}

impl Panel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let query = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
        cx.subscribe(&query, |this, _, _: &TextChanged, cx| this.search(cx)).detach();
        let mut this = Self {
            focus,
            query,
            snapshot: None,
            scan: ScanState::Nothing,
            groups: Arc::default(),
            checked: HashSet::new(),
            expanded: HashSet::new(),
            frees: Some(0),
            frees_version: 0,
            search_version: 0,
            phase: Phase::Idle,
            confirming: false,
            drop: None,
            searching: false,
            was_active: false,
            last_active: false,
            opened: std::time::Instant::now(),
            scroll: ScrollHandle::new(),
            _watch: Task::ready(()),
        };
        if let Some(broom) = app::broom() {
            broom.update(cx, |broom, cx| {
                if matches!(broom.scan_state(), ScanState::Nothing) {
                    broom.ensure_scan(cx);
                }
            });
        }
        this.sync(cx);
        this._watch = this.watch(cx);
        let _ = window;
        this
    }

    /// Follow the main view: pick up a finished scan, show progress while one
    /// runs, start one when there is none to search.
    fn watch(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(300)).await;
                if this.update(cx, |this, cx| this.sync(cx)).is_err() {
                    break;
                }
            }
        })
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(broom) = app::broom() else { return };
        let (snapshot, scan) = broom.read_with(cx, |broom, _| (broom.snapshot(), broom.scan_state()));
        let changed = match (&self.snapshot, &snapshot) {
            (Some(a), Some(b)) => !Arc::ptr_eq(&a.tree, &b.tree),
            (None, None) => false,
            _ => true,
        };
        self.scan = scan;
        if changed {
            // A different tree means different indices: nothing from the old one
            // may be drawn or ticked against it, not even for one frame.
            self.snapshot = snapshot;
            self.groups = Arc::default();
            self.checked.clear();
            self.expanded.clear();
            self.frees = Some(0);
            self.frees_version += 1;
            self.confirming = false;
            self.search(cx);
        }
        cx.notify();
    }

    fn text(&self, cx: &App) -> String {
        self.query.read(cx).as_str().trim().to_string()
    }

    /// Exact matches for the field's words, off the UI thread.
    fn search(&mut self, cx: &mut Context<Self>) {
        self.search_version += 1;
        let version = self.search_version;
        self.confirming = false;
        if !matches!(self.phase, Phase::Done(_)) {
            self.phase = Phase::Idle;
        }
        let query = self.text(cx);
        let Some(snapshot) = &self.snapshot else { return };
        let (tree, findings) = (snapshot.tree.clone(), snapshot.findings.clone());
        self.searching = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(120)).await;
            // No words: the findings alone, as suggestions. Words: the findings the
            // words name, plus every exact match.
            let debug = std::env::var_os("BROOM_MB_DEBUG").is_some();
            let started = std::time::Instant::now();
            let shown = query.clone();
            let (groups, inner) = cx
                .background_spawn(async move {
                    let t0 = std::time::Instant::now();
                    let groups = ask::candidates(&tree, &findings, &query);
                    (groups, t0.elapsed())
                })
                .await;
            if debug {
                eprintln!("panel: search {shown:?} → {} groups in {:?} (matching itself {inner:?})", groups.len(), started.elapsed());
            }
            this.update(cx, |this, cx| {
                if this.search_version != version {
                    if debug {
                        eprintln!("panel: search {shown:?} dropped (version {} ≠ {})", version, this.search_version);
                    }
                    return;
                }
                let listed: HashSet<usize> = groups.iter().flat_map(|g| g.nodes.iter().copied()).collect();
                this.checked.retain(|ix| listed.contains(ix));
                this.expanded.retain(|&g| g < groups.len());
                this.groups = Arc::new(groups);
                this.searching = false;
                this.frees_changed(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Enter: let the providers read the words and tick what they point at.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let query = self.text(cx);
        let Some(snapshot) = &self.snapshot else { return };
        if query.is_empty() || matches!(self.phase, Phase::Thinking) {
            return;
        }
        self.phase = Phase::Thinking;
        self.search_version += 1;
        let version = self.search_version;
        let (tree, findings, root) = (snapshot.tree.clone(), snapshot.findings.clone(), snapshot.root.clone());
        cx.spawn(async move |this, cx| {
            let (groups, answer) = cx
                .background_spawn(async move {
                    let groups = ask::candidates(&tree, &findings, &query);
                    if groups.is_empty() {
                        (groups, Err(ProviderError::BadResponse("no exact matches".into())))
                    } else {
                        let ctx = ask::AskContext { scan_root: root, focused: None };
                        let answer = ask::resolve(&ask::providers(), &query, &ctx, &groups);
                        (groups, answer)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.search_version != version {
                    return;
                }
                let listed: HashSet<usize> = groups.iter().flat_map(|g| g.nodes.iter().copied()).collect();
                this.checked.retain(|ix| listed.contains(ix));
                this.expanded.clear();
                this.groups = Arc::new(groups);
                this.apply(answer, cx);
            })
            .ok();
        })
        .detach();
    }

    fn apply(&mut self, answer: Result<ask::Understanding, ProviderError>, cx: &mut Context<Self>) {
        let note: String = match answer {
            Ok(u) if u.group != usize::MAX => {
                let mut chosen = u.groups.clone();
                if !chosen.contains(&u.group) {
                    chosen.insert(0, u.group);
                }
                let mut ticked = Vec::new();
                let mut shown = Vec::new();
                for gix in chosen {
                    if gix < self.groups.len() {
                        let name = self.groups[gix].label.split(" · ").next().unwrap_or("").to_string();
                        // Only precise matches get ticked on a model's word; a substring
                        // match ("downloads", "xcode") is opened for you to look at.
                        if u.intent == Intent::Delete && self.groups[gix].source.auto_tick() {
                            self.set_group(gix, true);
                            ticked.push(name);
                        } else {
                            shown.push(name);
                        }
                        self.expanded.insert(gix);
                    }
                }
                match u.intent {
                    Intent::Delete if ticked.is_empty() => format!("Read that as delete: {}. Opened below for you to tick; a name match is never ticked on its own.", shown.join(", ")),
                    Intent::Delete if shown.is_empty() => format!("Read that as delete: {}. Ticked below; nothing moves until you say so.", ticked.join(", ")),
                    Intent::Delete => format!("Read that as delete: {} ticked, {} opened for you to look at. Nothing moves until you say so.", ticked.join(", "), shown.join(", ")),
                    Intent::Locate => format!("Read that as find: {}.", ticked.iter().chain(shown.iter()).cloned().collect::<Vec<_>>().join(", ")),
                }
            }
            Ok(_) | Err(ProviderError::BadResponse(_)) => {
                if self.groups.is_empty() {
                    format!("Nothing in {} matches that.", self.root_name())
                } else {
                    "Not sure which you meant. Tick what you want below.".into()
                }
            }
            Err(ProviderError::NoKey) => "No reader is set up (Settings › Keys), so these are exact matches only.".into(),
            Err(ProviderError::Unauthorized) => "The reader's key was rejected; exact matches only.".into(),
            Err(ProviderError::Http(_)) => "Couldn't reach the reader; exact matches only.".into(),
        };
        self.phase = Phase::Note(note.into());
        self.frees_changed(cx);
        cx.notify();
    }

    fn root_name(&self) -> String {
        let Some(s) = &self.snapshot else { return "this scan".into() };
        if s.root == app::home_dir() { "your home folder".into() } else { scan::display_name(&s.root) }
    }

    /// The nodes of a group that may be deleted at all (never a home folder or
    /// an APFS volume slice).
    fn deletable(&self, gix: usize) -> Vec<usize> {
        let (Some(snapshot), Some(group)) = (&self.snapshot, self.groups.get(gix)) else { return Vec::new() };
        ask::collectable(&snapshot.tree, &snapshot.homes, group, Scope::WholeTree, 0)
    }

    fn set_group(&mut self, gix: usize, on: bool) {
        for ix in self.deletable(gix) {
            if on {
                self.checked.insert(ix);
            } else {
                self.checked.remove(&ix);
            }
        }
    }

    fn toggle_group(&mut self, gix: usize, cx: &mut Context<Self>) {
        let all = self.deletable(gix);
        let every = !all.is_empty() && all.iter().all(|ix| self.checked.contains(ix));
        self.set_group(gix, !every);
        self.frees_changed(cx);
    }

    fn toggle_node(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.checked.remove(&ix) {
            self.checked.insert(ix);
        }
        self.frees_changed(cx);
    }

    /// Ticked nodes with any ticked ancestor dropped: deleting the parent
    /// takes the child with it, and counting both would overstate the saving.
    fn selection(&self) -> Vec<usize> {
        let Some(snapshot) = &self.snapshot else { return Vec::new() };
        let tree = &snapshot.tree;
        let mut out: Vec<usize> = self
            .checked
            .iter()
            .copied()
            .filter(|&ix| tree.nodes[ix].parent.is_some())
            .filter(|&ix| !self.checked.iter().any(|&o| o != ix && tree.is_ancestor_or_self(o, ix)))
            .collect();
        out.sort_by(|&a, &b| tree.nodes[b].size.cmp(&tree.nodes[a].size));
        out
    }

    fn selected_size(&self) -> u64 {
        let Some(snapshot) = &self.snapshot else { return 0 };
        self.selection().iter().map(|&ix| snapshot.tree.nodes[ix].size).sum()
    }

    fn frees_changed(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        self.frees_version += 1;
        let version = self.frees_version;
        let Some(snapshot) = &self.snapshot else { return };
        let paths: Vec<PathBuf> = self.selection().iter().map(|&ix| snapshot.tree.path_of(ix)).collect();
        if paths.is_empty() {
            self.frees = Some(0);
            return;
        }
        self.frees = None;
        cx.spawn(async move |this, cx| {
            // Quick successive ticks start one survey, not one per tick.
            cx.background_executor().timer(Duration::from_millis(300)).await;
            if this.read_with(cx, |this, _| this.frees_version != version).unwrap_or(true) {
                return;
            }
            let frees = cx.background_spawn(async move { scan::frees_of(&paths) }).await;
            this.update(cx, |this, cx| {
                if this.frees_version == version {
                    this.frees = Some(frees);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Move the selection to the Trash and tell the main view.
    fn trash(&mut self, cx: &mut Context<Self>) {
        let items = self.selection();
        let Some(snapshot) = &self.snapshot else { return };
        if items.is_empty() || matches!(self.phase, Phase::Trashing) {
            return;
        }
        let paths: Vec<PathBuf> = items.iter().map(|&ix| snapshot.tree.path_of(ix)).collect();
        let paths_after = paths.clone();
        let tree = snapshot.tree.clone();
        if std::env::var_os("BROOM_MB_DEBUG").is_some() {
            eprintln!("panel: trashing {paths:?}");
        }
        let expected = self.frees.unwrap_or_else(|| self.selected_size());
        let sizes: Vec<(Kind, u64)> = items.iter().map(|&ix| (tree.nodes[ix].kind, tree.nodes[ix].size)).collect();
        let drop = trashcan::Drop::new(trashcan::Item::from_sizes(&sizes), expected);
        self.drop = Some(drop.clone());
        self.phase = Phase::Trashing;
        self.confirming = false;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { trashcan::send(&paths) }).await;
            if result.is_ok() {
                // Let the last item land and the lid close before the list comes back.
                drop.settle();
                cx.background_executor().timer(drop.remaining()).await;
            }
            this.update(cx, |this, cx| {
                this.drop = None;
                match result {
                    Ok(()) => {
                        let count = items.len();
                        // Only if the main view still holds the same tree: otherwise its
                        // indices mean other nodes, and its own rescan will catch up.
                        if let Some(broom) = app::broom() {
                            broom.update(cx, |broom, cx| {
                                if broom.snapshot().is_some_and(|s| Arc::ptr_eq(&s.tree, &tree)) {
                                    broom.remove_trashed(&items, cx);
                                }
                            });
                        }
                        this.checked.clear();
                        this.phase = Phase::Done(
                            format!(
                                "Moved {count} item{} ({}) to the Trash. Empty the Trash to get the space back, or put them back from Finder.",
                                if count == 1 { "" } else { "s" },
                                format_size(expected)
                            )
                            .into(),
                        );
                        cx.play_haptic_feedback(gpui::HapticFeedbackStyle::LevelChange);
                        this.sync(cx);
                    }
                    Err(error) => {
                        // Not atomic: whatever did go leaves the tree, the rest stays ticked.
                        let gone: Vec<usize> = items.iter().copied().zip(&paths_after).filter(|(_, p)| !p.exists()).map(|(ix, _)| ix).collect();
                        if !gone.is_empty() {
                            if let Some(broom) = app::broom() {
                                broom.update(cx, |broom, cx| {
                                    if broom.snapshot().is_some_and(|s| Arc::ptr_eq(&s.tree, &tree)) {
                                        broom.remove_trashed(&gone, cx);
                                    }
                                });
                            }
                            for ix in &gone {
                                this.checked.remove(ix);
                            }
                        }
                        this.phase = Phase::Note(format!("Couldn't move everything to the Trash: {error}. {} of {} went.", gone.len(), items.len()).into());
                        this.sync(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn close(&mut self, _: &ClosePanel, _: &mut Window, cx: &mut Context<Self>) {
        cx.defer(close);
    }

    fn submit_action(&mut self, _: &crate::app::SubmitQuery, _: &mut Window, cx: &mut Context<Self>) {
        self.submit(cx);
    }

    // ------------------------------------------------------------ rendering

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status: SharedString = match &self.scan {
            ScanState::Nothing => "Starting a scan of your home folder…".into(),
            ScanState::Scanning { files, bytes, current } => {
                format!("Scanning {} · {} files · {}", current, format_count(*files), format_size(*bytes)).into()
            }
            ScanState::Ready => match &self.snapshot {
                Some(s) => format!("{} · {}", self.root_name(), format_size(s.tree.nodes[0].size)).into(),
                None => "".into(),
            },
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .pt_3()
            .pb_2()
            .child(div().text_base().child("🧹"))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Broom"))
            .child(div().flex_1().min_w_0().text_xs().text_color(rgb(MUTED)).truncate().text_right().child(status))
            .child(
                div()
                    .id("open-main")
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .cursor_pointer()
                    .hover(|s| s.text_color(rgb(TEXT)))
                    .child("Open ↗")
                    .on_click(cx.listener(|_, _, _, cx| {
                        app::open_requested();
                        cx.defer(close);
                    })),
            )
    }

    fn render_search(&self) -> impl IntoElement {
        div().px_3().pb_2().child(
            text_input("broom-query")
                .placeholder("node_modules, dist, worktrees, old dmg…")
                .state(self.query.downgrade())
                .caret_blink_interval_500ms()
                .w_full()
                .h(px(34.))
                .px_3()
                .min_h_auto()
                .flex()
                .items_center()
                .rounded_lg()
                .border_1()
                .border_color(rgb(BORDER))
                .bg(rgb(CARD))
                .text_size(px(13.)),
        )
    }

    fn render_note(&self) -> Option<impl IntoElement> {
        if self.searching && matches!(self.phase, Phase::Idle) {
            return Some(div().px_4().pb_2().text_xs().text_color(rgb(MUTED)).child(SharedString::from("Searching…")));
        }
        let (text, color) = match &self.phase {
            Phase::Idle => return None,
            Phase::Thinking => ("Sending your words (not your files) to the reader…".into(), MUTED),
            Phase::Note(n) => (n.clone(), MUTED),
            Phase::Trashing => ("Moving to the Trash…".into(), MUTED),
            Phase::Done(n) => (n.clone(), SAFE),
        };
        Some(div().px_4().pb_2().text_xs().text_color(rgb(color)).child(text))
    }

    fn render_empty(&self, cx: &App) -> impl IntoElement {
        if self.snapshot.is_none() {
            let (caption, files, bytes) = match &self.scan {
                ScanState::Scanning { files, bytes, current } => (comic::caption(current, *files), *files, *bytes),
                _ => (
                    comic::Caption { line: "Grabbing the magnifier…".into(), place: "your home folder is next".into(), aside: "".into() },
                    0,
                    0,
                ),
            };
            return div()
                .px_2()
                .py_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(comic::investigator(caption, None, 64.0))
                .child(div().px_2().text_xs().text_color(rgb(MUTED)).text_center().child(if files > 0 {
                    format!("{} files · {} read so far. Results land the moment the scan is done.", format_count(files), format_size(bytes))
                } else {
                    "The first answer needs one read of your home folder.".to_string()
                }));
        }
        let text: SharedString = if self.text(cx).is_empty() {
            "Type what you want gone. Plain words work: “node modules”, “dist folders”, “git worktrees”, “dmg installers”, “caches”.".into()
        } else if matches!(self.phase, Phase::Thinking) {
            "".into()
        } else {
            format!("Nothing in {} is called that. Try another word, or press Enter to let the reader match it against the known hogs.", self.root_name()).into()
        };
        div().px_4().py_6().text_sm().text_color(rgb(MUTED)).text_center().child(text)
    }

    fn render_groups(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(snapshot) = &self.snapshot else { return div() };
        let tree = snapshot.tree.clone();
        let mut list = div().flex().flex_col().gap_1();
        if self.text(cx).is_empty() {
            list = list.child(div().px_1().pb_1().text_xs().text_color(rgb(MUTED)).child("Known space hogs. Or type what you're after."));
        }
        for (gix, group) in self.groups.iter().enumerate() {
            let deletable = self.deletable(gix);
            let on = deletable.iter().filter(|ix| self.checked.contains(ix)).count();
            let state = if on == 0 { Check::Off } else if on == deletable.len() { Check::On } else { Check::Some };
            let expanded = self.expanded.contains(&gix);
            let (pill, color) = match group.safety {
                Safety::Safe => ("safe", SAFE),
                Safety::Review => ("review", REVIEW),
            };
            // "node_modules · 2.1 GB in 14 places" → name, then the numbers muted.
            let (name, places) = group.label.split_once(" · ").map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((group.label.to_string(), String::new()));
            let mut card = div().id(("group", gix)).rounded_lg().bg(rgb(CARD)).border_1().border_color(rgb(BORDER)).flex().flex_col();
            card = card.child(
                div()
                    .id(("group-head", gix))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(CARD_HOVER)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&gix) {
                            this.expanded.insert(gix);
                        }
                        cx.notify();
                    }))
                    .child(checkbox(("group-check", gix), state, deletable.is_empty()).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_group(gix, cx);
                        cx.notify();
                    })))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().flex().items_center().gap_2().child(div().font_weight(FontWeight::MEDIUM).truncate().child(name)).child(
                                div().text_xs().px_1p5().rounded_full().text_color(rgb(color)).bg(gpui::hsla(0., 0., 1., 0.06)).child(pill),
                            ))
                            .child(div().text_xs().text_color(rgb(MUTED)).truncate().child(if places.is_empty() { group.detail.clone() } else { format!("{places} · {}", group.detail) })),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED)).child(if expanded { "▾" } else { "▸" })),
            );
            if expanded {
                let mut rows = div().flex().flex_col().border_t_1().border_color(rgb(BORDER)).py_1();
                for &ix in group.nodes.iter().take(MAX_ROWS) {
                    let node = &tree.nodes[ix];
                    let allowed = deletable.contains(&ix);
                    let on = self.checked.contains(&ix);
                    let path = tree.path_of(ix);
                    let shown = crate::app::abbreviate_home(&path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
                    rows = rows.child(
                        div()
                            .id(("node", ix))
                            .flex()
                            .items_center()
                            .gap_2()
                            .pl_2()
                            .pr_2()
                            .py_1()
                            .when(allowed, |d| d.cursor_pointer().hover(|s| s.bg(rgb(CARD_HOVER))))
                            .when(allowed, |d| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_node(ix, cx);
                                    cx.notify();
                                }))
                            })
                            .child(checkbox(("node-check", ix), if on { Check::On } else { Check::Off }, !allowed))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_sm().truncate().child(format!("{}{}", node.name, if node.kind == Kind::Dir { "/" } else { "" })))
                                    .child(div().text_xs().text_color(rgb(MUTED)).truncate().child(shown)),
                            )
                            .child(div().text_xs().text_color(rgb(MUTED)).font_family("Menlo").flex_none().child(format_size(node.size)))
                            .child(
                                div()
                                    .id(("reveal", ix))
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .px_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(rgb(TEXT)).bg(rgb(BORDER)))
                                    .child("⌕")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.reveal_path(&path);
                                    })),
                            ),
                    );
                }
                if group.nodes.len() > MAX_ROWS {
                    rows = rows.child(div().px_3().py_1().text_xs().text_color(rgb(MUTED)).child(format!(
                        "+ {} more, ticked with the group",
                        group.nodes.len() - MAX_ROWS
                    )));
                }
                card = card.child(rows);
            }
            list = list.child(card);
        }
        list
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selection = self.selection();
        let count = selection.len();
        let size = self.selected_size();
        let busy = matches!(self.phase, Phase::Trashing);
        let mut foot = div().flex().items_center().gap_2().px_3().py_2().border_t_1().border_color(rgb(BORDER)).bg(rgb(BG));
        if count == 0 {
            return foot.child(div().text_xs().text_color(rgb(MUTED)).child("Tick what should go. Enter asks the reader. Esc closes."));
        }
        let frees_text = match self.frees {
            Some(f) if f + size / 100 < size => format!("{} back once the Trash is emptied · {} is shared with files that stay", format_size(f), format_size(size - f)),
            Some(f) => format!("{} back once the Trash is emptied", format_size(f)),
            None => "working out what emptying the Trash would give back…".into(),
        };
        foot = foot.child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(format!("{count} selected · {}", format_size(size))))
                .child(div().text_xs().text_color(rgb(MUTED)).truncate().child(frees_text)),
        );
        if self.confirming {
            foot = foot
                .child(
                    div()
                        .id("cancel")
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(CARD_HOVER)))
                        .child("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirming = false;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("go")
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(DANGER))
                        .text_color(rgb(0xffffff))
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.9))
                        .child(format!("Move {count} to Trash"))
                        .on_click(cx.listener(|this, _, _, cx| this.trash(cx))),
                );
        } else {
            foot = foot.child(
                div()
                    .id("review")
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(if busy { BORDER } else { DANGER }))
                    .text_color(rgb(0xffffff))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.9))
                    .child(if busy { "Moving…".to_string() } else { "Move to Trash…".to_string() })
                    .when(!busy, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| {
                            this.confirming = true;
                            cx.notify();
                        }))
                    }),
            );
        }
        foot
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
    Off,
    Some,
    On,
}

fn checkbox(id: impl Into<gpui::ElementId>, state: Check, disabled: bool) -> gpui::Stateful<gpui::Div> {
    let on = state != Check::Off;
    div()
        .id(id)
        .flex_none()
        .size(px(16.))
        .rounded_sm()
        .border_1()
        .border_color(rgb(if on { DANGER } else { MUTED }))
        .bg(if on { rgb(DANGER) } else { rgb(0x00000000) })
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(rgb(0xffffff))
        .when(disabled, |d| d.opacity(0.3))
        .when(!disabled, |d| d.cursor_pointer())
        .child(match state {
            Check::On => "✓",
            Check::Some => "–",
            Check::Off => "",
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The panel is a pop-up: once it has had focus, losing it (a click
        // anywhere else) closes it, the way a menu would.
        let active = window.is_window_active();
        if std::env::var_os("BROOM_MB_DEBUG").is_some() && active != self.last_active {
            eprintln!("panel: active={active} after {:?}", self.opened.elapsed());
        }
        self.last_active = active;
        if active {
            self.was_active = true;
        } else if self.was_active
            && self.opened.elapsed() > Duration::from_millis(500)
            && !matches!(self.phase, Phase::Trashing)
            && std::env::var_os("BROOM_SNAPSHOT").is_none()
        {
            // Straight on the window, not through the handle: the panel is mid-update here.
            PANEL.with(|p| *p.borrow_mut() = None);
            CLOSED_AT.with(|c| c.set(Some(std::time::Instant::now())));
            window.remove_window();
        }
        let empty = self.groups.is_empty();
        let stage = self.drop.as_ref().filter(|_| matches!(self.phase, Phase::Trashing)).map(|drop| {
            let (landed, bytes) = drop.landed();
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().flex_1().min_h_0().w_full().child(trashcan::stage(drop.clone())))
                .child(div().text_xs().text_color(rgb(MUTED)).text_center().pb_2().child(format!(
                    "{landed} of {} in the bin · {}",
                    drop.total(),
                    format_size(bytes)
                )))
        });
        let staging = stage.is_some();
        div()
            .id("broom")
            .key_context("SearchPanel")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::submit_action))
            .size_full()
            .flex()
            .flex_col()
            .rounded_xl()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(gpui::hsla(0.65, 0.05, 0.11, 0.97))
            .text_color(rgb(TEXT))
            .text_sm()
            .font_family(".SystemUIFont")
            .overflow_hidden()
            .child(self.render_header(cx))
            .child(self.render_search())
            .children(self.render_note())
            .child(
                div()
                    .id("broom-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px_3()
                    .pb_2()
                    .when(self.searching, |d| d.opacity(0.45))
                    .children(stage)
                    .when(!staging && empty, |d| d.child(self.render_empty(cx)))
                    .when(!staging && !empty, |d| d.child(self.render_groups(cx))),
            )
            .child(self.render_footer(cx))
    }
}
