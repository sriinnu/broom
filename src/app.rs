use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AsyncApp, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, Entity, FocusHandle, Focusable,
    FontWeight, HapticFeedbackStyle, HitboxBehavior, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathPromptOptions, Pixels, Point, Rgba, ScrollHandle, ScrollStrategy, SharedString, Stateful, Task, TextAlign, TextRun,
    TitlebarOptions, UniformListScrollHandle, BorderStyle, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle,
    WindowOptions, actions, anchored, canvas, deferred, div, point, prelude::*, px, quad, relative, rgb, size,
    uniform_list, img, Image, ImageFormat,
};

use gpui_ce_elements::editable_text::{EditableTextState, StringStorage, TextChanged, text_input};
use objc2::MainThreadMarker;
use objc2::ClassType;
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
};
use palette::IntoColor;

use crate::ask::{self, CandidateGroup, Intent, ProviderError, Understanding};
use crate::classify::{self, CATEGORIES, Category};
use crate::clock;
use crate::comic;
use crate::disk;
use crate::eta;
use crate::onboarding;
use crate::findings::{self, Finding, Safety};
use crate::live;
use crate::menubar;
use crate::motion;
use crate::scan::{self, CloudRoot, Kind, Progress, Tree, Volume, format_count, format_size};
use crate::secrets;
use crate::settings::{self, Settings};
use crate::trashcan;
use crate::sunburst::{self, Geometry, Hit, Segment, Target};

actions!(broom, [
    GoUp, OpenFolder, Rescan, StartOver, FocusQuery, SubmitQuery, DismissQuery,
    OpenSettings, CloseSettings, About, CloseAbout,
    SelectNext, SelectPrev, OpenSelected, ToggleStaged, CloseReview
]);

const BG: u32 = 0x1c1d21;
const PANEL: u32 = 0x232529;
const CARD: u32 = 0x2a2c31;
const CARD_HOVER: u32 = 0x33363c;
const BORDER: u32 = 0x34363c;
const TEXT: u32 = 0xe8e9ec;
const MUTED: u32 = 0x8d919a;
const ACCENT: u32 = 0x4f9dff;
const DANGER: u32 = 0xe5484d;

/// "Safe to delete" green and "review first" amber, shared by findings and chips.
const SAFE: u32 = 0x3fb950;
const REVIEW: u32 = 0xd29922;

const ROW_HEIGHT: f32 = 30.0;
const RAIL_WIDTH: f32 = 200.0;
const NAV_WIDTH: f32 = 300.0;
const CLEANUP_WIDTH: f32 = 300.0;

/// How the navigator lists a folder's children.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SortBy {
    Size,
    Name,
}

/// What a row's checkbox shows about staging.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tick {
    Off,
    /// Staged itself.
    On,
    /// Inside a staged folder, so it goes with it.
    Implied,
    /// Holds something staged further down.
    Mixed,
}

/// The review card before a move to the Trash: the staged list, frozen at
/// open time so a background recount can't reorder it under the pointer.
struct ReviewState {
    items: Vec<usize>,
    /// What each item frees on its own (`scan::frees_of`), as the survey lands.
    frees: HashMap<usize, u64>,
    /// A delete is running; the buttons wait.
    working: bool,
    /// The items tumbling into the bin while the delete runs.
    drop: Option<Rc<trashcan::Drop>>,
}
const ZOOM_DURATION: Duration = Duration::from_millis(450);

/// How the chart is coloured.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorBy {
    /// Each slice its own hue around the circle, so neighbouring folders stand apart.
    Folder,
    /// By what things are: apps, caches, photos… (see `classify`).
    Kind,
}

#[derive(Clone)]
struct DraggedItem {
    node: usize,
    name: SharedString,
    size: u64,
}

/// A right-click menu for one node, opening where the click happened.
#[derive(Clone)]
struct ContextMenu {
    node: usize,
    at: Point<Pixels>,
}

/// Which tab of the settings panel is in front.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelTab {
    /// API keys, kept in the keychain.
    Keys,
    /// A short FAQ.
    Help,
    /// Version and credits.
    About,
}

/// The one modal card Broom has: Settings / Help / About. The key rows ride
/// along whichever tab is in front, so switching tabs needs no reload. The
/// card is a fixed height; the tab's content scrolls in a viewport of its
/// own, with a scrollbar painted into the card.
struct PanelState {
    tab: PanelTab,
    settings: SettingsState,
    /// Drives the panel's scroll area. The scrollbar reads it (offset,
    /// viewport) and drags write to it.
    scroll: ScrollHandle,
    /// Where the scrolled content starts and stops, in window coordinates,
    /// written by zero-height canvases at its ends. Their difference is the
    /// content's height — a number no div API hands out, and the one the
    /// scrollbar's thumb is proportioned against.
    content_top: Rc<Cell<Option<f32>>>,
    content_bottom: Rc<Cell<Option<f32>>>,
    /// A thumb drag in flight: y at press, scroll offset at press. `None`
    /// when the thumb isn't held.
    drag: Rc<Cell<Option<(f32, f32)>>>,
}

/// Settings lives as long as it's open; keychain work happens in the background and
/// lands back in these rows. `note_epoch` tags the note line so a slow check for an
/// older name can't overwrite what the user just asked about.
struct SettingsState {
    rows: Vec<KeyRow>,
    /// The "check another key name" field.
    custom: Entity<EditableTextState>,
    /// The "add excluded folder" field.
    exclude_input: Entity<EditableTextState>,
    note: Option<SharedString>,
    note_epoch: u64,
}

struct KeyRow {
    /// The keychain service name — the key under which the value is stored.
    service: SharedString,
    title: SharedString,
    hint: SharedString,
    status: KeyStatus,
}

enum KeyStatus {
    /// Reading the keychain in the background.
    Loading,
    /// No value stored for this service.
    Missing,
    /// Stored; shown only as dots plus the last characters.
    Set { masked: SharedString },
    /// Waiting for a paste: the key goes from the clipboard straight into the
    /// keychain, so it is never typed into — or shown in — a text field.
    Editing,
    /// A keychain write or removal is in flight; the row waits without buttons.
    Saving,
}

impl KeyStatus {
    fn of(key: Option<String>) -> Self {
        match key {
            Some(key) => KeyStatus::Set { masked: secrets::tail(&key) },
            None => KeyStatus::Missing,
        }
    }
}

/// A one-line tooltip; gpui's `.tooltip` wants a view.
struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x141519))
            .border_1()
            .border_color(rgb(BORDER))
            .text_xs()
            .text_color(rgb(TEXT))
            .child(self.0.clone())
    }
}

impl Render for DraggedItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded_full()
            .bg(rgb(ACCENT))
            .text_color(rgb(0xffffff))
            .text_sm()
            .font_family(".SystemUIFont")
            .child(format!("{}  ·  {}", self.name, format_size(self.size)))
    }
}

enum Screen {
    /// Nothing scanned: the rail is live, the chart area says to pick a disk.
    Start,
    Scanning(Scanning),
    Results(Results),
}

struct Scanning {
    root: PathBuf,
    progress: Arc<Progress>,
    started: Instant,
    /// Used bytes on the volume, when scanning a whole volume, so the chart can
    /// show how much is still to come.
    expected: Option<u64>,
    live: Option<Rc<LiveView>>,
    last_snapshot: Instant,
    /// Folder being looked at mid-scan, by name path from the root, so it survives the
    /// re-snapshots (node indices change between them).
    focus: Vec<SharedString>,
    hover: Option<Vec<SharedString>>,
    chart_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    eta: eta::Eta,
    finals: HashMap<Vec<SharedString>, Instant>,
    /// Segments ease between snapshots instead of jumping.
    motion: Rc<std::cell::RefCell<motion::ChartMotion>>,
    _tasks: [Task<()>; 2],
}

/// A snapshot of the running totals, laid out around the folder in focus.
struct LiveView {
    tree: Tree,
    /// Which folders are final, indexed like `tree.nodes`.
    done: Vec<bool>,
    focus: usize,
    segments: Rc<Vec<Segment>>,
    swatches: HashMap<usize, Hsla>,
    /// The "not scanned yet" slice of the startup disk, drawn pulsing.
    pending: Option<usize>,
    /// When each recently finalised folder became final, so it can glow briefly.
    settled_at: HashMap<usize, Instant>,
    /// Each segment's identity (folder path), aligned with `segments`, for smooth motion.
    keys: Vec<motion::Key>,
    /// Unique per layout, so the animation knows when to retarget.
    id: u64,
}

static NEXT_LAYOUT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// How long a folder glows after its total becomes final.
const SETTLE_GLOW: Duration = Duration::from_millis(700);

/// A settings value as a duration in milliseconds. Written as a free function
/// so every call site reads the same and a future field can't forget the unit.
fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

impl LiveView {
    /// `finals` remembers when each folder (by path) was first seen final, across snapshots.
    fn new(snapshot: live::LiveSnapshot, focus_path: &[SharedString], finals: &mut HashMap<Vec<SharedString>, Instant>, settle_glow: Duration) -> Self {
        let live::LiveSnapshot { tree, done } = snapshot;
        let pending = tree.nodes[Tree::ROOT]
            .children
            .iter()
            .copied()
            .find(|&c| tree.nodes[c].kind == Kind::Other && tree.nodes[c].name.as_ref() == disk::NOT_SCANNED);
        let now = clock::now();
        let mut settled_at = HashMap::new();
        for ix in (0..tree.nodes.len()).filter(|&ix| done[ix] && tree.nodes[ix].kind == Kind::Dir && ix != Tree::ROOT) {
            let at = *finals.entry(path_to(&tree, ix)).or_insert(now);
            if now - at < settle_glow {
                settled_at.insert(ix, at);
            }
        }
        let mut view = Self {
            tree,
            done,
            focus: Tree::ROOT,
            segments: Rc::default(),
            swatches: HashMap::new(),
            pending,
            settled_at,
            keys: Vec::new(),
            id: 0,
        };
        view.refocus(focus_path);
        view
    }

    fn refocus(&mut self, focus_path: &[SharedString]) {
        self.focus = resolve_path(&self.tree, focus_path);
        let segments = sunburst::layout(&self.tree, self.focus);
        self.swatches = segments
            .iter()
            .filter_map(|s| match s.target {
                Target::Node(ix) if s.depth == 1 => Some((ix, sunburst::base_color(s))),
                _ => None,
            })
            .collect();
        self.keys = segments
            .iter()
            .map(|s| match s.target {
                Target::Node(ix) => motion::Key::Node(path_to(&self.tree, ix)),
                Target::Small { parent } => motion::Key::Small(path_to(&self.tree, parent)),
            })
            .collect();
        self.segments = Rc::new(segments);
        self.id = NEXT_LAYOUT_ID.fetch_add(1, Ordering::Relaxed);
    }

    fn is_final(&self, ix: usize) -> bool {
        self.done[ix]
    }
}

/// Follow folder names down from the root; stops at the deepest folder that exists.
fn resolve_path(tree: &Tree, path: &[SharedString]) -> usize {
    let mut at = Tree::ROOT;
    for name in path {
        match tree.nodes[at].children.iter().find(|&&c| tree.nodes[c].kind == Kind::Dir && &tree.nodes[c].name == name) {
            Some(&child) => at = child,
            None => break,
        }
    }
    at
}

/// Whether `ix` is `ancestor` or lies somewhere below it.
fn is_within(tree: &Tree, mut ix: usize, ancestor: usize) -> bool {
    loop {
        if ix == ancestor {
            return true;
        }
        match tree.nodes[ix].parent {
            Some(parent) => ix = parent,
            None => return false,
        }
    }
}

/// Folder names from the root down to `ix` (excluding the root itself).
fn path_to(tree: &Tree, mut ix: usize) -> Vec<SharedString> {
    let mut names = Vec::new();
    while let Some(parent) = tree.nodes[ix].parent {
        names.push(tree.nodes[ix].name.clone());
        ix = parent;
    }
    names.reverse();
    names
}

struct Results {
    /// Shared with background searches, so they can read the whole tree without a copy;
    /// the rare mutation (after a trash) goes through `Arc::make_mut`.
    tree: Arc<Tree>,
    focus: usize,
    segments: Rc<Vec<Segment>>,
    swatches: HashMap<usize, Hsla>,
    chart_hover: Option<Hit>,
    list_hover: Option<usize>,
    anim_start: Instant,
    chart_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Staged for the Trash (the "cleanup plan"): top-level nodes only, never
    /// one inside another.
    collector: Vec<usize>,
    list_scroll: UniformListScrollHandle,
    /// The focused folder's children in list order (`sort`); the chart keeps size order.
    order: Vec<usize>,
    sort: SortBy,
    /// The keyboard's row in the navigator.
    selected: Option<usize>,
    elapsed: Duration,
    /// What the user asked to scan; for the startup disk this is `/`, while the tree
    /// itself is rooted at the Data volume.
    requested_root: PathBuf,
    findings: Vec<Finding>,
    /// What deleting the Collector's items frees (worked out in the background with
    /// `scan::frees_of`; `None` while calculating). `collector_version` guards against a
    /// slow result for an older selection.
    collector_frees: Cell<Option<u64>>,
    collector_version: u64,
    /// A banner over the chart: the scan-complete line, or `banner_text` when set
    /// (e.g. after a move to the Trash). Cleared by a timer (`show_banner`).
    banner: Option<Instant>,
    banner_text: Option<String>,
    banner_version: u64,
    color_by: ColorBy,
    /// What each node is, for colouring by kind. Filled in as nodes appear in the chart;
    /// node indices stay valid for the life of the tree, so nothing is classified twice.
    categories: HashMap<usize, Category>,
    /// The kind under the pointer in the chart's legend: the chart highlights just that.
    legend_hover: Option<Category>,
    /// Folders that look like someone's home folder (`home_folders`).
    homes: Vec<usize>,
    /// The query at the centre of the current ask state.
    query: SharedString,
    ask: AskState,
    /// Exact-match candidate groups for the current query text, from the background.
    matches: Option<Arc<Vec<CandidateGroup>>>,
    /// Bumped whenever the query text changes, so a slow background search for older
    /// text is dropped when it lands.
    search_version: u64,
}

impl Results {
    /// After the tree changed under the lists (nodes detached or grafted): drop
    /// everything that pointed into it and start the chart's settle animation.
    /// Returns whether the cleanup plan still has items, so its "frees" figure
    /// needs recomputing.
    fn tree_changed(&mut self) -> bool {
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

    fn new(tree: Tree, findings: Vec<Finding>, elapsed: Duration, requested_root: PathBuf, color_by: ColorBy, sort: SortBy) -> Self {
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
            list_scroll: UniformListScrollHandle::new(),
            order: Vec::new(),
            sort,
            selected: None,
            elapsed,
            requested_root,
            findings,
            collector_frees: Cell::new(None),
            collector_version: 0,
            banner: None,
            banner_text: None,
            banner_version: 0,
            color_by,
            categories: HashMap::new(),
            legend_hover: None,
            homes: Vec::new(),
            query: SharedString::default(),
            ask: AskState::Idle,
            matches: None,
            search_version: 0,
        };
        results.homes = home_folders(&results.tree);
        results.relayout();
        results
    }

