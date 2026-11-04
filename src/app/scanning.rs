//! The screen shown while a scan runs.

use super::*;

impl Broom {
    pub(super) fn render_scanning(&self, scanning: &Scanning, cx: &mut Context<Self>) -> impl IntoElement {
        let progress = &scanning.progress;
        let current = progress.current.lock().map(|c| abbreviate_home(&c)).unwrap_or_default();
        let scanned = progress.bytes.load(Ordering::Relaxed);
        let files = progress.files.load(Ordering::Relaxed);
        let elapsed = clock::since(scanning.started).as_secs_f32();
        let layout = progress.layout.get().cloned();
        let disk_name = layout.as_ref().map(|l| l.name.clone()).unwrap_or_else(|| scan::display_name(&scanning.root));
        let view = scanning.live.clone();
        if view.is_some() && std::env::var_os("BROOM_TIMING").is_some() {
            static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !REPORTED.swap(true, Ordering::Relaxed) {
                if let Some(launched) = LAUNCHED.get() {
                    eprintln!("timing: first live chart {:.0} ms after launch", launched.elapsed().as_secs_f64() * 1000.0);
                }
            }
        }
        let hovered = view.as_ref().zip(scanning.hover.as_ref()).map(|(v, path)| resolve_path(&v.tree, path));

        // Honest sizes: final folders show their size, folders still being counted show a lower bound.
        let size_label = |view: &LiveView, ix: usize| -> String {
            let node = &view.tree.nodes[ix];
            if node.kind == Kind::Other || view.is_final(ix) {
                format_size(node.size)
            } else if node.size == 0 {
                "counting…".to_string()
            } else {
                format!("≥ {}", format_size(node.size))
            }
        };

        // Sidebar: the focused folder's contents.
        let mut rows = div().id("live-rows").flex_1().min_h_0().px_2().overflow_y_scroll().flex().flex_col();
        let mut crumbs = None;
        if let Some(view) = &view {
            let focus = &view.tree.nodes[view.focus];
            if view.focus != Tree::ROOT {
                let up = path_to(&view.tree, focus.parent.unwrap_or(Tree::ROOT));
                crumbs = Some(
                    div()
                        .id("live-up")
                        .px_4()
                        .pb_2()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_color(rgb(colors().muted))
                        .cursor_pointer()
                        .hover(|s| s.text_color(rgb(colors().text)))
                        .child(format!("‹ {}", path_to(&view.tree, view.focus).iter().map(|n| n.to_string()).collect::<Vec<_>>().join(" › ")))
                        .on_click(cx.listener(move |this, _, _, cx| this.live_focus(up.clone(), cx))),
                );
            }
            for &ix in &focus.children {
                let node = &view.tree.nodes[ix];
                let swatch = view.swatches.get(&ix).copied().unwrap_or(gpui::hsla(0., 0., 0.38, 1.));
                let is_final = view.is_final(ix);
                let is_hovered = hovered == Some(ix);
                let openable = node.kind == Kind::Dir && !node.children.is_empty();
                let path = path_to(&view.tree, ix);
                let hover_path = path.clone();
                rows = rows.child(
                    div()
                        .id(("live-row", ix))
                        .w_full()
                        .h(px(theme::current().row_height))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .rounded(theme::radius(6.))
                        .when(is_hovered, |d| d.bg(gpui::hsla(swatch.hue.into_positive_degrees() / 360.0, swatch.saturation, 0.45, 0.16)))
                        .when(openable, |d| d.cursor_pointer())
                        .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                            this.live_hover(hovering.then(|| hover_path.clone()), cx);
                        }))
                        .when(openable, |d| d.on_click(cx.listener(move |this, _, _, cx| this.live_focus(path.clone(), cx))))
                        .child(kind_mark(node.kind, if is_final || node.kind == Kind::Other { swatch } else { sunburst::dim(swatch) }))
                        .child(div().flex_1().min_w_0().truncate().child(node.name.clone()))
                        .child(
                            div()
                                .w(px(80.))
                                .flex_none()
                                .text_right()
                                .text_xs()
                                .font_family(theme::current().mono)
                                .text_color(if is_final { rgb(colors().text) } else { rgb(colors().muted) })
                                .child(size_label(view, ix)),
                        )
                        .child(div().w(px(14.)).flex_none().text_color(rgb(colors().muted)).child(if openable { "›" } else { "" })),
                );
            }
        }

        let sidebar = self
            .navigator_frame(cx)
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_lg().font_family(theme::current().display).font_weight(FontWeight::SEMIBOLD).truncate().child(format!("Scanning “{disk_name}”…")))
                    .child(div().text_color(rgb(colors().muted)).child(format!("{} · {} files", format_size(scanned), format_count(files))))
                    .child(match progress.expected_items.get() {
                        Some(&expected) => {
                            let items = files + progress.dirs.load(Ordering::Relaxed);
                            let done = eta::fraction(items, expected);
                            let left = scanning.eta.remaining(elapsed as f64, items, expected).map(eta::describe);
                            div()
                                .mt_1()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .h(px(6.))
                                        .w_full()
                                        .rounded_full()
                                        .bg(rgb(colors().border))
                                        .child(div().h_full().w(relative(done)).rounded_full().bg(rgb(colors().accent))),
                                )
                                .child(div().text_xs().text_color(rgb(colors().muted)).child(match left {
                                    Some(left) => format!("{:.0}% · {left}", done * 100.0),
                                    None => format!("{:.0}%", done * 100.0),
                                }))
                                .into_any_element()
                        }
                        // A folder: no known total, so no percentage or countdown.
                        None => div().text_xs().text_color(rgb(colors().muted)).child("Scanning…").into_any_element(),
                    })
                    .child({
                        let done = progress.expected_items.get().map(|&expected| eta::fraction(files + progress.dirs.load(Ordering::Relaxed), expected));
                        div().mt_2().child(comic::investigator(comic::caption(&current, files), done, 56.0))
                    }),
            )
            .children(scanning.focus.is_empty().then(|| self.render_access_card(None, cx)).flatten())
            .children(crumbs)
            .child(rows)
            .child(div().px_4().py_2().text_xs().text_color(rgb(colors().muted)).child("Click a folder to look inside"));

        // Chart: share of the volume found so far for plain volume scans (the startup disk's
        // remainder is its own slice instead).
        let fraction = match scanning.expected {
            Some(expected) if expected > 0 => (scanned as f64 / expected as f64).min(1.0) as f32,
            _ => 1.0,
        };
        let started = scanning.started;
        let motion = scanning.motion.clone();
        let bounds_cell = scanning.chart_bounds.clone();
        let entity = cx.entity().downgrade();

        // Centre label: whatever is under the pointer, else the folder in focus.
        let (title, lines): (String, Vec<String>) = match (&view, hovered) {
            (Some(v), Some(ix)) if ix != v.focus => {
                let node = &v.tree.nodes[ix];
                let state = match node.kind {
                    Kind::Other if node.name.as_ref() == disk::NOT_SCANNED => "still to scan",
                    Kind::Other => "exact, from APFS",
                    _ if v.is_final(ix) => "final",
                    _ => "still counting",
                };
                (node.name.to_string(), vec![size_label(v, ix), state.to_string()])
            }
            (Some(v), _) if v.focus != Tree::ROOT => {
                let node = &v.tree.nodes[v.focus];
                let state = if v.is_final(v.focus) { "final" } else { "still counting" };
                (node.name.to_string(), vec![size_label(v, v.focus), state.to_string(), "↑ click to go back".into()])
            }
            _ => match &layout {
                Some(layout) => (
                    layout.name.clone(),
                    vec![
                        format!("{} used", format_size(layout.container_used)),
                        format!("{} of {} scanned", format_size(scanned), format_size(layout.data_used)),
                    ],
                ),
                None => (
                    format_size(view.as_ref().map(|v| v.tree.nodes[Tree::ROOT].size).unwrap_or(0).max(scanned)),
                    vec![match scanning.expected {
                        Some(expected) => format!("of {} used", format_size(expected)),
                        None => "scanning…".to_string(),
                    }],
                ),
            },
        };
        let label_width = scanning.chart_bounds.get().map(|b| Geometry::new(b).inner_radius * 1.7).unwrap_or(140.);
        let paint_view = view.clone();
        let hover_ix = hovered;

        let chart = div()
            .flex_1()
            .h_full()
            .relative()
            .child(
                canvas(
                    move |bounds, window, _| {
                        bounds_cell.set(Some(bounds));
                        window.insert_hitbox(bounds, HitboxBehavior::Normal)
                    },
                    move |bounds, hitbox, window, _| {
                        let geometry = Geometry::new(bounds);
                        geometry.paint_disc(window, geometry.outer_radius() + 6.0, to_hsla(colors().chart_surface));
                        let Some(view) = paint_view else { return };
                        let pulse = 0.22 + 0.04 * (clock::since(started).as_secs_f32() * 3.0).sin();
                        let mut motion = motion.borrow_mut();
                        motion.retarget(view.id as usize, &view.keys, &view.segments, fraction);
                        motion.step(clock::now());
                        let fraction = motion.fraction;
                        for moving in motion.segments_mut() {
                            let Some(i) = moving.current else {
                                // Leaving the layout: keep its last colour while it shrinks away.
                                let (r0, r1) = geometry.ring_at(moving.depth);
                                geometry.paint_band(window, r0, r1, moving.start * fraction, moving.end * fraction, moving.color);
                                continue;
                            };
                            // Hue follows the animated position, so colours shift smoothly too.
                            let segment = &Segment { start: moving.start, end: moving.end, ..view.segments[i] };
                            let color = match segment.target {
                                Target::Node(ix) if Some(ix) == view.pending => gpui::hsla(0.0, 0.0, pulse, 1.0),
                                Target::Node(ix) => {
                                    let base = sunburst::base_color(segment);
                                    // Folders still being counted are drawn muted; they "settle" into
                                    // full colour when their total is final.
                                    let base = if view.tree.nodes[ix].kind == Kind::Dir && !view.is_final(ix) { sunburst::dim(base) } else { base };
                                    // A brief glow as the folder's total becomes final.
                                    let base = match view.settled_at.get(&ix) {
                                        Some(at) => {
                                            let k = 1.0 - (clock::since(*at).as_secs_f32() / SETTLE_GLOW.as_secs_f32()).min(1.0);
                                            gpui::hsla(base.hue.into_positive_degrees() / 360.0, base.saturation, (base.lightness + 0.2 * k).min(0.92), 1.0)
                                        }
                                        None => base,
                                    };
                                    match hover_ix {
                                        Some(h) if is_within(&view.tree, ix, h) => sunburst::highlight(base),
                                        _ => base,
                                    }
                                }
                                Target::Small { .. } => sunburst::base_color(segment),
                            };
                            moving.color = color;
                            let (r0, r1) = geometry.ring_at(moving.depth);
                            geometry.paint_band(window, r0, r1, moving.start * fraction, moving.end * fraction, color);
                        }
                        drop(motion);
                        if fraction < 0.9999 {
                            geometry.paint_sector(window, 1, fraction, 1.0, gpui::hsla(0.0, 0.0, pulse, 1.0));
                        }
                        window.request_animation_frame();
                        geometry.paint_disc(window, geometry.inner_radius - 1.0, to_hsla(colors().chart_center));

                        // Hit testing against exactly what was painted, so a newer snapshot can't
                        // shift what a click means.
                        let target_at = {
                            let view = view.clone();
                            move |position| -> Option<(bool, Vec<SharedString>)> {
                                match geometry.hit_test(position, &view.segments)? {
                                    Hit::Center => Some((true, path_to(&view.tree, view.tree.nodes[view.focus].parent.unwrap_or(Tree::ROOT)))),
                                    Hit::Segment(i) => match view.segments[i].target {
                                        Target::Node(ix) => Some((false, path_to(&view.tree, ix))),
                                        Target::Small { .. } => None,
                                    },
                                }
                            }
                        };
                        let openable = {
                            let view = view.clone();
                            move |path: &[SharedString]| {
                                let ix = resolve_path(&view.tree, path);
                                view.tree.nodes[ix].kind == Kind::Dir && !view.tree.nodes[ix].children.is_empty()
                            }
                        };
                        if let Some(position) = window.mouse_position().into() {
                            if bounds.contains(&position) {
                                if let Some((center, path)) = target_at(position) {
                                    if center || openable(&path) {
                                        window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
                                    }
                                }
                            }
                        }
                        let (move_target, move_entity) = (target_at.clone(), entity.clone());
                        let hitbox_for_move = hitbox.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble || !hitbox_for_move.is_hovered(window) {
                                return;
                            }
                            let path = move_target(event.position).filter(|(center, _)| !center).map(|(_, p)| p);
                            move_entity.update(cx, |this, cx| this.live_hover(path, cx)).ok();
                        });
                        let click_entity = entity.clone();
                        let hitbox_for_click = hitbox.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx: &mut App| {
                            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !hitbox_for_click.is_hovered(window) {
                                return;
                            }
                            if let Some((center, path)) = target_at(event.position) {
                                if center || openable(&path) {
                                    click_entity.update(cx, |this, cx| this.live_focus(path, cx)).ok();
                                }
                            }
                        });
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .max_w(px(label_width))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_0p5()
                            .child(div().max_w_full().text_base().font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                            .children(lines.into_iter().map(|line| div().text_xs().text_color(rgb(colors().muted)).child(line))),
                    ),
            );

        div().size_full().flex().child(sidebar).child(chart)
    }
}
