//! The navigator column: the list of what the open folder holds.

use super::*;

/// One date in a navigator row, as an age.
pub(super) fn date_cell(when: Option<SystemTime>) -> impl IntoElement {
    div()
        .w(px(DATE_COLUMN))
        .flex_none()
        .text_right()
        .text_color(rgb(colors().muted))
        .text_xs()
        .font_family(theme::current().mono)
        .child(when.map(format_age).unwrap_or_default())
}

/// Names the date columns: two bare ages side by side couldn't be told apart.
/// Lines up with the rows, so it mirrors their padding and widths.
pub(super) fn date_header(columns: RowColumns) -> impl IntoElement {
    let label = |text: &'static str| div().w(px(DATE_COLUMN)).flex_none().text_right().child(text);
    div()
        .flex_none()
        .mx_2()
        .px_1p5()
        .border_x_1()
        .border_color(gpui::hsla(0., 0., 0., 0.))
        .flex()
        .justify_end()
        .gap_1p5()
        .text_size(px(10.))
        .text_color(rgb(colors().muted))
        .when(columns.growth, |d| d.child(div().w(px(64.)).flex_none()))
        .when(columns.modified, |d| d.child(label("Modified")))
        .when(columns.created, |d| d.child(label("Created")))
        .child(div().w(px(72.)).flex_none())
        .child(div().w(px(10.)).flex_none())
}

/// macOS turns ⌃-click into a right click before gpui passes it on, with the
/// control flag wiped, so only AppKit can still tell the two apart.
pub(super) fn control_held() -> bool {
    NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Control)
}

/// The optional columns of a navigator row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RowColumns {
    pub(super) modified: bool,
    pub(super) created: bool,
    pub(super) growth: bool,
}

/// How wide a date column is: "Modified" over it at the header's size, "12mo" in it.
pub(super) const DATE_COLUMN: f32 = 48.0;

/// Which optional columns a navigator row of this width can afford, modified
/// first. The name comes before any: below this much it stops saying what the row is.
pub(super) fn row_columns(pane: f32, modified: bool, created: bool, growth: bool) -> RowColumns {
    // List and row padding, checkbox, swatch, size, chevron, and the gaps between.
    const FIXED: f32 = 16.0 + 14.0 + 15.0 + 11.0 + 72.0 + 10.0 + 24.0;
    const NAME_FLOOR: f32 = 64.0;
    const DATE: f32 = DATE_COLUMN + 6.0;
    const GROWTH: f32 = 64.0 + 6.0;
    let mut room = pane - FIXED - NAME_FLOOR;
    let mut fits = |wanted: bool, width: f32| {
        let fits = wanted && room >= width;
        if fits {
            room -= width;
        }
        fits
    };
    let modified = fits(modified, DATE);
    let created = fits(created, DATE);
    let growth = fits(growth, GROWTH);
    RowColumns { modified, created, growth }
}

