//! The sunburst, its legend and the help under it.

use super::*;

/// The Help tab: a short FAQ, one breath per answer.
pub(super) fn render_help() -> impl IntoElement {
    const FAQ: &[(&str, &str)] = &[
        ("What does Broom scan?", "Any disk or folder you point it at. Sizes come straight from APFS, and scanning only reads — nothing changes until you confirm a move to the Trash."),
        ("Some folders say they couldn't be read", "macOS keeps Mail, Messages, Safari and other apps' data private. Grant Broom Full Disk Access in System Settings › Privacy & Security, then rescan."),
        ("How do I delete things?", "Tick the box on a folder or a finding to stage it in the cleanup plan on the right (or press space on the selected row). “Review and Move to Trash…” shows the exact list and what it really frees before anything moves."),
        ("What can I type in the Ask field?", "Plain language: “node_modules”, “old dmg files in Downloads”. Exact matches appear as you type, entirely offline. Press Enter and the reader picks the group the words mean — a fallback steps in when it can't — and the cleanup plan gets pre-ticked. You confirm the final list."),
        ("Where are my API keys kept?", "In your login keychain. Broom never shows a key or logs it — only its last characters, as a hint. Pasting a key sends it straight to the keychain, never through a text field. The fallback reader needs no key at all."),
        ("Is deleting safe?", "Everything goes to the Trash. The review step says how much will really be freed (space shared with files outside the selection isn't), and you can put things back from Finder's Trash."),
        ("What do the colours mean?", "Colour: Folder gives each child of the folder in view its own hue, lighter ring by ring, so zooming in recolours the children apart; Colour: Kind colours by what things are — apps, caches, photos. The rail and the icon share the same six-hue ladder."),
        ("What about iCloud, Google Drive, OneDrive?", "They're synced folders on this Mac, listed under Cloud storage. Broom measures what's downloaded here; “cloud-only” items use no space and are counted as such."),
        ("And the rings?", "Each ring is one level of folders, biggest at twelve o'clock going clockwise. The centre takes you up one level, a slice zooms in, and the list mirrors the chart when it's sorted by size."),
        ("What grew? Duplicates?", "Each scan is remembered (folder sizes only, kept a week), so the next scan of the same place shows what grew: +1.2 GB in amber, shrinkage in green. Duplicate files appear as a finding once the scan is done: identical copies, compared byte for byte, of your own files only. APFS clones and hard links are left out, since deleting them frees nothing."),
        ("When does the space come back?", "When the Trash is emptied. File › Empty Trash… (⇧⌘⌫), or the button on the banner after a cleanup, empties it from Broom: it says what's there first, and that this can't be undone, then shows how much the disk actually got back. Locked items stay in the Trash."),
        ("Merge as Clones?", "The Duplicate files finding can merge instead of delete: each extra copy becomes an APFS clone of the one that stays. Both files stay where they were, with their own names, dates and tags, but the bytes are stored once, so the space comes back with nothing deleted. Edit one later and only the changes take new space."),
        ("Getting around", "Sort the list by Size, Name or Modified; click the same one again to flip it. Drag a column's edge to resize it, double-click to reset. ⇧-click takes a range of rows, ⌘- or ⌃-click adds or removes one and ⌘A takes them all; space and ⌘C then act on all of them, and esc lets go. Settings can add date modified and date created to the list. ⌘[ and ⌘] step back and forward through the folders you've visited, ⌥⌘S hides the sidebar. ⌘C copies the selected row's path (or the open folder's), and hovering a path that's cut short shows all of it. A folder's date is when something directly inside it was added or removed, as in Finder."),
    ];
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().text_lg().font_family(theme::current().display).font_weight(FontWeight::SEMIBOLD).child("Help"))
        .child(div().text_xs().text_color(rgb(colors().muted)).child("The short version of everything Broom does."))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .children(FAQ.iter().map(|(q, a)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(*q))
                        .child(div().text_xs().text_color(rgb(colors().muted)).child(*a))
                })),
        )
}

