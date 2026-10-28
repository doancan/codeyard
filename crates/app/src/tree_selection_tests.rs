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
