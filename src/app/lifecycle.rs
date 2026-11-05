//! Opening the window, requests from other threads, and the app-wide handles.

use super::*;

/// A change in size worth showing ("+1.2 GB", "−340 MB"); under a megabyte
/// either way it's noise, and `None`.
pub(super) fn format_delta(bytes: i64) -> Option<String> {
    (bytes.unsigned_abs() >= 1 << 20).then(|| format!("{}{}", if bytes > 0 { "+" } else { "−" }, format_size(bytes.unsigned_abs())))
}

/// How long ago, in one short token: "5m", "3h", "2d", "6w", "4mo", "2y".
pub(super) fn format_age(when: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(when).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..60 => "now".to_string(),
        60..3_600 => format!("{}m", secs / 60),
        3_600..86_400 => format!("{}h", secs / 3_600),
        86_400..1_209_600 => format!("{}d", secs / 86_400),
        1_209_600..5_259_600 => format!("{}w", secs / 604_800),
        5_259_600..31_557_600 => format!("{}mo", secs / 2_629_800),
        _ => format!("{}y", secs / 31_557_600),
    }
}

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
pub(super) static BROOM_ENTITY: std::sync::OnceLock<Entity<Broom>> = std::sync::OnceLock::new();

/// "Open Broom" and the 🧹 click tap this from AppKit's side of the world (see
/// `menubar`'s `MenuBridge`); `arm_reopen`'s task drains the other end.
pub(super) static OPEN_TX: std::sync::OnceLock<mpsc::Sender<Request>> = std::sync::OnceLock::new();

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
pub(super) fn survey_pool() -> &'static rayon::ThreadPool {
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
pub(super) fn window_options(cx: &App) -> WindowOptions {
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

pub(super) fn request(what: Request) {
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
pub(super) fn reopen(cx: &mut App) {
    if std::env::var_os("BROOM_MB_DEBUG").is_some() {
        eprintln!("menubar: reopen, windows: {}", cx.windows().len());
    }
    // The main window, not any window: Settings can be open on its own.
    if !cx.windows().iter().any(|w| w.downcast::<Broom>().is_some()) {
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

/// Retune the window's vibrancy, once, on the main thread. GPUI's `Blurred`
/// background uses the `Selection` material, which in dark appearance reads
/// as a flat dark pane — no desktop in it. The under-window material is the
/// one macOS itself uses to frost what's behind a window, which is the glass
/// the Start screen wants. If the vibrancy view isn't there for some reason,
/// add one behind everything rather than leave the window flat.
pub(super) fn frost_window(mtm: MainThreadMarker) {
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
pub(super) fn startup_fraction(volumes: &[Volume]) -> f32 {
    volumes
        .first()
        .map(|v| v.total.saturating_sub(v.free) as f32 / v.total.max(1) as f32)
        .unwrap_or(0.0)
}

/// What the menu-bar menu shows per volume: name, capacity, free space.
pub(super) fn volume_lines(volumes: &[Volume]) -> Vec<(String, u64, u64, u64)> {
    volumes.iter().map(|v| (v.name.clone(), v.total, v.free, v.purgeable)).collect()
}