/// "Colour: Folder | Kind" in the toolbar.
pub(super) fn color_toggle(current: ColorBy, cx: &mut Context<Broom>) -> impl IntoElement {
    let option = |id: &'static str, label: &'static str, value: ColorBy, cx: &mut Context<Broom>| {
        let selected = current == value;
        div()
            .id(id)
            .px_2()
            .py_0p5()
            .rounded(theme::radius(6.))
            .cursor_pointer()
            .when(selected, |d| d.bg(rgb(colors().card_hover)).text_color(rgb(colors().text)))
            .when(!selected, |d| d.text_color(rgb(colors().muted)).hover(|s| s.text_color(rgb(colors().text))))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| this.set_color_by(value, cx)))
            .child(label)
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_0p5()
        .p_0p5()
        // The toolbar's one height, so the bar's controls line up.
        .h(px(28.))
        .rounded(theme::radius(8.))
        .border_1()
        .border_color(rgb(colors().border))
        .text_xs()
        .child(div().pl_1p5().pr_0p5().text_color(rgb(colors().muted)).child("Colour"))
        .child(option("color-folder", "Folder", ColorBy::Folder, cx))
        .child(option("color-kind", "Kind", ColorBy::Kind, cx))
}

/// Which colour means which kind, for the kinds in view. Hover a kind to highlight it.
pub(super) fn render_legend(r: &Results, cx: &mut Context<Broom>) -> impl IntoElement {
    let mut legend = div()
        .absolute()
        .left_4()
        .bottom_4()
        .occlude()
        .flex()
        .flex_col()
        .gap_0p5()
        .p_2()
        .rounded(theme::radius(8.))
        .bg(rgb(colors().panel))
        .border_1()
        .border_color(rgb(colors().border))
        .text_xs();
    for category in CATEGORIES.into_iter().filter(|&c| r.segments.iter().any(|s| r.category(s.target) == Some(c))) {
        let lit = r.legend_hover.is_none_or(|h| h == category);
        legend = legend.child(
            div()
                .id(("legend", category as usize))
                .flex()
                .items_center()
                .gap_2()
                .px_1()
                .text_color(rgb(if lit { colors().text } else { colors().muted }))
                .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                    if let Some(r) = this.results() {
                        if *hovering {
                            r.legend_hover = Some(category);
                        } else if r.legend_hover == Some(category) {
                            r.legend_hover = None;
                        }
                        cx.notify();
                    }
                }))
                .child(div().size(px(10.)).flex_none().rounded_full().bg(classify::color(category)))
                .child(category.label()),
        );
    }
    legend
}

