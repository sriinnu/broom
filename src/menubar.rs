//! The menu-bar extra: the flower and the startup disk's fill at a glance,
//! with every volume's free space in the menu. Broom's own UI stays gpui; this
//! is the one piece that reaches into AppKit, on the main thread only.

use objc2::ClassType;
use objc2::MainThreadMarker;
use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::NSObject;
use objc2::runtime::NSObjectProtocol;
use objc2::sel;
use std::cell::RefCell;

use objc2_app_kit::{NSApplication, NSEventMask, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem, NSScreen, NSStatusBar, NSStatusBarButton, NSStatusItem};
use objc2_foundation::NSString;

use crate::scan::format_size;

/// How many volume lines the menu reserves up front; extra slots stay hidden.
const MAX_VOLUME_LINES: usize = 6;

// Receives AppKit's menu-item actions so "Open Broom" can reach Rust. The
// selector does nothing AppKit-side; it just taps the channel that wakes the
// reopen task (see `app::arm_reopen`). The object lives on the main thread,
// retained by the `MenuBar`.
define_class!(
    #[unsafe(super = NSObject)]
    struct MenuBridge;

    unsafe impl NSObjectProtocol for MenuBridge {}

    impl MenuBridge {
        #[unsafe(method(openBroom:))]
        fn open_broom(&self, _sender: Option<&AnyObject>) {
            crate::app::open_requested();
        }

        /// The ✿ button itself: left click opens the search panel, right
        /// click pops the menu (volumes, Open, Quit).
        #[unsafe(method(statusClicked:))]
        fn status_clicked(&self, _sender: Option<&AnyObject>) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let right = NSApplication::sharedApplication(mtm)
                .currentEvent()
                .map(|event| {
                    matches!(event.r#type(), NSEventType::RightMouseUp | NSEventType::RightMouseDown)
                        || event.modifierFlags().contains(NSEventModifierFlags::Control)
                })
                .unwrap_or(false);
            if right {
                pop_menu(mtm);
            } else {
                crate::app::panel_requested();
            }
        }

        #[unsafe(method(searchBroom:))]
        fn search_broom(&self, _sender: Option<&AnyObject>) {
            crate::app::panel_requested();
        }
    }
);

thread_local! {
    /// The status item and its menu, for the right-click pop-up. Main thread
    /// only, like everything AppKit.
    static MENU: RefCell<Option<(Retained<NSStatusItem>, Retained<NSMenu>)>> = const { RefCell::new(None) };
}

/// Show the menu under the item the way a menu-bearing item would: attach it
/// for the duration of one synthetic click, then detach so the next left
/// click is ours again.
fn pop_menu(mtm: MainThreadMarker) {
    // Clone out of the cell first: `performClick` runs the menu's own event loop,
    // and a borrow held across it would panic any `MenuBar::new` meanwhile.
    let Some((item, menu)) = MENU.with(|cell| cell.borrow().clone()) else { return };
    item.setMenu(Some(&menu));
    if let Some(button) = item.button(mtm) {
        // Safety: `performClick:` takes any object or nil as the sender.
        unsafe { button.performClick(None) };
    }
    item.setMenu(None);
}

impl MenuBridge {
    fn new() -> Retained<Self> {
        // The class inherits NSObject's `new`, which allocates zeroed — our
        // class has no ivars of its own.
        unsafe { msg_send![Self::class(), new] }
    }
}

pub struct MenuBar {
    /// Keeps the status item alive; dropping it would remove it from the bar.
    _item: Retained<NSStatusItem>,
    /// Keeps the action target alive for as long as the menu can fire.
    _bridge: Retained<MenuBridge>,
    /// The bar's text ("✿ 62%"), updated in place.
    button: Retained<NSStatusBarButton>,
    /// One menu line per volume, hidden when unused, so titles can be updated
    /// in place instead of rebuilding the menu.
    lines: Vec<Retained<NSMenuItem>>,
}

fn ns(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}

impl MenuBar {
    /// Build the status item on the main thread: `✿ 62%` in the bar, volume
    /// lines below it, then Open and Quit. The action items target the shared
    /// application with selectors it already implements — no subclass needed.
    pub fn new(mtm: MainThreadMarker, startup_fraction: f32) -> Self {
        let bar = NSStatusBar::systemStatusBar();
        // -1.0 is `NSVariableStatusItemLength`: size to whatever we put in it.
        let item = bar.statusItemWithLength(-1.0);
        let button = item.button(mtm).expect("a fresh status item has a button");
        button.setTitle(&ns(&title_text(startup_fraction)));
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);