    fn relayout(&mut self) {
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

    /// The list order for the focused folder: the tree's (largest first) or by name.
    fn reorder(&mut self) {
        let mut order = self.tree.nodes[self.focus].children.clone();
        if self.sort == SortBy::Name {
            let tree = &self.tree;
            order.sort_by(|&a, &b| {
                // Folders first, then case-insensitive by name; the APFS remainder slices last.
                let key = |ix: usize| {
                    let n = &tree.nodes[ix];
                    (n.kind == Kind::Other, n.kind != Kind::Dir, n.name.to_lowercase())
                };
                key(a).cmp(&key(b))
            });
        }
        self.order = order;
        if let Some(sel) = self.selected {
            if !self.order.contains(&sel) {
                self.selected = None;
            }
        }
    }

    fn set_sort(&mut self, sort: SortBy) {
        self.sort = sort;
        self.reorder();
    }

    fn set_color_by(&mut self, color_by: ColorBy) {
        self.color_by = color_by;
        self.legend_hover = None;
        self.relayout();
    }

    fn category(&self, target: Target) -> Option<Category> {
        match target {
            Target::Node(ix) => self.categories.get(&ix).copied(),
            Target::Small { .. } => None,
        }
    }

    fn color(&self, segment: &Segment) -> Hsla {
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

    fn navigate(&mut self, ix: usize) {
        // Detached nodes (trashed while referenced by a finding) have no place in the chart.
        let detached = self.tree.nodes[ix].parent.is_none() && ix != Tree::ROOT;
        if detached || self.tree.nodes[ix].kind != Kind::Dir || ix == self.focus {
            return;
        }
        // Going up lands the keyboard on the folder just left; going down starts afresh.
        let came_from = (self.tree.nodes[self.focus].parent == Some(ix)).then_some(self.focus);
        self.focus = ix;
        self.chart_hover = None;
        self.list_hover = None;
        self.anim_start = clock::now();
        self.selected = came_from;
        self.relayout();
        match came_from.and_then(|c| self.order.iter().position(|&o| o == c)) {
            Some(pos) => self.list_scroll.scroll_to_item(pos, ScrollStrategy::Center),
            None => self.list_scroll.scroll_to_item(0, ScrollStrategy::Top),
        }
    }

    fn hovered(&self) -> Option<Target> {
        match self.chart_hover {
            Some(Hit::Segment(i)) => self.segments.get(i).map(|s| s.target),
            Some(Hit::Center) => None,
            None => self.list_hover.map(Target::Node),
        }
    }

    fn collect(&mut self, ix: usize) {
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

    fn collected_size(&self) -> u64 {
        self.collector.iter().map(|&c| self.tree.nodes[c].size).sum()
    }

    /// Whether `ix` may be staged at all (see `collect`).
    fn stageable(&self, ix: usize) -> bool {
        ix != Tree::ROOT
            && self.tree.nodes[ix].parent.is_some()
            && self.tree.nodes[ix].kind != Kind::Other
            && !self.homes.contains(&ix)
    }

    /// What the checkbox on `ix` shows.
    fn tick(&self, ix: usize) -> Tick {
        if self.collector.contains(&ix) {
            return Tick::On;
        }
        if self.collector.iter().any(|&c| self.tree.is_ancestor_or_self(c, ix)) {
            return Tick::Implied;
        }
        if self.collector.iter().any(|&c| self.tree.is_ancestor_or_self(ix, c)) {
            return Tick::Mixed;
        }
        Tick::Off
    }

    /// The checkbox on `ix` was clicked. A folder staged through its parent
    /// stays as it is: the parent's own box is where that decision lives.
    fn toggle_staged(&mut self, ix: usize) {
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
    fn group_tick(&self, nodes: &[usize]) -> Tick {
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
    fn toggle_group(&mut self, nodes: &[usize]) {
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

    /// Whether `ix` (a chart slice) is staged, itself or through a parent.
    fn is_staged(&self, ix: usize) -> bool {
        self.collector.iter().any(|&c| self.tree.is_ancestor_or_self(c, ix))
    }
}

/// What a node is. The APFS volume slices are the system's; the unscanned or unreadable
/// remainder could be anything.
fn category_of(tree: &Tree, homes: &[usize], ix: usize) -> Category {
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
fn home_folders(tree: &Tree) -> Vec<usize> {
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

/// Shows paths in the home folder as `~/…`, as the shell does.
pub(crate) fn abbreviate_home(path: &str) -> String {
    let Ok(home) = std::env::var("HOME") else { return path.to_string() };
    match path.strip_prefix(home.as_str()) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// When the process started, for launch-to-first-chart timing (`BROOM_TIMING=1`).
pub static LAUNCHED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// The root entity, held here so Broom — its state, its background tasks, its
/// menu-bar extra — outlives the window. Closing the window drops the view's
/// handle, not the app; `reopen` puts the same entity back in a new window.
static BROOM_ENTITY: std::sync::OnceLock<Entity<Broom>> = std::sync::OnceLock::new();

/// "Open Broom" and the 🧹 click tap this from AppKit's side of the world (see
/// `menubar`'s `MenuBridge`); `arm_reopen`'s task drains the other end.
static OPEN_TX: std::sync::OnceLock<mpsc::Sender<Request>> = std::sync::OnceLock::new();

/// What the menu-bar extra asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Bring the main window back.
    Open,
    /// Bring the main window back and scan this folder (a second `broom PATH`).
    OpenPath(PathBuf),
    /// Toggle the search-and-clean panel under the 🧹.
    Panel,
}

/// Two threads for the background clone surveys, so they never take the whole
/// machine from a search or a scan.
fn survey_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(2).thread_name(|i| format!("survey-{i}")).build().expect("survey pool"))
}

/// The app's one view, for the panel: it survives the window closing.
pub fn broom() -> Option<Entity<Broom>> {
    BROOM_ENTITY.get().cloned()
}

/// Where the user's files are, for the panel's first scan.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// A results snapshot the panel can search without touching the view: the tree
/// is shared, the rest is small.
pub struct Snapshot {
    pub tree: Arc<Tree>,
    pub findings: Vec<Finding>,
    pub root: PathBuf,
    pub homes: Vec<usize>,
}

/// Whether there is anything to search yet.
pub enum ScanState {
    /// Nothing scanned and nothing in progress.
    Nothing,
    Scanning { files: u64, bytes: u64, current: String },
    Ready,
}

/// The main window's options, shared by the first open and every reopen.
fn window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1400.), px(860.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Broom".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(16.), px(16.))),
        }),
        // Four columns need the room: rail, navigator, chart, cleanup.
        window_min_size: Some(size(px(1100.), px(600.))),
        // Kept blurred (and frosted in `frost_window`) so the menu-bar
        // residency self-test's expectations hold; every screen paints a
        // solid background over it.
        window_background: WindowBackgroundAppearance::Blurred,
        ..Default::default()
    }
}

/// Open the main window for the first time and remember the entity, so the
/// app survives after the window closes.
pub fn open_main_window(initial: Option<PathBuf>, loaded: settings::Loaded, cx: &mut App) -> WindowHandle<Broom> {
    cx.open_window(window_options(cx), |window, cx| {
        let broom = cx.new(|cx| Broom::new(initial, loaded, window, cx));
        let _ = BROOM_ENTITY.set(broom.clone());
        broom
    })
    .expect("failed to open window")
}

/// Wake the reopen task. Called from the menu-bar bridge's action, on the
/// main thread; never blocks.
pub fn open_requested() {
    request(Request::Open);
}

/// A second launch (`broom [PATH]`) found this one running: come forward, and
/// scan PATH if one was given. Any thread; never blocks.
pub fn open_path_requested(path: Option<PathBuf>) {
    request(match path {
        Some(path) => Request::OpenPath(path),
        None => Request::Open,
    });
}

/// The 🧹 was clicked: toggle the search panel. Main thread; never blocks.
pub fn panel_requested() {
    request(Request::Panel);
}

fn request(what: Request) {
    if std::env::var_os("BROOM_MB_DEBUG").is_some() {
        eprintln!("menubar: {what:?} requested, tx present: {}", OPEN_TX.get().is_some());
    }
    if let Some(tx) = OPEN_TX.get() {
        let _ = tx.send(what);
    }
}

/// Wire "Open Broom" clicks to window reopen: AppKit sends on a channel, this
/// task blocks on the other end in the background pool and hops back to the
/// app's thread when a click lands.
pub fn arm_reopen(cx: &mut App) {
    let (tx, rx) = mpsc::channel::<Request>();
    let _ = OPEN_TX.set(tx);
    let rx = Arc::new(std::sync::Mutex::new(rx));
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            let rx = rx.clone();
            // `Err` means every sender is gone (app teardown) — stop waiting.
            let clicked = cx.background_spawn(async move { rx.lock().expect("open-click receiver").recv() }).await;
            let Ok(what) = clicked else { break };
            if std::env::var_os("BROOM_MB_DEBUG").is_some() {
                eprintln!("menubar: reopen task woke: {what:?}");
            }
            let _ = cx.update(|cx| match what {
                Request::Open => reopen(cx),
                Request::OpenPath(path) => {
                    reopen(cx);
                    if let Some(broom) = broom() {
                        broom.update(cx, |b, cx| b.start_scan(path, cx));
                    }
                }
                Request::Panel => crate::panel::toggle(cx),
            });
        }
    })
    .detach();
}

/// Bring Broom's window back: a new window around the surviving entity if the
/// old one was closed, else just pull the app forward.
fn reopen(cx: &mut App) {
    if std::env::var_os("BROOM_MB_DEBUG").is_some() {
        eprintln!("menubar: reopen, windows: {}", cx.windows().len());
    }
    if cx.windows().is_empty() {
        if let Some(broom) = BROOM_ENTITY.get().cloned() {
            let _ = cx.open_window(window_options(cx), |window, cx| {
                // The new window needs the frost tuned like the first one;
                // Broom::new doesn't run again for a reopened entity.
                if let Some(mtm) = MainThreadMarker::new() {
                    frost_window(mtm);
                }
                // Focus what the screen expects: the modal card if one was open
                // when the window closed, else the app root.
                let focus = {
                    let this = broom.read(cx);
                    if this.modal.is_some() || this.review.is_some() { this.modal_focus.clone() } else { this.focus_handle.clone() }
                };
                window.focus(&focus, cx);
                broom
            });
        }
    }
    cx.activate(true);
}

/// Dev aid, `BROOM_MB_SELFTEST=1`: run the residency through its real paths
/// and report. Closes the window the way the red button does, checks the app
/// (and the 🧹 item's `MenuBar`) survived, fires the actual `openBroom:`
/// action, and checks the window came back with the right frost. Prints one
/// `selftest:` line per check and quits — for the harness, not for users.
pub fn arm_selftest(cx: &mut App) {
    if std::env::var_os("BROOM_MB_SELFTEST").is_none() {
        return;
    }
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut failures = 0;
        let check = |failures: &mut u32, what: &str, ok: bool| {
            eprintln!("selftest: {} — {}", if ok { "ok" } else { "FAIL" }, what);
            if !ok {
                *failures += 1;
            }
        };

        cx.background_executor().timer(Duration::from_millis(800)).await;
        // ObjC calls happen directly on the main thread (where this task runs),
        // NOT inside `cx.update`: performClose re-enters gpui's close handling
        // synchronously, which borrows the app — doing that from inside an
        // update closure would be a double borrow.
        let mut closed = false;
        if let Some(mtm) = MainThreadMarker::new() {
            if let Some(window) = NSApplication::sharedApplication(mtm).keyWindow() {
                window.performClose(None);
                closed = true;
            }
        }
        check(&mut failures, "window closed via performClose", closed);

        cx.background_executor().timer(Duration::from_millis(800)).await;
        let (survived, menu) = cx.update(|cx| {
            let survived = cx.windows().is_empty();
            let menu = BROOM_ENTITY.get().map(|broom| broom.read(cx).menubar.is_some()).unwrap_or(false);
            (survived, menu)
        });
        check(&mut failures, "app alive with no windows after close", survived);
        check(&mut failures, "menu-bar extra still alive", menu);

        let mut clicked = false;
        if let Some(mtm) = MainThreadMarker::new() {
            clicked = cx.update(|cx| {
                BROOM_ENTITY
                    .get()
                    .and_then(|broom| broom.read(cx).menubar.as_ref().map(|bar| bar.simulate_open_click(mtm)))
                    .unwrap_or(false)
            });
        }
        check(&mut failures, "openBroom: action dispatched", clicked);

        cx.background_executor().timer(Duration::from_millis(800)).await;
        let (windows, frost) = cx.update(|_cx| {
            let windows = _cx.windows().len();
            let Some(mtm) = MainThreadMarker::new() else { return (windows, false) };
            let frost = NSApplication::sharedApplication(mtm)
                .windows()
                .iter()
                .any(|window| {
                    window
                        .contentView()
                        .map(|content| {
                            content
                                .subviews()
                                .iter()
                                .any(|view| view.isKindOfClass(NSVisualEffectView::class()) && unsafe {
                                    Retained::cast_unchecked::<NSVisualEffectView>(view.clone()).material()
                                        == NSVisualEffectMaterial::UnderWindowBackground
                                })
                        })
                        .unwrap_or(false)
                });
            (windows, frost)
        });
        check(&mut failures, "window reopened", windows == 1);
        check(&mut failures, "reopened window frosted (UnderWindowBackground)", frost);

        eprintln!("selftest: {} ({})", if failures == 0 { "PASS" } else { "FAIL" }, failures);
        cx.update(|cx| cx.quit());
    })
    .detach();
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    /// Full Disk Access: nothing to show.
    Granted,
    /// Missing: show the card explaining how to grant it.
    Missing,
    /// Granted while Broom was running: suggest rescanning.
    JustGranted,
    /// The user said "Not now".
    Dismissed,
}

/// Where a submitted query stands. Nothing here ever deletes by itself: a confident
/// "delete" answer pre-selects the Collector, and the user confirms the exact list.
#[derive(Clone)]
enum AskState {
    Idle,
    /// A query was submitted; the providers are thinking.
    Thinking,
    /// A confident answer. `group` is the chosen candidates, snapshotted.
    Answered { intent: Intent, group: Arc<CandidateGroup> },
    /// The providers replied but nothing cleared the bar: the exact-match cards decide.
    Suggestions,
    /// No provider could be asked; the exact-match cards still work.
    Unavailable(Unavailable),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unavailable {
    /// No API key configured.
    NoKey,
    /// The key was rejected.
    Rejected,
    /// Network or service trouble.
    Unreachable,
}

pub struct Broom {
    screen: Screen,
    /// The rail's disks and synced folders, refreshed in the background.
    volumes: Vec<Volume>,
    clouds: Vec<CloudRoot>,
    /// Navigator order, kept across scans.
    sort: SortBy,
    /// The review card before a move to the Trash, if open.
    review: Option<ReviewState>,
    focus_handle: FocusHandle,
    error: Option<String>,
    access: Access,
    _access_watch: Option<Task<()>>,
    /// Kept here rather than on the results, so a rescan keeps the user's choice.
    color_by: ColorBy,
    /// The query field's content, owned by the input element; Broom holds the entity.
    query: Entity<EditableTextState>,
    /// Right-click menu, if one is open. Dropped whenever the node it points at is
    /// detached or the scan changes underneath it.
    context_menu: Option<ContextMenu>,
    /// The open modal card, if any.
    modal: Option<PanelState>,
    /// Focus target while a modal is open, so Escape lands on the card.
    modal_focus: FocusHandle,
    /// The menu-bar extra: flower + startup-disk fill, volumes in its menu.
    menubar: Option<menubar::MenuBar>,
    _menubar_watch: Option<Task<()>>,
    /// Re-reads the Trash when Finder changes it; see `watch_trash`.
    _trash_watch: Option<Task<()>>,
    /// Bumped every time Settings opens, so a keychain result from a previous
    /// session can't land on the rows of the current one.
    settings_epoch: u64,
    /// True while a native sheet or open panel has the window (trash confirm,
    /// folder picker). Our own modal must not open underneath those.
    busy: bool,
    /// What the user has chosen. Loaded once at startup and written back on
    /// every change; the live copy is the truth while the app runs.
    settings: Settings,
    /// Set when a damaged settings file had to be moved aside at startup, so
    /// the Settings panel can say so once instead of the app silently
    /// forgetting everything the user had set up.
    settings_salvaged: Option<PathBuf>,
}

impl Broom {
    pub fn new(initial: Option<PathBuf>, loaded: settings::Loaded, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings::Loaded { settings, salvaged } = loaded;
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let access = if onboarding::has_full_disk_access() { Access::Granted } else { Access::Missing };
        // Recordings of a sample folder don't need the Full Disk Access card.
        #[cfg(feature = "snapshot")]
        let access = if std::env::var_os("BROOM_HIDE_ACCESS").is_some() { Access::Dismissed } else { access };
        let mut this = Self {
            screen: Screen::Start,
            volumes: scan::volumes(),
            clouds: scan::cloud_roots(),
            sort: SortBy::Size,
            review: None,
            focus_handle,
            error: None,
            access,
            _access_watch: None,
            color_by: match settings.color_by {
                settings::ColorBy::Folder => ColorBy::Folder,
                settings::ColorBy::Kind => ColorBy::Kind,
            },
            // Created directly rather than `use_keyed`: that's a render-time hook
            // (element-state cache), and Broom owns this entity for the app's life anyway.
            query: cx.new(|cx| EditableTextState::new(StringStorage::default(), cx)),
            context_menu: None,
            modal: None,
            modal_focus: cx.focus_handle(),
            menubar: None,
            _menubar_watch: None,
            _trash_watch: None,
            settings_epoch: 0,
            busy: false,
            settings,
            settings_salvaged: salvaged,
        };
        // Every keystroke re-runs the (debounced, background) exact matching.
        cx.subscribe(&this.query, |this, _, _: &TextChanged, cx| this.query_changed(cx)).detach();
        // The menu-bar extra lives as long as the app; the window's UI thread
        // is the process's main thread, which AppKit status items require.
        if let Some(mtm) = MainThreadMarker::new() {
            frost_window(mtm);
            if this.settings.menubar {
                let startup = startup_fraction(&this.volumes);
                this.menubar = Some(menubar::MenuBar::new(mtm, startup));
                this.menubar.as_ref().unwrap().update(startup, &volume_lines(&this.volumes));
            }
        }
        // The rail's numbers (and the 🧹's) stay honest whether or not the extra is on.
        this._menubar_watch = Some(this.watch_menubar(cx));
        this._trash_watch = Some(this.watch_trash(cx));
        if access == Access::Missing {
            this._access_watch = Some(this.watch_access(cx));
        }
        // Before the scan starts, so a recording's clock is in charge from the first folder.
        #[cfg(feature = "snapshot")]
        crate::snapshot::install(window, cx);
        if let Some(path) = initial {
            this.start_scan(path, cx);
        }
        this
    }

    /// Where the 🧹 is, for hanging the panel under it.
    pub fn menubar_anchor(&self) -> Option<(f32, f32, f32)> {
        let mtm = MainThreadMarker::new()?;
        self.menubar.as_ref().and_then(|bar| bar.anchor(mtm))
    }

    /// The current results, for the panel to search.
    pub fn snapshot(&self) -> Option<Snapshot> {
        match &self.screen {
            Screen::Results(r) => Some(Snapshot {
                tree: r.tree.clone(),
                findings: r.findings.clone(),
                root: r.requested_root.clone(),
                homes: r.homes.clone(),
            }),
            _ => None,
        }
    }

    pub fn scan_state(&self) -> ScanState {
        match &self.screen {
            Screen::Results(_) => ScanState::Ready,
            Screen::Scanning(s) => ScanState::Scanning {
                files: s.progress.files.load(Ordering::Relaxed),
                bytes: s.progress.bytes.load(Ordering::Relaxed),
                current: s.progress.current.lock().map(|c| abbreviate_home(&c)).unwrap_or_default(),
            },
            Screen::Start => ScanState::Nothing,
        }
    }

