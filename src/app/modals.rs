//! Opening and closing the cards, the About card and the right-click menu.

use super::*;

/// Zero-height canvases at a scroll area's content ends record where it
/// starts and stops. When a recorded end moves (a tab switch, a row
/// growing), one more frame keeps the scrollbar truthful.
pub(super) fn measure(cell: Rc<Cell<Option<f32>>>) -> impl IntoElement {
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
}

impl Broom {
    /// Escape on the Help card (its key context is "Panel"). Settings has a
    /// window of its own now, which handles its own Escape.
    pub(super) fn close_settings(&mut self, _: &CloseSettings, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn close_about(&mut self, _: &CloseAbout, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() {
            self.close_modal(window, cx);
        }
    }

    /// Open the card on a tab — or, if it's already up, just switch to it.
    pub(super) fn open_panel(&mut self, tab: PanelTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.modal {
            p.tab = tab;
        } else {
            self.modal = Some(PanelState { tab, scroller: Scroller::default() });
        }
        window.focus(&self.modal_focus, cx);
        cx.notify();
    }

    /// Enter inside Settings: only the key-name field responds — Enter there
    /// checks the keychain. Keys are pasted rather than typed, so no other
    /// field gives Enter anything to do. `window` is the Settings window.
    pub(super) fn settings_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(focused) = window.focused(cx) else { return };
        let check = match &self.prefs {
            Some(p) => focused == p.settings.custom.read(cx).focus_handle(cx),
            None => false,
        };
        if check {
            self.add_custom_key(cx);
        }
    }

    pub(super) fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// The backdrop was clicked: whichever card is up goes away (a review that
    /// is mid-delete stays until it's done).
    pub(super) fn close_any_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.empty_trash.is_some() {
            self.close_empty_trash(&CloseEmptyTrash, window, cx);
            return;
        }
        if self.merge.is_some() {
            self.close_merge(&CloseMerge, window, cx);
            return;
        }
        if self.space.is_some() {
            self.close_space(&CloseSpace, window, cx);
            return;
        }
        if let Some(review) = &self.review {
            if review.working {
                return;
            }
            self.review = None;
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return;
        }
        if self.clone_picker.is_some() {
            self.close_clone_picker(window, cx);
            return;
        }
        self.close_modal(window, cx);
    }

    /// The dimmed backdrop with the modal card on top. Clicking the backdrop closes;
    /// the card carries a key context so Escape can be bound to closing it.
    pub(super) fn render_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (context, card) = match (&self.review, &self.modal) {
            (Some(review), _) => ("Review", self.render_review(review, cx).into_any_element()),
            (None, _) if self.empty_trash.is_some() => {
                ("EmptyTrash", self.render_empty_trash(self.empty_trash.as_ref().expect("checked"), cx))
            }
            (None, _) if self.merge.is_some() => ("Merge", self.render_merge(self.merge.as_ref().expect("checked"), cx)),
            (None, _) if self.space.is_some() => ("Space", self.render_space(self.space.as_ref().expect("checked"), cx)),
            (None, Some(p)) => (
                match p.tab {
                    PanelTab::Help => "Panel",
                    PanelTab::About => "About",
                },
                self.render_panel(p, cx).into_any_element(),
            ),
            (None, None) if self.clone_picker.is_some() => ("ClonePeers", self.render_clone_picker(cx)),
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
                            .when(context == "ClonePeers", |card| card
                                .on_action(cx.listener(|this, _: &CloseClonePeers, window, cx| this.close_clone_picker(window, cx)))
                                .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_clone_cursor(1, cx)))
                                .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.move_clone_cursor(-1, cx)))
                                .on_action(cx.listener(|this, _: &ToggleStaged, _, cx| this.choose_active_clone(false, cx)))
                                .on_action(cx.listener(|this, _: &KeepClone, _, cx| this.choose_active_clone(true, cx)))
                                .on_action(cx.listener(|this, _: &OpenSelected, window, cx| {
                                    if let Some(picker) = &this.clone_picker {
                                        this.show_clone_file(picker.rows[picker.cursor], window, cx);
                                    }
                                }))
                                .on_action(cx.listener(|this, _: &StageCloneChoices, window, cx| this.stage_clone_choices(window, cx))))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(card),
                    ),
            )
            .into_any_element(),
        )
    }

    /// The panel card: tabs across the top, the tab's content below.
    pub(super) fn render_panel(&self, p: &PanelState, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded(theme::radius(8.))
            .border_1()
            .border_color(rgb(colors().border))
            .bg(rgb(colors().bg))
            .text_xs();
        for (label, tab) in [("Help", PanelTab::Help), ("About", PanelTab::About)] {
            let selected = p.tab == tab;
            tabs = tabs.child(
                div()
                    .id(("panel-tab", tab as usize))
                    .px_2()
                    .py_0p5()
                    .rounded(theme::radius(6.))
                    .cursor_pointer()
                    .when(selected, |d| {
                        d.bg(rgb(colors().card_hover)).text_color(rgb(colors().text)).font_weight(FontWeight::MEDIUM)
                    })
                    .when(!selected, |d| d.text_color(rgb(colors().muted)).hover(|s| s.text_color(rgb(colors().text))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = &mut this.modal {
                            p.tab = tab;
                            // Each tab starts at the top, with nothing measured yet.
                            p.scroller.scroll.set_offset(point(px(0.), px(0.)));
                            p.scroller.content_top.set(None);
                            p.scroller.content_bottom.set(None);
                        }
                        cx.notify();
                    }))
                    .child(label),
            );
        }

        // Height follows the tab's content, capped: the header (tabs + Done)
        // stays put and only the content scrolls, in a viewport with Broom's
        // own scrollbar beside it — none when it all fits.
        div()
            .id("panel-card")
            .w(px(520.))
            .max_h(px(560.))
            .p_4()
            .rounded(theme::radius(12.))
            .bg(rgb(colors().panel))
            .border_1()
            .border_color(rgb(colors().border))
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
                            .track_scroll(&p.scroller.scroll)
                            .child(
                                div()
                                    .id("panel-content")
                                    .flex()
                                    .flex_col()
                                    .child(measure(p.scroller.content_top.clone()))
                                    .child(match p.tab {
                                        PanelTab::Help => render_help().into_any_element(),
                                        PanelTab::About => self.render_about(cx).into_any_element(),
                                    })
                                    .child(measure(p.scroller.content_bottom.clone())),
                            ),
                    )
                    .child(panel_scrollbar(&p.scroller)),
            )
    }
}