impl Broom {
    pub(super) fn render_chart(&self, r: &Results, cx: &mut Context<Self>) -> impl IntoElement {
        let tree = &r.tree;
        let hovered = r.hovered();
        let fills: Vec<Hsla> = r
            .segments
            .iter()
            .map(|s| {
                let base = r.color(s);
                if let Some(category) = r.legend_hover {
                    return if r.category(s.target) == Some(category) { sunburst::lift(base) } else { sunburst::fade(base) };
                }
                let Some(target) = hovered else { return base };
                let lit = match (s.target, target) {
                    (a, b) if a == b => true,
                    (Target::Node(n), Target::Node(h)) => tree.is_ancestor_or_self(h, n),
                    (Target::Small { parent }, Target::Node(h)) => tree.is_ancestor_or_self(h, parent),
                    _ => false,
                };
                match (lit, r.color_by) {
                    (true, ColorBy::Folder) => sunburst::highlight(base),
                    (true, ColorBy::Kind) => sunburst::lift(base),
                    (false, _) => sunburst::dim(base),
                }
            })
            .collect();

        let can_go_up = tree.nodes[r.focus].parent.is_some();
        let pointer = match r.chart_hover {
            Some(Hit::Center) => can_go_up,
            Some(Hit::Segment(i)) => r.segments.get(i).is_some_and(|s| s.kind == Kind::Dir && matches!(s.target, Target::Node(_))),
            None => false,
        };
        let center_hovered = r.chart_hover == Some(Hit::Center) && can_go_up;

        // Label in the middle of the chart describes whatever is under the pointer.
        let (title, subtitle): (SharedString, String) = match hovered {
            Some(Target::Node(ix)) => {
                let node = &tree.nodes[ix];
                let detail = match node.kind {
                    Kind::Dir => format!("{} files", format_count(node.items)),
                    Kind::File => "file".into(),
                    Kind::Other if node.name.as_ref() == disk::NOT_READABLE => "needs Full Disk Access".into(),
                    Kind::Other => "exact, from APFS".into(),
                };
                let kind = match r.category(Target::Node(ix)) {
                    Some(category) if r.color_by == ColorBy::Kind && node.kind != Kind::Other => format!("\n{}", category.label()),
                    _ => String::new(),
                };
                (node.name.clone(), format!("{}\n{detail}{kind}", format_size(node.size)))
            }
            Some(Target::Small { parent }) => {
                let seg_size: u64 = tree.nodes[parent]
                    .children
                    .iter()
                    .map(|&c| tree.nodes[c].size)
                    .filter(|&s| (s as f32) < tree.nodes[parent].size as f32 * 0.004)
                    .sum();
                ("Smaller objects".into(), format_size(seg_size))
            }
            None if center_hovered => ("↑ Back".into(), format!("to “{}”", tree.nodes[tree.nodes[r.focus].parent.unwrap()].name)),
            None => {
                let node = &tree.nodes[r.focus];
                (node.name.clone(), format_size(node.size))
            }
        };
        let label_width = r
            .chart_bounds
            .get()
            .map(|b| Geometry::new(b).inner_radius * 1.7)
            .unwrap_or(140.);

        let segments = r.segments.clone();
        let bounds_cell = r.chart_bounds.clone();
        let anim_start = r.anim_start;
        // Reduce Motion: the chart appears whole, with no sweep and no dust.
        let still = delight::reduce_motion();
        let dust = r.dust.clone().filter(|_| !still);
        // The freed number counts up for a moment; keep frames coming until it lands.
        let counting = !still && r.freed.is_some() && r.banner.is_some_and(|at| clock::since(at) < delight::COUNT_UP);
        let entity = cx.entity().downgrade();
        // Staged slices (themselves or through a parent) get the hatch.
        let staged: Vec<bool> = segments
            .iter()
            .map(|s| match s.target {
                Target::Node(ix) => r.is_staged(ix),
                Target::Small { parent } => r.is_staged(parent),
            })
            .collect();
        // Names for the two inner rings; whether one fits is decided at paint time.
        let labels: Vec<(usize, SharedString)> = segments
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s.target {
                Target::Node(ix) if s.depth <= 2 => Some((i, tree.nodes[ix].name.clone())),
                _ => None,
            })
            .collect();

