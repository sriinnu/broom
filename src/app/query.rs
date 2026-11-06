//! The "find by name, or ask" field and its answers.

use super::*;

impl Broom {
    pub(super) fn focus_query(&mut self, _: &FocusQuery, window: &mut Window, cx: &mut Context<Self>) {
        // Not while a modal owns the screen (the field is dimmed behind it) or a
        // native sheet has the window.
        if self.modal.is_some() || self.busy {
            return;
        }
        let handle = self.query.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// Escape in the field: clear it, hand focus back to the chart, and drop the ask state.
    pub(super) fn dismiss_query(&mut self, _: &DismissQuery, window: &mut Window, cx: &mut Context<Self>) {
        // With the Help / About card up, Escape closes the card instead.
        if self.modal.is_some() {
            self.close_modal(window, cx);
            return;
        }
        self.query.update(cx, |state, cx| state.emplace("", cx));
        window.focus(&self.focus_handle, cx);
        if let Some(r) = self.results() {
            r.search_version += 1;
            r.matches = None;
            r.ask = AskState::Idle;
        }
        cx.notify();
    }

    /// The query text changed: re-run exact matching in the background, debounced, and
    /// drop the result if the text has moved on. No network — this is the always-on
    /// baseline that the providers only add judgement to.
    pub(super) fn query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).as_str().trim().to_string();
        let Some(r) = self.results() else { return };
        r.search_version += 1;
        let version = r.search_version;
        r.ask = AskState::Idle;
        if query.is_empty() {
            r.matches = None;
            cx.notify();
            return;
        }
        let (tree, findings) = (r.tree.clone(), r.findings.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let groups = cx.background_spawn(async move { ask::candidates(&tree, &findings, &query) }).await;
            this.update(cx, |this, cx| {
                let Some(r) = this.results() else { return };
                if r.search_version != version {
                    return;
                }
                r.matches = (!groups.is_empty()).then(|| Arc::new(groups));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Enter in the field: ask the providers what the query means, then act on it.
    /// A delete answer pre-selects the Collector; nothing is ever trashed from the
    /// answer alone.
    pub(super) fn submit_query(&mut self, _: &SubmitQuery, _: &mut Window, cx: &mut Context<Self>) {
        // Not under the Help / About card. (Enter in a Settings field never
        // gets here: the Settings window handles it; see `SettingsWindow`.)
        if self.modal.is_some() {
            return;
        }
        let query = self.query.read(cx).as_str().trim().to_string();
        if query.is_empty() {
            return;
        }
        let Some(r) = self.results() else { return };
        r.query = query.clone().into();
        r.ask = AskState::Thinking;
        r.search_version += 1;
        let version = r.search_version;
        let tree = r.tree.clone();
        let findings = r.findings.clone();
        let scan_root = r.requested_root.clone();
        let focused = (r.focus != Tree::ROOT).then(|| tree.path_of(r.focus));
        // The folder in view when the query was asked: a "here"-scoped answer is
        // enforced against this, not wherever the user navigated while it thought.
        let focused_ix = r.focus;
        cx.spawn(async move |this, cx| {
            let (groups, answer) = cx
                .background_spawn(async move {
                    let groups = ask::candidates(&tree, &findings, &query);
                    if groups.is_empty() {
                        (groups, Err(ProviderError::BadResponse("no exact matches".into())))
                    } else {
                        let ctx = ask::AskContext { scan_root, focused };
                        let answer = ask::resolve(&ask::providers(), &query, &ctx, &groups);
                        (groups, answer)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(r) = this.results() else { return };
                if r.search_version != version {
                    return;
                }
                r.matches = (!groups.is_empty()).then(|| Arc::new(groups));
                this.ask_resolved(answer, focused_ix, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Apply a provider's answer — or its failure — to the results.
    pub(super) fn ask_resolved(&mut self, answer: Result<Understanding, ProviderError>, focus: usize, cx: &mut Context<Self>) {
        let mut collect_now = false;
        if let Some(r) = self.results() {
            match answer {
                Ok(u) if u.group != usize::MAX => {
                    // Which reader answered is a developer's concern, never shown in the UI.
                    if std::env::var_os("BROOM_ASK_DEBUG").is_some() {
                        eprintln!("ask: answered by {}", u.provider);
                    }
                    let group = r.matches.as_ref().and_then(|m| m.get(u.group)).cloned().map(Arc::new);
                    if let Some(group) = group {
                        match u.intent {
                            Intent::Delete => {
                                // Pre-select only what the safety rules allow; the Collector
                                // shows the exact list and the confirm prompt does the rest.
                                // Every group the words named, not just the best one.
                                let mut chosen: Vec<usize> = u.groups.clone();
                                if !chosen.contains(&u.group) {
                                    chosen.insert(0, u.group);
                                }
                                for gix in chosen {
                                    let Some(g) = r.matches.as_ref().and_then(|m| m.get(gix)).cloned() else { continue };
                                    // A substring or kind match is shown, never pre-selected.
                                    if !g.source.auto_tick() {
                                        continue;
                                    }
                                    for ix in ask::collectable(&r.tree, &r.homes, &g, u.scope, focus) {
                                        r.collect(ix);
                                    }
                                }
                                collect_now = !r.collector.is_empty();
                            }
                            Intent::Locate => {
                                if let Some(&ix) = group.nodes.iter().find(|&&ix| r.tree.nodes[ix].kind == Kind::Dir) {
                                    r.navigate(ix);
                                }
                            }
                        }
                        r.ask = AskState::Answered { intent: u.intent, group };
                    } else {
                        r.ask = AskState::Suggestions;
                    }
                }
                // Unsure of the group, or sure nothing matched: the cards decide.
                Ok(_) | Err(ProviderError::BadResponse(_)) => r.ask = AskState::Suggestions,
                Err(ProviderError::NoKey) => r.ask = AskState::Unavailable(Unavailable::NoKey),
                Err(ProviderError::Unauthorized) => r.ask = AskState::Unavailable(Unavailable::Rejected),
                Err(ProviderError::Http(_)) => r.ask = AskState::Unavailable(Unavailable::Unreachable),
            }
        }
        if collect_now {
            self.collector_changed(cx);
        }
        cx.notify();
    }

    /// The ask card: what the submitted query resolved to, with the exact-match groups
    /// as cards underneath (they show live while typing, no network involved). Click a
    /// card to go to its biggest folder; "+" collects the whole group.
    pub(super) fn render_ask(&self, r: &Results, cx: &mut Context<Self>) -> impl IntoElement {
        let note = |text: String| div().text_xs().text_color(rgb(colors().muted)).child(text);
        let mut card = div()
            .id("ask")
            .mx_3()
            .mb_2()
            .p_2()
            .rounded(theme::radius(8.))
            .bg(rgb(colors().bg))
            .border_1()
            .border_color(rgb(colors().border))
            .flex()
            .flex_col()
            .gap_1();

        match &r.ask {
            AskState::Idle => {}
            AskState::Thinking => card = card.child(note("asking the reader…".into())),
            AskState::Suggestions => card = card.child(note(match r.matches {
                Some(_) => "not sure which one you meant — pick a card below".to_string(),
                None => format!("nothing in this scan matches “{}”", r.query),
            })),
            AskState::Unavailable(why) => {
                card = card.child(note(match why {
                    Unavailable::NoKey => "the reader has no key and the fallback couldn't be reached — exact matches only".to_string(),
                    Unavailable::Rejected => "the reader's key was rejected — exact matches only".to_string(),
                    Unavailable::Unreachable => "couldn't reach the reader — exact matches only".to_string(),
                }))
            }
            AskState::Answered { intent, group } => {
                card = card.child(note(match intent {
                    Intent::Delete if r.collector.is_empty() => format!(
                        "The reader took that as delete, but nothing in “{}” may be staged — home folders and system volumes are off limits.",
                        group.label,
                    ),
                    Intent::Delete => format!("The reader took that as delete: {} staged in the cleanup plan.", group.label),
                    Intent::Locate => format!("The reader took that as find: showing {}.", group.label),
                }));
                if *intent == Intent::Delete && !r.collector.is_empty() {
                    card = card.child(
                        div().flex().justify_end().child(
                            primary_button("ask-trash", "Review and Move to Trash…", colors().danger)
                                .on_click(cx.listener(|this, _, window, cx| this.open_review(window, cx))),
                        ),
                    );
                }
            }
        }

        if let Some(groups) = &r.matches {
            let mut list = div().id("ask-groups").max_h(px(220.)).overflow_y_scroll().flex().flex_col().gap_0p5();
            for (i, group) in groups.iter().take(ask::MAX_GROUPS).enumerate() {
                let color = match group.safety {
                    Safety::Safe => colors().safe,
                    Safety::Review => colors().review,
                    Safety::Manage => colors().muted,
                };
                let nodes = group.nodes.clone();
                let biggest_dir = nodes.iter().copied().find(|&ix| r.tree.nodes[ix].kind == Kind::Dir);
                let tick = r.group_tick(&nodes);
                list = list.child(
                    div()
                        .id(("ask-group", i))
                        .px_2()
                        .py_1()
                        .rounded(theme::radius(6.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(colors().card_hover)))
                        .when_some(biggest_dir, |d, ix| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(r) = this.results() {
                                    r.navigate(ix);
                                    cx.notify();
                                }
                            }))
                        })
                        .child(checkbox(("ask-tick", i), tick, true).on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.click_group_tick(&nodes, cx);
                        })))
                        .child(div().size(px(8.)).flex_none().rounded_full().bg(rgb(color)))
                        .child(div().flex_1().min_w_0().truncate().child(group.label.clone())),
                );
            }
            card = card.child(list);
        }
        card
    }
}
