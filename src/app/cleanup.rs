//! Findings, the cleanup plan, the review card and the move to Trash.

use super::*;

impl Broom {
    /// Work out what the Collector frees, off the UI thread.
    pub(super) fn collector_changed(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.results() else { return };
        r.collector_version += 1;
        r.collector_frees.set(None);
        let version = r.collector_version;
        let paths: Vec<PathBuf> = r.collector.iter().map(|&ix| r.tree.path_of(ix)).collect();
        if paths.is_empty() {
            r.collector_frees.set(Some(0));
            return;
        }
        cx.spawn(async move |this, cx| {
            let frees = cx.background_spawn(async move { scan::frees_of(&paths) }).await;
            this.update(cx, |this, cx| {
                if let Some(r) = this.results() {
                    if r.collector_version == version {
                        r.collector_frees.set(Some(frees));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Review and Move to Trash…": open the review card over the staged list.
    pub(super) fn open_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.modal.is_some() {
            return;
        }
        let Some(r) = self.results() else { return };
        if r.collector.is_empty() {
            return;
        }
        let items = r.collector.clone();
        let paths: Vec<(usize, PathBuf)> = items.iter().map(|&ix| (ix, r.tree.path_of(ix))).collect();
        self.context_menu = None;
        self.review = Some(ReviewState { items, frees: HashMap::new(), working: false, drop: None });
        // Per-item savings, one at a time in the background; dropped if the card closed.
        cx.spawn(async move |this, cx| {
            for (ix, path) in paths {
                let frees = cx.background_spawn(async move { scan::frees_of(&[path]) }).await;
                let open = this
                    .update(cx, |this, cx| {
                        let Some(review) = &mut this.review else { return false };
                        review.frees.insert(ix, frees);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !open {
                    break;
                }
            }
        })
        .detach();
        window.focus(&self.modal_focus, cx);
        cx.notify();
    }

    pub(super) fn close_review(&mut self, _: &CloseReview, window: &mut Window, cx: &mut Context<Self>) {
        if self.review.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    /// The review card's "Move to Trash": the only place anything is deleted.
    pub(super) fn trash_staged(&mut self, cx: &mut Context<Self>) {
        let Some(review) = &mut self.review else { return };
        if review.working {
            return;
        }
        review.working = true;
        let items = review.items.clone();
        let Some(r) = self.results() else { return };
        // Only what's still in the tree and still staged.
        let items: Vec<usize> = items.into_iter().filter(|ix| r.tree.nodes[*ix].parent.is_some() && r.collector.contains(ix)).collect();
        if items.is_empty() {
            self.review = None;
            cx.notify();
            return;
        }
        let paths: Vec<PathBuf> = items.iter().map(|&ix| r.tree.path_of(ix)).collect();
        let paths_after = paths.clone();
        let freed = r.collector_frees.get().unwrap_or_else(|| r.collected_size());
        let sizes: Vec<(scan::Kind, u64)> = items.iter().map(|&ix| (r.tree.nodes[ix].kind, r.tree.nodes[ix].size)).collect();
        let drop = trashcan::Drop::new(trashcan::Item::from_sizes(&sizes), freed);
        if let Some(review) = &mut self.review {
            review.drop = Some(drop.clone());
        }
        cx.spawn(async move |this, cx| {
            let (result, landed) = cx.background_spawn(async move { trashcan::send_tracked(&paths) }).await;
            if result.is_ok() {
                // Let the last item land and the lid close before the card goes.
                drop.settle();
                cx.background_executor().timer(drop.remaining()).await;
            }
            this.update(cx, |this, cx| {
                this.review = None;
                match result {
                    Ok(()) => {
                        // Where the trashed things sit on the chart, read before
                        // they leave the tree, so dust can lift off those spots.
                        let specks = this
                            .results()
                            .map(|r| {
                                let gone = |n: usize| items.iter().any(|&ix| r.tree.is_ancestor_or_self(ix, n));
                                let slices: Vec<(f32, f32, usize)> = r
                                    .segments
                                    .iter()
                                    .filter(|s| match s.target {
                                        Target::Node(n) => gone(n),
                                        Target::Small { parent } => gone(parent),
                                    })
                                    .map(|s| (s.start, s.end, s.depth))
                                    .collect();
                                delight::specks(&slices)
                            })
                            .unwrap_or_default();
                        this.remove_trashed(&items, cx);
                        let milestone = delight::record_sweep(freed);
                        let what = if items.len() == 1 { "1 item".to_string() } else { format!("{} items", items.len()) };
                        // The plain text stays as the fallback (and for anything
                        // reading `banner_text`); `freed` drives the richer banner.
                        this.show_banner(
                            Some(format!("Moved {what} ({}) to the Trash\nEmpty the Trash to get the space back", format_size(freed))),
                            cx,
                        );
                        if let Some(r) = this.results() {
                            r.last_trashed = landed;
                            r.freed = Some(Freed { bytes: freed, items: items.len(), milestone, emptied: None });
                            r.dust = (!specks.is_empty() && !delight::reduce_motion())
                                .then(|| Rc::new(delight::Puff { start: clock::now(), specks }));
                        }
                        // A firmer tap than staging: the cleanup is done.
                        cx.play_haptic_feedback(HapticFeedbackStyle::LevelChange);
                    }
                    Err(error) => {
                        // Not atomic: whatever did go should leave the tree too.
                        let gone: Vec<usize> = items.iter().copied().zip(&paths_after).filter(|(_, p)| !p.exists()).map(|(ix, _)| ix).collect();
                        if !gone.is_empty() {
                            this.remove_trashed(&gone, cx);
                        }
                        this.error = Some(format!("Couldn’t move to Trash: {error}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// A checkbox on a row was clicked.
    pub(super) fn click_tick(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(r) = self.results() {
            if r.stageable(ix) {
                r.toggle_staged(ix);
                stage_tap(cx);
            }
        }
        self.collector_changed(cx);
        cx.notify();
    }

    /// "Never Offer This" / "Offer This Again": add `path` to the protect
    /// list (Settings › Protected) or take it off. Adding applies at once;
    /// taking off re-reads the disk, since what was held back has to be found
    /// again to be offered.
    pub(super) fn set_protected(&mut self, path: PathBuf, on: bool, cx: &mut Context<Self>) {
        let line = abbreviate_home(&path.to_string_lossy());
        let shown = line.clone();
        if on {
            if !self.settings.protected.contains(&line) {
                self.settings.protected.push(line);
            }
        } else {
            let home = home_dir();
            self.settings.protected.retain(|p| {
                let expanded = p.strip_prefix("~/").map(|rest| home.join(rest)).unwrap_or_else(|| PathBuf::from(p));
                expanded != path
            });
        }
        let _ = settings::save(&self.settings);
        self.apply_protection();
        self.collector_changed(cx);
        if on {
            self.show_banner(Some(format!("Protected {shown}\nBroom won't offer it or move it to the Trash. Settings › Protected lists it.")), cx);
        } else {
            self.show_banner(Some(format!("{shown} can be offered again\nReading the disk again to see what it holds…")), cx);
            self.refresh_in_place(cx);
        }
        cx.notify();
    }

    /// A checkbox on a group (a finding, an ask card) was clicked.
    pub(super) fn click_group_tick(&mut self, nodes: &[usize], cx: &mut Context<Self>) {
        if let Some(r) = self.results() {
            r.toggle_group(nodes);
            stage_tap(cx);
        }
        self.collector_changed(cx);
        cx.notify();
    }

    pub(super) fn unstage(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(r) = self.results() {
            r.collector.retain(|&c| c != ix);
            stage_tap(cx);
        }
        self.collector_changed(cx);
        cx.notify();
    }

    /// The right column: findings (whatever the scan has found so far) above
    /// the cleanup plan.
    pub(super) fn render_cleanup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let col = div()
            .w(px(self.pane_width(Pane::Cleanup)))
            .relative()
            .child(self.pane_handle(Pane::Cleanup, cx))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(colors().panel))
            .border_l_1()
            .border_color(rgb(colors().border));
        match &self.screen {
            Screen::Results(r) => col.child(self.render_findings(&r.findings, Some(r), cx)).child(self.render_plan(r, cx)),
            Screen::Scanning(s) => {
                let mut early = findings::early_findings(&s.progress.early_findings.lock().unwrap());
                let protected = self.settings.protect_paths();
                early.retain(|f| f.path.as_ref().is_none_or(|p| crate::protect::blocking(p, &protected).is_none()));
                col.child(self.render_findings(&early, None, cx))
                    .child(plan_placeholder("The plan opens when the scan finishes."))
            }
            Screen::Start => col
                .child(self.render_findings(&[], None, cx))
                .child(plan_placeholder("Tick folders and findings to stage them. Nothing moves until you review the plan.")),
        }
    }

    /// Known space hogs with exact sizes. With results, each has a checkbox that
    /// stages it, is tagged "in view" when it sits under the focused folder, and
    /// opens its folder on click; "Stage all safe" ticks every safe one.
    pub(super) fn render_findings(&self, findings: &[Finding], results: Option<&Results>, cx: &mut Context<Self>) -> impl IntoElement {
        let safe: u64 = findings.iter().filter(|f| f.safety == Safety::Safe).map(|f| f.size).sum();
        let safe_nodes: Vec<usize> = findings.iter().filter(|f| f.safety == Safety::Safe).flat_map(|f| tickable(f, results)).collect();
        let all_safe_staged = results.is_some_and(|r| !safe_nodes.is_empty() && r.group_tick(&safe_nodes) == Tick::On);
        let mut list = div().id("findings").max_h(px(290.)).overflow_y_scroll().flex().flex_col().gap_0p5().px_1p5();
        for (i, finding) in findings.iter().enumerate() {
            let (tag, color) = match finding.safety {
                Safety::Safe => ("Safe to delete", colors().safe),
                Safety::Review => ("Review first", colors().review),
                Safety::Manage => ("Use its tool", colors().muted),
            };
            let target = finding.nodes.first().copied().filter(|_| finding.path.is_some());
            // What the finding's checkbox stages: for a project finding, the
            // folders not built this week (`tickable`).
            let nodes = tickable(finding, results);
            let project = findings::PROJECT_FINDINGS.contains(&finding.title) && results.is_some();
            let recent = results.map_or(0, |r| finding.nodes.iter().filter(|ix| r.recent.contains_key(ix)).count());
            let unfolded = results.is_some_and(|r| r.unfolded.contains(finding.title));
            let title = finding.title;
            let in_view = results.is_some_and(|r| {
                r.focus != Tree::ROOT && finding.nodes.iter().any(|&n| r.tree.is_ancestor_or_self(r.focus, n))
            });
            // A Manage finding is explained, never staged: no checkbox.
            let tick = results.filter(|_| finding.safety != Safety::Manage).map(|r| r.group_tick(&nodes));
            list = list.child(
                div()
                    .id(("finding", i))
                    .px_1p5()
                    .py_1()
                    .rounded(theme::radius(6.))
                    .flex()
                    .items_start()
                    .gap_2()
                    .when(results.is_some() && target.is_some(), |d| {
                        d.cursor_pointer().hover(|s| s.bg(rgb(colors().card_hover))).on_click(cx.listener(move |this, _, _, cx| {
                            if let (Some(r), Some(ix)) = (this.results(), target) {
                                r.navigate(ix);
                                cx.notify();
                            }
                        }))
                    })
                    // A Manage finding has no checkbox; keep its title in line with the rest.
                    .when(results.is_some() && tick.is_none(), |d| d.child(div().flex_none().w(px(15.))))
                    .when_some(tick, |d, tick| {
                        let nodes = nodes.clone();
                        d.child(
                            div().pt_0p5().child(
                                checkbox(("finding-tick", i), tick, true)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.click_group_tick(&nodes, cx);
                                    })),
                            ),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(finding.title))
                                    .when(in_view, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .px_1()
                                                .rounded(theme::radius(4.))
                                                .bg(rgb(colors().card_hover))
                                                .text_xs()
                                                .text_color(rgb(colors().muted))
                                                .child("in view"),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_right()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .when(finding.pending, |d| d.text_color(rgb(colors().muted)))
                                            .child(if finding.pending { "…".to_string() } else { format_size(finding.size) }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .text_xs()
                                    .child(div().flex_none().text_color(rgb(color)).child(tag))
                                    .child(div().flex_none().text_color(rgb(colors().muted)).child("·"))
                                    .child(div().min_w_0().text_color(rgb(colors().muted)).truncate().child(finding.blurb.clone())),
                            )
                            // Project findings: say what the tick leaves out, and
                            // unfold to every folder, each with its own tick.
                            .when(project && recent > 0, |d| {
                                d.child(div().text_xs().text_color(rgb(colors().review)).child(format!(
                                    "{recent} changed this week, left unticked"
                                )))
                            })
                            .when(project && finding.nodes.len() > 1, |d| {
                                let n = finding.nodes.len();
                                d.child(
                                    div()
                                        .id(("unfold", i))
                                        .text_xs()
                                        .text_color(rgb(colors().accent))
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if let Some(r) = this.results() {
                                                if !r.unfolded.remove(title) {
                                                    r.unfolded.insert(title);
                                                }
                                                cx.notify();
                                            }
                                        }))
                                        .child(if unfolded { "Hide the list".to_string() } else { format!("Show all {n} ›") }),
                                )
                            })
                            .when_some(results.filter(|_| unfolded), |d, r| d.child(self.render_project_rows(finding, r, cx)))
                            // Duplicates have a second way out that deletes nothing.
                            .when(finding.title == DUPLICATES && results.is_some_and(|r| !r.dupe_pairs.is_empty()), |d| {
                                d.child(
                                    div().pt_1().child(
                                        button(("merge-clones", i), "Merge as Clones…")
                                            .text_xs()
                                            .py_0p5()
                                            .px_2()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.open_merge(window, cx);
                                            })),
                                    ),
                                )
                            }),
                    ),
            );
        }
        let summary = if findings.iter().any(|f| f.pending) {
            "working out savings…".to_string()
        } else if safe > 0 {
            format!("{} safe", format_size(safe))
        } else if findings.is_empty() {
            match results {
                Some(_) => "nothing piling up, all tidy".to_string(),
                None => "appear as the scan runs".to_string(),
            }
        } else {
            String::new()
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .pb_2()
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_1p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Findings"))
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(rgb(colors().muted)).child(summary))
                    .when(results.is_some() && !safe_nodes.is_empty(), |d| {
                        let nodes = safe_nodes.clone();
                        d.child(
                            button("stage-safe", if all_safe_staged { "Unstage safe" } else { "Stage all safe" })
                                .text_xs()
                                .py_0p5()
                                .px_2()
                                .on_click(cx.listener(move |this, _, _, cx| this.click_group_tick(&nodes, cx))),
                        )
                    }),
            )
            .child(list)
            // Said, not hidden: what the protect list kept off the findings.
            .when_some(results.filter(|r| !r.not_offered.is_empty()), |d, r| {
                let names: Vec<String> = r.not_offered.iter().map(|p| abbreviate_home(&p.to_string_lossy())).collect();
                let n = names.len();
                let shown = names.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                let more = if n > 2 { format!(" and {} more", n - 2) } else { String::new() };
                let all = names.join("\n");
                d.child(
                    path_tip(div().id("not-offered"), all)
                        .px_3()
                        .pt_1()
                        .text_xs()
                        .text_color(rgb(colors().muted))
                        .truncate()
                        .child(format!("Not offered, protected: {shown}{more}")),
                )
            })
    }

    /// A project finding unfolded: every folder, biggest first, with its
    /// own checkbox, the project it belongs to, and when it last changed if
    /// that was this week (a build or an install, most likely).
    fn render_project_rows(&self, finding: &Finding, r: &Results, cx: &mut Context<Self>) -> impl IntoElement {
        const ROWS: usize = 60;
        let mut nodes = finding.nodes.clone();
        nodes.sort_by(|a, b| r.tree.nodes[*b].size.cmp(&r.tree.nodes[*a].size));
        let more = nodes.len().saturating_sub(ROWS);
        let mut list = div().pt_1().flex().flex_col();
        for &ix in nodes.iter().take(ROWS) {
            let node = &r.tree.nodes[ix];
            let project = node.parent.map(|p| r.tree.nodes[p].name.to_string()).unwrap_or_default();
            let full = abbreviate_home(&r.tree.path_of(ix).to_string_lossy());
            let built = r.recent.get(&ix).map(|t| match format_age(*t).as_str() {
                "now" => "changed just now".to_string(),
                age => format!("changed {age} ago"),
            });
            list = list.child(
                div()
                    .id(("project-row", ix))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .py_0p5()
                    .text_xs()
                    .child(checkbox(("project-tick", ix), r.tick(ix), r.stageable(ix)).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.click_tick(ix, cx);
                    })))
                    .child(
                        path_tip(div().id(("project-name", ix)), full)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .cursor_pointer()
                            .hover(|s| s.text_color(rgb(colors().accent)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(r) = this.results() {
                                    r.navigate(ix);
                                    cx.notify();
                                }
                            }))
                            .child(format!("{project}/{}", node.name)),
                    )
                    .when_some(built, |d, built| d.child(div().flex_none().text_color(rgb(colors().review)).child(built)))
                    .child(div().flex_none().font_family(theme::current().mono).text_color(rgb(colors().muted)).child(format_size(node.size))),
            );
        }
        if more > 0 {
            list = list.child(div().text_xs().text_color(rgb(colors().muted)).child(format!("and {} more, smaller", format_count(more as u64))));
        }
        list
    }

    /// The cleanup plan: one line per staged item, the totals, and the way to the
    /// review card. Rows can still be dragged in.
    pub(super) fn render_plan(&self, r: &Results, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = r.collector.is_empty();
        let size = r.collected_size();
        let frees = r.collector_frees.get();
        let shared = frees.map(|f| size.saturating_sub(f));

        let mut list = div().id("plan-list").flex_1().min_h_0().overflow_y_scroll().px_1p5().flex().flex_col().gap_0p5();
        for &ix in r.collector.iter().take(PLAN_ROWS) {
            let node = &r.tree.nodes[ix];
            let path = r.tree.path_of(ix);
            let parent = path.parent().map(|p| abbreviate_home(&p.to_string_lossy())).unwrap_or_default();
            list = list.child(
                div()
                    .id(("plan", ix))
                    .px_1p5()
                    .py_1()
                    .rounded(theme::radius(6.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .hover(|s| s.bg(rgb(colors().card_hover)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().font_weight(FontWeight::MEDIUM).child(node.name.clone()))
                            .child(path_tip(div().id(("plan-path", ix)), parent.clone()).text_xs().text_color(rgb(colors().muted)).truncate().child(parent)),
                    )
                    .child(div().flex_none().text_xs().font_family(theme::current().mono).text_color(rgb(colors().muted)).child(format_size(node.size)))
                    .child(
                        div()
                            .id(("unstage", ix))
                            .flex_none()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme::radius(6.))
                            .cursor_pointer()
                            .text_color(rgb(colors().muted))
                            .hover(|s| s.bg(rgb(colors().border)).text_color(rgb(colors().text)))
                            .tooltip(|_, cx| cx.new(|_| Tip("Remove from the plan".into())).into())
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| this.unstage(ix, cx))),
                    ),
            );
        }

        let total_line = |label: &'static str, value: String, strong: bool| {
            div()
                .flex()
                .justify_between()
                .gap_2()
                .child(div().text_color(rgb(colors().muted)).child(label))
                .child(div().font_family(theme::current().mono).text_xs().when(strong, |d| d.text_color(rgb(colors().text)).font_weight(FontWeight::SEMIBOLD)).when(!strong, |d| d.text_color(rgb(colors().muted))).child(value))
        };

        div()
            .id("collector")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(colors().border))
            .on_drag_over::<DraggedItem>(|style, _, _, _| style.bg(rgb(colors().card)))
            .on_drop(cx.listener(|this, item: &DraggedItem, _, cx| {
                if let Some(r) = this.results() {
                    if r.picked.contains(&item.node) {
                        let rows = r.selection();
                        r.stage_rows(&rows);
                    } else {
                        r.collect(item.node);
                    }
                }
                this.collector_changed(cx);
                cx.notify();
            }))
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_1p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Cleanup plan"))
                    .child(div().flex_1().text_xs().text_color(rgb(colors().muted)).child(match r.collector.len() {
                        0 => String::new(),
                        1 => "1 item".to_string(),
                        n => format!("{n} items"),
                    }))
                    .when(!empty, |d| {
                        d.child(button("clear-collector", "Clear").text_xs().py_0p5().px_2().on_click(cx.listener(|this, _, _, cx| {
                            if let Some(r) = this.results() {
                                r.collector.clear();
                            }
                            this.collector_changed(cx);
                            cx.notify();
                        })))
                    }),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .flex_1()
                        .px_3()
                        .text_xs()
                        .text_color(rgb(colors().muted))
                        .child("Tick a folder or a finding to stage it — or drag a row here. Nothing moves until you review the plan."),
                )
            })
            .when(!empty, |d| d.child(list))
            .when(r.collector.len() > PLAN_ROWS, |d| {
                d.child(div().flex_none().px_3().pt_1().text_xs().text_color(rgb(colors().muted)).child(format!("and {} more; the totals and the review cover them all", format_count((r.collector.len() - PLAN_ROWS) as u64))))
            })
            .child(
                div()
                    .flex_none()
                    .mx_3()
                    .mt_2()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(rgb(colors().bg))
                    .border_1()
                    .border_color(rgb(colors().border))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_sm()
                    .child(total_line("Selected", format_size(size), false))
                    .child(total_line(
                        "Shared with other files",
                        match (empty, shared) {
                            (true, _) => "—".to_string(),
                            (false, Some(shared)) => format_size(shared),
                            (false, None) => "calculating…".to_string(),
                        },
                        false,
                    ))
                    .child(total_line(
                        "Will free",
                        match (empty, frees) {
                            (true, _) => "—".to_string(),
                            (false, Some(frees)) => format_size(frees),
                            (false, None) => format!("up to {}", format_size(size)),
                        },
                        true,
                    )),
            )
            .child(
                div().flex_none().p_3().child(
                    primary_button("trash", "Review and Move to Trash…", colors().danger)
                        .w_full()
                        .text_center()
                        .when(empty, |b| b.opacity(0.45).cursor_default())
                        .when(!empty, |b| b.on_click(cx.listener(|this, _, window, cx| this.open_review(window, cx)))),
                ),
            )
    }

    /// The review card: every staged item with its path and size, the honest
    /// total, and the one button that moves things.
    pub(super) fn render_review(&self, review: &ReviewState, cx: &mut Context<Self>) -> AnyElement {
        let Screen::Results(r) = &self.screen else { return div().into_any_element() };
        let items: Vec<usize> = review.items.iter().copied().filter(|&ix| r.tree.nodes[ix].parent.is_some()).collect();
        let size: u64 = items.iter().map(|&ix| r.tree.nodes[ix].size).sum();
        let frees = r.collector_frees.get();
        let shared = frees.map(|f| size.saturating_sub(f)).filter(|&s| s > size / 100);

        let mut list = div().id("review-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap_0p5();
        for &ix in &items {
            let node = &r.tree.nodes[ix];
            let path = abbreviate_home(&r.tree.path_of(ix).to_string_lossy());
            let item_frees = review.frees.get(&ix).copied();
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .rounded(theme::radius(6.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().font_weight(FontWeight::MEDIUM).child(node.name.clone()))
                            .child(path_tip(div().id(("review-path", ix)), path.clone()).text_xs().text_color(rgb(colors().muted)).font_family(theme::current().mono).truncate().child(path)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .items_end()
                            .child(div().text_xs().font_family(theme::current().mono).child(format_size(node.size)))
                            .child(div().text_xs().text_color(rgb(colors().muted)).child(match item_frees {
                                Some(f) if f + node.size / 100 < node.size => format!("frees {}", format_size(f)),
                                Some(_) => "frees all of it".to_string(),
                                None => "working out…".to_string(),
                            })),
                    ),
            );
        }
        let count = if items.len() == 1 { "1 item".to_string() } else { format!("{} items", items.len()) };
        let subtitle = match frees {
            Some(frees) => format!("{count} · {} selected · will free {}", format_size(size), format_size(frees)),
            None => format!("{count} · {} selected · working out what's shared…", format_size(size)),
        };

        let stage = review.drop.as_ref().filter(|_| review.working).map(|drop| {
            let (landed, bytes) = drop.landed();
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().h(px(230.)).w_full().child(trashcan::stage(drop.clone())))
                .child(div().text_xs().text_color(rgb(colors().muted)).text_center().child(format!(
                    "{landed} of {} in the bin · {}",
                    drop.total(),
                    format_size(bytes)
                )))
        });
        let list = if stage.is_some() { div().id("review-list") } else { list };

        div()
            .id("review-card")
            .w(px(560.))
            .max_h(px(600.))
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
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_lg().font_family(theme::current().display).font_weight(FontWeight::SEMIBOLD).child("Move these to the Trash?"))
                    .child(div().text_xs().text_color(rgb(colors().muted)).child(subtitle)),
            )
            .child(list)
            .children(stage)
            .when_some(shared, |d, shared| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded(theme::radius(8.))
                        .bg(theme::alpha(colors().review, 0.12))
                        .border_1()
                        .border_color(theme::alpha(colors().review, 0.35))
                        .text_xs()
                        .text_color(rgb(colors().review))
                        .child(format!(
                            "{} of this is shared with files outside the selection (APFS clones or hard links), so the Trash will free {} rather than {}.",
                            format_size(shared),
                            format_size(frees.unwrap_or(0)),
                            format_size(size)
                        )),
                )
            })
            .child(div().text_xs().text_color(rgb(colors().muted)).child("Everything goes to the Trash; you can put items back from Finder. The space comes back when you empty the Trash."))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        button("review-cancel", "Cancel")
                            .when(!review.working, |b| b.on_click(cx.listener(|this, _, window, cx| this.close_review(&CloseReview, window, cx)))),
                    )
                    .child(
                        primary_button("review-trash", if review.working { "Moving…" } else { "Move to Trash" }, colors().danger)
                            .when(review.working, |b| b.opacity(0.6))
                            .when(!review.working, |b| b.on_click(cx.listener(|this, _, _, cx| this.trash_staged(cx)))),
                    ),
            )
            .into_any_element()
    }
}

/// The lightest trackpad tick, as a box is ticked or unticked by a click. Only
/// felt with a finger on a Force Touch trackpad, which a click guarantees; the
/// keyboard path stays silent, as macOS's own controls do.
fn stage_tap(cx: &App) {
    cx.play_haptic_feedback(HapticFeedbackStyle::Alignment);
}

/// What a finding's checkbox stages. A project finding leaves out folders
/// built this week, unless that's all of them; anything else is all of it.
fn tickable(finding: &Finding, results: Option<&Results>) -> Vec<usize> {
    let Some(r) = results.filter(|_| findings::PROJECT_FINDINGS.contains(&finding.title)) else { return finding.nodes.clone() };
    let older: Vec<usize> = finding.nodes.iter().copied().filter(|ix| !r.recent.contains_key(ix)).collect();
    if older.is_empty() { finding.nodes.clone() } else { older }
}