        div()
            .flex_1()
            .h_full()
            .relative()
            .child(
                canvas(
                    move |bounds, window, _| {
                        bounds_cell.set(Some(bounds));
                        window.insert_hitbox(bounds, HitboxBehavior::Normal)
                    },
                    move |bounds, hitbox, window, cx| {
                        let geometry = Geometry::new(bounds);
                        let t = if still { 1.0 } else { (clock::since(anim_start).as_secs_f32() / ZOOM_DURATION.as_secs_f32()).min(1.0) };
                        let eased = 1.0 - (1.0 - t).powi(3);
                        if t < 1.0 {
                            window.request_animation_frame();
                        }

                        geometry.paint_disc(window, geometry.outer_radius() + 6.0, to_hsla(colors().chart_surface));
                        for ((segment, color), &hatched) in segments.iter().zip(&fills).zip(&staged) {
                            if segment.depth as f32 > 1.0 + eased * sunburst::MAX_DEPTH as f32 {
                                continue;
                            }
                            geometry.paint_sector(window, segment.depth, segment.start * eased, segment.end * eased, *color);
                            if hatched {
                                geometry.paint_hatched_sector(window, segment.depth, segment.start * eased, segment.end * eased, gpui::hsla(0., 0., 0.05, 0.55));
                            }
                        }
                        if t >= 1.0 {
                            let mut font = gpui::font(theme::current().font);
                            font.weight = FontWeight::MEDIUM;
                            for (i, name) in &labels {
                                let segment = &segments[*i];
                                let Some((at, width)) = geometry.label_slot(segment.depth, segment.start, segment.end) else { continue };
                                let Some(line) = fit_label(window, name, width, &font, px(11.)) else { continue };
                                let line_height = px(14.);
                                let origin = point(at.x - line.width() / 2.0, at.y - line_height / 2.0);
                                let _ = line.paint(origin, line_height, TextAlign::Left, None, window, cx);
                            }
                        }
                        let center = if center_hovered { to_hsla(colors().chart_center_hover) } else { to_hsla(colors().chart_center) };
                        geometry.paint_disc(window, geometry.inner_radius - 1.0, center);
                        if let Some(puff) = &dust {
                            paint_dust(window, &geometry, puff);
                        }
                        if counting {
                            window.request_animation_frame();
                        }

                        if pointer {
                            window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
                        }

                        let segments_for_move = segments.clone();
                        let entity_for_move = entity.clone();
                        let hitbox_for_move = hitbox.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble || !hitbox_for_move.is_hovered(window) {
                                return;
                            }
                            let hit = geometry.hit_test(event.position, &segments_for_move);
                            entity_for_move.update(cx, |this, cx| this.set_chart_hover(hit, cx)).ok();
                        });
                        let segments_for_click = segments.clone();
                        let entity_for_click = entity.clone();
                        let hitbox_for_click = hitbox.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx: &mut App| {
                            if phase != DispatchPhase::Bubble
                                || !matches!(event.button, MouseButton::Left | MouseButton::Right)
                                || !hitbox_for_click.is_hovered(window)
                            {
                                return;
                            }
                            let Some(hit) = geometry.hit_test(event.position, &segments_for_click) else {
                                return;
                            };
                            if event.button == MouseButton::Right {
                                // A menu for the segment under the pointer; the APFS
                                // volume slices have nothing to act on.
                                if let Hit::Segment(i) = hit {
                                    if let Some(Target::Node(ix)) = segments_for_click.get(i).map(|s| s.target) {
                                        if matches!(segments_for_click[i].kind, Kind::Dir | Kind::File) {
                                            let at = event.position;
                                            entity_for_click.update(cx, |this, cx| this.open_context_menu(ix, at, cx)).ok();
                                        }
                                    }
                                }
                                return;
                            }
                            entity_for_click.update(cx, |this, cx| this.chart_click(hit, cx)).ok();
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
                            .child(
                                div()
                                    .max_w_full()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(title),
                            )
                            .children(subtitle.lines().map(|line| {
                                div().text_color(rgb(colors().muted)).text_xs().child(line.to_string())
                            })),
                    ),
            )
            .when(r.color_by == ColorBy::Kind, |d| d.child(render_legend(r, cx)))
            .when(r.banner.is_some(), |d| {
                // The "done" moment: what the scan found, in one line — or, after
                // a move to the Trash, what moved.
                let total = tree.nodes[Tree::ROOT].size;
                let pending = r.findings.iter().any(|f| f.pending);
                let safe: u64 = r.findings.iter().filter(|f| f.safety == Safety::Safe).map(|f| f.size).sum();
                let savings = if pending {
                    " · working out savings…".to_string()
                } else if safe > 0 {
                    format!(" · {} safe to delete", format_size(safe))
                } else {
                    String::new()
                };
                if let Some(freed) = &r.freed {
                    return d.child(render_freed_banner(r, freed, still, cx).into_any_element());
                }
                let (headline, detail) = match &r.banner_text {
                    // A line break splits the bold part from the quiet detail.
                    Some(text) => match text.split_once('\n') {
                        Some((head, detail)) => (head.to_string(), detail.to_string()),
                        None => (text.clone(), String::new()),
                    },
                    None => (format!("Scan complete in {:.1} s", r.elapsed.as_secs_f32()), format!("{} accounted for{savings}", format_size(total))),
                };
                d.child(
                    div().absolute().top_4().left_0().right_0().px_4().flex().justify_center().child(
                        div()
                            .id("done-banner")
                            .occlude()
                            .max_w_full()
                            .min_w_0()
                            .px_4()
                            .py_2()
                            .rounded_full()
                            .bg(rgb(colors().card))
                            .border_1()
                            .border_color(rgb(colors().safe))
                            .shadow_lg()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .child(div().flex_none().text_color(rgb(colors().safe)).font_weight(FontWeight::BOLD).child("✓"))
                            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(headline))
                            .when(!detail.is_empty(), |d| d.child(div().min_w_0().truncate().text_xs().text_color(rgb(colors().muted)).child(detail)))
                            .when(!r.last_trashed.is_empty(), |d| {
                                d.child(
                                    button("put-back", "Put Back")
                                        .flex_none()
                                        .text_xs()
                                        .py_0p5()
                                        .px_2()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.put_back(cx);
                                        })),
                                )
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(r) = this.results() {
                                    r.banner = None;
                                    cx.notify();
                                }
                            })),
                    ),
                )
            })
    }
}