    /// The panel needs something to search: scan the home folder if nothing
    /// is scanned or scanning. Fast, and where node_modules and friends live.
    pub fn ensure_scan(&mut self, cx: &mut Context<Self>) {
        if matches!(self.screen, Screen::Start) {
            self.start_scan(home_dir(), cx);
        }
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
    fn watch_trash(&self, cx: &mut Context<Self>) -> Task<()> {
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
                                findings::prune_after_removal(&mut r.findings, &r.tree);
                                findings::refresh_location(&mut r.findings, &r.tree, &bases, "Trash");
                                recompute = r.tree_changed();
                            }
                        }
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
    fn watch_menubar(&self, cx: &mut Context<Self>) -> Task<()> {
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
    fn watch_access(&self, cx: &mut Context<Self>) -> Task<()> {
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

    fn render_access_card(&self, not_readable: Option<u64>, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (title, body) = match self.access {
            Access::Granted | Access::Dismissed => return None,
            Access::Missing => (
                "See everything on your disk",
                match not_readable {
                    Some(bytes) => format!(
                        "{} couldn't be read. macOS keeps some folders private (Mail, Messages, Safari, other apps' data) unless Broom has Full Disk Access.",
                        format_size(bytes)
                    ),
                    None => "macOS keeps some folders private (Mail, Messages, Safari, other apps' data). Give Broom Full Disk Access so the scan can include them.".to_string(),
                },
            ),
            Access::JustGranted => ("Full Disk Access is on", "Rescan to include the folders that were private before.".to_string()),
        };
        let actions = div().flex().gap_2().justify_end().mt_1();
        let actions = match self.access {
            Access::JustGranted => actions.child(
                primary_button("access-rescan", "Rescan", ACCENT).on_click(cx.listener(|this, _, window, cx| {
                    this.access = Access::Granted;
                    this.rescan(&Rescan, window, cx);
                })),
            ),
            _ => actions
                .child(button("access-later", "Not now").on_click(cx.listener(|this, _, _, cx| {
                    this.access = Access::Dismissed;
                    cx.notify();
                })))
                .child(
                    primary_button("access-open", "Open Privacy Settings", ACCENT)
                        .on_click(cx.listener(|_, _, _, cx| cx.open_url(onboarding::FULL_DISK_ACCESS_SETTINGS))),
                ),
        };
        Some(
            div()
                .mx_3()
                .mb_2()
                .p_3()
                .rounded_lg()
                .bg(rgb(CARD))
                .border_1()
                .border_color(rgb(ACCENT))
                .flex()
                .flex_col()
                .gap_1()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_xs().text_color(rgb(MUTED)).child(body))
                .when(self.access == Access::Missing, |d| {
                    d.child(div().text_xs().text_color(rgb(MUTED)).child("In Settings, turn on Broom (use + to add it if it isn't listed)."))
                })
                .child(actions),
        )
    }

    fn start_scan(&mut self, root: PathBuf, cx: &mut Context<Self>) {
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
                let (tree, findings) = cx
                    .background_spawn({
                        let progress = progress.clone();
                        let excludes = excludes.clone();
                        async move {
                            let tree = scan::scan(&root, &progress, &excludes, outline_depth);
                            // May survey clone sharing (e.g. pnpm's cloned node_modules), so
                            // keep it off the UI thread.
                            let findings = findings::from_tree(&tree, &findings::Bases::for_root(&tree.root_path));
                            (tree, findings)
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
                        let mut results = Results::new(tree, findings, clock::since(started), root_for_results, this.color_by, this.sort);
                        let ix = resolve_path(&results.tree, &focus);
                        if ix != Tree::ROOT {
                            results.navigate(ix);
                        }
                        // The chart is already on screen from the scan; don't replay the sweep.
                        results.anim_start = clock::now() - ZOOM_DURATION;
                        this.screen = Screen::Results(results);
                        this.resolve_pending_findings(cx);
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

    fn cancel_scan(&mut self, cx: &mut Context<Self>) {
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
    fn show_banner(&mut self, text: Option<String>, cx: &mut Context<Self>) {
        let seconds = self.settings.banner_seconds;
        let Some(r) = self.results() else { return };
        r.banner = Some(clock::now());
        r.banner_text = text;
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
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_folder(&mut self, _: &OpenFolder, _: &mut Window, cx: &mut Context<Self>) {
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

    fn rescan(&mut self, _: &Rescan, _: &mut Window, cx: &mut Context<Self>) {
        let root = match &self.screen {
            Screen::Results(r) => r.requested_root.clone(),
            Screen::Scanning(s) => s.root.clone(),
            Screen::Start => return,
        };
        self.cancel_scan(cx);
        self.start_scan(root, cx);
    }

    fn start_over(&mut self, _: &StartOver, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel_scan(cx);
    }

    fn go_up(&mut self, _: &GoUp, _: &mut Window, cx: &mut Context<Self>) {
        // Escape's first job is closing whatever's on top: the modal, then the menu;
        // only then does it navigate up.
        if self.modal.take().is_some() || self.review.take().is_some() || self.context_menu.take().is_some() {
            cx.notify();
            return;
        }
        if let Screen::Results(r) = &mut self.screen {
            if let Some(parent) = r.tree.nodes[r.focus].parent {
                r.navigate(parent);
                cx.notify();
            }
        }
    }

    fn open_context_menu(&mut self, node: usize, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.context_menu = Some(ContextMenu { node, at });
        cx.notify();
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        // Never under a native sheet (trash confirm, folder picker): the parent
        // window is disabled for its duration and our card would be dead on arrival.
        if self.busy {
            return;
        }
        self.open_panel(PanelTab::Keys, window, cx);
    }

    fn close_settings(&mut self, _: &CloseSettings, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() {
            self.close_modal(window, cx);
        }
    }

    pub(crate) fn show_about(&mut self, _: &About, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.open_panel(PanelTab::About, window, cx);
    }

    fn close_about(&mut self, _: &CloseAbout, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() {
            self.close_modal(window, cx);
        }
    }

    /// Open the panel on a tab — or, if it's already up, just switch to it.
    /// The key rows keep whatever the keychain last told them.
    /// Clears the salvaged-settings notice after its one display.
    fn open_panel(&mut self, tab: PanelTab, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_salvaged.take();
        if let Some(p) = &mut self.modal {
            p.tab = tab;
        } else {
            let rows = vec![
                KeyRow {
                    service: secrets::JEV_SERVICE.into(),
                    title: "Reader key (typesafe.ai)".into(),
                    hint: "Lets the reader judge what an Ask query means.".into(),
                    status: KeyStatus::Loading,
                },
            ];
            self.settings_epoch += 1;
            self.modal = Some(PanelState {
                tab,
                settings: SettingsState {
                    rows,
                    custom: cx.new(|cx| EditableTextState::new(StringStorage::default(), cx)),
                    exclude_input: cx.new(|cx| EditableTextState::new(StringStorage::default(), cx)),
                    note: None,
                    note_epoch: 0,
                },
                scroll: ScrollHandle::new(),
                content_top: Rc::default(),
                content_bottom: Rc::default(),
                drag: Rc::default(),
            });
            for ix in 0..1 {
                self.refresh_key_row(ix, cx);
            }
        }
        window.focus(&self.modal_focus, cx);
        cx.notify();
    }

    /// Enter inside Settings: only the key-name field responds — Enter there
    /// checks the keychain. Keys are pasted rather than typed, so no other
    /// field gives Enter anything to do.
    fn settings_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(focused) = window.focused(cx) else { return };
        let check = match &self.modal {
            Some(p) => focused == p.settings.custom.read(cx).focus_handle(cx),
            None => false,
        };
        if check {
            self.add_custom_key(cx);
        }
    }

    fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// The backdrop was clicked: whichever card is up goes away (a review that
    /// is mid-delete stays until it's done).
    fn close_any_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(review) = &self.review {
            if review.working {
                return;
            }
            self.review = None;
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return;
        }
        self.close_modal(window, cx);
    }

    /// A transient one-line status inside Settings ("saved", "couldn't…").
    fn set_note(&mut self, text: impl Into<SharedString>) {
        if let Some(p) = &mut self.modal {
            p.settings.note = Some(text.into());
        }
    }

    /// Re-read the keychain for one row, off the UI thread, and update its status.
    /// The epoch drops results from a Settings session that has since closed and
    /// been reopened.
    fn refresh_key_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(service) = self.modal.as_ref().and_then(|p| p.settings.rows.get(ix).map(|r| r.service.to_string())) else { return };
        if let Some(row) = self.modal.as_mut().and_then(|p| p.settings.rows.get_mut(ix)) {
            row.status = KeyStatus::Loading;
        }
        let epoch = self.settings_epoch;
        cx.spawn(async move |this, cx| {
            let key = cx.background_spawn(async move { secrets::read(&service) }).await;
            this.update(cx, |this, cx| {
                if this.settings_epoch != epoch {
                    return;
                }
                if let Some(row) = this.modal.as_mut().and_then(|p| p.settings.rows.get_mut(ix)) {
                    row.status = KeyStatus::of(key);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Arm a row for pasting: the next clipboard paste goes straight to the keychain.
    fn edit_key(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.modal.as_mut().and_then(|p| p.settings.rows.get_mut(ix)) {
            if matches!(row.status, KeyStatus::Missing | KeyStatus::Set { .. }) {
                row.status = KeyStatus::Editing;
            }
        }
        cx.notify();
    }

    /// Take whatever text is on the clipboard as the row's new key. Nothing is
    /// rendered: the value goes from the pasteboard into the keychain directly,
    /// never through a visible field.
    fn paste_key(&mut self, ix: usize, cx: &mut Context<Self>) {
        let service = match &self.modal {
            Some(p) => match p.settings.rows.get(ix).map(|r| (r.service.to_string(), &r.status)) {
                Some((service, KeyStatus::Editing)) => service,
                _ => return,
            },
            None => return,
        };
        let Some(secret) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.set_note("the clipboard holds no text \u{2014} copy the key first");
            return;
        };
        let secret = secret.trim().to_string();
        if secret.is_empty() {
            self.set_note("the clipboard holds no text \u{2014} copy the key first");
            return;
        }
        self.store_key(ix, &service, secret, cx);
    }

    /// Write a value into the keychain in the background; the row waits as
    /// `Saving` so its buttons can't fire again mid-write. Errors carry status
    /// text only, never key material.
    fn store_key(&mut self, ix: usize, service: &str, secret: String, cx: &mut Context<Self>) {
        if let Some(row) = self.modal.as_mut().and_then(|p| p.settings.rows.get_mut(ix)) {
            row.status = KeyStatus::Saving;
        }
        self.set_note("saving to the keychain\u{2026}");
        let service = service.to_string();
        let epoch = self.settings_epoch;
        cx.spawn(async move |this, cx| {
            let name = service.clone();
            let saved = cx.background_spawn(async move { secrets::write(&service, &secret) }).await;
            this.update(cx, |this, cx| {
                if this.settings_epoch != epoch {
                    return;
                }
                ask::invalidate_keys();
                match saved {
                    Ok(()) => this.set_note(format!("\u{201c}{name}\u{201d} saved to the keychain")),
                    Err(error) => this.set_note(format!("couldn\u{2019}t save \u{201c}{name}\u{201d}: {error}")),
                }
                this.refresh_key_row(ix, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Drop the paste state and go back to whatever the keychain says.
    fn cancel_edit_key(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.refresh_key_row(ix, cx);
    }

    /// Remove a service's value from the keychain.
    fn remove_key(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(service) = self.modal.as_ref().and_then(|p| p.settings.rows.get(ix).map(|r| r.service.to_string())) else { return };
        if let Some(row) = self.modal.as_mut().and_then(|p| p.settings.rows.get_mut(ix)) {
            row.status = KeyStatus::Saving;
        }
        self.set_note("removing\u{2026}");
        let epoch = self.settings_epoch;
        cx.spawn(async move |this, cx| {
            let name = service.clone();
            let removed = cx.background_spawn(async move { secrets::delete(&service) }).await;
            this.update(cx, |this, cx| {
                if this.settings_epoch != epoch {
                    return;
                }
                ask::invalidate_keys();
                match removed {
                    Ok(()) => this.set_note(format!("\u{201c}{name}\u{201d} removed from the keychain")),
                    Err(error) => this.set_note(format!("couldn\u{2019}t remove \u{201c}{name}\u{201d}: {error}")),
                }
                this.refresh_key_row(ix, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Look a service name up in the keychain and add it as a row, masked. The
    /// note is epoch-tagged so a slow read for an older name can't overwrite the
    /// answer to the one the user just asked about.
    fn add_custom_key(&mut self, cx: &mut Context<Self>) {
        let service = match &self.modal {
            Some(p) => p.settings.custom.read(cx).as_str().trim().to_string(),
            None => return,
        };
        if service.is_empty() {
            self.set_note("type a key name first");
            return;
        }
        let note_epoch = match &mut self.modal {
            Some(p) => {
                let s = &mut p.settings;
                if s.rows.iter().any(|r| r.service.as_ref() == service) {
                    s.note = Some(format!("\u{201c}{service}\u{201d} is already listed").into());
                    cx.notify();
                    return;
                }
                s.note_epoch += 1;
                s.custom.update(cx, |state, cx| state.emplace("", cx));
                s.rows.push(KeyRow {
                    title: service.clone().into(),
                    service: service.clone().into(),
                    hint: "Any keychain key, watched here.".into(),
                    status: KeyStatus::Loading,
                });
                s.note_epoch
            }
            None => return,
        };
        let ix = match &self.modal {
            Some(p) => p.settings.rows.len() - 1,
            None => return,
        };
        let epoch = self.settings_epoch;
        cx.spawn(async move |this, cx| {
            let name = service.clone();
            let key = cx.background_spawn(async move { secrets::read(&service) }).await;
            this.update(cx, |this, cx| {
                if this.settings_epoch != epoch {
                    return;
                }
                if let Some(p) = &mut this.modal {
                    let s = &mut p.settings;
                    if s.note_epoch == note_epoch {
                        s.note = Some(match &key {
                            Some(_) => format!("\u{201c}{name}\u{201d} found in the keychain"),
                            None => format!("no \u{201c}{name}\u{201d} in the keychain \u{2014} press Paste to add one"),
                        }.into());
                        if let Some(row) = s.rows.get_mut(ix) {
                            row.status = KeyStatus::of(key);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn focus_query(&mut self, _: &FocusQuery, window: &mut Window, cx: &mut Context<Self>) {
        // Not while a modal owns the screen (the field is dimmed behind it) or a
        // native sheet has the window.
        if self.modal.is_some() || self.busy {
            return;
        }
        let handle = self.query.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// Escape in the field: clear it, hand focus back to the chart, and drop the ask state.
    fn dismiss_query(&mut self, _: &DismissQuery, window: &mut Window, cx: &mut Context<Self>) {
        // The binding sits at EditableText depth, so this also fires from Settings
        // fields — where escape should close the modal instead.
        if self.modal.is_some() {
            self.close_modal(window, cx);
            return;
        }
        self.query.update(cx, |state, cx| state.emplace("", cx));
        window.focus(&self.focus_handle, cx);
        if let Some(r) = self.results() {
            r.search_version += 1;
            r.matches = None;
            r.ask = AskState::Idle;
        }
        cx.notify();
    }

    /// The query text changed: re-run exact matching in the background, debounced, and
    /// drop the result if the text has moved on. No network — this is the always-on
    /// baseline that the providers only add judgement to.
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).as_str().trim().to_string();
        let Some(r) = self.results() else { return };
        r.search_version += 1;
        let version = r.search_version;
        r.ask = AskState::Idle;
        if query.is_empty() {
            r.matches = None;
            cx.notify();
            return;
        }
        let (tree, findings) = (r.tree.clone(), r.findings.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let groups = cx.background_spawn(async move { ask::candidates(&tree, &findings, &query) }).await;
            this.update(cx, |this, cx| {
                let Some(r) = this.results() else { return };
                if r.search_version != version {
                    return;
                }
                r.matches = (!groups.is_empty()).then(|| Arc::new(groups));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Enter in the field: ask the providers what the query means, then act on it.
    /// A delete answer pre-selects the Collector; nothing is ever trashed from the
    /// answer alone.
    fn submit_query(&mut self, _: &SubmitQuery, window: &mut Window, cx: &mut Context<Self>) {
        // Enter lives at EditableText depth, so it fires from Settings fields too —
        // there it means "check" or "save", never "ask".
        if self.modal.is_some() {
            self.settings_enter(window, cx);
            return;
        }
        let query = self.query.read(cx).as_str().trim().to_string();
        if query.is_empty() {
            return;
        }
        let Some(r) = self.results() else { return };
        r.query = query.clone().into();
        r.ask = AskState::Thinking;
        r.search_version += 1;
        let version = r.search_version;
        let tree = r.tree.clone();
        let findings = r.findings.clone();
        let scan_root = r.requested_root.clone();
        let focused = (r.focus != Tree::ROOT).then(|| tree.path_of(r.focus));
        // The folder in view when the query was asked: a "here"-scoped answer is
        // enforced against this, not wherever the user navigated while it thought.
        let focused_ix = r.focus;
        cx.spawn(async move |this, cx| {
            let (groups, answer) = cx
                .background_spawn(async move {
                    let groups = ask::candidates(&tree, &findings, &query);
                    if groups.is_empty() {
                        (groups, Err(ProviderError::BadResponse("no exact matches".into())))
                    } else {
                        let ctx = ask::AskContext { scan_root, focused };
                        let answer = ask::resolve(&ask::providers(), &query, &ctx, &groups);
                        (groups, answer)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(r) = this.results() else { return };
                if r.search_version != version {
                    return;
                }
                r.matches = (!groups.is_empty()).then(|| Arc::new(groups));
                this.ask_resolved(answer, focused_ix, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Apply a provider's answer — or its failure — to the results.
    fn ask_resolved(&mut self, answer: Result<Understanding, ProviderError>, focus: usize, cx: &mut Context<Self>) {
        let mut collect_now = false;
        if let Some(r) = self.results() {
            match answer {
                Ok(u) if u.group != usize::MAX => {
                    // Which reader answered is a developer's concern, never shown in the UI.
                    if std::env::var_os("BROOM_ASK_DEBUG").is_some() {
                        eprintln!("ask: answered by {}", u.provider);
                    }
                    let group = r.matches.as_ref().and_then(|m| m.get(u.group)).cloned().map(Arc::new);
                    if let Some(group) = group {
                        match u.intent {
                            Intent::Delete => {
                                // Pre-select only what the safety rules allow; the Collector
                                // shows the exact list and the confirm prompt does the rest.
                                // Every group the words named, not just the best one.
                                let mut chosen: Vec<usize> = u.groups.clone();
                                if !chosen.contains(&u.group) {
                                    chosen.insert(0, u.group);
                                }
                                for gix in chosen {
                                    let Some(g) = r.matches.as_ref().and_then(|m| m.get(gix)).cloned() else { continue };
                                    // A substring or kind match is shown, never pre-selected.
                                    if !g.source.auto_tick() {
                                        continue;
                                    }
                                    for ix in ask::collectable(&r.tree, &r.homes, &g, u.scope, focus) {
                                        r.collect(ix);
                                    }
                                }
                                collect_now = !r.collector.is_empty();
                            }
                            Intent::Locate => {
                                if let Some(&ix) = group.nodes.iter().find(|&&ix| r.tree.nodes[ix].kind == Kind::Dir) {
                                    r.navigate(ix);
                                }
                            }
                        }
                        r.ask = AskState::Answered { intent: u.intent, group };
                    } else {
                        r.ask = AskState::Suggestions;
                    }
                }
                // Unsure of the group, or sure nothing matched: the cards decide.
                Ok(_) | Err(ProviderError::BadResponse(_)) => r.ask = AskState::Suggestions,
                Err(ProviderError::NoKey) => r.ask = AskState::Unavailable(Unavailable::NoKey),
                Err(ProviderError::Unauthorized) => r.ask = AskState::Unavailable(Unavailable::Rejected),
                Err(ProviderError::Http(_)) => r.ask = AskState::Unavailable(Unavailable::Unreachable),
            }
        }
        if collect_now {
            self.collector_changed(cx);
        }
        cx.notify();
    }

    fn scanning(&mut self) -> Option<&mut Scanning> {
        match &mut self.screen {
            Screen::Scanning(s) => Some(s),
            _ => None,
        }
    }

    fn live_focus(&mut self, path: Vec<SharedString>, cx: &mut Context<Self>) {
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

    fn live_hover(&mut self, path: Option<Vec<SharedString>>, cx: &mut Context<Self>) {
        if let Some(scanning) = self.scanning() {
            if scanning.hover != path {
                scanning.hover = path;
                cx.notify();
            }
        }
    }

    /// Work out what the Collector frees, off the UI thread.
    fn collector_changed(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.results() else { return };
        r.collector_version += 1;
        r.collector_frees.set(None);
        let version = r.collector_version;
        let paths: Vec<PathBuf> = r.collector.iter().map(|&ix| r.tree.path_of(ix)).collect();
        if paths.is_empty() {
            r.collector_frees.set(Some(0));
            return;
        }
        cx.spawn(async move |this, cx| {
            let frees = cx.background_spawn(async move { scan::frees_of(&paths) }).await;
            this.update(cx, |this, cx| {
                if let Some(r) = this.results() {
                    if r.collector_version == version {
                        r.collector_frees.set(Some(frees));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Findings whose real saving needs a clone survey (e.g. node_modules).
    fn resolve_pending_findings(&mut self, cx: &mut Context<Self>) {
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

    /// For the recorder: has the scan finished?
    #[cfg(feature = "snapshot")]
    pub fn showing_results(&self) -> bool {
        matches!(self.screen, Screen::Results(_))
    }

    fn results(&mut self) -> Option<&mut Results> {
        match &mut self.screen {
            Screen::Results(r) => Some(r),
            _ => None,
        }
    }

    fn set_color_by(&mut self, color_by: ColorBy, cx: &mut Context<Self>) {
        self.color_by = color_by;
        self.settings.color_by = match color_by {
            ColorBy::Folder => settings::ColorBy::Folder,
            ColorBy::Kind => settings::ColorBy::Kind,
        };
        let _ = settings::save(&self.settings);
        if let Some(r) = self.results() {
            r.set_color_by(color_by);
        }
        cx.notify();
    }

    /// Toggle the menu-bar 🧹 icon on or off at runtime. Creating the status
    /// item requires the main thread, which the UI thread is.
    fn toggle_menubar(&mut self, cx: &mut Context<Self>) {
        self.settings.menubar = !self.settings.menubar;
        if self.settings.menubar {
            if let Some(mtm) = MainThreadMarker::new() {
                let startup = startup_fraction(&self.volumes);
                self.menubar = Some(menubar::MenuBar::new(mtm, startup));
                self.menubar.as_ref().unwrap().update(startup, &volume_lines(&self.volumes));
            }
        } else {
            // Dropping the MenuBar releases the NSStatusItem; AppKit removes
            // it from the bar on the next event-loop iteration. The volume
            // watch stays: the rail reads it too.
            self.menubar = None;
        }
        let _ = settings::save(&self.settings);
        cx.notify();
    }

    fn set_chart_hover(&mut self, hit: Option<Hit>, cx: &mut Context<Self>) {
        if let Some(r) = self.results() {
            if r.chart_hover != hit {
                r.chart_hover = hit;
                cx.notify();
            }
        }
    }

    fn chart_click(&mut self, hit: Hit, cx: &mut Context<Self>) {
        let Some(r) = self.results() else { return };
        match hit {
            Hit::Center => {
                if let Some(parent) = r.tree.nodes[r.focus].parent {
                    r.navigate(parent);
                }
            }
            Hit::Segment(i) => {
                if let Some(Target::Node(ix)) = r.segments.get(i).map(|s| s.target) {
                    r.navigate(ix);
                }
            }
        }
        cx.notify();
    }

    /// "Review & move to Trash": open the review card over the staged list.
    fn open_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.modal.is_some() {
            return;
        }
        let Some(r) = self.results() else { return };
        if r.collector.is_empty() {
            return;
        }
        let items = r.collector.clone();
        let paths: Vec<(usize, PathBuf)> = items.iter().map(|&ix| (ix, r.tree.path_of(ix))).collect();
        self.context_menu = None;
        self.review = Some(ReviewState { items, frees: HashMap::new(), working: false, drop: None });
        // Per-item savings, one at a time in the background; dropped if the card closed.
        cx.spawn(async move |this, cx| {
            for (ix, path) in paths {
                let frees = cx.background_spawn(async move { scan::frees_of(&[path]) }).await;
                let open = this
                    .update(cx, |this, cx| {
                        let Some(review) = &mut this.review else { return false };
                        review.frees.insert(ix, frees);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !open {
                    break;
                }
            }
        })
        .detach();
        window.focus(&self.modal_focus, cx);
        cx.notify();
    }

    fn close_review(&mut self, _: &CloseReview, window: &mut Window, cx: &mut Context<Self>) {
        if self.review.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    /// The review card's "Move to Trash": the only place anything is deleted.
    fn trash_staged(&mut self, cx: &mut Context<Self>) {
        let Some(review) = &mut self.review else { return };
        if review.working {
            return;
        }
        review.working = true;
        let items = review.items.clone();
        let Some(r) = self.results() else { return };
        // Only what's still in the tree and still staged.
        let items: Vec<usize> = items.into_iter().filter(|ix| r.tree.nodes[*ix].parent.is_some() && r.collector.contains(ix)).collect();
        if items.is_empty() {
            self.review = None;
            cx.notify();
            return;
        }
        let paths: Vec<PathBuf> = items.iter().map(|&ix| r.tree.path_of(ix)).collect();
        let paths_after = paths.clone();
        let freed = r.collector_frees.get().unwrap_or_else(|| r.collected_size());
        let sizes: Vec<(scan::Kind, u64)> = items.iter().map(|&ix| (r.tree.nodes[ix].kind, r.tree.nodes[ix].size)).collect();
        let drop = trashcan::Drop::new(trashcan::Item::from_sizes(&sizes), freed);
        if let Some(review) = &mut self.review {
            review.drop = Some(drop.clone());
        }
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { trashcan::send(&paths) }).await;
            if result.is_ok() {
                // Let the last item land and the lid close before the card goes.
                drop.settle();
                cx.background_executor().timer(drop.remaining()).await;
            }
            this.update(cx, |this, cx| {
                this.review = None;
                match result {
                    Ok(()) => {
                        this.remove_trashed(&items, cx);
                        let what = if items.len() == 1 { "1 item".to_string() } else { format!("{} items", items.len()) };
                        this.show_banner(
                            Some(format!("Moved {what} ({}) to the Trash. Empty the Trash to get the space back.", format_size(freed))),
                            cx,
                        );
                    }
                    Err(error) => {
                        // Not atomic: whatever did go should leave the tree too.
                        let gone: Vec<usize> = items.iter().copied().zip(&paths_after).filter(|(_, p)| !p.exists()).map(|(ix, _)| ix).collect();
                        if !gone.is_empty() {
                            this.remove_trashed(&gone, cx);
                        }
                        this.error = Some(format!("Couldn’t move to Trash: {error}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Keyboard in the navigator: ↑/↓ move the selection along the list.
    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.modal.is_some() || self.review.is_some() {
            return;
        }
        let Some(r) = self.results() else { return };
        if r.order.is_empty() {
            return;
        }
        let last = r.order.len() - 1;
        let pos = match r.selected.and_then(|s| r.order.iter().position(|&o| o == s)) {
            None if delta >= 0 => 0,
            None => last,
            Some(p) => (p as isize + delta).clamp(0, last as isize) as usize,
        };
        r.selected = Some(r.order[pos]);
        r.list_scroll.scroll_to_item(pos, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    /// → or Enter: open the selected folder.
    fn open_selected(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() || self.review.is_some() {
            return;
        }
        let Some(r) = self.results() else { return };
        if let Some(ix) = r.selected {
            if r.tree.nodes[ix].kind == Kind::Dir && !r.tree.nodes[ix].children.is_empty() {
                r.navigate(ix);
                cx.notify();
            }
        }
    }

    /// Space: stage or unstage the selected row.
    fn toggle_selected(&mut self, _: &ToggleStaged, _: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() || self.review.is_some() {
            return;
        }
        let Some(r) = self.results() else { return };
        let Some(ix) = r.selected else { return };
        if !r.stageable(ix) {
            return;
        }
        r.toggle_staged(ix);
        self.collector_changed(cx);
        cx.notify();
    }

    /// A checkbox on a row was clicked.
    fn click_tick(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(r) = self.results() {
            if r.stageable(ix) {
                r.toggle_staged(ix);
            }
        }
        self.collector_changed(cx);
        cx.notify();
    }

    /// A checkbox on a group (a finding, an ask card) was clicked.
    fn click_group_tick(&mut self, nodes: &[usize], cx: &mut Context<Self>) {
        if let Some(r) = self.results() {
            r.toggle_group(nodes);
        }
        self.collector_changed(cx);
        cx.notify();
    }

    fn unstage(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(r) = self.results() {
            r.collector.retain(|&c| c != ix);
        }
        self.collector_changed(cx);
        cx.notify();
    }
}

impl Render for Broom {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The middle two columns (navigator + chart) depend on the screen; the
        // rail on the left and the cleanup column on the right are always there.
        let content = match &self.screen {
            Screen::Start => self.render_empty(cx).into_any_element(),
            Screen::Scanning(scanning) => self.render_scanning(scanning, cx).into_any_element(),
            Screen::Results(_) => self.render_results(window, cx).into_any_element(),
        };
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(self.render_rail(cx))
            .child(div().flex_1().min_w_0().h_full().flex().child(content))
            .child(self.render_cleanup(cx));

        // A menu is only rendered while its node is still part of the tree.
        let menu = match (&self.screen, &self.context_menu) {
            (Screen::Results(r), Some(m)) if r.tree.nodes[m.node].parent.is_some() || m.node == Tree::ROOT => {
                Some(m.clone())
            }
            (Screen::Results(_), Some(_)) => {
                self.context_menu = None;
                None
            }
            _ => None,
        };

        div()
            .id("broom")
            .key_context("Broom")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::rescan))
            .on_action(cx.listener(Self::start_over))
            .on_action(cx.listener(Self::go_up))
            .on_action(cx.listener(Self::focus_query))
            .on_action(cx.listener(Self::submit_query))
            .on_action(cx.listener(Self::dismiss_query))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::close_settings))
            .on_action(cx.listener(Self::show_about))
            .on_action(cx.listener(Self::close_about))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::open_selected))
            .on_action(cx.listener(Self::toggle_selected))
            .on_action(cx.listener(Self::close_review))
            .size_full()
            .flex()
            .flex_col()
            .bg(to_hsla(BG))
            .text_color(rgb(TEXT))
            .font_family(".SystemUIFont")
            .text_sm()
            .child(self.render_toolbar(cx))
            .child(body)
            // Any click outside the menu closes it (the menu stops this handler).
            .on_mouse_down(MouseButton::Left, cx.listener(close_menu_on_outside))
            .on_mouse_down(MouseButton::Right, cx.listener(close_menu_on_outside))
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .id("error")
                        .occlude()
                        .absolute()
                        .bottom_4()
                        .right_4()
                        .max_w(px(420.))
                        .px_4()
                        .py_2()
                        .rounded_lg()
                        .bg(rgb(DANGER))
                        .text_color(rgb(0xffffff))
                        .cursor_pointer()
                        .child(error)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.error = None;
                            cx.notify();
                        })),
                )
            })
            .when_some(menu, |el, menu| el.child(self.render_context_menu(menu, cx)))
            .when_some(self.render_modal(cx), |el, modal| el.child(modal))
    }
}

impl Broom {
    /// The dimmed backdrop with the modal card on top. Clicking the backdrop closes;
    /// the card carries a key context so Escape can be bound to closing it.
    fn render_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (context, card) = match (&self.review, &self.modal) {
            (Some(review), _) => ("Review", self.render_review(review, cx).into_any_element()),
            (None, Some(p)) => (
                match p.tab {
                    PanelTab::Keys => "Settings",
                    PanelTab::Help => "Panel",
                    PanelTab::About => "About",
                },
                self.render_panel(p, cx).into_any_element(),
            ),
            (None, None) => return None,
        };
        Some(
            deferred(
                div()
                    .id("modal-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui::hsla(0., 0., 0., 0.55))
                    // Occlude: hover, clicks and tooltips on the dimmed UI behind
                    // the card are dead while the modal is up.
                    .occlude()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_any_modal(window, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| this.close_any_modal(window, cx)))
                    .child(
                        div()
                            .id("modal-card")
                            .key_context(context)
                            .track_focus(&self.modal_focus)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(card),
                    ),
            )
            .into_any_element(),
        )
    }

    /// The panel card: tabs across the top, the tab's content below.
    fn render_panel(&self, p: &PanelState, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
            .text_xs();
        for (label, tab) in [
            ("Settings", PanelTab::Keys),
            ("Help", PanelTab::Help),
            ("About", PanelTab::About),
        ] {
            let selected = p.tab == tab;
            tabs = tabs.child(
                div()
                    .id(("panel-tab", tab as usize))
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |d| {
                        d.bg(rgb(CARD_HOVER)).text_color(rgb(TEXT)).font_weight(FontWeight::MEDIUM)
                    })
                    .when(!selected, |d| d.text_color(rgb(MUTED)).hover(|s| s.text_color(rgb(TEXT))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = &mut this.modal {
                            p.tab = tab;
                            // Each tab starts at the top, with nothing measured yet.
                            p.scroll.set_offset(point(px(0.), px(0.)));
                            p.content_top.set(None);
                            p.content_bottom.set(None);
                        }
                        cx.notify();
                    }))
                    .child(label),
            );
        }

        // Zero-height canvases at the content's ends record where it starts and
        // stops. When a recorded end moves (a tab switch, a row growing), one
        // more frame keeps the scrollbar truthful.
        let measure = |cell: Rc<Cell<Option<f32>>>| {
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let y: f32 = bounds.origin.y.into();
                    if cell.get() != Some(y) {
                        cell.set(Some(y));
                        window.request_animation_frame();
                    }
                },
            )
            .w_full()
            .h(px(0.))
        };

        // Height follows the tab's content, capped: the header (tabs + Done)
        // stays put and only the content scrolls, in a viewport with Broom's
        // own scrollbar beside it — none when it all fits.
        div()
            .id("panel-card")
            .w(px(520.))
            .max_h(px(560.))
            .p_4()
            .rounded_xl()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div().flex().flex_none().items_center().justify_between().child(tabs).child(
                    button("panel-done", "Done").on_click(cx.listener(|this, _, window, cx| this.close_modal(window, cx))),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("panel-scroll")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .track_scroll(&p.scroll)
                            .child(
                                div()
                                    .id("panel-content")
                                    .flex()
                                    .flex_col()
                                    .child(measure(p.content_top.clone()))
                                    .child(match p.tab {
                                        PanelTab::Keys => self.render_settings(&p.settings, cx).into_any_element(),
                                        PanelTab::Help => render_help().into_any_element(),
                                        PanelTab::About => self.render_about(cx).into_any_element(),
                                    })
                                    .child(measure(p.content_bottom.clone())),
                            ),
                    )
                    .child(self.render_panel_scrollbar(p)),
            )
    }

    /// The panel's own scrollbar: a slim column beside the scrolling content,
    /// painted as track + rounded thumb and dragged through raw mouse events.
    /// `ScrollHandle::set_offset` writes exactly what it's handed — no
    /// clamping — so every write here clamps first.
    fn render_panel_scrollbar(&self, p: &PanelState) -> impl IntoElement {
        let scroll = p.scroll.clone();
        let top_cell = p.content_top.clone();
        let bottom_cell = p.content_bottom.clone();
        let drag = p.drag.clone();
        canvas(
            move |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _| {
                let Some(top) = top_cell.get() else { return };
                let Some(bottom) = bottom_cell.get() else { return };
                let viewport = scroll.bounds();
                let (content_h, viewport_h, track_h): (f32, f32, f32) =
                    (bottom - top, viewport.size.height.into(), bounds.size.height.into());
                // Nothing overflows, or nothing measurable yet: no scrollbar.
                if content_h <= viewport_h + 0.5 || viewport_h <= 0.0 || track_h <= 1.0 {
                    return;
                }
                let max_scroll = content_h - viewport_h;
                let offset: f32 = scroll.offset().y.into();
                let progress = (offset / max_scroll).clamp(0.0, 1.0);
                let thumb_h = (viewport_h / content_h * track_h).clamp(24.0, track_h);
                let travel = track_h - thumb_h;
                let left: f32 = bounds.origin.x.into();
                let track_top: f32 = bounds.origin.y.into();
                let thumb_top = track_top + travel * progress;

                // Track and thumb, light on the dark card and quiet at rest.
                window.paint_quad(quad(
                    Bounds::from_corners(
                        point(px(left + 1.), px(track_top + 1.)),
                        point(px(left + 9.), px(track_top + track_h - 1.)),
                    ),
                    px(4.),
                    gpui::hsla(0., 0., 1., 0.05),
                    px(0.),
                    gpui::hsla(0., 0., 0., 0.),
                    BorderStyle::default(),
                ));
                let held = drag.get().is_some();
                window.paint_quad(quad(
                    Bounds::from_corners(point(px(left + 2.), px(thumb_top)), point(px(left + 8.), px(thumb_top + thumb_h))),
                    px(3.),
                    gpui::hsla(0., 0., 1., if held { 0.38 } else { 0.22 }),
                    px(0.),
                    gpui::hsla(0., 0., 0., 0.),
                    BorderStyle::default(),
                ));

                if hitbox.is_hovered(window) {
                    window.set_cursor_style(CursorStyle::ResizeUpDown, &hitbox);
                }

                // Press: on the thumb, drag from where it is; on the track,
                // page the thumb there first and drag from that spot.
                let (scroll_d, drag_d, hitbox_d) = (scroll.clone(), drag.clone(), hitbox.clone());
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !hitbox_d.is_hovered(window) {
                        return;
                    }
                    let y: f32 = event.position.y.into();
                    let from = if (thumb_top..=thumb_top + thumb_h).contains(&y) {
                        offset
                    } else {
                        ((y - track_top - thumb_h / 2.0) / travel).clamp(0.0, 1.0) * max_scroll
                    };
                    drag_d.set(Some((y, from)));
                    scroll_d.set_offset(point(px(0.), px(from.clamp(0.0, max_scroll))));
                    window.request_animation_frame();
                });
                // Move while held: follow the pointer, clamped to the content.
                let (scroll_m, drag_m) = (scroll.clone(), drag.clone());
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    // No hitbox check: a drag that wanders off the scrollbar keeps going.
                    let Some((y0, from)) = drag_m.get() else { return };
                    let y: f32 = event.position.y.into();
                    let to = (from + (y - y0) * (max_scroll / travel)).clamp(0.0, max_scroll);
                    scroll_m.set_offset(point(px(0.), px(to)));
                    window.request_animation_frame();
                });
                let drag_u = drag.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, window, _| {
                    if phase == DispatchPhase::Bubble && drag_u.replace(None).is_some() {
                        window.request_animation_frame();
                    }
                });
            },
        )
        .w(px(10.))
        .flex_none()
        .h_full()
    }

    fn render_settings(&self, s: &SettingsState, cx: &mut Context<Self>) -> impl IntoElement {
        let mut card = div()
            .id("settings-card")
            .flex()
            .flex_col()
            .gap_3();

        // Salvaged-files notice: shown once if the last session's settings
        // were damaged and moved aside. Cleared on the open that shows it.
        if let Some(ref salvaged) = self.settings_salvaged {
            card = card.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(0x2a2818))
                    .border_1()
                    .border_color(rgb(0x4a4428))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().text_color(rgb(0xc8b86e)).child(
                        "Your settings file was damaged. It has been moved aside; Broom started with defaults.",
                    ))
                    .child(
                        div().text_xs().text_color(rgb(MUTED)).child(format!("Location: {}", salvaged.display())),
                    ),
            );
        }
        // Cleared in open_panel so it doesn't reappear on subsequent opens.

        // --- General -----------------------------------------------------------
        card = card
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child("General")),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Show menu-bar 🧹 icon"))
                    .child(
                        div()
                            .id("toggle-menubar")
                            .rounded_md()
                            .px_2()
                            .py_0p5()
                            .text_xs()
                            .cursor_pointer()
                            .when(self.settings.menubar, |d| {
                                d.bg(rgb(ACCENT)).text_color(rgb(0xffffff))
                            })
                            .when(!self.settings.menubar, |d| {
                                d.bg(rgb(CARD_HOVER)).text_color(rgb(MUTED))
                            })
                            .hover(|d| d.text_color(rgb(TEXT)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_menubar(cx);
                            }))
                            .child(if self.settings.menubar { "On" } else { "Off" }),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Colour chart by"))
                    .child(
                        div().flex().gap_1().children(
                            ["Folder", "Kind"].iter().enumerate().map(|(ix, &label)| {
                                let active = match (self.color_by, self.settings.color_by) {
                                    (_, settings::ColorBy::Folder) => label == "Folder",
                                    (_, settings::ColorBy::Kind) => label == "Kind",
                                };
                                let target = match label {
                                    "Kind" => ColorBy::Kind,
                                    _ => ColorBy::Folder,
                                };
                                div()
                                    .id(("color-by", ix))
                                    .rounded_md()
                                    .px_2()
                                    .py_0p5()
                                    .text_xs()
                                    .cursor_pointer()
                                    .when(active, |d| d.bg(rgb(ACCENT)).text_color(rgb(0xffffff)))
                                    .when(!active, |d| d.text_color(rgb(MUTED)).hover(|s| s.text_color(rgb(TEXT))))
                                    .on_click(cx.listener(move |this, _, _, cx| this.set_color_by(target, cx)))
                                    .child(label)
                            }),
                        ),
                    ),
            );

        // --- Scanning ----------------------------------------------------------
        card = card
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Scanning")),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Outline depth"))
                    .child(
                        div().flex().gap_1().children(
                            (0..=4).map(|d| {
                                let active = self.settings.outline_depth == d;
                                div()
                                    .id(("outline-depth", d))
                                    .rounded_md()
                                    .px_2()
                                    .py_0p5()
                                    .text_xs()
                                    .cursor_pointer()
                                    .when(active, |d| d.bg(rgb(ACCENT)).text_color(rgb(0xffffff)))
                                    .when(!active, |d| d.text_color(rgb(MUTED)).hover(|s| s.text_color(rgb(TEXT))))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.settings.outline_depth = d;
                                        let _ = settings::save(&this.settings);
                                        cx.notify();
                                    }))
                                    .child(d.to_string())
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Banner duration (seconds)"))
                    .child(
                        div().flex().gap_1().children(
                            [4usize, 8, 15, 30].iter().map(|&s| {
                                let active = self.settings.banner_seconds == s as u64;
                                div()
                                    .id(("banner-sec", s))
                                    .rounded_md()
                                    .px_2()
                                    .py_0p5()
                                    .text_xs()
                                    .cursor_pointer()
                                    .when(active, |d| d.bg(rgb(ACCENT)).text_color(rgb(0xffffff)))
                                    .when(!active, |d| d.text_color(rgb(MUTED)).hover(|s| s.text_color(rgb(TEXT))))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.settings.banner_seconds = s as u64;
                                        let _ = settings::save(&this.settings);
                                        cx.notify();
                                    }))
                                    .child(format!("{s}"))
                            }),
                        ),
                    ),
            );

        // --- Excluded folders ---------------------------------------------------
        card = card.child(
            div()
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(rgb(CARD))
                .border_1()
                .border_color(rgb(BORDER))
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_sm().child("Excluded folders"))
                .child(
                    div().flex().flex_col().gap_1().children(
                        self.settings.excludes.iter().enumerate().map(|(ix, path)| {
                            let path = path.clone();
                            div()
                                .id(("exclude-item", ix))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(div().text_xs().font_family("Menlo").truncate().child(path.clone()))
                                .child(
                                    div()
                                        .id(("exclude-remove", ix))
                                        .text_xs()
                                        .text_color(rgb(DANGER))
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.settings.excludes.retain(|p| *p != path);
                                            let _ = settings::save(&this.settings);
                                            cx.notify();
                                        }))
                                        .child("✕"),
                                )
                        }),
                    ),
                )
                .child(
                    div().flex().gap_2().items_center().child(
                        div()
                            .flex_1()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                text_input("exclude-path")
                                    .placeholder("e.g. ~/Downloads")
                                    .state(s.exclude_input.downgrade())
                                    .caret_blink_interval_500ms()
                                    .w_full()
                                    .h(px(28.))
                                    .px_2()
                                    .min_h_auto()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .bg(rgb(BG))
                                    .text_size(px(12.)),
                            ),
                    )
                    .child(
                        button("exclude-add", "Add").on_click(cx.listener(|this, _, _, cx| {
                            let text = this.modal.as_ref().and_then(|p| {
                                let input = p.settings.exclude_input.read(cx);
                                let text = input.as_str();
                                if text.trim().is_empty() { None } else { Some(text.to_string()) }
                            });
                            if let Some(path) = text {
                                if !this.settings.excludes.contains(&path) {
                                    this.settings.excludes.push(path);
                                    let _ = settings::save(&this.settings);
                                }
                                if let Some(p) = &mut this.modal {
                                    p.settings.exclude_input.update(cx, |input, cx| input.emplace("", cx));
                                }
                                cx.notify();
                            }
                        })),
                    ),
                ),
        );

        // --- API keys ----------------------------------------------------------
        card = card
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child("API keys"))
                    .child(
                        div().text_xs().text_color(rgb(MUTED)).child(
                            "Stored in your login keychain. Broom shows only the last characters — never the key itself.",
                        ),
                    )
                    .child(
                        div().text_xs().text_color(rgb(MUTED)).child(
                            "A fallback reader needs no key — it answers whenever the main one can't.",
                        ),
                    ),
            );
        for (ix, row) in s.rows.iter().enumerate() {
            card = card.child(self.render_key_row(ix, row, cx));
        }
        card = card.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().text_color(rgb(MUTED)).child("Check another key name:"))
                .child(
                    div().flex().gap_2().items_center().child(
                        div()
                            .flex_1()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                text_input("custom-key")
                                    .placeholder("e.g. openai")
                                    .state(s.custom.downgrade())
                                    .caret_blink_interval_500ms()
                                    .w_full()
                                    .h(px(28.))
                                    .px_2()
                                    .min_h_auto()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .bg(rgb(BG))
                                    .text_size(px(12.)),
                            ),
                    )
                    .child(
                        button("check-key", "Check").on_click(cx.listener(|this, _, _, cx| this.add_custom_key(cx))),
                    ),
                ),
        );
        if let Some(note) = &s.note {
            card = card.child(div().text_xs().text_color(rgb(MUTED)).child(note.clone()));
        }
        card
    }

    fn render_key_row(&self, ix: usize, row: &KeyRow, cx: &mut Context<Self>) -> impl IntoElement {
        // Env keys outrank the keychain; say so when one is set.
        let hint = match row.service.as_ref() {
            secrets::JEV_SERVICE
                if std::env::var_os("TYPESAFE_API_KEY").or_else(|| std::env::var_os("JEV_API_KEY")).is_some() =>
                format!("{} An env key is set and takes precedence.", row.hint),
            _ => row.hint.to_string(),
        };
        let mut head = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().font_weight(FontWeight::MEDIUM).child(row.title.clone()))
                    .child(div().text_xs().text_color(rgb(MUTED)).truncate().child(hint)),
            );
        match &row.status {
            KeyStatus::Loading => {
                head = head.child(div().text_xs().text_color(rgb(MUTED)).child("checking keychain…"));
            }
            KeyStatus::Missing => {
                head = head.child(
                    button(("key-add", ix), "Add")
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_key(ix, cx))),
                );
            }
            KeyStatus::Set { masked } => {
                head = head
                    .child(div().text_xs().font_family("Menlo").text_color(rgb(MUTED)).child(masked.clone()))
                    .child(
                        button(("key-edit", ix), "Update")
                            .on_click(cx.listener(move |this, _, _, cx| this.edit_key(ix, cx))),
                    )
                    .child(
                        button(("key-remove", ix), "Remove")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_key(ix, cx))),
                    );
            }
            KeyStatus::Editing | KeyStatus::Saving => {}
        }
        let mut line = div()
            .id(("key-row", ix))
            .px_3()
            .py_2()
            .rounded_lg()
            .bg(rgb(CARD))
            .border_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .gap_1()
            .child(head);
        if matches!(row.status, KeyStatus::Editing) {
            // No text field: the key is pasted from the clipboard straight into
            // the keychain, so it is never displayed.
            line = line.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(0x1e2840))
                    .border_1()
                    .border_color(rgb(0x2a3d5c))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div().text_xs().text_color(rgb(0x8bb8ff)).child(
                            "1. Copy the API key to your clipboard  (⌘C)",
                        ),
                    )
                    .child(
                        div().text_xs().text_color(rgb(0x8bb8ff)).child(
                            "2. Click \"Paste from Clipboard\" below — Broom reads it from there, never shows it on screen.",
                        ),
                    )
                    .child(
                        div().flex().gap_2().items_center().justify_end().child(
                            primary_button(("key-paste", ix), "Paste from Clipboard", ACCENT)
                                .on_click(cx.listener(move |this, _, _, cx| this.paste_key(ix, cx))),
                        )
                        .child(
                            button(("key-cancel", ix), "Cancel")
                                .on_click(cx.listener(move |this, _, _, cx| this.cancel_edit_key(ix, cx))),
                        ),
                    ),
            );
        }
        if matches!(row.status, KeyStatus::Saving) {
            line = line.child(
                div().text_xs().text_color(rgb(MUTED)).child("working in the keychain\u{2026}"),
            );
        }
        line
    }

    fn render_about(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .pt_2()
            .child(img(ABOUT_MARK.clone()).size(px(72.)).flex_none())
            .child(div().text_3xl().font_weight(FontWeight::BOLD).child("Broom"))
            .child(div().text_color(rgb(MUTED)).child(format!("Version {}", env!("CARGO_PKG_VERSION"))))
            .child(div().mt_2().text_sm().child("A disk-space explorer for macOS."))
            .child(
                div()
                    .px_6()
                    .text_center()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("Ask in plain language — the reader judges the words, code matches the files, you confirm the list."),
            )
            .child(div().mt_3().text_sm().child("Made by Srinivas Pendela"))
            .child(
                div().text_xs().text_color(rgb(MUTED)).child("© 2026 Srinivas Pendela · Built with Rust and GPUI"),
            )
    }

    /// Finder-like actions on one node. The APFS volume slices (Kind::Other) aren't
    /// real folders, so they only offer Open/Reveal/Copy.
    fn render_context_menu(&mut self, menu: ContextMenu, cx: &mut Context<Self>) -> AnyElement {
        let (name, path, actionable) = match &self.screen {
            Screen::Results(r) => {
                let node = &r.tree.nodes[menu.node];
                (node.name.clone(), r.tree.path_of(menu.node), node.kind != Kind::Other)
            }
            _ => return div().into_any_element(),
        };
        let node = menu.node;

        let item = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(CARD_HOVER)))
                .child(label)
        };
        let mut items = vec![
            item("menu-open", "Open").on_click({
                let path = path.clone();
                cx.listener(move |this, _, _, cx| {
                    this.context_menu = None;
                    cx.open_with_system(&path);
                    cx.notify();
                })
            }),
            item("menu-reveal", "Reveal in Finder").on_click({
                let path = path.clone();
                cx.listener(move |this, _, _, cx| {
                    this.context_menu = None;
                    cx.reveal_path(&path);
                    cx.notify();
                })
            }),
            item("menu-copy", "Copy Path (relative)").on_click({
                let path = path.clone();
                cx.listener(move |this, _, _, cx| {
                    this.context_menu = None;
                    cx.write_to_clipboard(ClipboardItem::new_string(abbreviate_home(&path.to_string_lossy())));
                    cx.notify();
                })
            }),
            item("menu-copy-abs", "Copy Full Path").on_click({
                let path = path.clone();
                cx.listener(move |this, _, _, cx| {
                    this.context_menu = None;
                    cx.write_to_clipboard(ClipboardItem::new_string(path.to_string_lossy().into_owned()));
                    cx.notify();
                })
            }),
        ];
        if actionable {
            let staged = matches!(&self.screen, Screen::Results(r) if r.tick(node) == Tick::On);
            items.push(item("menu-collect", if staged { "Remove from cleanup plan" } else { "Stage for cleanup" }).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.context_menu = None;
                    this.click_tick(node, cx);
                },
            )));
            items.push(item("menu-trash", "Move to Trash…").on_click(cx.listener(move |this, _, window, cx| {
                this.context_menu = None;
                if let Some(r) = this.results() {
                    r.collect(node);
                }
                this.collector_changed(cx);
                cx.notify();
                this.open_review(window, cx);
            })));
        }

