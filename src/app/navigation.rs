//! Moving around: up, back and forward, the keyboard row, sorting and copying paths.

use super::*;

impl Broom {
    pub(super) fn go_up(&mut self, _: &GoUp, _: &mut Window, cx: &mut Context<Self>) {
        // Escape's first job is closing whatever's on top: the modal, then the menu;
        // only then does it navigate up.
        if self.modal.take().is_some() || self.review.take().is_some() || self.context_menu.take().is_some() {
            cx.notify();
            return;
        }
        if let Screen::Results(r) = &mut self.screen {
            // A picking is the next thing on top: it took effort, and going up would lose it.
            if !r.picked.is_empty() {
                r.picked.clear();
                cx.notify();
                return;
            }
            if let Some(parent) = r.tree.nodes[r.focus].parent {
                r.navigate(parent);
                cx.notify();
            }
        }
    }

    /// ‹Back and Forward›: a step through the folders visited. Nothing moves
    /// behind an open card.
    pub(super) fn travel(&mut self, way: Travel, cx: &mut Context<Self>) {
        if self.modal.is_some() || self.review.is_some() {
            return;
        }
        if let Screen::Results(r) = &mut self.screen {
            if r.travel(way) {
                cx.notify();
            }
        }
    }

    pub(super) fn go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        self.travel(Travel::Back, cx);
    }

    pub(super) fn go_forward(&mut self, _: &GoForward, _: &mut Window, cx: &mut Context<Self>) {
        self.travel(Travel::Forward, cx);
    }

    pub(super) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() || self.review.is_some() {
            return;
        }
        if let Some(r) = self.results() {
            r.pick_all();
            cx.notify();
        }
    }

    pub(super) fn open_context_menu(&mut self, node: usize, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.context_menu = Some(ContextMenu { node, at });
        cx.notify();
    }

    pub(super) fn set_chart_hover(&mut self, hit: Option<Hit>, cx: &mut Context<Self>) {
        if let Some(r) = self.results() {
            if r.chart_hover != hit {
                r.chart_hover = hit;
                cx.notify();
            }
        }
    }

    pub(super) fn chart_click(&mut self, hit: Hit, cx: &mut Context<Self>) {
        let Some(r) = self.results() else { return };
        match hit {
            Hit::Center => {
                if let Some(parent) = r.tree.nodes[r.focus].parent {
                    r.navigate(parent);
                }
            }
            Hit::Segment(i) => {
                if let Some(Target::Node(ix)) = r.segments.get(i).map(|s| s.target) {
                    r.navigate(ix);
                }
            }
        }
        cx.notify();
    }

    /// Keyboard in the navigator: ↑/↓ move the selection along the list.
    pub(super) fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.modal.is_some() || self.review.is_some() {
            return;
        }
        let Some(r) = self.results() else { return };
        if r.order.is_empty() {
            return;
        }
        let last = r.order.len() - 1;
        let pos = match r.selected.and_then(|s| r.order.iter().position(|&o| o == s)) {
            None if delta >= 0 => 0,
            None => last,
            Some(p) => (p as isize + delta).clamp(0, last as isize) as usize,
        };
        r.picked.clear();
        r.selected = Some(r.order[pos]);
        r.list_scroll.scroll_to_item(pos, ScrollStrategy::Nearest);
        cx.notify();
    }

    pub(super) fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    pub(super) fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    /// → or Enter: open the selected folder.
    pub(super) fn open_selected(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() || self.review.is_some() {
            return;
        }
        let Some(r) = self.results() else { return };
        if let Some(ix) = r.selected {
            if r.tree.nodes[ix].kind == Kind::Dir && !r.tree.nodes[ix].children.is_empty() {
                r.navigate(ix);
                cx.notify();
            }
        }
    }

    /// Space: stage or unstage the selected row.
    pub(super) fn toggle_selected(&mut self, _: &ToggleStaged, _: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() || self.review.is_some() {
            return;
        }
        let Some(r) = self.results() else { return };
        let rows = r.selection();
        if rows.is_empty() {
            return;
        }
        r.toggle_rows(&rows);
        self.collector_changed(cx);
        cx.notify();
    }

    /// ⌘C in the navigator: the selected row's full path, or the open folder's.
    pub(super) fn copy_path(&mut self, _: &CopyPath, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.path_to_copy() {
            cx.write_to_clipboard(ClipboardItem::new_string(path));
        }
    }

    /// ⇧⌘C: the same, with the home folder as `~`.
    pub(super) fn copy_relative_path(&mut self, _: &CopyRelativePath, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.path_to_copy() {
            cx.write_to_clipboard(ClipboardItem::new_string(abbreviate_home(&path)));
        }
    }

    pub(super) fn path_to_copy(&mut self) -> Option<String> {
        if self.modal.is_some() || self.review.is_some() {
            return None;
        }
        let r = self.results()?;
        Some(r.paths_to_copy())
    }

    /// Sort the list by `value`; asking for the current one again flips it.
    pub(super) fn choose_sort(&mut self, value: SortBy, cx: &mut Context<Self>) {
        self.sort_descending = if self.sort == value { !self.sort_descending } else { value.default_descending() };
        self.sort = value;
        let descending = self.sort_descending;
        if let Some(r) = self.results() {
            r.set_sort(value, descending);
        }
        cx.notify();
    }
}