/// The dust lifting off where trashed slices were: specks drift outward and up,
/// shrinking and fading, gone within `delight::DUST`.
fn paint_dust(window: &mut Window, geometry: &Geometry, puff: &delight::Puff) {
    let k = clock::since(puff.start).as_secs_f32() / delight::DUST.as_secs_f32();
    if k >= 1.0 {
        return;
    }
    // The investigator's dust, so the two read as the same broom.
    let mut color = to_hsla(delight::DUST_COLOR);
    for speck in &puff.specks {
        let ring = geometry.rings[speck.depth.clamp(1, sunburst::MAX_DEPTH) - 1];
        let (x, y, size, alpha) = delight::speck_at(speck, ring, k);
        color.alpha = alpha;
        let at = point(geometry.center.x + px(x), geometry.center.y + px(y));
        Geometry::ring(at, 0.0, size).paint_disc(window, size, color);
    }
    window.request_animation_frame();
}

/// The banner after a move to the Trash: the freed size counting up, what
/// it's worth in everyday things, the one useful reminder, and a word when the
/// lifetime tally passes a milestone.
fn render_freed_banner(r: &Results, freed: &Freed, still: bool, cx: &mut Context<Broom>) -> impl IntoElement {
    let elapsed = r.banner.map(clock::since).unwrap_or(delight::COUNT_UP);
    let shown = if still { freed.bytes } else { delight::counted(freed.bytes, elapsed) };
    // Only the digits are monospaced; the unit reads with the sentence.
    let counter = delight::format_like(shown, freed.bytes);
    let (number, unit) = counter.rsplit_once(' ').map(|(n, u)| (n.to_string(), u.to_string())).unwrap_or((counter.clone(), String::new()));
    let what = items_phrase(freed.items);
    let like = delight::comparison(freed.bytes).map(|like| format!(", {like}")).unwrap_or_default();
    let (lead, tail, detail) = match freed.emptied {
        Some(note) => ("Emptied", format!("{unit} from the Trash"), format!("{what}{like}. {}", emptied_detail(freed.bytes, note))),
        None => ("Swept", format!("{unit} into the Trash"), format!("{what}{like}. Empty the Trash to get the space back.")),
    };
    div().absolute().top_4().left_0().right_0().px_4().flex().justify_center().child(
        div()
            .id("done-banner")
            .occlude()
            .max_w_full()
            .min_w_0()
            .px_4()
            .py_2()
            .rounded(theme::radius(14.))
            .bg(rgb(colors().card))
            .border_1()
            .border_color(rgb(colors().safe))
            .shadow_lg()
            .flex()
            .items_start()
            .gap_2()
            .cursor_pointer()
            .child(div().flex_none().text_color(rgb(colors().safe)).font_weight(FontWeight::BOLD).child("✓"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(lead)
                            // Monospaced and padded to its final width, so the
                            // banner doesn't jiggle while the number climbs.
                            .child(div().flex_none().font_family(theme::current().mono).child(number))
                            .child(tail),
                    )
                    .child(div().text_xs().text_color(rgb(colors().muted)).child(detail))
                    .when_some(freed.milestone, |d, m| {
                        d.child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(rgb(colors().safe)).child(delight::milestone_line(m)))
                    }),
            )
            .when(!r.last_trashed.is_empty(), |d| {
                d.child(
                    button("put-back", "Put Back")
                        .flex_none()
                        .text_xs()
                        .py_0p5()
                        .px_2()
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.put_back(cx);
                        })),
                )
            })
            .when(freed.emptied.is_none(), |d| {
                d.child(
                    button("banner-empty-trash", "Empty Trash…")
                        .flex_none()
                        .text_xs()
                        .py_0p5()
                        .px_2()
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_empty_trash(&EmptyTrash, window, cx);
                        })),
                )
            })
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(r) = this.results() {
                    r.banner = None;
                    r.freed = None;
                    cx.notify();
                }
            })),
    )
}