        deferred(
            anchored()
                .position(menu.at)
                .snap_to_window()
                .child(
                    div()
                        .id("context-menu")
                        // Clicks inside the menu are menu business, not "close it".
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .w(px(230.))
                        .p_1()
                        .rounded_lg()
                        .bg(rgb(PANEL))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().px_2().py_1().text_xs().text_color(rgb(MUTED)).truncate().child(name))
                        .children(items),
                ),
        )
        .into_any_element()
    }
}

fn button_base(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .border_1()
        .cursor_pointer()
        .active(|s| s.opacity(0.8))
        // Keep clicks from starting a window drag in the toolbar.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(label.into())
}

fn button(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>) -> Stateful<gpui::Div> {
    button_base(id, label)
        .bg(rgb(CARD))
        .border_color(rgb(BORDER))
        .hover(|s| s.bg(rgb(CARD_HOVER)))
}

fn primary_button(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>, color: u32) -> Stateful<gpui::Div> {
    button_base(id, label)
        .bg(rgb(color))
        .border_color(rgb(color))
        .text_color(rgb(0xffffff))
        .font_weight(FontWeight::MEDIUM)
        .hover(move |s| s.bg(lighten(rgb(color))))
}

fn lighten(color: Rgba) -> Hsla {
    let mut hsla: Hsla = color.into_color();
    hsla.lightness = (hsla.lightness + 0.07).min(1.0);
    hsla
}

