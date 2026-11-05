//! The toolbar and the sidebar (disks, folders, cloud).

use super::*;

/// The sidebar's show/hide button: a little window with its left column filled
/// while the column is there. Drawn, because glyphs for this go through the
/// emoji path and all but disappear on glass.
pub(super) fn rail_toggle(hidden: bool, cx: &mut Context<Broom>) -> impl IntoElement {
    button_base("rail-toggle", "")
        .bg(rgb(colors().card))
        .border_color(rgb(colors().border))
        .hover(|s| s.bg(rgb(colors().card_hover)))
        .px_2()
        .h(px(28.))
        .flex()
        .items_center()
        .child(
            div()
                .w(px(16.))
                .h(px(12.))
                .rounded(px(2.))
                .border_1()
                .border_color(rgb(colors().muted))
                .when(!hidden, |d| d.child(div().w(px(5.)).h_full().bg(rgb(colors().muted)))),
        )
        .tooltip(move |_, cx| cx.new(|_| Tip(if hidden { "Show sidebar ⌥⌘S".into() } else { "Hide sidebar ⌥⌘S".into() })).into())
        .on_click(cx.listener(|this, _, window, cx| this.toggle_rail(&ToggleRail, window, cx)))
}

/// The cleanup column's show/hide button: the rail toggle's mirror, its right
/// column filled while the column is there. Hidden with things staged, it says
/// how many, so a plan is never out of sight and out of mind.
pub(super) fn cleanup_toggle(hidden: bool, staged: usize, cx: &mut Context<Broom>) -> impl IntoElement {
    button_base("cleanup-toggle", "")
        .bg(rgb(colors().card))
        .border_color(rgb(colors().border))
        .hover(|s| s.bg(rgb(colors().card_hover)))
        .px_2()
        .h(px(28.))
        .flex()
        .items_center()
        .gap_1p5()
        .when(hidden && staged > 0, |b| {
            b.child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(colors().accent)).child(format!("{staged} staged")))
        })
        .child(
            div()
                .w(px(16.))
                .h(px(12.))
                .rounded(px(2.))
                .border_1()
                .border_color(rgb(colors().muted))
                .flex()
                .justify_end()
                .when(!hidden, |d| d.child(div().w(px(5.)).h_full().bg(rgb(colors().muted)))),
        )
        .tooltip(move |_, cx| {
            cx.new(|_| Tip(if hidden { "Show cleanup column ⌥⌘0".into() } else { "Hide cleanup column ⌥⌘0".into() })).into()
        })
        .on_click(cx.listener(|this, _, window, cx| this.toggle_cleanup(&ToggleCleanup, window, cx)))
}

