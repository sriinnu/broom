//! The menu bar, laid out as the Human Interface Guidelines lay out a Mac
//! app's: the app menu, File, Edit, View, Go, Window and Help, each holding
//! the items people look for there.

use super::*;
use gpui::{Menu, MenuItem, SystemMenuType};

actions!(broom, [Quit, HideApp, HideOthers, ShowAll, CloseWindow, Minimize, Zoom, BringAllToFront, OpenHelp]);

/// The menus. Built again whenever a column is hidden or shown, so the View
/// menu says what choosing the item will do ("Hide Sidebar", "Show Sidebar")
/// rather than an "or".
pub fn app_menus(rail_hidden: bool, cleanup_hidden: bool) -> Vec<Menu> {
    let menu = |name: &str, items: Vec<MenuItem>| Menu { name: name.to_string().into(), items, disabled: false };
    vec![
        menu("Broom", vec![
            MenuItem::action("About Broom", About),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Broom", HideApp),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Broom", Quit),
        ]),
        menu("File", vec![
            MenuItem::action("Open Folder…", OpenFolder),
            MenuItem::action("Rescan", Rescan),
            MenuItem::action("Full Rescan", FullRescan),
            MenuItem::separator(),
            MenuItem::action("Empty Trash…", EmptyTrash),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
            MenuItem::separator(),
            MenuItem::action("Clear Scan", StartOver),
        ]),
        menu("Edit", vec![
            MenuItem::action("Copy Path", CopyPath),
            MenuItem::action("Copy Path with ~", CopyRelativePath),
            MenuItem::separator(),
            MenuItem::action("Select All", SelectAll),
            MenuItem::separator(),
            MenuItem::action("Find or Ask…", FocusQuery),
        ]),
        menu("View", vec![
            MenuItem::action("Sort by Size", SortBySize),
            MenuItem::action("Sort by Name", SortByName),
            MenuItem::action("Sort by Date Modified", SortByModified),
            MenuItem::separator(),
            MenuItem::action(if rail_hidden { "Show Sidebar" } else { "Hide Sidebar" }, ToggleRail),
            MenuItem::action(if cleanup_hidden { "Show Cleanup Column" } else { "Hide Cleanup Column" }, ToggleCleanup),
            MenuItem::separator(),
            MenuItem::action("Where Is My Space…", ShowSpace),
        ]),
        menu("Go", vec![
            MenuItem::action("Back", GoBack),
            MenuItem::action("Forward", GoForward),
            MenuItem::action("Enclosing Folder", GoUp),
        ]),
        menu("Window", vec![
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Bring All to Front", BringAllToFront),
        ]),
        menu("Help", vec![MenuItem::action("Broom Help", OpenHelp)]),
    ]
}

/// The app-wide items: they work with no window open, as Broom keeps running
/// in the menu bar after its window closes.
pub fn handle_app_menu_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &BringAllToFront, cx| cx.activate(true));
    cx.on_action(|_: &OpenHelp, cx| cx.open_url(&format!("{REPO_URL}#readme")));
    // Settings is its own window, so ⌘, works from the main window, from
    // Settings itself, and with no window open at all.
    cx.on_action(|_: &OpenSettings, cx| open_settings_window(cx));
}

impl Broom {
    pub(super) fn close_window(&mut self, _: &CloseWindow, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }

    pub(super) fn minimize(&mut self, _: &Minimize, window: &mut Window, _: &mut Context<Self>) {
        window.minimize_window();
    }

    pub(super) fn zoom(&mut self, _: &Zoom, window: &mut Window, _: &mut Context<Self>) {
        window.zoom_window();
    }

    /// Rebuilt after a column is hidden or shown; see `app_menus`.
    pub(super) fn refresh_menus(&self, cx: &mut Context<Self>) {
        cx.set_menus(app_menus(self.settings.rail_collapsed, self.settings.cleanup_collapsed));
    }
}