        let head = NSMenuItem::new(mtm);
        head.setTitle(&ns("Broom"));
        head.setEnabled(false);
        menu.addItem(&head);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let mut lines = Vec::new();
        for _ in 0..MAX_VOLUME_LINES {
            let line = NSMenuItem::new(mtm);
            line.setTitle(&ns(""));
            line.setEnabled(false);
            line.setHidden(true);
            menu.addItem(&line);
            lines.push(line);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // "Open Broom" targets our bridge so the click reaches Rust and can
        // reopen the window; Quit goes straight to NSApplication, which
        // implements `terminate:` without our help.
        // Safety: both `Retained::cast_unchecked` upcasts are to the root
        // object type, which is what `setTarget` takes; the selectors exist
        // (one on `MenuBridge`, one on NSApplication).
        let bridge = MenuBridge::new();
        unsafe {
            let search = NSMenuItem::new(mtm);
            search.setTitle(&ns("Search & Clean…"));
            search.setTarget(Some(&Retained::cast_unchecked::<AnyObject>(bridge.clone())));
            search.setAction(Some(sel!(searchBroom:)));
            search.setEnabled(true);
            menu.addItem(&search);

            let open = NSMenuItem::new(mtm);
            open.setTitle(&ns("Open Broom"));
            open.setTarget(Some(&Retained::cast_unchecked::<AnyObject>(bridge.clone())));
            open.setAction(Some(sel!(openBroom:)));
            open.setEnabled(true);
            menu.addItem(&open);

            let app = Retained::cast_unchecked::<AnyObject>(NSApplication::sharedApplication(mtm));
            let quit = NSMenuItem::new(mtm);
            quit.setTitle(&ns("Quit Broom"));
            quit.setTarget(Some(&app));
            quit.setAction(Some(sel!(terminate:)));
            quit.setEnabled(true);
            menu.addItem(&quit);

            // No menu attached: a left click is an action (the search panel)
            // and a right click pops the menu by hand (`pop_menu`).
            button.setTarget(Some(&Retained::cast_unchecked::<AnyObject>(bridge.clone())));
            button.setAction(Some(sel!(statusClicked:)));
            button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        }
        MENU.with(|cell| *cell.borrow_mut() = Some((item.clone(), menu)));
        if std::env::var_os("BROOM_MB_DEBUG").is_some() {
            eprintln!(
                "menubar: policy={:?} title={:?} hidden={} length={:?} in_window={} frame={:?} win_visible={:?}",
                NSApplication::sharedApplication(mtm).activationPolicy(),
                title_text(startup_fraction),
                button.isHidden(),
                item.length(),
                button.window().is_some(),
                button.frame(),
                button.window().map(|w| w.isVisible()),
            );
        }
        Self { _item: item, _bridge: bridge, button, lines }
    }

    /// Where to hang the search panel: the item's centre x and the bottom of the
    /// menu bar, in gpui's global coordinates (origin at the primary display's
    /// top-left, y down), plus the item's width. Main thread only.
    pub fn anchor(&self, mtm: MainThreadMarker) -> Option<(f32, f32, f32)> {
        let window = self.button.window()?;
        let frame = window.frame();
        let primary = NSScreen::screens(mtm).firstObject()?.frame();
        let bottom = primary.size.height - frame.origin.y;
        Some(((frame.origin.x + frame.size.width / 2.0) as f32, bottom as f32, frame.size.width as f32))
    }

    /// Refresh the bar's percentage and the volume lines; main thread only.
    pub fn update(&self, startup_fraction: f32, volumes: &[(String, u64, u64)]) {        self.button.setTitle(&ns(&title_text(startup_fraction)));
        for (ix, line) in self.lines.iter().enumerate() {
            match volumes.get(ix) {
                Some((name, total, free)) => {
                    line.setTitle(&ns(&format!("{name} — {} free of {}", format_size(*free), format_size(*total))));
                    line.setHidden(false);
                }
                None => line.setHidden(true),
            }
        }
    }

    /// Dev aid (`BROOM_MB_SELFTEST`): fire "Open Broom" through AppKit's own
    /// action dispatch, as close to choosing the menu item as code can get.
    /// Returns whether AppKit found and invoked the action.
    pub fn simulate_open_click(&self, mtm: MainThreadMarker) -> bool {
        // Safety: `MenuBridge` implements `openBroom:`, and the cast to the
        // root object type is what `sendAction`'s parameter wants.
        unsafe {
            let handled = NSApplication::sharedApplication(mtm).sendAction_to_from(
                sel!(openBroom:),
                Some(&Retained::cast_unchecked::<AnyObject>(self._bridge.clone())),
                None,
            );
            if std::env::var_os("BROOM_MB_DEBUG").is_some() {
                eprintln!("menubar: sendAction handled={handled}");
            }
            handled
        }
    }
}

fn title_text(fraction: f32) -> String {
    if let Ok(custom) = std::env::var("BROOM_MB_TITLE") {
        return custom;
    }
    format!("🧹 {:.0}%", (fraction * 100.0).clamp(0.0, 100.0))
}
