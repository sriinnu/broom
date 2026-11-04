//! The chart while a scan is still running.

use super::*;

pub(super) struct Scanning {
    pub(super) root: PathBuf,
    pub(super) progress: Arc<Progress>,
    pub(super) started: Instant,
    /// Used bytes on the volume, when scanning a whole volume, so the chart can
    /// show how much is still to come.
    pub(super) expected: Option<u64>,
    pub(super) live: Option<Rc<LiveView>>,
    pub(super) last_snapshot: Instant,
    /// Folder being looked at mid-scan, by name path from the root, so it survives the
    /// re-snapshots (node indices change between them).
    pub(super) focus: Vec<SharedString>,
    pub(super) hover: Option<Vec<SharedString>>,
    pub(super) chart_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub(super) eta: eta::Eta,
    pub(super) finals: HashMap<Vec<SharedString>, Instant>,
    /// Segments ease between snapshots instead of jumping.
    pub(super) motion: Rc<std::cell::RefCell<motion::ChartMotion>>,
    pub(super) _tasks: [Task<()>; 2],
}

/// A snapshot of the running totals, laid out around the folder in focus.
pub(super) struct LiveView {
    pub(super) tree: Tree,
    /// Which folders are final, indexed like `tree.nodes`.
    pub(super) done: Vec<bool>,
    pub(super) focus: usize,
    pub(super) segments: Rc<Vec<Segment>>,
    pub(super) swatches: HashMap<usize, Hsla>,
    /// The "not scanned yet" slice of the startup disk, drawn pulsing.
    pub(super) pending: Option<usize>,
    /// When each recently finalised folder became final, so it can glow briefly.
    pub(super) settled_at: HashMap<usize, Instant>,
    /// Each segment's identity (folder path), aligned with `segments`, for smooth motion.
    pub(super) keys: Vec<motion::Key>,
    /// Unique per layout, so the animation knows when to retarget.
    pub(super) id: u64,
}

pub(super) static NEXT_LAYOUT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(super) static NEXT_SCAN_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// How long a folder glows after its total becomes final.
pub(super) const SETTLE_GLOW: Duration = Duration::from_millis(700);

/// A settings value as a duration in milliseconds. Written as a free function
/// so every call site reads the same and a future field can't forget the unit.
pub(super) fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

impl LiveView {
    /// `finals` remembers when each folder (by path) was first seen final, across snapshots.
    pub(super) fn new(snapshot: live::LiveSnapshot, focus_path: &[SharedString], finals: &mut HashMap<Vec<SharedString>, Instant>, settle_glow: Duration) -> Self {
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

    pub(super) fn refocus(&mut self, focus_path: &[SharedString]) {
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

    pub(super) fn is_final(&self, ix: usize) -> bool {
        self.done[ix]
    }
}

/// Follow folder names down from the root; stops at the deepest folder that exists.
pub(super) fn resolve_path(tree: &Tree, path: &[SharedString]) -> usize {
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
pub(super) fn is_within(tree: &Tree, mut ix: usize, ancestor: usize) -> bool {
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
pub(super) fn path_to(tree: &Tree, mut ix: usize) -> Vec<SharedString> {
    let mut names = Vec::new();
    while let Some(parent) = tree.nodes[ix].parent {
        names.push(tree.nodes[ix].name.clone());
        ix = parent;
    }
    names.reverse();
    names
}
