//! Column widths: fitting them to the window and the drag handles.

use super::*;

/// Shows paths in the home folder as `~/…`, as the shell does.
/// Column widths (rail, navigator, cleanup) squeezed so they and the chart's
/// minimum fit `room`: the widest gives first, none below its floor.
pub(super) fn fit_columns(mut widths: [f32; 3], room: f32) -> [f32; 3] {
    const PANES: [Pane; 3] = [Pane::Rail, Pane::Navigator, Pane::Cleanup];
    let mut over = widths.iter().sum::<f32>() + CHART_MIN_WIDTH - room;
    while over > 0.5 {
        let slack = |w: &[f32; 3], i: usize| w[i] - PANES[i].range().0;
        let Some(i) = (0..3).filter(|&i| slack(&widths, i) > 0.5).max_by(|&a, &b| widths[a].total_cmp(&widths[b])) else { break };
        // Down to the next-widest that can still give, then they share.
        let next = (0..3).filter(|&j| j != i && slack(&widths, j) > 0.5).map(|j| widths[j]).fold(f32::MIN, f32::max);
        let step = if next > f32::MIN { (widths[i] - next).max(1.0) } else { f32::MAX };
        let cut = over.min(slack(&widths, i)).min(step);
        widths[i] -= cut;
        over -= cut;
    }
    widths
}

impl Broom {
    /// What a column is drawn at: its saved width, unless the window is too
    /// narrow for every saved width and the chart's minimum (see `fit_widths`).
    pub(super) fn pane_width(&self, pane: Pane) -> f32 {
        self.fitted[pane as usize]
    }

    /// Squeeze the columns to fit the window, widest first and never below
    /// their minimum, leaving the saved widths alone so a wider window gets
    /// them back.
    pub(super) fn fit_widths(&mut self, window: &Window) {
        let room: f32 = window.viewport_size().width.into();
        let rail = if self.rail_hidden() { 0.0 } else { self.rail_width };
        let cleanup = if self.cleanup_hidden() { 0.0 } else { self.cleanup_width };
        self.fitted = fit_columns([rail, self.nav_width, cleanup], room);
    }

    pub(super) fn set_pane_width(&mut self, pane: Pane, width: f32) {
        match pane {
            Pane::Rail => self.rail_width = width,
            Pane::Navigator => self.nav_width = width,
            Pane::Cleanup => self.cleanup_width = width,
        }
    }

    /// Written once a drag ends, not on every frame of it.
    pub(super) fn save_pane_widths(&mut self) {
        self.settings.rail_width = self.rail_width;
        self.settings.navigator_width = self.nav_width;
        self.settings.cleanup_width = self.cleanup_width;
        let _ = settings::save(&self.settings);
    }

    /// The grab strip on a column's inner edge: drag to resize, double-click
    /// for the default width. Deferred so its hitbox sits above both
    /// neighbours, and driven by raw mouse events like the panel scrollbar so a
    /// drag that outruns the strip keeps going.
    pub(super) fn pane_handle(&self, pane: Pane, cx: &mut Context<Self>) -> impl IntoElement {
        let drag = self.pane_drag.clone();
        let view = cx.entity().downgrade();
        let width = self.pane_width(pane);
        let others = self.fitted.iter().sum::<f32>() - width;
        // The cleanup column's edge is on its left, so dragging left widens it.
        let sign = if pane == Pane::Cleanup { -1.0 } else { 1.0 };
        let (lo, hi) = pane.range();
        let strip = canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::BlockMouse),
            move |bounds, hitbox, window, _| {
                let held = drag.get().is_some_and(|(p, ..)| p == pane);
                let hovered = hitbox.is_hovered(window);
                if held || hovered {
                    let accent = to_hsla(colors().accent);
                    let line = gpui::hsla(
                        accent.hue.into_positive_degrees() / 360.0,
                        accent.saturation,
                        accent.lightness,
                        if held { 0.9 } else { 0.55 },
                    );
                    let mid: f32 = (bounds.origin.x + bounds.size.width / 2.0).into();
                    window.paint_quad(quad(
                        Bounds::from_corners(
                            point(px(mid - 1.), bounds.origin.y),
                            point(px(mid + 1.), bounds.origin.y + bounds.size.height),
                        ),
                        px(0.),
                        line,
                        px(0.),
                        gpui::hsla(0., 0., 0., 0.),
                        BorderStyle::default(),
                    ));
                }
                if held {
                    window.set_window_cursor_style(CursorStyle::ResizeLeftRight);
                } else {
                    window.set_cursor_style(CursorStyle::ResizeLeftRight, &hitbox);
                }

                let (drag_d, view_d) = (drag.clone(), view.clone());
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !hitbox.is_hovered(window) {
                        return;
                    }
                    cx.stop_propagation();
                    if event.click_count == 2 {
                        drag_d.set(None);
                        let _ = view_d.update(cx, |this, cx| {
                            this.set_pane_width(pane, pane.default_width());
                            this.save_pane_widths();
                            cx.notify();
                        });
                    } else {
                        drag_d.set(Some((pane, event.position.x.into(), width)));
                        // The root's close-on-outside-click never sees this press.
                        let _ = view_d.update(cx, |this, cx| {
                            if this.context_menu.take().is_some() {
                                cx.notify();
                            }
                        });
                        window.refresh();
                    }
                });
                let (drag_m, view_m) = (drag.clone(), view.clone());
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let Some((p, x0, w0)) = drag_m.get() else { return };
                    if p != pane {
                        return;
                    }
                    // Released outside the window: the up never came.
                    if event.pressed_button != Some(MouseButton::Left) {
                        drag_m.set(None);
                        let _ = view_m.update(cx, |this, cx| {
                            this.save_pane_widths();
                            cx.notify();
                        });
                        return;
                    }
                    let room: f32 = window.viewport_size().width.into();
                    let max = hi.min(room - others - CHART_MIN_WIDTH).max(lo);
                    let x: f32 = event.position.x.into();
                    let to = (w0 + sign * (x - x0)).clamp(lo, max);
                    let _ = view_m.update(cx, |this, cx| {
                        this.set_pane_width(pane, to);
                        cx.notify();
                    });
                });
                let (drag_u, view_u) = (drag.clone(), view.clone());
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && drag_u.get().is_some_and(|(p, ..)| p == pane) {
                        drag_u.set(None);
                        let _ = view_u.update(cx, |this, cx| {
                            this.save_pane_widths();
                            cx.notify();
                        });
                    }
                });
            },
        )
        .size_full();
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(7.))
            .when(pane == Pane::Cleanup, |d| d.left(px(-4.)))
            .when(pane != Pane::Cleanup, |d| d.right(px(-4.)))
            .child(deferred(strip))
    }
}
