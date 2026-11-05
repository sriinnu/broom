//! The `Broom` view: its state, its preferences and the top of the render tree.

use super::*;

pub struct Broom {
    pub(super) screen: Screen,
    /// The rail's disks and synced folders, refreshed in the background.
    pub(super) volumes: Vec<Volume>,
    pub(super) clouds: Vec<CloudRoot>,
    /// Navigator order, kept across scans.
    pub(super) sort: SortBy,
    pub(super) sort_descending: bool,
    /// Column widths: the live values, written back to settings when a drag ends.
    pub(super) rail_width: f32,
    pub(super) nav_width: f32,
    pub(super) cleanup_width: f32,
    /// A column-edge drag in flight: which column, pointer x and width at press.
    pub(super) pane_drag: Rc<Cell<Option<(Pane, f32, f32)>>>,
    /// The widths actually drawn this frame, by `Pane` order.
    pub(super) fitted: [f32; 3],
    /// The review card before a move to the Trash, if open.
    pub(super) review: Option<ReviewState>,
    pub(super) focus_handle: FocusHandle,
    pub(super) error: Option<String>,
    pub(super) access: Access,
    pub(super) _access_watch: Option<Task<()>>,
    /// Kept here rather than on the results, so a rescan keeps the user's choice.
    pub(super) color_by: ColorBy,
    /// Show the Modified column in the navigator. Lives here, next to `color_by`,
    /// for the same reason: it survives a rescan and takes effect immediately.
    pub(super) show_modified: bool,
    /// The query field's content, owned by the input element; Broom holds the entity.
    pub(super) query: Entity<EditableTextState>,
    /// Right-click menu, if one is open. Dropped whenever the node it points at is
    /// detached or the scan changes underneath it.
    pub(super) context_menu: Option<ContextMenu>,
    /// The open modal card, if any.
    pub(super) modal: Option<PanelState>,
    /// Focus target while a modal is open, so Escape lands on the card.
    pub(super) modal_focus: FocusHandle,
    /// The menu-bar extra: flower + startup-disk fill, volumes in its menu.
    pub(super) menubar: Option<menubar::MenuBar>,
    pub(super) _menubar_watch: Option<Task<()>>,
    /// Re-reads the Trash when Finder changes it; see `watch_trash`.
    pub(super) _trash_watch: Option<Task<()>>,
    /// Repaints in the other palette when macOS flips between light and dark.
    pub(super) _appearance_watch: Option<Subscription>,
    /// Bumped every time Settings opens, so a keychain result from a previous
    /// session can't land on the rows of the current one.
    pub(super) settings_epoch: u64,
    /// True while a native sheet or open panel has the window (trash confirm,
    /// folder picker). Our own modal must not open underneath those.
    pub(super) busy: bool,
    /// What the user has chosen. Loaded once at startup and written back on
    /// every change; the live copy is the truth while the app runs.
    pub(super) settings: Settings,
    /// Set when a damaged settings file had to be moved aside at startup, so
    /// the Settings panel can say so once instead of the app silently
    /// forgetting everything the user had set up.
    pub(super) settings_salvaged: Option<PathBuf>,
}