fn to_hsla(color: u32) -> Hsla {
    rgb(color).into_color()
}

/// Retune the window's vibrancy, once, on the main thread. GPUI's `Blurred`
/// background uses the `Selection` material, which in dark appearance reads
/// as a flat dark pane — no desktop in it. The under-window material is the
/// one macOS itself uses to frost what's behind a window, which is the glass
/// the Start screen wants. If the vibrancy view isn't there for some reason,
/// add one behind everything rather than leave the window flat.
fn frost_window(mtm: MainThreadMarker) {
    for window in NSApplication::sharedApplication(mtm).windows() {
        let Some(content) = window.contentView() else { continue };
        let existing = content
            .subviews()
            .iter()
            .find(|view| view.isKindOfClass(NSVisualEffectView::class()))
            .map(|view| unsafe { Retained::cast_unchecked::<NSVisualEffectView>(view.clone()) });
        let view = existing.unwrap_or_else(|| {
            let view = NSVisualEffectView::new(mtm);
            view.setFrame(content.bounds());
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            content.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, None);
            view
        });
        view.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
        view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        view.setState(NSVisualEffectState::Active);
    }
}

/// The startup disk's fill, for the menu-bar title.
fn startup_fraction(volumes: &[Volume]) -> f32 {
    volumes
        .first()
        .map(|v| v.total.saturating_sub(v.free) as f32 / v.total.max(1) as f32)
        .unwrap_or(0.0)
}

/// What the menu-bar menu shows per volume: name, capacity, free space.
fn volume_lines(volumes: &[Volume]) -> Vec<(String, u64, u64, u64)> {
    volumes.iter().map(|v| (v.name.clone(), v.total, v.free, v.purgeable)).collect()
}

