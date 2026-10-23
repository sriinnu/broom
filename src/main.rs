mod app;
mod ask;
mod panel;
mod classify;
mod clock;
mod comic;
mod dirlist;
mod disk;
mod eta;
mod findings;
mod live;
mod menubar;
mod motion;
mod onboarding;
mod scan;
mod secrets;
mod settings;
mod single;
#[cfg(feature = "snapshot")]
mod snapshot;
mod sunburst;
mod trashcan;
mod cli;

use std::path::PathBuf;

use gpui::{App, KeyBinding, Menu, MenuItem, actions};

use app::{
    About, CloseAbout, CloseReview, CloseSettings, DismissQuery, FocusQuery, GoUp, OpenFolder, OpenSelected, OpenSettings, Rescan,
    SelectNext, SelectPrev, StartOver, SubmitQuery, ToggleStaged,
};

actions!(broom, [Quit]);

fn main() {
    let _ = app::LAUNCHED.set(std::time::Instant::now());
    let args: Vec<String> = std::env::args().collect();
    // Headless subcommands for scripts and agents: `broom scan|find|clean|trash|help`.
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }
    // Headless benchmark: `broom --bench-scan <path> [runs]`
    if args.get(1).map(String::as_str) == Some("--bench-scan") {
        let path = PathBuf::from(args.get(2).expect("path required"));
        let runs = args.get(3).and_then(|r| r.parse().ok()).unwrap_or(5);
        scan::bench(&path, runs);
        return;
    }
    // Diagnostic: does this process have Full Disk Access? (Launch through `open` to ask
    // about Broom.app itself rather than the terminal that started it.)
    if args.get(1).map(String::as_str) == Some("--check-access") {
        println!("full disk access: {}", if onboarding::has_full_disk_access() { "yes" } else { "no" });
        return;
    }
    // Headless live-chart benchmark: `broom --bench-live <path> [runs]`
    if args.get(1).map(String::as_str) == Some("--bench-live") {
        let path = PathBuf::from(args.get(2).expect("path required"));
        let runs = args.get(3).and_then(|r| r.parse().ok()).unwrap_or(3);
        live::bench(&path, runs);
        return;
    }
    // Read preferences once, here, and hand the same value to the view: a
    // second load would see a damaged file already moved aside by the first.
    let loaded = settings::load();
    // On first launch, start scanning the startup disk straight away.
    let initial = args
        .get(1)
        .map(PathBuf::from)
        .or_else(|| onboarding::is_first_run().then(|| loaded.settings.startup_root()));

    // One Broom: a second launch hands its folder to the running one and leaves.
    if !single::claim(initial.as_deref()) {
        return;
    }

    gpui_platform::application().run(move |cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        // The text input's defaults go in first: for the same key and context, the later
        // binding wins, so enter and escape below take over inside the query field.
        cx.bind_keys(
            gpui_ce_elements::editable_text::actions::default_bindings().as_keybindings(Some("EditableText")),
        );
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-o", OpenFolder, None),
            KeyBinding::new("cmd-r", Rescan, None),
            KeyBinding::new("cmd-up", GoUp, None),
            KeyBinding::new("backspace", GoUp, None),
            KeyBinding::new("escape", GoUp, None),
            KeyBinding::new("cmd-shift-d", StartOver, None),
            KeyBinding::new("cmd-f", FocusQuery, None),
            KeyBinding::new("enter", SubmitQuery, Some("EditableText")),
            KeyBinding::new("escape", DismissQuery, Some("EditableText")),
            KeyBinding::new("cmd-comma", OpenSettings, None),
            // Escape while a modal card is focused closes it (the card carries the
            // context); inside a modal text field the input's own escape wins.
            KeyBinding::new("escape", CloseSettings, Some("Settings")),
            KeyBinding::new("escape", CloseSettings, Some("Panel")),
            KeyBinding::new("escape", CloseAbout, Some("About")),
            KeyBinding::new("escape", CloseReview, Some("Review")),
            // The navigator's keyboard. Not inside a text field: there the
            // arrows, enter and space belong to the input.
            KeyBinding::new("up", SelectPrev, Some("Broom && !EditableText")),
            KeyBinding::new("down", SelectNext, Some("Broom && !EditableText")),
            KeyBinding::new("right", OpenSelected, Some("Broom && !EditableText")),
            KeyBinding::new("enter", OpenSelected, Some("Broom && !EditableText")),
            KeyBinding::new("left", GoUp, Some("Broom && !EditableText")),
            KeyBinding::new("space", ToggleStaged, Some("Broom && !EditableText")),
            // The menu-bar panel: escape closes it, enter asks.
            KeyBinding::new("escape", panel::ClosePanel, Some("SearchPanel")),
        ]);
        cx.set_menus(vec![
            Menu {
                name: "Broom".into(),
                items: vec![
                    MenuItem::action("About Broom", About),
                    MenuItem::separator(),
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Quit Broom", Quit),
                ],
                disabled: false,
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("Open Folder…", OpenFolder),
                    MenuItem::action("Ask Broom…", FocusQuery),
                    MenuItem::action("Rescan", Rescan),
                    MenuItem::separator(),
                    MenuItem::action("Clear Scan", StartOver),
                ],
                disabled: false,
            },
            Menu {
                name: "Go".into(),
                items: vec![MenuItem::action("Enclosing Folder", GoUp)],
                disabled: false,
            },
        ]);

        // Broom is a menu-bar utility: closing the window leaves the app (and
        // its 🧹 item) running; "Open Broom" in that menu brings the window
        // back, and Quit — the menu, ⌘Q or the Dock — is what ends it.
        app::open_main_window(initial, loaded, cx);
        app::arm_reopen(cx);
        app::arm_selftest(cx);
        // Dev aid, `BROOM_OPEN_PANEL=1`: pop the menu-bar panel shortly after
        // launch, so it can be screenshotted without a click on the 🧹.
        if std::env::var_os("BROOM_OPEN_PANEL").is_some() {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                let _ = cx.update(panel::toggle);
            })
            .detach();
        }
        cx.activate(true);
    });
}