/// Broom's own scrollbar: a slim column beside the scrolling content,
/// painted as track + rounded thumb and dragged through raw mouse events.
/// `ScrollHandle::set_offset` writes exactly what it's handed — no
/// clamping — so every write here clamps first.
pub(super) fn panel_scrollbar(p: &Scroller) -> impl IntoElement + use<> {
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
            // gpui keeps scroll offsets at or below zero (content moved up);
            // the thumb wants the distance scrolled, so flip the sign both ways.
            let offset: f32 = -f32::from(scroll.offset().y);
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
                theme::overlay(0.05),
                px(0.),
                gpui::hsla(0., 0., 0., 0.),
                BorderStyle::default(),
            ));
            let held = drag.get().is_some();
            window.paint_quad(quad(
                Bounds::from_corners(point(px(left + 2.), px(thumb_top)), point(px(left + 8.), px(thumb_top + thumb_h))),
                px(3.),
                theme::overlay(if held { 0.38 } else { 0.22 }),
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
                scroll_d.set_offset(point(px(0.), px(-from.clamp(0.0, max_scroll))));
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
                scroll_m.set_offset(point(px(0.), px(-to)));
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

impl Broom {
    pub(super) fn render_about(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .pt_2()
            .child(img(ABOUT_MARK.clone()).size(px(72.)).flex_none())
            .child(div().text_3xl().font_family(theme::current().display).font_weight(FontWeight::BOLD).child("Broom"))
            .child(div().text_color(rgb(colors().muted)).child(format!("Version {}", env!("CARGO_PKG_VERSION"))))
            .child(div().mt_2().text_sm().child("A disk-space explorer for macOS."))
            .child(
                div()
                    .px_6()
                    .text_center()
                    .text_xs()
                    .text_color(rgb(colors().muted))
                    .child("Ask in plain language — the reader judges the words, code matches the files, you confirm the list."),
            )
            .child(
                div()
                    .id("about-github")
                    .mt_2()
                    .px_2()
                    .py_0p5()
                    .rounded(theme::radius(6.))
                    .text_sm()
                    .text_color(rgb(colors().accent))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(colors().card_hover)).underline())
                    .on_click(cx.listener(|_, _, _, cx| cx.open_url(REPO_URL)))
                    .tooltip(|_, cx| cx.new(|_| Tip("Open the project on GitHub".into())).into())
                    .child("github.com/sriinnu/broom ↗"),
            )
            // The lifetime tally, once there is one: a quiet line, not a score.
            .children(crate::delight::tally().summary().map(|line| div().text_sm().text_color(rgb(colors().muted)).child(line)))
            .child(div().mt_2().text_sm().child("Made by Srinivas Pendela"))
            .child(
                div().text_xs().text_color(rgb(colors().muted)).child("© 2026 Srinivas Pendela · Built with Rust and GPUI"),
            )
    }

    pub(super) fn open_clone_peers(&mut self, current: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.results() else { return };
        let mut rows = r.tree.clone_copies(current).to_vec();
        if rows.len() < 2 {
            return;
        }
        rows.sort_by_cached_key(|&ix| r.tree.path_of(ix));
        let cursor = rows.iter().position(|&ix| ix == current).unwrap_or(0);
        self.clone_picker = Some(ClonePicker {
            scan_id: r.scan_id, current, keeper: current, rows, cursor, marked: HashSet::new(), scroll: ScrollHandle::new(),
        });
        self.context_menu = None;
        window.focus(&self.modal_focus, cx);
        cx.notify();
    }

    fn close_clone_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clone_picker = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn move_clone_cursor(&mut self, step: isize, cx: &mut Context<Self>) {
        if let Some(picker) = &mut self.clone_picker {
            picker.cursor = picker.cursor.saturating_add_signed(step).min(picker.rows.len() - 1);
            picker.scroll.scroll_to_item(picker.cursor);
            cx.notify();
        }
    }

    fn choose_active_clone(&mut self, keep: bool, cx: &mut Context<Self>) {
        if let Some(picker) = &self.clone_picker {
            self.choose_clone(picker.rows[picker.cursor], keep, cx);
        }
    }

    fn choose_clone(&mut self, ix: usize, keep: bool, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.clone_picker else { return };
        let Screen::Results(r) = &self.screen else { return };
        if picker.scan_id != r.scan_id || !r.tree.clone_copies(picker.current).contains(&ix) {
            return;
        }
        if keep {
            picker.keeper = ix;
            picker.marked.remove(&ix);
        } else if ix != picker.keeper && r.stageable(ix) && !r.is_staged(ix) && !picker.marked.remove(&ix) {
            picker.marked.insert(ix);
        }
        cx.notify();
    }

    fn show_clone_file(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.clone_picker else { return };
        let Screen::Results(r) = &mut self.screen else { return };
        if picker.scan_id != r.scan_id || !r.tree.clone_copies(picker.current).contains(&ix) {
            return;
        }
        if let Some(parent) = r.tree.nodes[ix].parent {
            r.navigate(parent);
            r.pick(ix, false, false);
            if let Some(position) = r.order.iter().position(|&node| node == ix) {
                r.list_scroll.scroll_to_item(position, ScrollStrategy::Nearest);
            }
            self.close_clone_picker(window, cx);
        }
    }

    fn stage_clone_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.clone_picker else { return };
        let Screen::Results(r) = &mut self.screen else { return };
        let copies: HashSet<_> = r.tree.clone_copies(picker.current).iter().copied().collect();
        if picker.scan_id != r.scan_id || !copies.contains(&picker.keeper) || r.is_staged(picker.keeper) {
            return;
        }
        let rows: Vec<_> = picker.rows.iter().copied()
            .filter(|ix| *ix != picker.keeper && picker.marked.contains(ix) && copies.contains(ix) && r.stageable(*ix))
            .collect();
        if rows.is_empty() {
            return;
        }
        r.stage_rows(&rows);
        self.close_clone_picker(window, cx);
        self.collector_changed(cx);
    }

    fn render_clone_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let (Some(picker), Screen::Results(r)) = (&self.clone_picker, &self.screen) else {
            return div().into_any_element();
        };
        let copies: HashSet<_> = r.tree.clone_copies(picker.current).iter().copied().collect();
        let current_family = r.scan_id == picker.scan_id && copies.contains(&picker.keeper);
        let keeper_staged = current_family && r.is_staged(picker.keeper);
        let mut marked = 0;
        let mut rows = div().id("clone-peers-list").track_scroll(&picker.scroll).max_h(px(330.)).overflow_y_scroll().flex().flex_col();
        if current_family {
            for (row, &ix) in picker.rows.iter().enumerate().filter(|(_, ix)| copies.contains(ix)) {
                let path = r.tree.path_of(ix);
                let filename = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
                let parent = path.parent().unwrap_or(&r.tree.root_path);
                let relative = parent.strip_prefix(&r.tree.root_path).unwrap_or(parent);
                let folder = if relative.as_os_str().is_empty() { "Scan root".into() } else { relative.to_string_lossy().into_owned() };
                let keeper = ix == picker.keeper;
                let queued = r.is_staged(ix);
                let selectable = !keeper && !queued && r.stageable(ix);
                let selected = selectable && picker.marked.contains(&ix);
                marked += usize::from(selected);
                rows = rows.child(path_tip(
                    div().id(("clone-peer", ix)).flex().items_center().gap_2().px_2().py_2().rounded(theme::radius(6.))
                        .when(row == picker.cursor, |row| row.bg(rgb(colors().card_hover)))
                        .child(div().min_w_0().flex_1()
                            .child(div().truncate().child(filename))
                            .child(div().truncate().text_xs().text_color(rgb(colors().muted))
                                .child(if ix == picker.current { format!("{folder} · Current item") } else { folder })))
                        .child(if keeper {
                            div().text_xs().text_color(rgb(colors().safe)).child("Keeper").into_any_element()
                        } else {
                            button(("clone-keep", ix), "Keep").flex_none().text_xs()
                                .on_click(cx.listener(move |this, _, _, cx| this.choose_clone(ix, true, cx)))
                                .into_any_element()
                        })
                        .when(selectable, |row| row.child(button(("clone-mark", ix), if selected { "Marked" } else { "Mark" })
                            .flex_none().text_xs()
                            .when(selected, |button| button.border_color(rgb(colors().safe)))
                            .on_click(cx.listener(move |this, _, _, cx| this.choose_clone(ix, false, cx)))))
                        .when(queued, |row| row.child(div().text_xs().text_color(rgb(colors().muted)).child("Already staged")))
                        .child(button(("clone-show", ix), "Show").flex_none().text_xs()
                            .on_click(cx.listener(move |this, _, window, cx| this.show_clone_file(ix, window, cx))))
                        .when(!keeper && !queued && !selectable, |row| row.child(div().text_xs().text_color(rgb(colors().muted)).child("Protected"))),
                    path.to_string_lossy().into_owned(),
                ));
            }
        }
        let hint = if !current_family {
            "This clone group changed. Close this card and open it again."
        } else if keeper_staged {
            "The keeper is already in your cleanup plan. Unstage it or choose another keeper."
        } else {
            "Choose a file to keep, then mark individual files to add to your cleanup plan."
        };
        div().w(px(560.)).p_4().rounded(theme::radius(10.))
            .bg(rgb(colors().panel)).border_1().border_color(rgb(colors().border)).shadow_lg()
            .flex().flex_col().gap_2().text_sm()
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Items sharing storage"))
            .child(div().text_color(rgb(colors().muted)).child(hint))
            .child(rows)
            .child(div().flex().justify_between().items_center().gap_2()
                .child(div().text_xs().text_color(rgb(colors().muted)).child("↑↓ Move · K Keep · Space Mark · → Show"))
                .child(div().flex().gap_2()
                    .child(button("close-clone-picker", "Close").on_click(cx.listener(|this, _, window, cx| this.close_clone_picker(window, cx))))
                    .when(marked > 0 && !keeper_staged, |row| row.child(
                        button("stage-clone-choices", format!("Stage {marked} marked"))
                            .on_click(cx.listener(|this, _, window, cx| this.stage_clone_choices(window, cx))))))
            )
            .into_any_element()
    }

    /// Finder-like actions on one node. The APFS volume slices (Kind::Other) aren't
    /// real folders, so they only offer Open/Reveal/Copy.
    pub(super) fn render_context_menu(&mut self, menu: ContextMenu, cx: &mut Context<Self>) -> AnyElement {
        let (name, path, actionable) = match &self.screen {
            Screen::Results(r) => {
                let Some(node) = r.tree.nodes.get(menu.node) else { return div().into_any_element() };
                (node.name.clone(), r.tree.path_of(menu.node), node.kind != Kind::Other)
            }
            _ => return div().into_any_element(),
        };
        let node = menu.node;
        // Protected rows offer nothing that would move them (`Results::shielded`).
        let actionable = actionable && !matches!(&self.screen, Screen::Results(r) if r.shielded(node));

        let item = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded(theme::radius(6.))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(colors().card_hover)))
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
            // Named and ordered as in the Edit menu, so the same command reads
            // the same wherever it's chosen.
            item("menu-copy-abs", "Copy Path").on_click({
                let path = path.clone();
                cx.listener(move |this, _, _, cx| {
                    this.context_menu = None;
                    cx.write_to_clipboard(ClipboardItem::new_string(path.to_string_lossy().into_owned()));
                    cx.notify();
                })
            }),
            item("menu-copy", "Copy Path with ~").on_click({
                let path = path.clone();
                cx.listener(move |this, _, _, cx| {
                    this.context_menu = None;
                    cx.write_to_clipboard(ClipboardItem::new_string(abbreviate_home(&path.to_string_lossy())));
                    cx.notify();
                })
            }),
        ];
        if actionable {
            let staged = matches!(&self.screen, Screen::Results(r) if r.tick(node) == Tick::On);
            items.push(item("menu-collect", if staged { "Remove from Cleanup Plan" } else { "Stage for Cleanup" }).on_click(cx.listener(
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
        if actionable {
            items.insert(1, item("menu-quick-look", "Quick Look  ⌘Y")
                .on_click(cx.listener(move |this, _, window, cx| this.quick_look(node, window, cx))));
        }
        if self.results().is_some_and(|r| !r.tree.clone_copies(node).is_empty()) {
            items.push(item("menu-clone-peers", "Shares Storage With…  ⌥⌘Y").on_click(cx.listener(move |this, _, window, cx| {
                this.open_clone_peers(node, window, cx);
            })));
        }
        // The protect list, from where you see the thing.
        if let Screen::Results(r) = &self.screen {
            if node != Tree::ROOT && r.tree.nodes[node].kind != Kind::Other {
                let own = r.protected.contains(&node);
                if own {
                    items.push(item("menu-protect", "Offer This Again").on_click({
                        let path = path.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.set_protected(path.clone(), false, cx);
                        })
                    }));
                } else if !r.shielded(node) {
                    items.push(item("menu-protect", "Never Offer This").on_click({
                        let path = path.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.set_protected(path.clone(), true, cx);
                        })
                    }));
                } else {
                    // Inside or above something protected: say so, offer nothing.
                    items.push(
                        div()
                            .id("menu-protected")
                            .px_2()
                            .py_1()
                            .text_color(rgb(colors().muted))
                            .child(if r.holds_protected.contains(&node) { "Holds Something Protected" } else { "Inside Something Protected" }),
                    );
                }
            }
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
                        .rounded(theme::radius(8.))
                        .bg(rgb(colors().panel))
                        .border_1()
                        .border_color(rgb(colors().border))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().px_2().py_1().text_xs().text_color(rgb(colors().muted)).truncate().child(name))
                        .children(items),
                ),
        )
        .into_any_element()
    }
}