impl Broom {
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = div()
            .id("toolbar")
            .h(px(46.))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .pl(px(84.))
            .pr_3()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(to_hsla(PANEL))
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            });

        match &self.screen {
            Screen::Results(r) => {
                let can_go_up = r.tree.nodes[r.focus].parent.is_some();
                bar = bar.child(
                    button("up", "‹")
                        .text_base()
                        .px_2()
                        .when(!can_go_up, |b| b.opacity(0.4))
                        .on_click(cx.listener(|this, _, window, cx| this.go_up(&GoUp, window, cx))),
                );
                let chain = r.tree.ancestry(r.focus);
                let mut crumbs = div().flex().items_center().gap_1().min_w_0().overflow_hidden();
                for (i, &ix) in chain.iter().enumerate() {
                    let is_last = i + 1 == chain.len();
                    if i > 0 {
                        crumbs = crumbs.child(div().text_color(rgb(MUTED)).child("›"));
                    }
                    crumbs = crumbs.child(
                        div()
                            .id(("crumb", ix))
                            .px_1p5()
                            .py_0p5()
                            .rounded_md()
                            .flex_shrink(1.)
                            .min_w(px(24.))
                            .truncate()
                            .when(is_last, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .when(!is_last, |d| d.text_color(rgb(MUTED)).flex_shrink_0())
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(CARD_HOVER)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(r) = this.results() {
                                    r.navigate(ix);
                                    cx.notify();
                                }
                            }))
                            .child(r.tree.nodes[ix].name.clone()),
                    );
                }
                bar = bar.child(crumbs).child(div().flex_1());
                // The query field. Stop mouse-down here so a click into it doesn't
                // start a window drag on the toolbar.
                bar = bar.child(
                    div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(
                        text_input("query")
                            .placeholder("Find by name, or ask…")
                            .state(self.query.downgrade())
                            .caret_blink_interval_500ms()
                            .w(px(220.))
                            .h(px(28.))
                            .px_2()
                            .min_h_auto()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(BG))
                            .text_size(px(12.)),
                    ),
                );
                bar = bar
                    .child(color_toggle(r.color_by, cx))
                    .child(
                        div().text_xs().text_color(rgb(MUTED)).flex_none().child(format!(
                            "Scanned in {:.1}s",
                            r.elapsed.as_secs_f32()
                        )),
                    )
                    .child(
                        button("rescan", "Rescan")
                            .on_click(cx.listener(|this, _, window, cx| this.rescan(&Rescan, window, cx))),
                    );
            }
            Screen::Scanning(_) => {
                bar = bar.child(div().flex_1()).child(
                    primary_button("cancel", "Cancel Scan", DANGER).on_click(cx.listener(|this, _, _, cx| this.cancel_scan(cx))),
                );
            }
            // Nothing scanned: the rail is the call to action; up here only the
            // drag surface and Settings.
            Screen::Start => {}
        }
        // Settings is reachable from every screen. A word, not a gear glyph:
        // "⚙" renders through the emoji path and all but disappears on glass.
        bar.child(div().flex_1()).child(
            button("settings", "Settings")
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(&OpenSettings, window, cx)))
                .tooltip(move |_, cx| cx.new(|_| Tip("Settings ⌘,".into())).into()),
        )
    }

    /// The left rail: every disk with its fill, the home folder, synced cloud
    /// folders, a folder picker, and the keyboard hint. Clicking anything scans it.
    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active: Option<PathBuf> = match &self.screen {
            Screen::Results(r) => Some(r.requested_root.clone()),
            Screen::Scanning(s) => Some(s.root.clone()),
            Screen::Start => None,
        };
        let heading = |text: &'static str| {
            div().px_3().pt_3().pb_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(MUTED)).child(text)
        };
        let row = |id: gpui::ElementId, is_active: bool| {
            div()
                .id(id)
                .mx_1p5()
                .px_2()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .when(is_active, |d| d.bg(rgb(CARD_HOVER)))
                .when(!is_active, |d| d.hover(|s| s.bg(rgb(CARD))))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };

        let mut list = div().id("rail-scroll").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().pb_2();
        list = list.child(heading("DISKS"));
        for (i, volume) in self.volumes.iter().enumerate() {
            let used = volume.total.saturating_sub(volume.free);
            let fraction = used as f32 / volume.total.max(1) as f32;
            let hue = sunburst::hue_color(i);
            let is_active = active.as_deref() == Some(volume.path.as_path());
            let path = volume.path.clone();
            list = list.child(
                row(("volume", i).into(), is_active)
                    .on_click(cx.listener(move |this, _, _, cx| this.start_scan(path.clone(), cx)))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(8.)).flex_none().rounded_full().bg(level_color(fraction, hue)))
                            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(volume.name.clone())),
                    )
                    .child(meter(fraction, hue))
                    .child(div().text_xs().text_color(rgb(MUTED)).truncate().child(format!(
                        "{} free of {}",
                        format_size(volume.free),
                        format_size(volume.total)
                    )))
                    // About This Mac counts what macOS would purge on demand as
                    // available; say so, so its bigger number doesn't look like a bug.
                    .when(volume.purgeable >= menubar::PURGEABLE_WORTH_A_WORD, |d| {
                        d.child(
                            div()
                                .id(("purgeable", i))
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .truncate()
                                .tooltip(|_, cx| {
                                    cx.new(|_| Tip("Local Time Machine snapshots, caches and re-downloadable iCloud files. macOS frees them on demand; About This Mac counts them as available.".into())).into()
                                })
                                .child(format!("+{} purgeable", format_size(volume.purgeable))),
                        )
                    }),
            );
        }

        let home = home_dir();
        let home_active = active.as_deref() == Some(home.as_path());
        list = list.child(heading("FOLDERS")).child(
            row("home".into(), home_active)
                .on_click(cx.listener(move |this, _, _, cx| this.start_scan(home_dir(), cx)))
                .flex()
                .flex_col()
                .child(div().font_weight(FontWeight::MEDIUM).child("Home folder"))
                .child(div().text_xs().text_color(rgb(MUTED)).truncate().child(abbreviate_home(&home.to_string_lossy()))),
        );
        if !self.clouds.is_empty() {
            list = list.child(heading("CLOUD"));
            for (i, cloud) in self.clouds.iter().enumerate() {
                let is_active = active.as_deref() == Some(cloud.path.as_path());
                let path = cloud.path.clone();
                list = list.child(
                    row(("cloud", i).into(), is_active)
                        .on_click(cx.listener(move |this, _, _, cx| this.start_scan(path.clone(), cx)))
                        .flex()
                        .flex_col()
                        .child(div().font_weight(FontWeight::MEDIUM).truncate().child(cloud.name.clone()))
                        .child(div().text_xs().text_color(rgb(MUTED)).truncate().child(abbreviate_home(&cloud.path.to_string_lossy()))),
                );
            }
        }
        list = list.child(
            div().px_3().pt_3().child(
                button("choose", "Choose folder…")
                    .w_full()
                    .text_center()
                    .on_click(cx.listener(|this, _, window, cx| this.open_folder(&OpenFolder, window, cx)))
                    .tooltip(|_, cx| cx.new(|_| Tip("Open Folder ⌘O".into())).into()),
            ),
        );

        div()
            .w(px(RAIL_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(list)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("↑↓ → ← navigate · ␣ stage"),
            )
    }

    /// The navigator column's frame, shared by every screen.
    fn navigator_frame(&self) -> gpui::Div {
        div()
            .w(px(NAV_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
    }

    /// Nothing scanned yet: an empty navigator and, where the chart goes, a nudge
    /// towards the rail.
    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = self
            .navigator_frame()
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).text_color(rgb(MUTED)).child("No scan yet"))
                    .child(div().text_xs().text_color(rgb(MUTED)).child("Folders appear here as the scan runs.")),
            )
            .children(self.render_access_card(None, cx));
        let chart = div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .child(div().max_w(px(420.)).child(comic::investigator(comic::idle(), None, 56.0)))
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Pick a disk to scan"))
            .child(
                div()
                    .max_w(px(380.))
                    .text_center()
                    .text_color(rgb(MUTED))
                    .child("Choose a disk or folder on the left. The chart fills in live while the scan runs; findings and the cleanup plan land on the right."),
            );
        div().size_full().flex().child(nav).child(chart)
    }

    /// The right column: findings (whatever the scan has found so far) above
    /// the cleanup plan.
    fn render_cleanup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let col = div()
            .w(px(CLEANUP_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_l_1()
            .border_color(rgb(BORDER));
        match &self.screen {
            Screen::Results(r) => col.child(self.render_findings(&r.findings, Some(r), cx)).child(self.render_plan(r, cx)),
            Screen::Scanning(s) => {
                let early = findings::early_findings(&s.progress.early_findings.lock().unwrap());
                col.child(self.render_findings(&early, None, cx))
                    .child(plan_placeholder("The plan opens when the scan finishes."))
            }
            Screen::Start => col
                .child(self.render_findings(&[], None, cx))
                .child(plan_placeholder("Tick folders and findings to stage them. Nothing moves until you review the plan.")),
        }
    }

    fn render_scanning(&self, scanning: &Scanning, cx: &mut Context<Self>) -> impl IntoElement {
        let progress = &scanning.progress;
        let current = progress.current.lock().map(|c| abbreviate_home(&c)).unwrap_or_default();
        let scanned = progress.bytes.load(Ordering::Relaxed);
        let files = progress.files.load(Ordering::Relaxed);
        let elapsed = clock::since(scanning.started).as_secs_f32();
        let layout = progress.layout.get().cloned();
        let disk_name = layout.as_ref().map(|l| l.name.clone()).unwrap_or_else(|| scan::display_name(&scanning.root));
        let view = scanning.live.clone();
        if view.is_some() && std::env::var_os("BROOM_TIMING").is_some() {
            static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !REPORTED.swap(true, Ordering::Relaxed) {
                if let Some(launched) = LAUNCHED.get() {
                    eprintln!("timing: first live chart {:.0} ms after launch", launched.elapsed().as_secs_f64() * 1000.0);
                }
            }
        }
        let hovered = view.as_ref().zip(scanning.hover.as_ref()).map(|(v, path)| resolve_path(&v.tree, path));

        // Honest sizes: final folders show their size, folders still being counted show a lower bound.
        let size_label = |view: &LiveView, ix: usize| -> String {
            let node = &view.tree.nodes[ix];
            if node.kind == Kind::Other || view.is_final(ix) {
                format_size(node.size)
            } else if node.size == 0 {
                "counting…".to_string()
            } else {
                format!("≥ {}", format_size(node.size))
            }
        };

        // Sidebar: the focused folder's contents.
        let mut rows = div().id("live-rows").flex_1().min_h_0().px_2().overflow_y_scroll().flex().flex_col();
        let mut crumbs = None;
        if let Some(view) = &view {
            let focus = &view.tree.nodes[view.focus];
            if view.focus != Tree::ROOT {
                let up = path_to(&view.tree, focus.parent.unwrap_or(Tree::ROOT));
                crumbs = Some(
                    div()
                        .id("live-up")
                        .px_4()
                        .pb_2()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_color(rgb(MUTED))
                        .cursor_pointer()
                        .hover(|s| s.text_color(rgb(TEXT)))
                        .child(format!("‹ {}", path_to(&view.tree, view.focus).iter().map(|n| n.to_string()).collect::<Vec<_>>().join(" › ")))
                        .on_click(cx.listener(move |this, _, _, cx| this.live_focus(up.clone(), cx))),
                );
            }
            for &ix in &focus.children {
                let node = &view.tree.nodes[ix];
                let swatch = view.swatches.get(&ix).copied().unwrap_or(gpui::hsla(0., 0., 0.38, 1.));
                let is_final = view.is_final(ix);
                let is_hovered = hovered == Some(ix);
                let openable = node.kind == Kind::Dir && !node.children.is_empty();
                let path = path_to(&view.tree, ix);
                let hover_path = path.clone();
                rows = rows.child(
                    div()
                        .id(("live-row", ix))
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .rounded_md()
                        .when(is_hovered, |d| d.bg(gpui::hsla(swatch.hue.into_positive_degrees() / 360.0, swatch.saturation, 0.45, 0.16)))
                        .when(openable, |d| d.cursor_pointer())
                        .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                            this.live_hover(hovering.then(|| hover_path.clone()), cx);
                        }))
                        .when(openable, |d| d.on_click(cx.listener(move |this, _, _, cx| this.live_focus(path.clone(), cx))))
                        .child(kind_mark(node.kind, if is_final || node.kind == Kind::Other { swatch } else { sunburst::dim(swatch) }))
                        .child(div().flex_1().min_w_0().truncate().child(node.name.clone()))
                        .child(
                            div()
                                .w(px(80.))
                                .flex_none()
                                .text_right()
                                .text_xs()
                                .font_family("Menlo")
                                .text_color(if is_final { rgb(TEXT) } else { rgb(MUTED) })
                                .child(size_label(view, ix)),
                        )
                        .child(div().w(px(14.)).flex_none().text_color(rgb(MUTED)).child(if openable { "›" } else { "" })),
                );
            }
        }

        let sidebar = self
            .navigator_frame()
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).truncate().child(format!("Scanning “{disk_name}”…")))
                    .child(div().text_color(rgb(MUTED)).child(format!("{} · {} files", format_size(scanned), format_count(files))))
                    .child(match progress.expected_items.get() {
                        Some(&expected) => {
                            let items = files + progress.dirs.load(Ordering::Relaxed);
                            let done = eta::fraction(items, expected);
                            let left = scanning.eta.remaining(elapsed as f64, items, expected).map(eta::describe);
                            div()
                                .mt_1()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .h(px(6.))
                                        .w_full()
                                        .rounded_full()
                                        .bg(rgb(BORDER))
                                        .child(div().h_full().w(relative(done)).rounded_full().bg(rgb(ACCENT))),
                                )
                                .child(div().text_xs().text_color(rgb(MUTED)).child(match left {
                                    Some(left) => format!("{:.0}% · {left}", done * 100.0),
                                    None => format!("{:.0}%", done * 100.0),
                                }))
                                .into_any_element()
                        }
                        // A folder: no known total, so no percentage or countdown.
                        None => div().text_xs().text_color(rgb(MUTED)).child("Scanning…").into_any_element(),
                    })
                    .child({
                        let done = progress.expected_items.get().map(|&expected| eta::fraction(files + progress.dirs.load(Ordering::Relaxed), expected));
                        div().mt_2().child(comic::investigator(comic::caption(&current, files), done, 56.0))
                    }),
            )
            .children(scanning.focus.is_empty().then(|| self.render_access_card(None, cx)).flatten())
            .children(crumbs)
            .child(rows)
            .child(div().px_4().py_2().text_xs().text_color(rgb(MUTED)).child("Click a folder to look inside"));

        // Chart: share of the volume found so far for plain volume scans (the startup disk's
        // remainder is its own slice instead).
        let fraction = match scanning.expected {
            Some(expected) if expected > 0 => (scanned as f64 / expected as f64).min(1.0) as f32,
            _ => 1.0,
        };
        let started = scanning.started;
        let motion = scanning.motion.clone();
        let bounds_cell = scanning.chart_bounds.clone();
        let entity = cx.entity().downgrade();

        // Centre label: whatever is under the pointer, else the folder in focus.
        let (title, lines): (String, Vec<String>) = match (&view, hovered) {
            (Some(v), Some(ix)) if ix != v.focus => {
                let node = &v.tree.nodes[ix];
                let state = match node.kind {
                    Kind::Other if node.name.as_ref() == disk::NOT_SCANNED => "still to scan",
                    Kind::Other => "exact, from APFS",
                    _ if v.is_final(ix) => "final",
                    _ => "still counting",
                };
                (node.name.to_string(), vec![size_label(v, ix), state.to_string()])
            }
            (Some(v), _) if v.focus != Tree::ROOT => {
                let node = &v.tree.nodes[v.focus];
                let state = if v.is_final(v.focus) { "final" } else { "still counting" };
                (node.name.to_string(), vec![size_label(v, v.focus), state.to_string(), "↑ click to go back".into()])
            }
            _ => match &layout {
                Some(layout) => (
                    layout.name.clone(),
                    vec![
                        format!("{} used", format_size(layout.container_used)),
                        format!("{} of {} scanned", format_size(scanned), format_size(layout.data_used)),
                    ],
                ),
                None => (
                    format_size(view.as_ref().map(|v| v.tree.nodes[Tree::ROOT].size).unwrap_or(0).max(scanned)),
                    vec![match scanning.expected {
                        Some(expected) => format!("of {} used", format_size(expected)),
                        None => "scanning…".to_string(),
                    }],
                ),
            },
        };
        let label_width = scanning.chart_bounds.get().map(|b| Geometry::new(b).inner_radius * 1.7).unwrap_or(140.);
        let paint_view = view.clone();
        let hover_ix = hovered;

        let chart = div()
            .flex_1()
            .h_full()
            .relative()
            .child(
                canvas(
                    move |bounds, window, _| {
                        bounds_cell.set(Some(bounds));
                        window.insert_hitbox(bounds, HitboxBehavior::Normal)
                    },
                    move |bounds, hitbox, window, _| {
                        let geometry = Geometry::new(bounds);
                        geometry.paint_disc(window, geometry.outer_radius() + 6.0, to_hsla(0x18191c));
                        let Some(view) = paint_view else { return };
                        let pulse = 0.22 + 0.04 * (clock::since(started).as_secs_f32() * 3.0).sin();
                        let mut motion = motion.borrow_mut();
                        motion.retarget(view.id as usize, &view.keys, &view.segments, fraction);
                        motion.step(clock::now());
                        let fraction = motion.fraction;
                        for moving in motion.segments_mut() {
                            let Some(i) = moving.current else {
                                // Leaving the layout: keep its last colour while it shrinks away.
                                let (r0, r1) = geometry.ring_at(moving.depth);
                                geometry.paint_band(window, r0, r1, moving.start * fraction, moving.end * fraction, moving.color);
                                continue;
                            };
                            // Hue follows the animated position, so colours shift smoothly too.
                            let segment = &Segment { start: moving.start, end: moving.end, ..view.segments[i] };
                            let color = match segment.target {
                                Target::Node(ix) if Some(ix) == view.pending => gpui::hsla(0.0, 0.0, pulse, 1.0),
                                Target::Node(ix) => {
                                    let base = sunburst::base_color(segment);
                                    // Folders still being counted are drawn muted; they "settle" into
                                    // full colour when their total is final.
                                    let base = if view.tree.nodes[ix].kind == Kind::Dir && !view.is_final(ix) { sunburst::dim(base) } else { base };
                                    // A brief glow as the folder's total becomes final.
                                    let base = match view.settled_at.get(&ix) {
                                        Some(at) => {
                                            let k = 1.0 - (clock::since(*at).as_secs_f32() / SETTLE_GLOW.as_secs_f32()).min(1.0);
                                            gpui::hsla(base.hue.into_positive_degrees() / 360.0, base.saturation, (base.lightness + 0.2 * k).min(0.92), 1.0)
                                        }
                                        None => base,
                                    };
                                    match hover_ix {
                                        Some(h) if is_within(&view.tree, ix, h) => sunburst::highlight(base),
                                        _ => base,
                                    }
                                }
                                Target::Small { .. } => sunburst::base_color(segment),
                            };
                            moving.color = color;
                            let (r0, r1) = geometry.ring_at(moving.depth);
                            geometry.paint_band(window, r0, r1, moving.start * fraction, moving.end * fraction, color);
                        }
                        drop(motion);
                        if fraction < 0.9999 {
                            geometry.paint_sector(window, 1, fraction, 1.0, gpui::hsla(0.0, 0.0, pulse, 1.0));
                        }
                        window.request_animation_frame();
                        geometry.paint_disc(window, geometry.inner_radius - 1.0, to_hsla(0x2b2d33));

                        // Hit testing against exactly what was painted, so a newer snapshot can't
                        // shift what a click means.
                        let target_at = {
                            let view = view.clone();
                            move |position| -> Option<(bool, Vec<SharedString>)> {
                                match geometry.hit_test(position, &view.segments)? {
                                    Hit::Center => Some((true, path_to(&view.tree, view.tree.nodes[view.focus].parent.unwrap_or(Tree::ROOT)))),
                                    Hit::Segment(i) => match view.segments[i].target {
                                        Target::Node(ix) => Some((false, path_to(&view.tree, ix))),
                                        Target::Small { .. } => None,
                                    },
                                }
                            }
                        };
                        let openable = {
                            let view = view.clone();
                            move |path: &[SharedString]| {
                                let ix = resolve_path(&view.tree, path);
                                view.tree.nodes[ix].kind == Kind::Dir && !view.tree.nodes[ix].children.is_empty()
                            }
                        };
                        if let Some(position) = window.mouse_position().into() {
                            if bounds.contains(&position) {
                                if let Some((center, path)) = target_at(position) {
                                    if center || openable(&path) {
                                        window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
                                    }
                                }
                            }
                        }
                        let (move_target, move_entity) = (target_at.clone(), entity.clone());
                        let hitbox_for_move = hitbox.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble || !hitbox_for_move.is_hovered(window) {
                                return;
                            }
                            let path = move_target(event.position).filter(|(center, _)| !center).map(|(_, p)| p);
                            move_entity.update(cx, |this, cx| this.live_hover(path, cx)).ok();
                        });
                        let click_entity = entity.clone();
                        let hitbox_for_click = hitbox.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx: &mut App| {
                            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !hitbox_for_click.is_hovered(window) {
                                return;
                            }
                            if let Some((center, path)) = target_at(event.position) {
                                if center || openable(&path) {
                                    click_entity.update(cx, |this, cx| this.live_focus(path, cx)).ok();
                                }
                            }
                        });
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .max_w(px(label_width))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_0p5()
                            .child(div().max_w_full().text_base().font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                            .children(lines.into_iter().map(|line| div().text_xs().text_color(rgb(MUTED)).child(line))),
                    ),
            );

        div().size_full().flex().child(sidebar).child(chart)
    }

    fn render_results(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Screen::Results(r) = &self.screen else { unreachable!() };
        div()
            .size_full()
            .flex()
            .child(self.render_navigator(r, cx))
            .child(self.render_chart(r, cx))
    }

    /// The navigator: the focused folder's name and totals, the sort toggle, the
    /// ask card when a query is live, and the folder list.
    fn render_navigator(&self, r: &Results, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = &r.tree.nodes[r.focus];
        let count = r.order.len();
        let sort_option = |id: &'static str, label: &'static str, value: SortBy, cx: &mut Context<Self>| {
            let selected = r.sort == value;
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .when(selected, |d| d.bg(rgb(CARD_HOVER)).text_color(rgb(TEXT)))
                .when(!selected, |d| d.text_color(rgb(MUTED)).hover(|s| s.text_color(rgb(TEXT))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sort = value;
                    if let Some(r) = this.results() {
                        r.set_sort(value);
                    }
                    cx.notify();
                }))
                .child(label)
        };
        let sort_toggle = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .text_xs()
            .child(sort_option("sort-size", "Size", SortBy::Size, cx))
            .child(sort_option("sort-name", "Name", SortBy::Name, cx));

        self.navigator_frame()
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .child(div().flex_1().min_w_0().text_lg().font_weight(FontWeight::SEMIBOLD).truncate().child(focus.name.clone()))
                            .child(sort_toggle),
                    )
                    .child(div().text_color(rgb(MUTED)).child(format!(
                        "{} · {} files",
                        format_size(focus.size),
                        format_count(focus.items)
                    )))
                    .when(r.tree.cloud_only > 0, |d| {
                        d.child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                            "{} cloud-only folders not downloaded (use no space here)",
                            format_count(r.tree.cloud_only)
                        )))
                    })
                    .when(r.tree.errors > 0, |d| {
                        d.child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                            "{} items couldn’t be read (permissions)",
                            format_count(r.tree.errors)
                        )))
                    }),
            )
            .children((r.focus == Tree::ROOT).then(|| {
                let not_readable = r.tree.nodes[Tree::ROOT]
                    .children
                    .iter()
                    .find(|&&c| r.tree.nodes[c].kind == Kind::Other && r.tree.nodes[c].name.as_ref() == disk::NOT_READABLE)
                    .map(|&c| r.tree.nodes[c].size);
                self.render_access_card(not_readable, cx)
            }).flatten())
            .when(r.matches.is_some() || !matches!(r.ask, AskState::Idle), |d| d.child(self.render_ask(r, cx)))
            .child(
                div().flex_1().min_h_0().px_2().pb_2().child(
                    uniform_list("children", count, cx.processor(Self::render_rows))
                        .track_scroll(&r.list_scroll)
                        .size_full(),
                ),
            )
    }

    /// Known space hogs with exact sizes. With results, each has a checkbox that
    /// stages it, is tagged "in view" when it sits under the focused folder, and
    /// opens its folder on click; "Stage all safe" ticks every safe one.
    fn render_findings(&self, findings: &[Finding], results: Option<&Results>, cx: &mut Context<Self>) -> impl IntoElement {
        let safe: u64 = findings.iter().filter(|f| f.safety == Safety::Safe).map(|f| f.size).sum();
        let safe_nodes: Vec<usize> = findings.iter().filter(|f| f.safety == Safety::Safe).flat_map(|f| f.nodes.iter().copied()).collect();
        let all_safe_staged = results.is_some_and(|r| !safe_nodes.is_empty() && r.group_tick(&safe_nodes) == Tick::On);
        let mut list = div().id("findings").max_h(px(290.)).overflow_y_scroll().flex().flex_col().gap_0p5().px_1p5();
        for (i, finding) in findings.iter().enumerate() {
            let (tag, color) = match finding.safety {
                Safety::Safe => ("Safe to delete", SAFE),
                Safety::Review => ("Review first", REVIEW),
            };
            let target = finding.nodes.first().copied().filter(|_| finding.path.is_some());
            let nodes = finding.nodes.clone();
            let in_view = results.is_some_and(|r| {
                r.focus != Tree::ROOT && finding.nodes.iter().any(|&n| r.tree.is_ancestor_or_self(r.focus, n))
            });
            let tick = results.map(|r| r.group_tick(&finding.nodes));
            list = list.child(
                div()
                    .id(("finding", i))
                    .px_1p5()
                    .py_1()
                    .rounded_md()
                    .flex()
                    .items_start()
                    .gap_2()
                    .when(results.is_some() && target.is_some(), |d| {
                        d.cursor_pointer().hover(|s| s.bg(rgb(CARD_HOVER))).on_click(cx.listener(move |this, _, _, cx| {
                            if let (Some(r), Some(ix)) = (this.results(), target) {
                                r.navigate(ix);
                                cx.notify();
                            }
                        }))
                    })
                    .when_some(tick, |d, tick| {
                        let nodes = nodes.clone();
                        d.child(
                            div().pt_0p5().child(
                                checkbox(("finding-tick", i), tick, true)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.click_group_tick(&nodes, cx);
                                    })),
                            ),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(finding.title))
                                    .when(in_view, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .px_1()
                                                .rounded_sm()
                                                .bg(rgb(CARD_HOVER))
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child("in view"),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_right()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .when(finding.pending, |d| d.text_color(rgb(MUTED)))
                                            .child(if finding.pending { "…".to_string() } else { format_size(finding.size) }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .text_xs()
                                    .child(div().flex_none().text_color(rgb(color)).child(tag))
                                    .child(div().flex_none().text_color(rgb(MUTED)).child("·"))
                                    .child(div().min_w_0().text_color(rgb(MUTED)).truncate().child(finding.blurb.clone())),
                            ),
                    ),
            );
        }
        let summary = if findings.iter().any(|f| f.pending) {
            "working out savings…".to_string()
        } else if safe > 0 {
            format!("{} safe", format_size(safe))
        } else if findings.is_empty() {
            match results {
                Some(_) => "nothing of note".to_string(),
                None => "appear as the scan runs".to_string(),
            }
        } else {
            String::new()
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .pb_2()
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_1p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Findings"))
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(rgb(MUTED)).child(summary))
                    .when(results.is_some() && !safe_nodes.is_empty(), |d| {
                        let nodes = safe_nodes.clone();
                        d.child(
                            button("stage-safe", if all_safe_staged { "Unstage safe" } else { "Stage all safe" })
                                .text_xs()
                                .py_0p5()
                                .px_2()
                                .on_click(cx.listener(move |this, _, _, cx| this.click_group_tick(&nodes, cx))),
                        )
                    }),
            )
            .child(list)
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<Stateful<gpui::Div>> {
        let Screen::Results(r) = &self.screen else { return Vec::new() };
        let focus = &r.tree.nodes[r.focus];
        let hovered = r.hovered();
        range
            .filter_map(|i| r.order.get(i).copied())
            .map(|ix| {
                let node = &r.tree.nodes[ix];
                let is_hovered = hovered == Some(Target::Node(ix));
                let is_selected = r.selected == Some(ix);
                let swatch = r.swatches.get(&ix).copied().unwrap_or(gpui::hsla(0., 0., 0.38, 1.));
                let fraction = node.size as f32 / focus.size.max(1) as f32;
                let is_dir = node.kind == Kind::Dir;
                let openable = is_dir && !node.children.is_empty();
                let kind = node.kind;
                let dragged = DraggedItem { node: ix, name: node.name.clone(), size: node.size };
                let path = r.tree.path_of(ix);
                let tick = r.tick(ix);
                let stageable = r.stageable(ix);
                let tint = gpui::hsla(swatch.hue.into_positive_degrees() / 360.0, swatch.saturation, 0.45, 0.16);

                div()
                    .id(("row", ix))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px_1p5()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(if is_selected { to_hsla(ACCENT) } else { gpui::hsla(0., 0., 0., 0.) })
                    .when(is_hovered || is_selected, |d| d.bg(tint))
                    .when(openable, |d| d.cursor_pointer())
                    .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                        if let Some(r) = this.results() {
                            if *hovering {
                                r.list_hover = Some(ix);
                            } else if r.list_hover == Some(ix) {
                                r.list_hover = None;
                            }
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(r) = this.results() {
                            r.selected = Some(ix);
                            if openable {
                                r.navigate(ix);
                            }
                            cx.notify();
                        }
                    }))
                    .on_drag(dragged, |item, _, _, cx| cx.new(|_| item.clone()))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        if kind != Kind::Other {
                            this.open_context_menu(ix, event.position, cx);
                        }
                    }))
                    .child(
                        checkbox(("tick", ix), tick, stageable)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.click_tick(ix, cx);
                            })),
                    )
                    .child(kind_mark(kind, swatch))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().when(tick != Tick::Off, |d| d.text_color(rgb(MUTED))).child(node.name.clone()))
                            .when(is_dir, |d| {
                                d.child(
                                    div()
                                        .h(px(3.))
                                        .w(relative(fraction.max(0.005)))
                                        .rounded_full()
                                        .bg(swatch)
                                        .opacity(0.85),
                                )
                            }),
                    )
                    .when(is_hovered && node.kind != Kind::Other, |d| {
                        let path2 = path.clone();
                        d.child(
                            icon_button(("cp-path", ix), "⎘", "Copy Path").on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.write_to_clipboard(ClipboardItem::new_string(path2.to_string_lossy().into_owned()));
                                },
                            )),
                        )
                        .child(
                            icon_button(("reveal", ix), "⌕", "Reveal in Finder").on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.reveal_path(&path);
                                },
                            )),
                        )
                    })
                    .child(
                        div()
                            .w(px(72.))
                            .flex_none()
                            .text_right()
                            .text_color(rgb(MUTED))
                            .text_xs()
                            .font_family("Menlo")
                            .child(format_size(node.size)),
                    )
                    .child(
                        div()
                            .w(px(10.))
                            .flex_none()
                            .text_color(rgb(MUTED))
                            .child(if openable { "›" } else { "" }),
                    )
            })
            .collect()
    }

    /// The cleanup plan: one line per staged item, the totals, and the way to the
    /// review card. Rows can still be dragged in.
    fn render_plan(&self, r: &Results, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = r.collector.is_empty();
        let size = r.collected_size();
        let frees = r.collector_frees.get();
        let shared = frees.map(|f| size.saturating_sub(f));

        let mut list = div().id("plan-list").flex_1().min_h_0().overflow_y_scroll().px_1p5().flex().flex_col().gap_0p5();
        for &ix in &r.collector {
            let node = &r.tree.nodes[ix];
            let path = r.tree.path_of(ix);
            let parent = path.parent().map(|p| abbreviate_home(&p.to_string_lossy())).unwrap_or_default();
            list = list.child(
                div()
                    .id(("plan", ix))
                    .px_1p5()
                    .py_1()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .hover(|s| s.bg(rgb(CARD_HOVER)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().font_weight(FontWeight::MEDIUM).child(node.name.clone()))
                            .child(div().text_xs().text_color(rgb(MUTED)).truncate().child(parent)),
                    )
                    .child(div().flex_none().text_xs().font_family("Menlo").text_color(rgb(MUTED)).child(format_size(node.size)))
                    .child(
                        div()
                            .id(("unstage", ix))
                            .flex_none()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(rgb(MUTED))
                            .hover(|s| s.bg(rgb(BORDER)).text_color(rgb(TEXT)))
                            .tooltip(|_, cx| cx.new(|_| Tip("Remove from the plan".into())).into())
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| this.unstage(ix, cx))),
                    ),
            );
        }

        let total_line = |label: &'static str, value: String, strong: bool| {
            div()
                .flex()
                .justify_between()
                .gap_2()
                .child(div().text_color(rgb(MUTED)).child(label))
                .child(div().font_family("Menlo").text_xs().when(strong, |d| d.text_color(rgb(TEXT)).font_weight(FontWeight::SEMIBOLD)).when(!strong, |d| d.text_color(rgb(MUTED))).child(value))
        };

        div()
            .id("collector")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(BORDER))
            .on_drag_over::<DraggedItem>(|style, _, _, _| style.bg(rgb(CARD)))
            .on_drop(cx.listener(|this, item: &DraggedItem, _, cx| {
                if let Some(r) = this.results() {
                    r.collect(item.node);
                }
                this.collector_changed(cx);
                cx.notify();
            }))
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_1p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Cleanup plan"))
                    .child(div().flex_1().text_xs().text_color(rgb(MUTED)).child(match r.collector.len() {
                        0 => String::new(),
                        1 => "1 item".to_string(),
                        n => format!("{n} items"),
                    }))
                    .when(!empty, |d| {
                        d.child(button("clear-collector", "Clear").text_xs().py_0p5().px_2().on_click(cx.listener(|this, _, _, cx| {
                            if let Some(r) = this.results() {
                                r.collector.clear();
                            }
                            this.collector_changed(cx);
                            cx.notify();
                        })))
                    }),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .flex_1()
                        .px_3()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Tick a folder or a finding to stage it — or drag a row here. Nothing moves until you review the plan."),
                )
            })
            .when(!empty, |d| d.child(list))
            .child(
                div()
                    .flex_none()
                    .mx_3()
                    .mt_2()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(BG))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_sm()
                    .child(total_line("Selected", format_size(size), false))
                    .child(total_line(
                        "Shared with other files",
                        match (empty, shared) {
                            (true, _) => "—".to_string(),
                            (false, Some(shared)) => format_size(shared),
                            (false, None) => "calculating…".to_string(),
                        },
                        false,
                    ))
                    .child(total_line(
                        "Will free",
                        match (empty, frees) {
                            (true, _) => "—".to_string(),
                            (false, Some(frees)) => format_size(frees),
                            (false, None) => format!("up to {}", format_size(size)),
                        },
                        true,
                    )),
            )
            .child(
                div().flex_none().p_3().child(
                    primary_button("trash", "Review & move to Trash", DANGER)
                        .w_full()
                        .text_center()
                        .when(empty, |b| b.opacity(0.45).cursor_default())
                        .when(!empty, |b| b.on_click(cx.listener(|this, _, window, cx| this.open_review(window, cx)))),
                ),
            )
    }

    /// The review card: every staged item with its path and size, the honest
    /// total, and the one button that moves things.
    fn render_review(&self, review: &ReviewState, cx: &mut Context<Self>) -> AnyElement {
        let Screen::Results(r) = &self.screen else { return div().into_any_element() };
        let items: Vec<usize> = review.items.iter().copied().filter(|&ix| r.tree.nodes[ix].parent.is_some()).collect();
        let size: u64 = items.iter().map(|&ix| r.tree.nodes[ix].size).sum();
        let frees = r.collector_frees.get();
        let shared = frees.map(|f| size.saturating_sub(f)).filter(|&s| s > size / 100);

        let mut list = div().id("review-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap_0p5();
        for &ix in &items {
            let node = &r.tree.nodes[ix];
            let path = abbreviate_home(&r.tree.path_of(ix).to_string_lossy());
            let item_frees = review.frees.get(&ix).copied();
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().font_weight(FontWeight::MEDIUM).child(node.name.clone()))
                            .child(div().text_xs().text_color(rgb(MUTED)).font_family("Menlo").truncate().child(path)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .items_end()
                            .child(div().text_xs().font_family("Menlo").child(format_size(node.size)))
                            .child(div().text_xs().text_color(rgb(MUTED)).child(match item_frees {
                                Some(f) if f + node.size / 100 < node.size => format!("frees {}", format_size(f)),
                                Some(_) => "frees all of it".to_string(),
                                None => "working out…".to_string(),
                            })),
                    ),
            );
        }
        let count = if items.len() == 1 { "1 item".to_string() } else { format!("{} items", items.len()) };
        let subtitle = match frees {
            Some(frees) => format!("{count} · {} selected · will free {}", format_size(size), format_size(frees)),
            None => format!("{count} · {} selected · working out what's shared…", format_size(size)),
        };

        let stage = review.drop.as_ref().filter(|_| review.working).map(|drop| {
            let (landed, bytes) = drop.landed();
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().h(px(230.)).w_full().child(trashcan::stage(drop.clone())))
                .child(div().text_xs().text_color(rgb(MUTED)).text_center().child(format!(
                    "{landed} of {} in the bin · {}",
                    drop.total(),
                    format_size(bytes)
                )))
        });
        let list = if stage.is_some() { div().id("review-list") } else { list };

        div()
            .id("review-card")
            .w(px(560.))
            .max_h(px(600.))
            .p_4()
            .rounded_xl()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Move these to the Trash?"))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(subtitle)),
            )
            .child(list)
            .children(stage)
            .when_some(shared, |d, shared| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(rgb(0x2a2818))
                        .border_1()
                        .border_color(rgb(0x4a4428))
                        .text_xs()
                        .text_color(rgb(0xc8b86e))
                        .child(format!(
                            "{} of this is shared with files outside the selection (APFS clones or hard links), so the Trash will free {} rather than {}.",
                            format_size(shared),
                            format_size(frees.unwrap_or(0)),
                            format_size(size)
                        )),
                )
            })
            .child(div().text_xs().text_color(rgb(MUTED)).child("Everything goes to the Trash; you can put items back from Finder. The space comes back when you empty the Trash."))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        button("review-cancel", "Cancel")
                            .when(!review.working, |b| b.on_click(cx.listener(|this, _, window, cx| this.close_review(&CloseReview, window, cx)))),
                    )
                    .child(
                        primary_button("review-trash", if review.working { "Moving…" } else { "Move to Trash" }, DANGER)
                            .when(review.working, |b| b.opacity(0.6))
                            .when(!review.working, |b| b.on_click(cx.listener(|this, _, _, cx| this.trash_staged(cx)))),
                    ),
            )
            .into_any_element()
    }

    /// The ask card: what the submitted query resolved to, with the exact-match groups
    /// as cards underneath (they show live while typing, no network involved). Click a
    /// card to go to its biggest folder; "+" collects the whole group.
    fn render_ask(&self, r: &Results, cx: &mut Context<Self>) -> impl IntoElement {
        let note = |text: String| div().text_xs().text_color(rgb(MUTED)).child(text);
        let mut card = div()
            .id("ask")
            .mx_3()
            .mb_2()
            .p_2()
            .rounded_lg()
            .bg(rgb(BG))
            .border_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .gap_1();

        match &r.ask {
            AskState::Idle => {}
            AskState::Thinking => card = card.child(note("asking the reader…".into())),
            AskState::Suggestions => card = card.child(note(match r.matches {
                Some(_) => "not sure which one you meant — pick a card below".to_string(),
                None => format!("nothing in this scan matches “{}”", r.query),
            })),
            AskState::Unavailable(why) => {
                card = card.child(note(match why {
                    Unavailable::NoKey => "the reader has no key and the fallback couldn't be reached — exact matches only".to_string(),
                    Unavailable::Rejected => "the reader's key was rejected — exact matches only".to_string(),
                    Unavailable::Unreachable => "couldn't reach the reader — exact matches only".to_string(),
                }))
            }
            AskState::Answered { intent, group } => {
                card = card.child(note(match intent {
                    Intent::Delete if r.collector.is_empty() => format!(
                        "The reader took that as delete, but nothing in “{}” may be staged — home folders and system volumes are off limits.",
                        group.label,
                    ),
                    Intent::Delete => format!("The reader took that as delete: {} staged in the cleanup plan.", group.label),
                    Intent::Locate => format!("The reader took that as find: showing {}.", group.label),
                }));
                if *intent == Intent::Delete && !r.collector.is_empty() {
                    card = card.child(
                        div().flex().justify_end().child(
                            primary_button("ask-trash", "Review & move to Trash", DANGER)
                                .on_click(cx.listener(|this, _, window, cx| this.open_review(window, cx))),
                        ),
                    );
                }
            }
        }

        if let Some(groups) = &r.matches {
            let mut list = div().id("ask-groups").max_h(px(220.)).overflow_y_scroll().flex().flex_col().gap_0p5();
            for (i, group) in groups.iter().take(ask::MAX_GROUPS).enumerate() {
                let color = match group.safety {
                    Safety::Safe => SAFE,
                    Safety::Review => REVIEW,
                };
                let nodes = group.nodes.clone();
                let biggest_dir = nodes.iter().copied().find(|&ix| r.tree.nodes[ix].kind == Kind::Dir);
                let tick = r.group_tick(&nodes);
                list = list.child(
                    div()
                        .id(("ask-group", i))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(CARD_HOVER)))
                        .when_some(biggest_dir, |d, ix| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(r) = this.results() {
                                    r.navigate(ix);
                                    cx.notify();
                                }
                            }))
                        })
                        .child(checkbox(("ask-tick", i), tick, true).on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.click_group_tick(&nodes, cx);
                        })))
                        .child(div().size(px(8.)).flex_none().rounded_full().bg(rgb(color)))
                        .child(div().flex_1().min_w_0().truncate().child(group.label.clone())),
                );
            }
            card = card.child(list);
        }
        card
    }

    fn render_chart(&self, r: &Results, cx: &mut Context<Self>) -> impl IntoElement {
        let tree = &r.tree;
        let hovered = r.hovered();
        let colors: Vec<Hsla> = r
            .segments
            .iter()
            .map(|s| {
                let base = r.color(s);
                if let Some(category) = r.legend_hover {
                    return if r.category(s.target) == Some(category) { sunburst::lift(base) } else { sunburst::fade(base) };
                }
                let Some(target) = hovered else { return base };
                let lit = match (s.target, target) {
                    (a, b) if a == b => true,
                    (Target::Node(n), Target::Node(h)) => tree.is_ancestor_or_self(h, n),
                    (Target::Small { parent }, Target::Node(h)) => tree.is_ancestor_or_self(h, parent),
                    _ => false,
                };
                match (lit, r.color_by) {
                    (true, ColorBy::Folder) => sunburst::highlight(base),
                    (true, ColorBy::Kind) => sunburst::lift(base),
                    (false, _) => sunburst::dim(base),
                }
            })
            .collect();

        let can_go_up = tree.nodes[r.focus].parent.is_some();
        let pointer = match r.chart_hover {
            Some(Hit::Center) => can_go_up,
            Some(Hit::Segment(i)) => r.segments.get(i).is_some_and(|s| s.kind == Kind::Dir && matches!(s.target, Target::Node(_))),
            None => false,
        };
        let center_hovered = r.chart_hover == Some(Hit::Center) && can_go_up;

        // Label in the middle of the chart describes whatever is under the pointer.
        let (title, subtitle): (SharedString, String) = match hovered {
            Some(Target::Node(ix)) => {
                let node = &tree.nodes[ix];
                let detail = match node.kind {
                    Kind::Dir => format!("{} files", format_count(node.items)),
                    Kind::File => "file".into(),
                    Kind::Other if node.name.as_ref() == disk::NOT_READABLE => "needs Full Disk Access".into(),
                    Kind::Other => "exact, from APFS".into(),
                };
                let kind = match r.category(Target::Node(ix)) {
                    Some(category) if r.color_by == ColorBy::Kind && node.kind != Kind::Other => format!("\n{}", category.label()),
                    _ => String::new(),
                };
                (node.name.clone(), format!("{}\n{detail}{kind}", format_size(node.size)))
            }
            Some(Target::Small { parent }) => {
                let seg_size: u64 = tree.nodes[parent]
                    .children
                    .iter()
                    .map(|&c| tree.nodes[c].size)
                    .filter(|&s| (s as f32) < tree.nodes[parent].size as f32 * 0.004)
                    .sum();
                ("Smaller objects".into(), format_size(seg_size))
            }
            None if center_hovered => ("↑ Back".into(), format!("to “{}”", tree.nodes[tree.nodes[r.focus].parent.unwrap()].name)),
            None => {
                let node = &tree.nodes[r.focus];
                (node.name.clone(), format_size(node.size))
            }
        };
        let label_width = r
            .chart_bounds
            .get()
            .map(|b| Geometry::new(b).inner_radius * 1.7)
            .unwrap_or(140.);

        let segments = r.segments.clone();
        let bounds_cell = r.chart_bounds.clone();
        let anim_start = r.anim_start;
        let entity = cx.entity().downgrade();
        // Staged slices (themselves or through a parent) get the hatch.
        let staged: Vec<bool> = segments
            .iter()
            .map(|s| match s.target {
                Target::Node(ix) => r.is_staged(ix),
                Target::Small { parent } => r.is_staged(parent),
            })
            .collect();
        // Names for the two inner rings; whether one fits is decided at paint time.
        let labels: Vec<(usize, SharedString)> = segments
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s.target {
                Target::Node(ix) if s.depth <= 2 => Some((i, tree.nodes[ix].name.clone())),
                _ => None,
            })
            .collect();

        div()
            .flex_1()
            .h_full()
            .relative()
            .child(
                canvas(
                    move |bounds, window, _| {
                        bounds_cell.set(Some(bounds));
                        window.insert_hitbox(bounds, HitboxBehavior::Normal)
                    },
                    move |bounds, hitbox, window, cx| {
                        let geometry = Geometry::new(bounds);
                        let t = (clock::since(anim_start).as_secs_f32() / ZOOM_DURATION.as_secs_f32()).min(1.0);
                        let eased = 1.0 - (1.0 - t).powi(3);
                        if t < 1.0 {
                            window.request_animation_frame();
                        }

                        geometry.paint_disc(window, geometry.outer_radius() + 6.0, to_hsla(0x18191c));
                        for ((segment, color), &hatched) in segments.iter().zip(&colors).zip(&staged) {
                            if segment.depth as f32 > 1.0 + eased * sunburst::MAX_DEPTH as f32 {
                                continue;
                            }
                            geometry.paint_sector(window, segment.depth, segment.start * eased, segment.end * eased, *color);
                            if hatched {
                                geometry.paint_hatched_sector(window, segment.depth, segment.start * eased, segment.end * eased, gpui::hsla(0., 0., 0.05, 0.55));
                            }
                        }
                        if t >= 1.0 {
                            let mut font = gpui::font(".SystemUIFont");
                            font.weight = FontWeight::MEDIUM;
                            for (i, name) in &labels {
                                let segment = &segments[*i];
                                let Some((at, width)) = geometry.label_slot(segment.depth, segment.start, segment.end) else { continue };
                                let Some(line) = fit_label(window, name, width, &font, px(11.)) else { continue };
                                let line_height = px(14.);
                                let origin = point(at.x - line.width() / 2.0, at.y - line_height / 2.0);
                                let _ = line.paint(origin, line_height, TextAlign::Left, None, window, cx);
                            }
                        }
                        let center = if center_hovered { to_hsla(0x3a3d44) } else { to_hsla(0x2b2d33) };
                        geometry.paint_disc(window, geometry.inner_radius - 1.0, center);

                        if pointer {
                            window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
                        }

                        let segments_for_move = segments.clone();
                        let entity_for_move = entity.clone();
                        let hitbox_for_move = hitbox.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble || !hitbox_for_move.is_hovered(window) {
                                return;
                            }
                            let hit = geometry.hit_test(event.position, &segments_for_move);
                            entity_for_move.update(cx, |this, cx| this.set_chart_hover(hit, cx)).ok();
                        });
                        let segments_for_click = segments.clone();
                        let entity_for_click = entity.clone();
                        let hitbox_for_click = hitbox.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx: &mut App| {
                            if phase != DispatchPhase::Bubble
                                || !matches!(event.button, MouseButton::Left | MouseButton::Right)
                                || !hitbox_for_click.is_hovered(window)
                            {
                                return;
                            }
                            let Some(hit) = geometry.hit_test(event.position, &segments_for_click) else {
                                return;
                            };
                            if event.button == MouseButton::Right {
                                // A menu for the segment under the pointer; the APFS
                                // volume slices have nothing to act on.
                                if let Hit::Segment(i) = hit {
                                    if let Some(Target::Node(ix)) = segments_for_click.get(i).map(|s| s.target) {
                                        if matches!(segments_for_click[i].kind, Kind::Dir | Kind::File) {
                                            let at = event.position;
                                            entity_for_click.update(cx, |this, cx| this.open_context_menu(ix, at, cx)).ok();
                                        }
                                    }
                                }
                                return;
                            }
                            entity_for_click.update(cx, |this, cx| this.chart_click(hit, cx)).ok();
                        });
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .max_w(px(label_width))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_0p5()
                            .child(
                                div()
                                    .max_w_full()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(title),
                            )
                            .children(subtitle.lines().map(|line| {
                                div().text_color(rgb(MUTED)).text_xs().child(line.to_string())
                            })),
                    ),
            )
            .when(r.color_by == ColorBy::Kind, |d| d.child(render_legend(r, cx)))
            .when(r.banner.is_some(), |d| {
                // The "done" moment: what the scan found, in one line — or, after
                // a move to the Trash, what moved.
                let total = tree.nodes[Tree::ROOT].size;
                let pending = r.findings.iter().any(|f| f.pending);
                let safe: u64 = r.findings.iter().filter(|f| f.safety == Safety::Safe).map(|f| f.size).sum();
                let savings = if pending {
                    " · working out savings…".to_string()
                } else if safe > 0 {
                    format!(" · {} safe to delete", format_size(safe))
                } else {
                    String::new()
                };
                let (headline, detail) = match &r.banner_text {
                    Some(text) => (text.clone(), String::new()),
                    None => (format!("Scan complete in {:.1} s", r.elapsed.as_secs_f32()), format!("{} accounted for{savings}", format_size(total))),
                };
                d.child(
                    div().absolute().top_4().left_0().right_0().px_4().flex().justify_center().child(
                        div()
                            .id("done-banner")
                            .occlude()
                            .max_w_full()
                            .min_w_0()
                            .px_4()
                            .py_2()
                            .rounded_full()
                            .bg(rgb(CARD))
                            .border_1()
                            .border_color(rgb(SAFE))
                            .shadow_lg()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .child(div().flex_none().text_color(rgb(SAFE)).font_weight(FontWeight::BOLD).child("✓"))
                            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(headline))
                            .when(!detail.is_empty(), |d| d.child(div().min_w_0().truncate().text_xs().text_color(rgb(MUTED)).child(detail)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(r) = this.results() {
                                    r.banner = None;
                                    cx.notify();
                                }
                            })),
                    ),
                )
            })
    }
}