impl Broom {
    pub(super) fn render_access_card(&self, not_readable: Option<u64>, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (title, body) = match self.access {
            Access::Granted | Access::Dismissed => return None,
            Access::Missing => (
                "See everything on your disk",
                match not_readable {
                    Some(bytes) => format!(
                        "{} couldn't be read. macOS keeps some folders private (Mail, Messages, Safari, other apps' data) unless Broom has Full Disk Access.",
                        format_size(bytes)
                    ),
                    None => "macOS keeps some folders private (Mail, Messages, Safari, other apps' data). Give Broom Full Disk Access so the scan can include them.".to_string(),
                },
            ),
            Access::JustGranted => ("Full Disk Access is on", "Rescan to include the folders that were private before.".to_string()),
        };
        let actions = div().flex().gap_2().justify_end().mt_1();
        let actions = match self.access {
            Access::JustGranted => actions.child(
                primary_button("access-rescan", "Rescan", colors().accent).on_click(cx.listener(|this, _, window, cx| {
                    this.access = Access::Granted;
                    this.rescan(&Rescan, window, cx);
                })),
            ),
            _ => actions
                .child(button("access-later", "Not now").on_click(cx.listener(|this, _, _, cx| {
                    this.access = Access::Dismissed;
                    cx.notify();
                })))
                .child(
                    primary_button("access-open", "Open Privacy Settings", colors().accent)
                        .on_click(cx.listener(|_, _, _, cx| cx.open_url(onboarding::FULL_DISK_ACCESS_SETTINGS))),
                ),
        };
        Some(
            div()
                .mx_3()
                .mb_2()
                .p_3()
                .rounded(theme::radius(8.))
                .bg(rgb(colors().card))
                .border_1()
                .border_color(rgb(colors().accent))
                .flex()
                .flex_col()
                .gap_1()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_xs().text_color(rgb(colors().muted)).child(body))
                .when(self.access == Access::Missing, |d| {
                    d.child(div().text_xs().text_color(rgb(colors().muted)).child("In Settings, turn on Broom (use + to add it if it isn't listed)."))
                })
                .child(actions),
        )
    }

    /// The navigator column's frame, shared by every screen.
    pub(super) fn navigator_frame(&self, cx: &mut Context<Self>) -> gpui::Div {
        self.navigator_column().child(self.pane_handle(Pane::Navigator, cx))
    }

    pub(super) fn navigator_column(&self) -> gpui::Div {
        div()
            .w(px(self.pane_width(Pane::Navigator)))
            .relative()
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(colors().panel))
            .border_r_1()
            .border_color(rgb(colors().border))
    }

    /// Nothing scanned yet: an empty navigator and, where the chart goes, a nudge
    /// towards the rail.
    pub(super) fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // An empty list needn't hold its saved width; the welcome gets the room.
        // That width is fixed here, so there's no edge to drag.
        let nav = self
            .navigator_column()
            .w(px(self.pane_width(Pane::Navigator).min(settings::NAVIGATOR_WIDTH)))
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_lg().font_family(theme::current().display).font_weight(FontWeight::SEMIBOLD).text_color(rgb(colors().muted)).child("No scan yet"))
                    .child(div().text_xs().text_color(rgb(colors().muted)).child("Folders appear here as the scan runs.")),
            )
            .children(self.render_access_card(None, cx));
        // Everything takes the column's width, up to its own limit, so a narrow
        // column wraps the words instead of pushing them under the cleanup plan.
        let chart = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .child(div().w_full().max_w(px(420.)).child(comic::investigator(comic::idle(), None, 56.0)))
            .child(div().text_lg().text_center().font_family(theme::current().display).font_weight(FontWeight::SEMIBOLD).child("Pick a disk to scan"))
            .child(
                div()
                    .w_full()
                    .max_w(px(380.))
                    .text_center()
                    .text_color(rgb(colors().muted))
                    .child("Choose a disk or folder on the left. The chart fills in live while the scan runs; findings and the cleanup plan land on the right."),
            );
        div().size_full().flex().child(nav).child(chart)
    }

    pub(super) fn render_results(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Screen::Results(r) = &self.screen else { unreachable!() };
        div()
            .size_full()
            .flex()
            .child(self.render_navigator(r, cx))
            .child(self.render_chart(r, cx))
    }

    /// The navigator: the focused folder's name and totals, the sort toggle, the
    /// ask card when a query is live, and the folder list.
    pub(super) fn render_navigator(&self, r: &Results, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = row_columns(self.pane_width(Pane::Navigator), r.dates_shown(), r.show_created, r.growth_listed);
        let focus = &r.tree.nodes[r.focus];
        let count = r.order.len();
        // Click a key to sort by it; click it again to flip the direction.
        let sort_option = |id: &'static str, value: SortBy, cx: &mut Context<Self>| {
            let selected = r.sort == value;
            let label = match (value, selected, r.descending) {
                (SortBy::Size, false, _) => "Size",
                (SortBy::Size, true, true) => "Size ↓",
                (SortBy::Size, true, false) => "Size ↑",
                (SortBy::Name, false, _) => "Name",
                (SortBy::Name, true, false) => "A–Z",
                (SortBy::Name, true, true) => "Z–A",
                (SortBy::Modified, false, _) => "Modified",
                (SortBy::Modified, true, true) => "Newest",
                (SortBy::Modified, true, false) => "Oldest",
            };
            let tip = match value {
                SortBy::Size => "Sort by size ⌘1 · again to flip",
                SortBy::Name => "Sort by name ⌘2 · again to flip",
                SortBy::Modified => "Sort by date modified ⌘3 · again to flip",
            };
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(theme::radius(6.))
                .cursor_pointer()
                .when(selected, |d| d.bg(rgb(colors().card_hover)).text_color(rgb(colors().text)))
                .when(!selected, |d| d.text_color(rgb(colors().muted)).hover(|s| s.text_color(rgb(colors().text))))
                .on_click(cx.listener(move |this, _, _, cx| this.choose_sort(value, cx)))
                .tooltip(move |_, cx| cx.new(|_| Tip(tip.into())).into())
                .child(label)
        };
        let sort_toggle = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded(theme::radius(8.))
            .border_1()
            .border_color(rgb(colors().border))
            .text_xs()
            .child(sort_option("sort-size", SortBy::Size, cx))
            .child(sort_option("sort-name", SortBy::Name, cx))
            .child(sort_option("sort-modified", SortBy::Modified, cx));

        self.navigator_frame(cx)
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().min_w_0().text_lg().font_family(theme::current().display).font_weight(FontWeight::SEMIBOLD).truncate().child(focus.name.clone()))
                    .child(div().text_color(rgb(colors().muted)).child(format!(
                        "{} · {} files",
                        format_size(focus.size),
                        format_count(focus.items)
                    )))
                    .when_some(r.growth_since.filter(|_| r.growth.contains_key(&r.focus)), |d, since| {
                        let delta = r.growth[&r.focus];
                        let when = match format_age(since).as_str() {
                            "now" => "just now".to_string(),
                            age => format!("{age} ago"),
                        };
                        d.child(div().text_xs().text_color(rgb(colors().muted)).child(format!(
                            "{} since the scan {when}",
                            format_delta(delta).unwrap_or_else(|| "About the same".to_string()),
                        )))
                    })
                    .child(div().pt_1().flex().child(sort_toggle))
                    .when(r.tree.cloud_only > 0, |d| {
                        d.child(div().text_xs().text_color(rgb(colors().muted)).child(format!(
                            "{} cloud-only folders not downloaded (use no space here)",
                            format_count(r.tree.cloud_only)
                        )))
                    })
                    .when(r.tree.errors > 0, |d| {
                        d.child(div().text_xs().text_color(rgb(colors().muted)).child(format!(
                            "{} items couldn’t be read (permissions)",
                            format_count(r.tree.errors)
                        )))
                    }),
            )
            .children((r.focus == Tree::ROOT).then(|| {
                let not_readable = r.tree.nodes[Tree::ROOT]
                    .children
                    .iter()
                    .find(|&&c| r.tree.nodes[c].kind == Kind::Other && r.tree.nodes[c].name.as_ref() == disk::NOT_READABLE)
                    .map(|&c| r.tree.nodes[c].size);
                self.render_access_card(not_readable, cx)
            }).flatten())
            .when(r.matches.is_some() || !matches!(r.ask, AskState::Idle), |d| d.child(self.render_ask(r, cx)))
            .when(r.picked.len() > 1, |d| {
                let size: u64 = r.picked.iter().map(|&ix| r.tree.nodes[ix].size).sum();
                d.child(
                    div()
                        .flex_none()
                        .px_4()
                        .pb_1()
                        .text_xs()
                        .text_color(rgb(colors().muted))
                        .truncate()
                        .child(format!("{} picked · {} · space stages · esc clears", r.picked.len(), format_size(size))),
                )
            })
            .when(columns.modified || columns.created, |d| d.child(date_header(columns)))
            .child(
                div().flex_1().min_h_0().px_2().pb_2().child(
                    uniform_list("children", count, cx.processor(Self::render_rows))
                        .track_scroll(&r.list_scroll)
                        .size_full(),
                ),
            )
    }

    pub(super) fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<Stateful<gpui::Div>> {
        let Screen::Results(r) = &self.screen else { return Vec::new() };
        let focus = &r.tree.nodes[r.focus];
        let hovered = r.hovered();
        let columns = row_columns(self.pane_width(Pane::Navigator), r.dates_shown(), r.show_created, r.growth_listed);
        range
            .filter_map(|i| r.order.get(i).copied())
            .map(|ix| {
                let node = &r.tree.nodes[ix];
                let is_hovered = hovered == Some(Target::Node(ix));
                let is_cursor = r.selected == Some(ix);
                // Lit: picked with ⌃/⌘, or holding the cursor with nothing
                // else picked. The cursor keeps its own show, so you can still
                // tell where Space and ⌘C will land.
                let is_selected = r.picked.contains(&ix) || (is_cursor && r.picked.is_empty());
                let swatch = r.swatches.get(&ix).copied().unwrap_or(gpui::hsla(0., 0., 0.38, 1.));
                let fraction = node.size as f32 / focus.size.max(1) as f32;
                let is_dir = node.kind == Kind::Dir;
                let openable = is_dir && !node.children.is_empty();
                let kind = node.kind;
                let dragged = DraggedItem { node: ix, name: node.name.clone(), size: node.size };
                let path = r.tree.path_of(ix);
                let tick = r.tick(ix);
                let stageable = r.stageable(ix);
                let tint = gpui::hsla(swatch.hue.into_positive_degrees() / 360.0, swatch.saturation, 0.45, 0.16);

                div()
                    .id(("row", ix))
                    .w_full()
                    .h(px(theme::current().row_height))
                    .px_1p5()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded(theme::radius(6.))
                    .border_1()
                    .border_color(if is_selected { to_hsla(colors().accent) } else { gpui::hsla(0., 0., 0., 0.) })
                    .when(is_hovered || is_selected, |d| d.bg(tint))
                    .when(openable, |d| d.cursor_pointer())
                    .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                        if let Some(r) = this.results() {
                            if *hovering {
                                r.list_hover = Some(ix);
                            } else if r.list_hover == Some(ix) {
                                r.list_hover = None;
                            }
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        let keys = event.modifiers();
                        let (additive, range) = (keys.platform || keys.control, keys.shift);
                        if let Some(r) = this.results() {
                            r.pick(ix, additive, range);
                            // A modifier-click adds to the picking; it must not also open the folder.
                            if openable && !(additive || range) {
                                r.navigate(ix);
                            }
                            cx.notify();
                        }
                    }))
                    .on_drag(dragged, |item, _, _, cx| cx.new(|_| item.clone()))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        if control_held() {
                            if let Some(r) = this.results() {
                                r.pick(ix, true, false);
                                cx.notify();
                            }
                        } else if kind != Kind::Other {
                            this.open_context_menu(ix, event.position, cx);
                        }
                    }))
                    .child(
                        checkbox(("tick", ix), tick, stageable)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.click_tick(ix, cx);
                            })),
                    )
                    .child(kind_mark(kind, swatch))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().when(tick != Tick::Off, |d| d.text_color(rgb(colors().muted))).child(node.name.clone()))
                            .when(is_dir, |d| {
                                d.child(
                                    div()
                                        .h(px(3.))
                                        .w(relative(fraction.max(0.005)))
                                        .rounded_full()
                                        .bg(swatch)
                                        .opacity(0.85),
                                )
                            }),
                    )
                    .when(is_hovered && node.kind != Kind::Other, |d| {
                        let path2 = path.clone();
                        d.child(
                            icon_button(("cp-path", ix), "⎘", "Copy Path").on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.write_to_clipboard(ClipboardItem::new_string(path2.to_string_lossy().into_owned()));
                                },
                            )),
                        )
                        .child(
                            icon_button(("reveal", ix), "⌕", "Reveal in Finder").on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.reveal_path(&path);
                                },
                            )),
                        )
                    })
                    .when(columns.growth, |d| {
                        let delta = r.growth.get(&ix).copied().and_then(|g| format_delta(g).map(|text| (g, text)));
                        d.child(
                            div()
                                .w(px(64.))
                                .flex_none()
                                .text_right()
                                .text_xs()
                                .font_family(theme::current().mono)
                                .when_some(delta, |d, (g, text)| {
                                    d.text_color(rgb(if g > 0 { colors().review } else { colors().safe })).child(text)
                                }),
                        )
                    })
                    .when(columns.modified, |d| d.child(date_cell(r.mtimes.get(&ix).copied().flatten())))
                    .when(columns.created, |d| d.child(date_cell(r.ctimes.get(&ix).copied().flatten())))
                    .child(
                        div()
                            .w(px(72.))
                            .flex_none()
                            .text_right()
                            .text_color(rgb(colors().muted))
                            .text_xs()
                            .font_family(theme::current().mono)
                            .child(format_size(node.size)),
                    )
                    .child(
                        div()
                            .w(px(10.))
                            .flex_none()
                            .text_color(rgb(colors().muted))
                            .child(if openable { "›" } else { "" }),
                    )
            })
            .collect()
    }
}
