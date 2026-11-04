//! Opening and closing the cards, the About card and the right-click menu.

use super::*;

impl Broom {
    pub(crate) fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        // Never under a native sheet (trash confirm, folder picker): the parent
        // window is disabled for its duration and our card would be dead on arrival.
        if self.busy {
            return;
        }
        self.open_panel(PanelTab::Keys, window, cx);
    }

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

    /// Open the panel on a tab — or, if it's already up, just switch to it.
    /// The key rows keep whatever the keychain last told them.
    /// Clears the salvaged-settings notice after its one display.
    pub(super) fn open_panel(&mut self, tab: PanelTab, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn settings_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(focused) = window.focused(cx) else { return };
        let check = match &self.modal {
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

    /// The dimmed backdrop with the modal card on top. Clicking the backdrop closes;
    /// the card carries a key context so Escape can be bound to closing it.
    pub(super) fn render_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
    pub(super) fn render_panel_scrollbar(&self, p: &PanelState) -> impl IntoElement {
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
            .child(div().mt_2().text_sm().child("Made by Srinivas Pendela"))
            .child(
                div().text_xs().text_color(rgb(colors().muted)).child("© 2026 Srinivas Pendela · Built with Rust and GPUI"),
            )
    }

    /// Finder-like actions on one node. The APFS volume slices (Kind::Other) aren't
    /// real folders, so they only offer Open/Reveal/Copy.
    pub(super) fn render_context_menu(&mut self, menu: ContextMenu, cx: &mut Context<Self>) -> AnyElement {
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