/// The Help tab: a short FAQ, one breath per answer.
fn render_help() -> impl IntoElement {
    const FAQ: &[(&str, &str)] = &[
        ("What does Broom scan?", "Any disk or folder you point it at. Sizes come straight from APFS, and scanning only reads — nothing changes until you confirm a move to the Trash."),
        ("Some folders say they couldn't be read", "macOS keeps Mail, Messages, Safari and other apps' data private. Grant Broom Full Disk Access in System Settings › Privacy & Security, then rescan."),
        ("How do I delete things?", "Tick the box on a folder or a finding to stage it in the cleanup plan on the right (or press space on the selected row). “Review & move to Trash” shows the exact list and what it really frees before anything moves."),
        ("What can I type in the Ask field?", "Plain language: “node_modules”, “old dmg files in Downloads”. Exact matches appear as you type, entirely offline. Press Enter and the reader picks the group the words mean — a fallback steps in when it can't — and the cleanup plan gets pre-ticked. You confirm the final list."),
        ("Where are my API keys kept?", "In your login keychain. Broom never shows a key or logs it — only its last characters, as a hint. Pasting a key sends it straight to the keychain, never through a text field. The fallback reader needs no key at all."),
        ("Is deleting safe?", "Everything goes to the Trash. The review step says how much will really be freed (space shared with files outside the selection isn't), and you can put things back from Finder's Trash."),
        ("What do the colours mean?", "Colour: Folder gives each child of the folder in view its own hue, lighter ring by ring, so zooming in recolours the children apart; Colour: Kind colours by what things are — apps, caches, photos. The rail and the icon share the same six-hue ladder."),
        ("What about iCloud, Google Drive, OneDrive?", "They're synced folders on this Mac, listed under Cloud storage. Broom measures what's downloaded here; “cloud-only” items use no space and are counted as such."),
        ("And the rings?", "Each ring is one level of folders, biggest at twelve o'clock going clockwise. The centre takes you up one level, a slice zooms in, and the list mirrors the chart."),
    ];
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Help"))
        .child(div().text_xs().text_color(rgb(MUTED)).child("The short version of everything Broom does."))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .children(FAQ.iter().map(|(q, a)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(*q))
                        .child(div().text_xs().text_color(rgb(MUTED)).child(*a))
                })),
        )
}