impl Render for Broom {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.fit_widths(window);
        self.fetch_mtimes(cx);
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
            .children((!self.rail_hidden()).then(|| self.render_rail(cx)))
            .child(div().flex_1().min_w_0().h_full().flex().child(content))
            .children((!self.cleanup_hidden()).then(|| self.render_cleanup(cx)));

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
            .on_action(cx.listener(Self::copy_path))
            .on_action(cx.listener(Self::copy_relative_path))
            .on_action(cx.listener(|this, _: &SortBySize, _, cx| this.choose_sort(SortBy::Size, cx)))
            .on_action(cx.listener(|this, _: &SortByName, _, cx| this.choose_sort(SortBy::Name, cx)))
            .on_action(cx.listener(|this, _: &SortByModified, _, cx| this.choose_sort(SortBy::Modified, cx)))
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::go_forward))
            .on_action(cx.listener(Self::toggle_rail))
            .on_action(cx.listener(Self::toggle_cleanup))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::minimize))
            .on_action(cx.listener(Self::zoom))
            .on_action(cx.listener(Self::select_all))
            .size_full()
            .flex()
            .flex_col()
            .bg(to_hsla(colors().bg))
            .text_color(rgb(colors().text))
            .font_family(theme::current().font)
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
                        .rounded(theme::radius(8.))
                        .bg(rgb(colors().danger))
                        .text_color(rgb(colors().on_danger))
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
            sort_descending: true,
            rail_width: settings.rail_width,
            nav_width: settings.navigator_width,
            cleanup_width: settings.cleanup_width,
            pane_drag: Rc::default(),
            fitted: [settings.rail_width, settings.navigator_width, settings.cleanup_width],
            review: None,
            focus_handle,
            error: None,
            access,
            _access_watch: None,
            color_by: match settings.color_by {
                settings::ColorBy::Folder => ColorBy::Folder,
                settings::ColorBy::Kind => ColorBy::Kind,
            },
            show_modified: settings.show_modified,
            // Created directly rather than `use_keyed`: that's a render-time hook
            // (element-state cache), and Broom owns this entity for the app's life anyway.
            query: cx.new(|cx| EditableTextState::new(StringStorage::default(), cx)),
            context_menu: None,
            modal: None,
            modal_focus: cx.focus_handle(),
            menubar: None,
            _menubar_watch: None,
            _trash_watch: None,
            _appearance_watch: None,
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
        theme::apply(this.settings.theme, this.settings.appearance, cx);
        this._appearance_watch = Some(cx.observe_window_appearance(window, |this, _, cx| this.restyle(cx)));
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

    /// For the recorder: has the scan finished?
    #[cfg(feature = "snapshot")]
    pub fn showing_results(&self) -> bool {
        matches!(self.screen, Screen::Results(_))
    }

    pub(super) fn results(&mut self) -> Option<&mut Results> {
        match &mut self.screen {
            Screen::Results(r) => Some(r),
            _ => None,
        }
    }

    pub(super) fn set_color_by(&mut self, color_by: ColorBy, cx: &mut Context<Self>) {
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

    /// Show or hide dates in the list. A Modified sort asks for them either way
    /// (room permitting), so this never changes how the list is ordered.
    pub(super) fn set_show_modified(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_modified = show;
        self.settings.show_modified = show;
        let _ = settings::save(&self.settings);
        if let Some(r) = self.results() {
            r.show_modified = show;
            r.reorder();
        }
        cx.notify();
    }

    pub(super) fn set_show_created(&mut self, show: bool, cx: &mut Context<Self>) {
        self.settings.show_created = show;
        let _ = settings::save(&self.settings);
        if let Some(r) = self.results() {
            r.show_created = show;
            r.reorder();
        }
        cx.notify();
    }

    pub(super) fn set_theme(&mut self, name: theme::ThemeName, cx: &mut Context<Self>) {
        self.settings.theme = name;
        let _ = settings::save(&self.settings);
        self.restyle(cx);
    }

    pub(super) fn set_appearance(&mut self, appearance: theme::Appearance, cx: &mut Context<Self>) {
        self.settings.appearance = appearance;
        let _ = settings::save(&self.settings);
        self.restyle(cx);
    }

    /// Bring the palette in line with Settings and macOS. The chart's swatches
    /// are worked out at layout time, so a finished scan lays out again.
    pub(super) fn restyle(&mut self, cx: &mut Context<Self>) {
        theme::apply(self.settings.theme, self.settings.appearance, cx);
        if let Some(r) = self.results() {
            r.relayout();
        }
        cx.notify();
    }

    /// Toggle the menu-bar 🧹 icon on or off at runtime. Creating the status
    /// item requires the main thread, which the UI thread is.
    pub(super) fn toggle_menubar(&mut self, cx: &mut Context<Self>) {
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
}
