//! Small shared types: sort keys, panes, modal state and what the screen is showing.

use super::*;

/// Where the source, releases and issues live.
pub(super) const REPO_URL: &str = "https://github.com/sriinnu/broom";

/// Plan rows drawn. Beyond this a list is unreadable anyway, and every row costs a frame.
pub(super) const PLAN_ROWS: usize = 300;

/// The chart keeps at least this much when a column is dragged wider.
pub(super) const CHART_MIN_WIDTH: f32 = 320.0;

/// How the navigator lists a folder's children.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SortBy {
    Size,
    Name,
    Modified,
}

impl SortBy {
    /// Which way a fresh click on this key lists: largest, A–Z, newest first.
    pub(super) fn default_descending(self) -> bool {
        self != SortBy::Name
    }
}

/// A column whose width the user can drag.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Pane {
    Rail,
    Navigator,
    Cleanup,
}

impl Pane {
    pub(super) fn default_width(self) -> f32 {
        match self {
            Pane::Rail => settings::RAIL_WIDTH,
            Pane::Navigator => settings::NAVIGATOR_WIDTH,
            Pane::Cleanup => settings::CLEANUP_WIDTH,
        }
    }

    pub(super) fn range(self) -> (f32, f32) {
        match self {
            Pane::Rail => settings::RAIL_RANGE,
            Pane::Navigator => settings::NAVIGATOR_RANGE,
            Pane::Cleanup => settings::CLEANUP_RANGE,
        }
    }
}

/// What a row's checkbox shows about staging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tick {
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
pub(super) struct ReviewState {
    pub(super) items: Vec<usize>,
    /// What each item frees on its own (`scan::frees_of`), as the survey lands.
    pub(super) frees: HashMap<usize, u64>,
    /// A delete is running; the buttons wait.
    pub(super) working: bool,
    /// The items tumbling into the bin while the delete runs.
    pub(super) drop: Option<Rc<trashcan::Drop>>,
}

pub(super) const ZOOM_DURATION: Duration = Duration::from_millis(450);

/// How the chart is coloured.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ColorBy {
    /// Each slice its own hue around the circle, so neighbouring folders stand apart.
    Folder,
    /// By what things are: apps, caches, photos… (see `classify`).
    Kind,
}

#[derive(Clone)]
pub(super) struct DraggedItem {
    pub(super) node: usize,
    pub(super) name: SharedString,
    pub(super) size: u64,
}

/// A right-click menu for one node, opening where the click happened.
#[derive(Clone)]
pub(super) struct ContextMenu {
    pub(super) node: usize,
    pub(super) at: Point<Pixels>,
}

/// Which tab of the Help / About card is in front.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PanelTab {
    /// A short FAQ.
    Help,
    /// Version and credits.
    About,
}

/// The one modal card Broom has: Help / About. Settings is not in it: the
/// Human Interface Guidelines give Settings a window of its own, which
/// leaves the main window usable while it's open (see `SettingsPane`). The
/// card is a fixed height; the tab's content scrolls in a viewport of its
/// own, with a scrollbar painted into the card.
pub(super) struct PanelState {
    pub(super) tab: PanelTab,
    pub(super) scroller: Scroller,
}

/// A scroll area with Broom's own scrollbar, shared by the card and the
/// Settings window.
#[derive(Default)]
pub(super) struct Scroller {
    /// Drives the scroll area. The scrollbar reads it (offset, viewport) and
    /// drags write to it.
    pub(super) scroll: ScrollHandle,
    /// Where the scrolled content starts and stops, in window coordinates,
    /// written by zero-height canvases at its ends. Their difference is the
    /// content's height — a number no div API hands out, and the one the
    /// scrollbar's thumb is proportioned against.
    pub(super) content_top: Rc<Cell<Option<f32>>>,
    pub(super) content_bottom: Rc<Cell<Option<f32>>>,
    /// A thumb drag in flight: y at press, scroll offset at press. `None`
    /// when the thumb isn't held.
    pub(super) drag: Rc<Cell<Option<(f32, f32)>>>,
}

/// The Settings window's state, held by Broom while that window is open so
/// the keychain work in `api_keys` keeps landing where it always has.
pub(super) struct SettingsPane {
    pub(super) settings: SettingsState,
    pub(super) scroller: Scroller,
    /// The window showing it; `None` only between creating the state and the
    /// window opening.
    pub(super) window: Option<WindowHandle<SettingsWindow>>,
}

/// Settings lives as long as it's open; keychain work happens in the background and
/// lands back in these rows. `note_epoch` tags the note line so a slow check for an
/// older name can't overwrite what the user just asked about.
pub(super) struct SettingsState {
    pub(super) rows: Vec<KeyRow>,
    /// The "check another key name" field.
    pub(super) custom: Entity<EditableTextState>,
    /// The "add excluded folder" field.
    pub(super) exclude_input: Entity<EditableTextState>,
    pub(super) protect_input: Entity<EditableTextState>,
    pub(super) note: Option<SharedString>,
    pub(super) note_epoch: u64,
    /// Where a damaged settings file was moved at startup, said once: taken
    /// from Broom when this Settings session opens, so it shows for the
    /// whole session and never again.
    pub(super) salvaged: Option<PathBuf>,
}

pub(super) struct KeyRow {
    /// The keychain service name — the key under which the value is stored.
    pub(super) service: SharedString,
    pub(super) title: SharedString,
    pub(super) hint: SharedString,
    pub(super) status: KeyStatus,
}

pub(super) enum KeyStatus {
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
    pub(super) fn of(key: Option<String>) -> Self {
        match key {
            Some(key) => KeyStatus::Set { masked: secrets::tail(&key) },
            None => KeyStatus::Missing,
        }
    }
}

/// A one-line tooltip; gpui's `.tooltip` wants a view.
pub(super) struct Tip(pub(super) SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(theme::radius(6.))
            .bg(rgb(colors().tooltip))
            .border_1()
            .border_color(rgb(colors().border))
            .text_xs()
            .text_color(rgb(colors().text))
            .child(self.0.clone())
    }
}

impl Render for DraggedItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded_full()
            .bg(rgb(colors().accent))
            .text_color(rgb(colors().on_accent))
            .text_sm()
            .font_family(theme::current().font)
            .child(format!("{}  ·  {}", self.name, format_size(self.size)))
    }
}

pub(super) enum Screen {
    /// Nothing scanned: the rail is live, the chart area says to pick a disk.
    Start,
    Scanning(Scanning),
    Results(Results),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Access {
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
pub(super) enum AskState {
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
pub(super) enum Unavailable {
    /// No API key configured.
    NoKey,
    /// The key was rejected.
    Rejected,
    /// Network or service trouble.
    Unreachable,
}
