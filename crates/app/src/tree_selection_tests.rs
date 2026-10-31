use super::*;

#[gpui::test]
fn explorer_tree_mouse_modifiers_toggle_and_extend_without_opening_files(cx: &mut gpui::TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (tree, cx) = cx.add_window_view(|_, cx| {
        let mut tree = FileTree::new(Arc::new(host::local()), cx);
        tree.root = Some("/fixture".into());
        tree.children.insert(
            "/fixture".into(),
            Load::Loaded(
                ["a", "b", "c", "d"].into_iter().map(|name| proto::DirEntry { name: name.into(), kind: proto::Kind::File, target_kind: None }).collect(),
            ),
        );
        tree.rows = ["a", "b", "c", "d"]
            .into_iter()
            .map(|name| Row { path: format!("/fixture/{name}"), name: name.into(), depth: 0, is_dir: false, loading: false, error: None, chain: false })
            .collect();
        tree
    });
    cx.run_until_parked();
    let click = |cx: &mut gpui::VisualTestContext, row: usize, modifiers: gpui::Modifiers| {
        let point = gpui::point(px(100.), px(ROW_H * row as f32 + ROW_H / 2.));
        cx.simulate_mouse_down(point, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(point, MouseButton::Left, modifiers);
    };
    click(cx, 0, gpui::Modifiers::none());
    let mut toggle = gpui::Modifiers::none();
    if cfg!(target_os = "macos") {
        toggle.platform = true;
    } else {
        toggle.control = true;
    }
    click(cx, 2, toggle);
    assert_eq!(tree.read_with(cx, |t, _| t.selected_paths()), ["/fixture/a", "/fixture/c"]);
    click(cx, 3, gpui::Modifiers::shift());
    assert_eq!(tree.read_with(cx, |t, _| t.selected_paths()), ["/fixture/c", "/fixture/d"]);
    click(cx, 2, toggle);
    assert_eq!(tree.read_with(cx, |t, _| t.selected_paths()), ["/fixture/d"]);
    let right = gpui::point(px(100.), px(ROW_H * 3. + ROW_H / 2.));
    cx.simulate_mouse_down(right, MouseButton::Right, gpui::Modifiers::none());
    assert_eq!(tree.read_with(cx, |t, _| t.selected_paths()), ["/fixture/d"]);
}

#[gpui::test]
fn explorer_tree_collapse_drops_hidden_marks_and_sorting_keeps_visible_paths(cx: &mut gpui::TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let tree = cx.new(|cx| {
        let mut tree = FileTree::new(Arc::new(host::local()), cx);
        tree.root = Some("/fixture".into());
        tree.opts.compact = false;
        let entry = |name: &str, kind| proto::DirEntry { name: name.into(), kind, target_kind: None };
        tree.children.insert("/fixture".into(), Load::Loaded(vec![entry("folder", proto::Kind::Dir), entry("z", proto::Kind::File)]));
        tree.children.insert("/fixture/folder".into(), Load::Loaded(vec![entry("child", proto::Kind::File)]));
        tree.expanded.insert("/fixture/folder".into());
        tree.rebuild();
        tree.select_with(1, false, false, cx);
        tree.select_with(2, false, true, cx);
        tree
    });
    tree.update(cx, |tree, cx| tree.collapse_recursive("/fixture/folder", cx));
    assert_eq!(tree.read_with(cx, |t, _| t.selected_paths()), ["/fixture/z"]);
    tree.update(cx, |tree, cx| tree.set_options(ViewOptions { compact: false, sort: SortOrder::FilesFirst }, cx));
    assert_eq!(tree.read_with(cx, |t, _| t.selected_paths()), ["/fixture/z"]);
    tree.update(cx, |tree, cx| tree.set_root(Arc::new(host::local()), None, cx));
    assert!(tree.read_with(cx, |t, _| t.selected_paths()).is_empty());
}

#[gpui::test]
fn explorer_tree_handles_ten_thousand_selected_entries(cx: &mut gpui::TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (tree, cx) = cx.add_window_view(|_, cx| {
        let mut tree = FileTree::new(Arc::new(host::local()), cx);
        tree.root = Some("/fixture".into());
        tree.children.insert(
            "/fixture".into(),
            Load::Loaded((0..10_000).map(|i| proto::DirEntry { name: format!("file-{i:05}.txt"), kind: proto::Kind::File, target_kind: None }).collect()),
        );
        tree.rebuild();
        tree
    });
    cx.run_until_parked();
    let start = std::time::Instant::now();
    tree.update_in(cx, |tree, window, cx| tree.select_all_entries(&SelectAllEntries, window, cx));
    cx.run_until_parked();
    eprintln!("selection preparation: {:?}", start.elapsed());
    assert_eq!(tree.read_with(cx, |t, _| t.selected_paths()).len(), 10_000);
}

#[gpui::test]
fn explorer_tree_late_directory_reads_cannot_replace_a_new_selection_or_root(cx: &mut gpui::TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let tree = cx.new(|cx| FileTree::new(Arc::new(host::local()), cx));
    tree.update(cx, |tree, cx| {
        tree.root = Some("/fixture".into());
        tree.load_epochs.insert("/fixture".into(), 2);
        let entry = |name: &str| proto::DirEntry { name: name.into(), kind: proto::Kind::File, target_kind: None };
        tree.finish_load("/fixture".into(), 2, Ok(vec![entry("new.txt")]), cx);
        tree.select_with(0, false, false, cx);
        tree.finish_load("/fixture".into(), 1, Ok(vec![entry("old.txt")]), cx);
        assert_eq!(tree.selected_paths(), ["/fixture/new.txt"]);
        assert_eq!(tree.row_names(), ["new.txt"]);
        tree.set_root(Arc::new(host::local()), None, cx);
        tree.finish_load("/fixture".into(), 2, Ok(vec![entry("old.txt")]), cx);
        assert!(tree.children.is_empty() && tree.selected_paths().is_empty());
    });
}

#[gpui::test]
fn explorer_recursive_read_errors_stop_without_scheduling_an_endless_retry(cx: &mut gpui::TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let tree = cx.new(|cx| FileTree::new(Arc::new(host::local()), cx));
    tree.update(cx, |tree, cx| {
        tree.root = Some("/fixture".into());
        tree.children.insert("/fixture".into(), Load::Loading);
        tree.load_epochs.insert("/fixture".into(), 7);
        tree.recursive.insert("/fixture".into());
        tree.finish_load("/fixture".into(), 7, Err(host::HostError::Op(proto::Error::PermissionDenied("denied".into()))), cx);
        assert!(matches!(tree.children.get("/fixture"), Some(Load::Failed(_))));
        assert!(tree.load_epochs.is_empty());
        assert!(tree.recursive.is_empty());
    });
    cx.run_until_parked();
    assert!(tree.read_with(cx, |tree, _| tree.load_epochs.is_empty()));
}

/// A folder deleted while it is expanded leaves no row behind, whichever read comes back first: the watcher re-reads
/// the folder itself (now missing) and the delete re-reads its parent. Deleted outside Codeyard, only the folder's own
/// change may arrive: its parent is read again then too.
#[gpui::test]
fn explorer_a_deleted_expanded_folder_leaves_no_not_found_row(cx: &mut gpui::TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    // (compact folders, the folder's own read first, the parent read at all)
    for (compact, folder_first, parent) in
        [(true, true, true), (true, false, true), (false, true, true), (false, false, true), (true, true, false), (false, true, false)]
    {
        let dir = crate::qa_workbench_tests::fresh(&format!("tree-deleted-{compact}-{folder_first}-{parent}"));
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/f.txt"), "f").unwrap();
        std::fs::write(dir.join("a/keep.txt"), "k").unwrap();
        let root = crate::ui::local_path(&dir);
        let (a, b) = (format!("{root}/a"), format!("{root}/a/b"));
        let tree = cx.new(|cx| FileTree::new(Arc::new(host::local()), cx));
        tree.update(cx, |t, cx| {
            t.opts.compact = compact;
            t.set_root(Arc::new(host::local()), Some(root.clone()), cx);
        });
        cx.run_until_parked();
        // A root with one folder opens it by itself; open whatever is still closed.
        for d in [&a, &b] {
            if !tree.read_with(cx, |t, _| t.expanded.contains(d.as_str())) {
                tree.update(cx, |t, cx| t.toggle(d, cx));
                cx.run_until_parked();
            }
        }
        let opened = tree.read_with(cx, |t, _| t.row_names());
        assert!(opened.contains(&"f.txt".to_string()), "b is open: {opened:?}");
        std::fs::remove_dir_all(dir.join("a/b")).unwrap();
        tree.update(cx, |t, cx| {
            let order = if folder_first { [&b, &a] } else { [&a, &b] };
            for d in order.into_iter().filter(|d| parent || **d == b) {
                t.reload(d, cx);
            }
        });
        cx.run_until_parked();
        let (names, errors) = tree.read_with(cx, |t, _| (t.row_names(), t.row_errors()));
        assert!(errors.is_empty(), "compact {compact}, folder first {folder_first}, parent {parent}: {errors:?} in {names:?}");
        assert!(!names.iter().any(|n| n == "b" || n == "f.txt"), "compact {compact}: {names:?}");
    }
}

/// Right Arrow (`Expand`) on folders in every load state: a closed folder opens and keeps the selection; an open folder
/// moves into its first child only when it has a visible one; an empty, loading or unreadable open folder, and a file,
/// keep the selection where it is.
#[gpui::test]
fn explorer_right_arrow_enters_only_a_visible_child(cx: &mut gpui::TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let entry = |name: &str, kind| proto::DirEntry { name: name.into(), kind, target_kind: None };
    let (tree, cx) = cx.add_window_view(|_, cx| {
        let mut tree = FileTree::new(Arc::new(host::local()), cx);
        // No compaction, so a folder with a single child stays a row of its own.
        tree.opts.compact = false;
        tree.root = Some("/right".into());
        let dirs = ["broken", "closed", "empty", "full", "loading"];
        let mut top: Vec<proto::DirEntry> = dirs.iter().map(|d| entry(d, proto::Kind::Dir)).collect();
        top.push(entry("z.txt", proto::Kind::File));
        tree.children.insert("/right".into(), Load::Loaded(top));
        tree.children.insert("/right/broken".into(), Load::Failed("permission denied".into()));
        tree.children.insert("/right/closed".into(), Load::Loaded(vec![entry("inner.txt", proto::Kind::File)]));
        tree.children.insert("/right/empty".into(), Load::Loaded(Vec::new()));
        tree.children.insert("/right/full".into(), Load::Loaded(vec![entry("a.txt", proto::Kind::File), entry("b.txt", proto::Kind::File)]));
        tree.children.insert("/right/loading".into(), Load::Loading);
        for d in ["broken", "empty", "full", "loading"] {
            tree.expanded.insert(format!("/right/{d}"));
        }
        tree.rebuild();
        tree
    });
    cx.run_until_parked();
    let right_on = |path: &str, cx: &mut gpui::VisualTestContext| {
        tree.update_in(cx, |t, window, cx| {
            t.a11y_select(path, false, cx);
            t.expand(&Expand, window, cx);
        });
        cx.run_until_parked();
        tree.read_with(cx, |t, _| t.selected.and_then(|i| t.rows.get(i)).map(|r| r.path.clone()))
    };
    assert_eq!(tree.read_with(cx, |t, _| t.row_names()), ["broken", "closed", "empty", "full", "a.txt", "b.txt", "loading", "z.txt"]);
    // Open with children: the first child.
    assert_eq!(right_on("/right/full", cx).as_deref(), Some("/right/full/a.txt"));
    // Open but empty, still loading or unreadable: no child row, so the folder stays selected (not the next sibling).
    assert_eq!(right_on("/right/empty", cx).as_deref(), Some("/right/empty"));
    assert_eq!(right_on("/right/loading", cx).as_deref(), Some("/right/loading"));
    assert_eq!(right_on("/right/broken", cx).as_deref(), Some("/right/broken"));
    // A file: nothing.
    assert_eq!(right_on("/right/z.txt", cx).as_deref(), Some("/right/z.txt"));
    // Closed: it opens and stays selected; a second Right goes in.
    assert_eq!(right_on("/right/closed", cx).as_deref(), Some("/right/closed"));
    assert!(tree.read_with(cx, |t, _| t.expanded.contains("/right/closed")));
    tree.update_in(cx, |t, window, cx| t.expand(&Expand, window, cx));
    assert_eq!(tree.read_with(cx, |t, _| t.selected.map(|i| t.rows[i].path.clone())).as_deref(), Some("/right/closed/inner.txt"));
    // The loading folder's children arrive: now Right goes in.
    tree.update(cx, |t, _| {
        t.children.insert("/right/loading".into(), Load::Loaded(vec![entry("late.txt", proto::Kind::File)]));
        t.rebuild();
    });
    assert_eq!(right_on("/right/loading", cx).as_deref(), Some("/right/loading/late.txt"));
}
