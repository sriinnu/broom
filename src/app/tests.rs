use super::*;

#[cfg(test)]
mod pick_tests {
    use super::*;

    fn results_for(root: &std::path::Path, tree: &Tree) -> Results {
        Results::new(
            tree.clone(),
            Vec::new(),
            Duration::ZERO,
            root.to_path_buf(),
            ColorBy::Folder,
            SortBy::Size,
            true,
            false,
        )
    }

    fn tree_with_three(root: &std::path::Path) -> Tree {
        let write = |rel: &str, bytes: usize| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![7u8; bytes]).unwrap();
        };
        write("alpha/a.bin", 1 << 20);
        write("beta/b.bin", 1 << 19);
        write("gamma/c.bin", 1 << 18);
        scan::scan(root, &scan::Progress::default(), &[], settings::Settings::default().outline_depth)
    }

    fn row_named(tree: &Tree, name: &str) -> usize {
        tree.nodes[Tree::ROOT]
            .children
            .iter()
            .copied()
            .find(|&c| tree.nodes[c].name.as_ref() == name)
            .expect("named row")
    }

    /// Three rows sorted biggest first, so ⇧-clicking the ends takes all three.
    fn rows_in_order(r: &Results) -> Vec<String> {
        r.order
            .iter()
            .map(|&ix| r.tree.nodes[ix].name.to_string())
            .collect()
    }

    /// Protected things, what's inside them and what holds them can't be
    /// staged by any route, and protecting something takes it off the plan.
    #[test]
    fn protected_rows_cannot_be_staged() {
        let root = std::env::temp_dir().join(format!("broom-protect-rows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let (alpha, beta) = (row_named(&tree, "alpha"), row_named(&tree, "beta"));
        let a_file = tree.nodes[alpha].children[0];
        r.collect(alpha);
        assert_eq!(r.collector, vec![alpha]);

        r.protect(&[root.join("alpha/a.bin")]);
        assert!(r.collector.is_empty(), "protecting took the folder holding it off the plan");
        assert!(!r.stageable(alpha), "holds something protected");
        assert!(!r.stageable(a_file), "protected itself");
        assert!(r.stageable(beta));
        r.collect(alpha);
        assert!(r.collector.is_empty(), "collect");
        r.stage_rows(&[alpha, a_file]);
        assert!(r.collector.is_empty(), "stage_rows");
        r.toggle_group(&[alpha, a_file]);
        assert!(r.collector.is_empty(), "toggle_group");
        r.toggle_staged(a_file);
        assert!(r.collector.is_empty(), "toggle_staged");

        r.protect(&[root.join("alpha")]);
        assert!(!r.stageable(a_file), "inside something protected");
        r.protect(&[]);
        assert!(r.stageable(alpha) && r.stageable(a_file), "off the list, back to normal");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shift_click_takes_the_range() {
        let root = std::env::temp_dir().join(format!("broom-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let order = rows_in_order(&r);
        assert_eq!(order, ["alpha", "beta", "gamma"], "biggest first");

        let (alpha, gamma) = (row_named(&tree, "alpha"), row_named(&tree, "gamma"));
        // Cursor on the first row, then ⇧-click the last.
        r.selected = Some(alpha);
        r.pick(gamma, false, true);
        assert_eq!(r.picked.len(), 3, "the whole range, ends included");
        assert!(r.picked.contains(&alpha) && r.picked.contains(&gamma));
        // The cursor stays where it was, so a further ⇧-click re-measures from it.
        assert_eq!(r.selected, Some(alpha));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ctrl_click_toggles_one_row() {
        let root = std::env::temp_dir().join(format!("broom-pick2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let (alpha, beta) = (row_named(&tree, "alpha"), row_named(&tree, "beta"));

        r.pick(alpha, true, false);
        assert!(r.picked.contains(&alpha) && r.picked.len() == 1);

        // ⌃-click a second row: both stay lit.
        r.pick(beta, true, false);
        assert_eq!(r.picked.len(), 2, "⌃-click adds without losing the rest");

        // ⌃-click the first again: it drops out, the other stays.
        r.pick(alpha, true, false);
        assert!(!r.picked.contains(&alpha));
        assert!(r.picked.contains(&beta));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// With nothing picked, Space and ⌘C still act on the cursor row, and the
    /// picked-then-unpicked state falls back to it cleanly.
    #[test]
    fn selection_falls_back_to_the_cursor() {
        let root = std::env::temp_dir().join(format!("broom-pick3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let beta = row_named(&tree, "beta");

        assert!(r.selection().is_empty(), "nothing lit yet");

        r.pick(beta, false, false);
        assert_eq!(r.selection(), vec![beta], "cursor row acts alone");

        r.picked.clear();
        assert_eq!(r.selection(), vec![beta], "cursor still holds it");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Dates are wanted when asked for, and when sorting by them: a Modified sort
    /// with the column off has to stat too, or it never orders anything.
    #[test]
    fn times_are_fetched_whenever_dates_are_shown_or_sorted_by() {
        let root = std::env::temp_dir().join(format!("broom-col-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let listed = tree.nodes[Tree::ROOT].children.len();
        let mut r = results_for(&root, &tree);

        assert!(r.mtimes_wanted.is_empty(), "off under a Size sort: no disk work");
        r.show_modified = true;
        r.reorder();
        assert_eq!(r.mtimes_wanted.len(), listed, "on under a Size sort");

        r.show_modified = false;
        r.set_sort(SortBy::Modified, true);
        assert!(r.dates_shown(), "the Modified sort shows its dates");
        assert_eq!(r.mtimes_wanted.len(), listed, "and has to read them");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An empty growth column and dates once left the name 24pt.
    #[test]
    fn optional_columns_never_squeeze_the_name_away() {
        let cols = |modified, created, growth| RowColumns { modified, created, growth };
        assert_eq!(row_columns(340.0, false, false, false), cols(false, false, false));
        assert_eq!(row_columns(340.0, true, false, false), cols(true, false, false));
        assert_eq!(row_columns(340.0, false, true, false), cols(false, true, false));
        assert_eq!(row_columns(340.0, false, false, true), cols(false, false, true));
        assert_eq!(row_columns(340.0, true, true, false), cols(true, true, false));
        assert_eq!(row_columns(300.0, true, true, true), cols(true, false, false), "modified wins when only one fits");
        assert_eq!(row_columns(380.0, true, false, true), cols(true, false, true), "the default width holds dates and growth");
        assert_eq!(row_columns(500.0, true, true, true), cols(true, true, true));
        assert_eq!(row_columns(220.0, true, true, true), cols(false, false, false), "narrowest pane: just the name");
        for pane in [220.0, 260.0, 300.0, 340.0, 400.0, 680.0] {
            let c = row_columns(pane, true, true, true);
            let taken = 162.0 + if c.modified { 54.0 } else { 0.0 } + if c.created { 54.0 } else { 0.0 } + if c.growth { 70.0 } else { 0.0 };
            assert!(pane - taken >= 58.0, "name left with {} at {pane}", pane - taken);
        }
    }

    /// One new row must not undo the rows already staged.
    #[test]
    fn space_on_a_mixed_picking_stages_the_rest() {
        let root = std::env::temp_dir().join(format!("broom-mix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let (alpha, beta) = (row_named(&tree, "alpha"), row_named(&tree, "beta"));

        r.toggle_rows(&[alpha]);
        assert_eq!(r.collector, vec![alpha]);
        r.toggle_rows(&[alpha, beta]);
        assert_eq!(r.collector, vec![alpha, beta], "beta joined, alpha stayed");
        r.toggle_rows(&[alpha, beta]);
        assert!(r.collector.is_empty(), "all on: all off");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Staging together follows the same rules as one at a time: nothing nested.
    #[test]
    fn staging_a_folder_replaces_what_was_staged_inside_it() {
        let root = std::env::temp_dir().join(format!("broom-nest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let alpha = row_named(&tree, "alpha");
        let inside = tree.nodes[alpha].children[0];

        r.stage_rows(&[inside]);
        assert_eq!(r.collector, vec![inside]);
        r.stage_rows(&[alpha]);
        assert_eq!(r.collector, vec![alpha]);
        r.stage_rows(&[inside]);
        assert_eq!(r.collector, vec![alpha], "already covered");
        r.stage_rows(&[Tree::ROOT]);
        assert_eq!(r.collector, vec![alpha], "never the root");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The cursor never sits on an unlit row, so Enter can't open what you can't see.
    #[test]
    fn un_picking_a_row_takes_the_cursor_off_it() {
        let root = std::env::temp_dir().join(format!("broom-cur-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let (alpha, beta) = (row_named(&tree, "alpha"), row_named(&tree, "beta"));

        r.pick(alpha, true, false);
        r.pick(beta, true, false);
        r.pick(beta, true, false);
        assert_eq!(r.selected, None);
        assert_eq!(r.selection(), vec![alpha]);

        // ⌘-clicking the lit row unlights it straight away, as in Finder.
        r.picked.clear();
        r.pick(alpha, false, false);
        r.pick(alpha, true, false);
        assert!(r.selection().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Asking for created dates alone still reads the times.
    #[test]
    fn created_dates_ask_for_times_too() {
        let root = std::env::temp_dir().join(format!("broom-crt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        r.show_created = true;
        r.reorder();
        assert_eq!(r.mtimes_wanted.len(), tree.nodes[Tree::ROOT].children.len());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⌘A takes the real rows and nothing that can't be acted on.
    #[test]
    fn select_all_takes_every_real_row() {
        let root = std::env::temp_dir().join(format!("broom-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        r.pick_all();
        assert_eq!(r.selection(), r.order);
        assert_eq!(r.selection().len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// No past scan differences means no growth column at all.
    #[test]
    fn the_growth_column_only_exists_for_a_real_change() {
        let root = std::env::temp_dir().join(format!("broom-grow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let alpha = row_named(&tree, "alpha");

        r.growth.insert(alpha, 10);
        r.reorder();
        assert!(!r.growth_listed, "under a megabyte is noise");

        r.growth.insert(alpha, 5 << 20);
        r.reorder();
        assert!(r.growth_listed);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Several rows act together, in the order they're listed, whatever order
    /// they were picked in; and ⌘C copies exactly those.
    #[test]
    fn a_picking_acts_on_every_row_in_list_order() {
        let root = std::env::temp_dir().join(format!("broom-pick5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let (alpha, gamma) = (row_named(&tree, "alpha"), row_named(&tree, "gamma"));

        r.pick(gamma, true, false);
        r.pick(alpha, true, false);
        assert_eq!(r.selection(), vec![alpha, gamma]);
        let copied = r.paths_to_copy();
        let lines: Vec<&str> = copied.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("alpha") && lines[1].ends_with("gamma"), "{copied}");

        // One row left picked: that row is copied, not whatever the cursor was on.
        r.pick(alpha, true, false);
        assert!(r.paths_to_copy().ends_with("gamma"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The lit row counts when you start adding, as in Finder; un-picking the
    /// last row leaves nothing lit rather than falling back to it.
    #[test]
    fn adding_starts_from_the_lit_row() {
        let root = std::env::temp_dir().join(format!("broom-pick6-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let (alpha, beta) = (row_named(&tree, "alpha"), row_named(&tree, "beta"));

        r.pick(alpha, false, false);
        r.pick(beta, true, false);
        assert_eq!(r.selection(), vec![alpha, beta]);

        r.pick(beta, true, false);
        r.pick(alpha, true, false);
        assert!(r.selection().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A trashed row can't stay picked, or space would stage something gone.
    #[test]
    fn a_trashed_row_leaves_the_picking() {
        let root = std::env::temp_dir().join(format!("broom-pick7-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let (alpha, beta) = (row_named(&tree, "alpha"), row_named(&tree, "beta"));

        r.pick(alpha, true, false);
        r.pick(beta, true, false);
        Arc::make_mut(&mut r.tree).remove(alpha);
        r.relayout();
        assert_eq!(r.selection(), vec![beta]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Changing folders must drop the picking: the node indices it holds belong
    /// to the old listing, and staging from them would hit the wrong rows.
    #[test]
    fn navigating_clears_the_picking() {
        let root = std::env::temp_dir().join(format!("broom-pick4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_three(&root);
        let mut r = results_for(&root, &tree);
        let (alpha, beta) = (row_named(&tree, "alpha"), row_named(&tree, "beta"));

        r.pick(alpha, true, false);
        r.pick(beta, true, false);
        assert_eq!(r.picked.len(), 2);

        r.navigate(alpha);
        assert!(r.picked.is_empty(), "the picking doesn't follow you");
        assert_eq!(r.focus, alpha);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    /// A real tree on disk: root with two folders, `a` holding a nested one.
    fn tree_with_nested(root: &std::path::Path) -> Tree {
        let write = |rel: &str, bytes: usize| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![7u8; bytes]).unwrap();
        };
        write("a/inner/one.bin", 1 << 18);
        write("a/two.bin", 1 << 19);
        write("b/three.bin", 1 << 17);
        scan::scan(root, &scan::Progress::default(), &[], settings::Settings::default().outline_depth)
    }

    fn child_of(tree: &Tree, parent: usize, name: &str) -> usize {
        tree.nodes[parent]
            .children
            .iter()
            .copied()
            .find(|&c| tree.nodes[c].name.as_ref() == name)
            .expect("named child")
    }

    /// Walk root → a → inner, then step back twice and forward twice, checking
    /// the focus each time and that the opposite stack is what we'd expect.
    #[test]
    fn back_and_forward_walk_the_trail() {
        let root = std::env::temp_dir().join(format!("broom-hist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_nested(&root);
        let mut r = Results::new(
            tree.clone(),
            Vec::new(),
            Duration::ZERO,
            root.clone(),
            ColorBy::Folder,
            SortBy::Size,
            true,
            false,
        );

        let a = child_of(&tree, Tree::ROOT, "a");
        let inner = child_of(&tree, a, "inner");
        assert_eq!(r.focus, Tree::ROOT);

        r.navigate(a);
        assert_eq!(r.focus, a);
        r.navigate(inner);
        assert_eq!(r.focus, inner);
        assert_eq!(r.back, vec![Tree::ROOT, a], "both steps recorded");
        assert!(r.fwd.is_empty());

        // Two steps back: inner → a → root, with the reverse building up.
        assert!(r.travel(Travel::Back));
        assert_eq!(r.focus, a);
        assert!(r.travel(Travel::Back));
        assert_eq!(r.focus, Tree::ROOT);
        assert_eq!(r.fwd, vec![inner, a], "retreat fills the forward stack");
        assert!(r.back.is_empty());

        // Nothing left behind: a further Back is a no-op, not a move.
        assert!(!r.travel(Travel::Back));
        assert_eq!(r.focus, Tree::ROOT);

        // Forward again.
        assert!(r.travel(Travel::Forward));
        assert_eq!(r.focus, a);
        assert!(r.travel(Travel::Forward));
        assert_eq!(r.focus, inner);
        assert_eq!(r.back, vec![Tree::ROOT, a], "walking forward rebuilds the trail");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A refresh swaps in a new tree with new node numbers. The user's place goes
    /// across by path: the focused folder, the trail, the cursor, the picking and
    /// the plan; what was deleted in between is dropped, and a focus inside a
    /// deleted folder lands on the nearest folder above it that's still there.
    #[test]
    fn a_refresh_keeps_the_users_place() {
        let root = std::env::temp_dir().join(format!("broom-refresh-carry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_nested(&root);
        let a = child_of(&tree, Tree::ROOT, "a");
        let inner = child_of(&tree, a, "inner");
        let b = child_of(&tree, Tree::ROOT, "b");
        let two = child_of(&tree, a, "two.bin");
        let mut old = Results::new(tree.clone(), Vec::new(), Duration::ZERO, root.clone(), ColorBy::Folder, SortBy::Size, true, false);
        old.navigate(b);
        old.navigate(a);
        old.selected = Some(two);
        old.picked = [two, inner].into_iter().collect();
        old.collect(inner);
        old.collect(b);

        // On disk meanwhile: b goes, a new folder sorts first, so every number shifts.
        std::fs::remove_dir_all(root.join("b")).unwrap();
        std::fs::create_dir_all(root.join("aaa")).unwrap();
        std::fs::write(root.join("aaa/new.bin"), vec![1u8; 1 << 20]).unwrap();
        let fresh = scan::scan(&root, &scan::Progress::default(), &[], settings::Settings::default().outline_depth);
        let mut new = Results::new(fresh, Vec::new(), Duration::ZERO, root.clone(), ColorBy::Folder, SortBy::Size, true, false);
        new.carry_over_from(&old);

        let path = |r: &Results, ix: usize| r.tree.path_of(ix);
        assert_ne!(new.tree.find(&root.join("a")), Some(a), "the numbers did change");
        assert_eq!(path(&new, new.focus), root.join("a"));
        assert_eq!(new.back.iter().map(|&ix| path(&new, ix)).collect::<Vec<_>>(), vec![root.clone()], "b left the trail");
        assert_eq!(new.selected.map(|ix| path(&new, ix)), Some(root.join("a/two.bin")));
        let mut picked: Vec<PathBuf> = new.picked.iter().map(|&ix| path(&new, ix)).collect();
        picked.sort();
        assert_eq!(picked, vec![root.join("a/inner"), root.join("a/two.bin")]);
        assert_eq!(new.collector.iter().map(|&ix| path(&new, ix)).collect::<Vec<_>>(), vec![root.join("a/inner")], "b left the plan");
        assert_eq!(new.order, new.tree.nodes[new.focus].children, "the list shows the carried focus");

        // Standing inside something that's gone: up to what's left.
        let mut deep = Results::new(new.tree.as_ref().clone(), Vec::new(), Duration::ZERO, root.clone(), ColorBy::Folder, SortBy::Size, true, false);
        deep.navigate(new.tree.find(&root.join("a/inner")).unwrap());
        std::fs::remove_dir_all(root.join("a/inner")).unwrap();
        let fresh = scan::scan(&root, &scan::Progress::default(), &[], settings::Settings::default().outline_depth);
        let mut after = Results::new(fresh, Vec::new(), Duration::ZERO, root.clone(), ColorBy::Folder, SortBy::Size, true, false);
        after.carry_over_from(&deep);
        assert_eq!(after.tree.path_of(after.focus), root.join("a"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Stepping back out of a folder lights the row you came from, as going Up
    /// does, and leaves any picking behind.
    #[test]
    fn back_lights_the_folder_just_left_and_drops_the_picking() {
        let root = std::env::temp_dir().join(format!("broom-hist5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_nested(&root);
        let a = child_of(&tree, Tree::ROOT, "a");
        let inner = child_of(&tree, a, "inner");
        let mut r = Results::new(tree.clone(), Vec::new(), Duration::ZERO, root.clone(), ColorBy::Folder, SortBy::Size, true, false);

        r.navigate(a);
        r.pick(inner, true, false);
        assert!(r.travel(Travel::Back));
        assert_eq!(r.focus, Tree::ROOT);
        assert_eq!(r.selected, Some(a));
        assert!(r.picked.is_empty(), "those rows aren't listed here");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The Back button can't look live when only trashed folders are left behind it.
    #[test]
    fn nothing_to_go_back_to_when_only_trashed_folders_remain() {
        let root = std::env::temp_dir().join(format!("broom-hist6-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_nested(&root);
        let (a, b) = (child_of(&tree, Tree::ROOT, "a"), child_of(&tree, Tree::ROOT, "b"));
        let mut r = Results::new(tree.clone(), Vec::new(), Duration::ZERO, root.clone(), ColorBy::Folder, SortBy::Size, true, false);

        assert!(!r.can_travel(Travel::Back));
        r.navigate(a);
        assert!(r.can_travel(Travel::Back));
        r.back.clear();
        r.back.push(b);
        Arc::make_mut(&mut r.tree).remove(b);
        assert!(!r.can_travel(Travel::Back));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Arriving somewhere by Forward must leave the list exactly as arriving
    /// there by clicking did — same folder focused, same row lit. A plain
    /// navigate lands the keyboard on the child it came from, so Forward has
    /// to apply the same test rather than just taking the parent.
    #[test]
    fn forward_lands_where_navigate_would() {
        let root = std::env::temp_dir().join(format!("broom-hist4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_nested(&root);
        let a = child_of(&tree, Tree::ROOT, "a");
        let inner = child_of(&tree, a, "inner");

        // Walk in and back out, then Forward.
        let mut walked = Results::new(
            tree.clone(),
            Vec::new(),
            Duration::ZERO,
            root.clone(),
            ColorBy::Folder,
            SortBy::Size,
            true,
            false,
        );
        walked.navigate(a);
        walked.navigate(inner);
        walked.travel(Travel::Back);
        walked.travel(Travel::Forward);

        // Now arrive the same place by clicking down the tree.
        let mut clicked = Results::new(
            tree.clone(),
            Vec::new(),
            Duration::ZERO,
            root.clone(),
            ColorBy::Folder,
            SortBy::Size,
            true,
            false,
        );
        clicked.navigate(a);
        clicked.navigate(inner);

        assert_eq!(walked.focus, clicked.focus, "same folder");
        assert_eq!(walked.selected, clicked.selected, "same row lit");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A fresh step must kill the forward stack, or Forward would jump to a
    /// branch we deliberately walked away from.
    #[test]
    fn a_new_step_clears_forward() {
        let root = std::env::temp_dir().join(format!("broom-hist2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_nested(&root);
        let mut r = Results::new(
            tree.clone(),
            Vec::new(),
            Duration::ZERO,
            root.clone(),
            ColorBy::Folder,
            SortBy::Size,
            true,
            false,
        );

        let a = child_of(&tree, Tree::ROOT, "a");
        let b = child_of(&tree, Tree::ROOT, "b");
        let inner = child_of(&tree, a, "inner");
        r.navigate(a);
        r.navigate(b);
        r.travel(Travel::Back);
        assert!(!r.fwd.is_empty(), "stepping back filled Forward");
        // We're on `a` now, so navigate somewhere new rather than back to `a`
        // (re-navigating to the current focus is a no-op and wouldn't clear it).
        assert_eq!(r.focus, a);

        // Walk somewhere new: Forward must be dead again.
        r.navigate(inner);
        assert!(r.fwd.is_empty(), "a fresh step invalidates the forward trail");
        assert!(!r.travel(Travel::Forward));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Trashing a folder detaches it from the tree. Back must skip it rather
    /// than land the keyboard on a node that's no longer in the chart.
    #[test]
    fn back_skips_folders_that_are_gone() {
        let root = std::env::temp_dir().join(format!("broom-hist3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = tree_with_nested(&root);
        let a = child_of(&tree, Tree::ROOT, "a");
        let inner = child_of(&tree, a, "inner");
        let mut r = Results::new(
            tree.clone(),
            Vec::new(),
            Duration::ZERO,
            root.clone(),
            ColorBy::Folder,
            SortBy::Size,
            true,
            false,
        );

        r.navigate(a);
        r.navigate(inner);
        assert_eq!(r.back, vec![Tree::ROOT, a]);

        // `a` is trashed: detach it, the way a completed deletion does.
        Arc::make_mut(&mut r.tree).remove(a);
        assert!(r.travel(Travel::Back));
        assert_eq!(r.focus, Tree::ROOT, "stepped over the detached node");
        assert!(r.back.is_empty(), "the dead entry was dropped, not kept");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod column_tests {
    use super::*;

    #[test]
    fn columns_that_fit_are_left_alone() {
        assert_eq!(fit_columns([200.0, 300.0, 300.0], 1400.0), [200.0, 300.0, 300.0]);
    }

    #[test]
    fn a_narrow_window_squeezes_the_widest_first() {
        let fitted = fit_columns([360.0, 680.0, 560.0], 1440.0);
        let total: f32 = fitted.iter().sum::<f32>() + CHART_MIN_WIDTH;
        assert!((total - 1440.0).abs() < 1.0, "{fitted:?}");
        assert!(fitted[1] <= 680.0 - 120.0, "{fitted:?}");
        assert!(fitted[0] >= 150.0 && fitted[1] >= 220.0 && fitted[2] >= 240.0);
    }

    #[test]
    fn floors_hold_even_when_nothing_fits() {
        assert_eq!(fit_columns([360.0, 680.0, 560.0], 400.0), [150.0, 220.0, 240.0]);
    }
}

#[cfg(test)]
mod tick_tests {
    use super::*;

    /// The definition `tick` had before it was indexed, kept as the oracle.
    fn naive(r: &Results, ix: usize) -> Tick {
        if r.collector.contains(&ix) {
            Tick::On
        } else if r.collector.iter().any(|&c| r.tree.is_ancestor_or_self(c, ix)) {
            Tick::Implied
        } else if r.collector.iter().any(|&c| r.tree.is_ancestor_or_self(ix, c)) {
            Tick::Mixed
        } else {
            Tick::Off
        }
    }

    #[test]
    fn indexed_ticks_match_the_definition_as_the_plan_changes() {
        let root = std::env::temp_dir().join(format!("broom-tick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for rel in ["a/x/1.bin", "a/x/2.bin", "a/y/3.bin", "b/z/w/4.bin", "b/5.bin", "c/6.bin"] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![7u8; 4096]).unwrap();
        }
        let tree = scan::scan(&root, &scan::Progress::default(), &[], settings::Settings::default().outline_depth);
        let mut r = Results::new(tree.clone(), Vec::new(), Duration::ZERO, root.clone(), ColorBy::Folder, SortBy::Size, true, false);
        let all: Vec<usize> = (0..r.tree.nodes.len()).collect();
        let check = |r: &Results| {
            for &ix in &all {
                assert_eq!(r.tick(ix), naive(r, ix), "node {ix} with plan {:?}", r.collector);
            }
        };
        check(&r);
        // Stage, unstage and restage in an order that hits every state, the
        // way the UI does: some through `collect`, some by editing in place.
        for &ix in all.iter().rev() {
            r.collect(ix);
            check(&r);
        }
        let first = r.collector[0];
        r.collector.retain(|&c| c != first);
        check(&r);
        for &ix in &all {
            r.toggle_staged(ix);
            check(&r);
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