impl Broom {
    /// Hide or show the findings and cleanup plan, for more room for the chart
    /// and the list. Not on the start screen, as with the rail.
    pub(super) fn toggle_cleanup(&mut self, _: &ToggleCleanup, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.screen, Screen::Start) {
            return;
        }
        self.settings.cleanup_collapsed = !self.settings.cleanup_collapsed;
        let _ = settings::save(&self.settings);
        self.refresh_menus(cx);
        cx.notify();
    }

    pub(super) fn cleanup_hidden(&self) -> bool {
        self.settings.cleanup_collapsed && !matches!(self.screen, Screen::Start)
    }

    /// Hide or show the disks and folders column. With nothing scanned it's
    /// the only way to start, so there it stays, and the shortcut does nothing.
    pub(super) fn toggle_rail(&mut self, _: &ToggleRail, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.screen, Screen::Start) {
            return;
        }
        self.settings.rail_collapsed = !self.settings.rail_collapsed;
        let _ = settings::save(&self.settings);
        self.refresh_menus(cx);
        cx.notify();
    }

    pub(super) fn rail_hidden(&self) -> bool {
        self.settings.rail_collapsed && !matches!(self.screen, Screen::Start)
    }

    pub(super) fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .border_color(rgb(colors().border))
            .bg(to_hsla(colors().panel))
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            });

        if !matches!(self.screen, Screen::Start) {
            bar = bar.child(rail_toggle(self.rail_hidden(), cx));
        }
        match &self.screen {
            Screen::Results(r) => {
                let can_go_up = r.tree.nodes[r.focus].parent.is_some();
                // Dimmed, not hidden: the row keeps its width and the crumbs don't
                // jump on every move.
                bar = bar
                    .child(
                        button("back", "‹")
                            .text_base()
                            .px_2()
                            .when(!r.can_travel(Travel::Back), |b| b.opacity(0.4))
                            .tooltip(|_, cx| cx.new(|_| Tip("Back ⌘[".into())).into())
                            .on_click(cx.listener(|this, _, window, cx| this.go_back(&GoBack, window, cx))),
                    )
                    .child(
                        button("forward", "›")
                            .text_base()
                            .px_2()
                            .when(!r.can_travel(Travel::Forward), |b| b.opacity(0.4))
                            .tooltip(|_, cx| cx.new(|_| Tip("Forward ⌘]".into())).into())
                            .on_click(cx.listener(|this, _, window, cx| this.go_forward(&GoForward, window, cx))),
                    )
                    .child(
                        button("up", "↑")
                            .text_base()
                            .px_2()
                            .when(!can_go_up, |b| b.opacity(0.4))
                            .tooltip(|_, cx| cx.new(|_| Tip("Enclosing folder ⌘↑".into())).into())
                            .on_click(cx.listener(|this, _, window, cx| this.go_up(&GoUp, window, cx))),
                    );
                let chain = r.tree.ancestry(r.focus);
                let mut crumbs = div().flex().items_center().gap_1().min_w_0().overflow_hidden();
                for (i, &ix) in chain.iter().enumerate() {
                    let is_last = i + 1 == chain.len();
                    if i > 0 {
                        crumbs = crumbs.child(div().text_color(rgb(colors().muted)).child("›"));
                    }
                    crumbs = crumbs.child(
                        div()
                            .id(("crumb", ix))
                            .px_1p5()
                            .py_0p5()
                            .rounded(theme::radius(6.))
                            .flex_shrink(1.)
                            .min_w(px(24.))
                            .truncate()
                            .when(is_last, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .when(!is_last, |d| d.text_color(rgb(colors().muted)).flex_shrink_0())
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(colors().card_hover)))
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
                let here: SharedString = r.tree.path_of(r.focus).to_string_lossy().into_owned().into();
                // icon_button's look, but with the full path in a caption box for a tooltip.
                let copy = div()
                    .id("copy-here")
                    .size(px(20.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::radius(6.))
                    .text_color(rgb(colors().muted))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(colors().border)).text_color(rgb(colors().text)))
                    .child("⎘")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click({
                        let here = here.clone();
                        move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(here.to_string()))
                    })
                    .tooltip(move |_, cx| cx.new(|_| comic::PathCaption(format!("{here}  ·  click or ⌘C to copy").into())).into());
                bar = bar.child(crumbs).child(copy).child(div().flex_1());
                // The query field. Stop mouse-down here so a click into it doesn't
                // start a window drag on the toolbar.
                bar = bar.child(
                    div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(
                        text_input("query")
                            .placeholder("Find by name, or ask…")
                            .placeholder_color(theme::overlay(0.5))
                            .caret_color(theme::input_caret())
                            .selection_color(theme::input_selection())
                            .state(self.query.downgrade())
                            .caret_blink_interval_500ms()
                            .w(px(220.))
                            .h(px(28.))
                            .px_2()
                            .min_h_auto()
                            .flex()
                            .items_center()
                            .rounded(theme::radius(6.))
                            .border_1()
                            .border_color(rgb(colors().border))
                            .bg(rgb(colors().bg))
                            .text_size(px(12.))
                            // One line as tall as the field inside its border: the element draws
                            // from the top, so this, not flex, is what centres the text.
                            .line_height(px(26.)),
                    ),
                );
                bar = bar
                    .child(color_toggle(r.color_by, cx))
                    .child(
                        div().text_xs().text_color(rgb(colors().muted)).flex_none().child(format!(
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
                    primary_button("cancel", "Cancel Scan", colors().danger).on_click(cx.listener(|this, _, _, cx| this.cancel_scan(cx))),
                );
            }
            // Nothing scanned: the rail is the call to action; up here only the
            // drag surface and Settings.
            Screen::Start => {}
        }
        // Settings is reachable from every screen. A word, not a gear glyph:
        // "⚙" renders through the emoji path and all but disappears on glass.
        let bar = bar.child(div().flex_1()).child(
            button("settings", "Settings")
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(&OpenSettings, window, cx)))
                .tooltip(move |_, cx| cx.new(|_| Tip("Settings ⌘,".into())).into()),
        );
        if matches!(self.screen, Screen::Start) {
            return bar;
        }
        let staged = match &self.screen {
            Screen::Results(r) => r.collector.len(),
            _ => 0,
        };
        bar.child(cleanup_toggle(self.cleanup_hidden(), staged, cx))
    }

    /// The left rail: every disk with its fill, the home folder, synced cloud
    /// folders, a folder picker, and the keyboard hint. Clicking anything scans it.
    pub(super) fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active: Option<PathBuf> = match &self.screen {
            Screen::Results(r) => Some(r.requested_root.clone()),
            Screen::Scanning(s) => Some(s.root.clone()),
            Screen::Start => None,
        };
        let heading = |text: &'static str| {
            div().px_3().pt_3().pb_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(colors().muted)).child(text)
        };
        let row = |id: gpui::ElementId, is_active: bool| {
            div()
                .id(id)
                .mx_1p5()
                .px_2()
                .py_1p5()
                .rounded(theme::radius(6.))
                .cursor_pointer()
                .when(is_active, |d| d.bg(rgb(colors().card_hover)))
                .when(!is_active, |d| d.hover(|s| s.bg(rgb(colors().card))))
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
                    .child(div().text_xs().text_color(rgb(colors().muted)).truncate().child(format!(
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
                                .text_color(rgb(colors().muted))
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
                .child(path_tip(div().id("home-path"), abbreviate_home(&home.to_string_lossy())).text_xs().text_color(rgb(colors().muted)).truncate().child(abbreviate_home(&home.to_string_lossy()))),
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
                        .child(path_tip(div().id(("cloud-path", i)), abbreviate_home(&cloud.path.to_string_lossy())).text_xs().text_color(rgb(colors().muted)).truncate().child(abbreviate_home(&cloud.path.to_string_lossy()))),
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
            .w(px(self.pane_width(Pane::Rail)))
            .relative()
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(colors().panel))
            .border_r_1()
            .border_color(rgb(colors().border))
            .child(list)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(colors().border))
                    .text_xs()
                    .text_color(rgb(colors().muted))
                    .child("↑↓ → ← navigate · ␣ stage"),
            )
            .child(self.pane_handle(Pane::Rail, cx))
    }
}