/// "Colour: Folder | Kind" in the toolbar.
fn color_toggle(current: ColorBy, cx: &mut Context<Broom>) -> impl IntoElement {
    let option = |id: &'static str, label: &'static str, value: ColorBy, cx: &mut Context<Broom>| {
        let selected = current == value;
        div()
            .id(id)
            .px_2()
            .py_0p5()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |d| d.bg(rgb(CARD_HOVER)).text_color(rgb(TEXT)))
            .when(!selected, |d| d.text_color(rgb(MUTED)).hover(|s| s.text_color(rgb(TEXT))))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| this.set_color_by(value, cx)))
            .child(label)
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_0p5()
        .p_0p5()
        .rounded_lg()
        .border_1()
        .border_color(rgb(BORDER))
        .text_xs()
        .child(div().pl_1p5().pr_0p5().text_color(rgb(MUTED)).child("Colour"))
        .child(option("color-folder", "Folder", ColorBy::Folder, cx))
        .child(option("color-kind", "Kind", ColorBy::Kind, cx))
}

/// Which colour means which kind, for the kinds in view. Hover a kind to highlight it.
fn render_legend(r: &Results, cx: &mut Context<Broom>) -> impl IntoElement {
    let mut legend = div()
        .absolute()
        .left_4()
        .bottom_4()
        .occlude()
        .flex()
        .flex_col()
        .gap_0p5()
        .p_2()
        .rounded_lg()
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgb(BORDER))
        .text_xs();
    for category in CATEGORIES.into_iter().filter(|&c| r.segments.iter().any(|s| r.category(s.target) == Some(c))) {
        let lit = r.legend_hover.is_none_or(|h| h == category);
        legend = legend.child(
            div()
                .id(("legend", category as usize))
                .flex()
                .items_center()
                .gap_2()
                .px_1()
                .text_color(rgb(if lit { TEXT } else { MUTED }))
                .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                    if let Some(r) = this.results() {
                        if *hovering {
                            r.legend_hover = Some(category);
                        } else if r.legend_hover == Some(category) {
                            r.legend_hover = None;
                        }
                        cx.notify();
                    }
                }))
                .child(div().size(px(10.)).flex_none().rounded_full().bg(classify::color(category)))
                .child(category.label()),
        );
    }
    legend
}

/// Clicking outside an open context menu closes it; the menu stops this handler
/// for clicks inside itself.
fn close_menu_on_outside(this: &mut Broom, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Broom>) {
    if this.context_menu.take().is_some() {
        cx.notify();
    }
}

/// A staging checkbox. Clicks are wired by the caller; `enabled` is false for
/// things that can't be staged (the root, home folders, volume slices).
fn checkbox(id: impl Into<gpui::ElementId>, tick: Tick, enabled: bool) -> Stateful<gpui::Div> {
    let accent = to_hsla(ACCENT);
    let base = div()
        .id(id)
        .size(px(15.))
        .flex_none()
        .rounded_sm()
        .border_1()
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    let check = || div().text_color(rgb(0xffffff)).text_size(px(10.)).font_weight(FontWeight::BOLD).child("✓");
    let base = match tick {
        Tick::Off => base.border_color(rgb(0x5d616b)).bg(rgb(BG)).when(enabled, |d| d.hover(|s| s.border_color(rgb(ACCENT)))),
        Tick::On => base.border_color(accent).bg(accent).child(check()),
        Tick::Implied => base
            .border_color(gpui::hsla(accent.hue.into_positive_degrees() / 360.0, accent.saturation, accent.lightness, 0.5))
            .bg(gpui::hsla(accent.hue.into_positive_degrees() / 360.0, accent.saturation, accent.lightness, 0.45))
            .child(check()),
        Tick::Mixed => base.border_color(accent).bg(rgb(BG)).child(div().w(px(7.)).h(px(2.)).rounded_full().bg(accent)),
    };
    base.when(enabled && tick != Tick::Implied, |d| d.cursor_pointer()).when(!enabled, |d| d.opacity(0.3))
}

/// What a row is, as a mark rather than an emoji: a filled square for a folder,
/// an outlined one for a file, a grey one for an APFS slice.
fn kind_mark(kind: Kind, swatch: Hsla) -> impl IntoElement {
    let mark = div().size(px(11.)).flex_none().rounded_sm();
    match kind {
        Kind::Dir => mark.bg(swatch),
        Kind::File => mark.border_1().border_color(swatch).bg(gpui::hsla(swatch.hue.into_positive_degrees() / 360.0, swatch.saturation, swatch.lightness, 0.25)),
        Kind::Other => mark.bg(rgb(0x5d616b)),
    }
}

/// The plan's stand-in before there are results.
fn plan_placeholder(text: &'static str) -> impl IntoElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .border_t_1()
        .border_color(rgb(BORDER))
        .child(div().px_3().pt_3().pb_1p5().font_weight(FontWeight::SEMIBOLD).child("Cleanup plan"))
        .child(div().px_3().text_xs().text_color(rgb(MUTED)).child(text))
}

/// Shape a chart label that fits in `max_width`, truncating with an ellipsis;
/// `None` when not even two characters fit.
fn fit_label(window: &Window, text: &str, max_width: f32, font: &gpui::Font, size: Pixels) -> Option<gpui::ShapedLine> {
    let color = gpui::hsla(0., 0., 0.08, 0.92);
    let shape = |t: SharedString| {
        let run = TextRun { len: t.len(), font: font.clone(), color, background_color: None, underline: None, strikethrough: None, letter_spacing: None };
        window.text_system().shape_line(t, size, &[run], None)
    };
    let full = shape(text.to_string().into());
    if f32::from(full.width()) <= max_width {
        return Some(full);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len().saturating_sub(1);
    while n >= 2 {
        let mut t: String = chars[..n].iter().collect();
        t.push('…');
        let line = shape(t.into());
        if f32::from(line.width()) <= max_width {
            return Some(line);
        }
        n -= 1;
    }
    None
}

fn icon_button(id: impl Into<gpui::ElementId>, glyph: &'static str, tooltip: &'static str) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_color(rgb(MUTED))
        .cursor_pointer()
        .tooltip(move |_, cx| cx.new(|_| Tip(tooltip.into())).into())
        .hover(|s| s.bg(rgb(BORDER)).text_color(rgb(TEXT)))
        .child(glyph)
}

/// The app icon, as the About card's brand mark (a 256 px copy of packaging/AppIcon.png).
static ABOUT_MARK: LazyLock<Arc<Image>> =
    LazyLock::new(|| Arc::new(Image::from_bytes(ImageFormat::Png, include_bytes!("../packaging/AppIcon-256.png").to_vec())));

/// A volume over 90% full reads as danger whatever its hue.
fn level_color(fraction: f32, base: Hsla) -> Hsla {
    if fraction > 0.9 {
        to_hsla(DANGER)
    } else {
        base
    }
}

fn meter(fraction: f32, fill: Hsla) -> impl IntoElement {
    let color = level_color(fraction, fill);
    div()
        .mt_1()
        .h(px(6.))
        .w_full()
        .rounded_full()
        // Translucent, like the card it sits on: a solid dark track would
        // read as a stray line over glass.
        .bg(gpui::hsla(0., 0., 1., 0.12))
        .child(div().h_full().w(relative(fraction.clamp(0.0, 1.0))).rounded_full().bg(color))
}
