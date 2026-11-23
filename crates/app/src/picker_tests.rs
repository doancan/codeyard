use super::*;

fn setup<'a>(cx: &'a mut gpui::TestAppContext, source: Source, query: &str) -> (gpui::Entity<Picker>, &'a mut gpui::VisualTestContext) {
    crate::qa_workbench_tests::globals(cx);
    let query = query.to_string();
    let (picker, cx) = cx.add_window_view(|_, cx| Picker::new(vec![source], "", &query, None, cx));
    picker.update_in(cx, |p, window, cx| window.focus(&p.focus_handle(cx)));
    (picker, cx)
}

fn shortcut(key: &str) -> String {
    format!("{}-{key}", if cfg!(target_os = "macos") { "cmd" } else { "ctrl" })
}

#[gpui::test]
fn picker_select_all_replaces_a_remote_folder_path(cx: &mut gpui::TestAppContext) {
    let (p, cx) = setup(cx, Source::Dirs { items: Vec::new(), status: "".into() }, "/home/user/");
    cx.simulate_keystrokes(&shortcut("a"));
    cx.simulate_input("/srv/İstanbul/🦀");
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "/srv/İstanbul/🦀");
    cx.simulate_keystrokes(&format!("{} backspace", shortcut("a")));
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "");
}

#[gpui::test]
fn picker_clipboard_shortcuts_replace_the_selection(cx: &mut gpui::TestAppContext) {
    let (p, cx) = setup(cx, Source::Dirs { items: Vec::new(), status: "".into() }, "/home/user/");
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("/srv/project\r\n".into()));
    cx.simulate_keystrokes(&format!("{} {}", shortcut("a"), shortcut("v")));
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "/srv/project");
    cx.simulate_keystrokes(&format!("{} {}", shortcut("a"), shortcut("c")));
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("/srv/project"));
    cx.simulate_keystrokes(&shortcut("x"));
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "");
}

#[gpui::test]
fn picker_secret_can_be_pasted_without_copying_it_out(cx: &mut gpui::TestAppContext) {
    let (p, cx) = setup(cx, Source::Secret { verb: "Send" }, "");
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(" secret ".into()));
    cx.simulate_keystrokes(&shortcut("v"));
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), " secret ");
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("unchanged".into()));
    cx.simulate_keystrokes(&format!("{} {} {}", shortcut("a"), shortcut("c"), shortcut("x")));
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("unchanged"));
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), " secret ");
}

/// In a workspace with several folders, an extra folder's file is found by typing its folder name as a path prefix
/// (`repoB/src/lib.rs`), and that file comes first, not the first folder's file with the same relative path.
#[gpui::test]
fn an_extra_folders_file_is_found_by_folder_and_path(cx: &mut gpui::TestAppContext) {
    let (p, cx) = setup(cx, Source::paths(""), "");
    let sep = crate::workbench::ROOT_SEP;
    p.update(cx, |p, cx| p.set_paths(0, vec!["src/lib.rs".into(), format!("repoB{sep}src/lib.rs"), format!("repoB{sep}README.md")], false, cx));
    p.update(cx, |p, cx| p.set_query("repoB/src/lib.rs".into(), cx));
    let rows = p.read_with(cx, |p, _| p.rows.iter().map(|r| (r.label.to_string(), r.detail.as_ref().map(|d| d.to_string()))).collect::<Vec<_>>());
    assert_eq!(rows.first(), Some(&("lib.rs".to_string(), Some(format!("repoB{sep}src")))), "{rows:?}");
    assert!(!rows.iter().any(|(_, d)| d.as_deref() == Some("src")), "the first folder's lib.rs does not match repoB/…: {rows:?}");
}

/// The first folder's files are found by its name as a prefix too, and only them; the highlight stays on the file name.
#[gpui::test]
fn the_first_folders_file_is_found_by_folder_and_path(cx: &mut gpui::TestAppContext) {
    let (p, cx) = setup(cx, Source::paths(""), "");
    let sep = crate::workbench::ROOT_SEP;
    p.update(cx, |p, cx| {
        p.set_first_folder(Some("repoA".into()));
        p.set_paths(0, vec!["src/lib.rs".into(), format!("repoB{sep}src/lib.rs")], false, cx);
        p.set_query("repoA/src/lib.rs".into(), cx);
    });
    let rows =
        p.read_with(cx, |p, _| p.rows.iter().map(|r| (r.label.to_string(), r.detail.as_ref().map(|d| d.to_string()), r.hits.clone())).collect::<Vec<_>>());
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].0.as_str(), rows[0].1.as_deref()), ("lib.rs", Some("src")));
    assert_eq!(rows[0].2, vec![0, 1, 2, 3, 4, 5], "the whole file name is highlighted");
}

/// A typo in the middle of a file query can be corrected without deleting its suffix.
#[gpui::test]
fn picker_query_edits_at_the_caret_and_keeps_results_in_sync(cx: &mut gpui::TestAppContext) {
    let (p, cx) = setup(cx, Source::paths(""), "main.rs");
    p.update(cx, |p, cx| p.set_paths(0, vec!["main.rs".into(), "main.ts".into()], false, cx));
    cx.simulate_keystrokes("left left delete");
    cx.simulate_input("t");
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "main.ts");
    assert_eq!(p.read_with(cx, |p, _| p.labels()), ["main.ts"]);
    cx.simulate_keystrokes("home right delete");
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "min.ts");
    cx.simulate_keystrokes("end backspace");
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "min.t");
}

#[gpui::test]
fn picker_shift_selection_replaces_copies_and_cuts_only_the_selected_part(cx: &mut gpui::TestAppContext) {
    let (p, cx) = setup(cx, Source::Dirs { items: Vec::new(), status: "".into() }, "/srv/İstanbul/🦀");
    cx.simulate_keystrokes("shift-left");
    p.update_in(cx, |p, window, cx| {
        let selection = p.selected_text_range(false, window, cx).unwrap();
        assert_eq!(selection.range, 14..16, "emoji occupies two UTF-16 units");
        assert!(selection.reversed, "Shift-Left leaves the caret before its anchor");
    });
    cx.simulate_keystrokes(&shortcut("c"));
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("🦀"));
    cx.simulate_keystrokes(&shortcut("x"));
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "/srv/İstanbul/");
    cx.simulate_keystrokes("home shift-right shift-right shift-right shift-right");
    cx.simulate_input("/opt");
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "/opt/İstanbul/");
    cx.simulate_keystrokes("end shift-home right");
    p.read_with(cx, |p, _| {
        assert_eq!(p.cursor, p.query.len(), "Right collapses selection to its end");
        assert!(p.query_selection().is_empty());
    });
}

/// UTF-16 replacement and composition use the requested range, including selections
/// inside the query. They must not first clear an unrelated active selection.
#[gpui::test]
fn picker_ime_replaces_a_middle_range_and_reports_its_selection(cx: &mut gpui::TestAppContext) {
    let (p, cx) = setup(cx, Source::Dirs { items: Vec::new(), status: "".into() }, "a🦀z");
    p.update_in(cx, |p, window, cx| {
        p.replace_and_mark_text_in_range(Some(1..3), "İ中", Some(1..1), window, cx);
        assert_eq!(p.query(), "aİ中z");
        assert_eq!(p.marked_text_range(window, cx), Some(1..3));
        assert_eq!(p.selected_text_range(false, window, cx).unwrap().range, 2..2);
        p.replace_text_in_range(None, "🦀", window, cx);
        assert_eq!(p.query(), "a🦀z");
        assert_eq!(p.selected_text_range(false, window, cx).unwrap().range, 3..3);
        assert_eq!(p.marked_text_range(window, cx), None);
        // An OS range landing inside a surrogate pair is rounded to a valid UTF-8 boundary.
        let mut actual = None;
        assert_eq!(p.text_for_range(2..3, &mut actual, window, cx).as_deref(), Some("🦀"));
        assert_eq!(actual, Some(1..3));
        p.select_all(&PickSelectAll, window, cx);
        p.replace_text_in_range(Some(0..1), "b", window, cx);
        assert_eq!(p.query(), "b🦀z", "an explicit platform range takes precedence over the active selection");
    });
}

#[gpui::test]
fn moving_the_picker_caret_does_not_emit_query_or_preview_events(cx: &mut gpui::TestAppContext) {
    let (p, cx) = setup(cx, Source::Dirs { items: Vec::new(), status: "".into() }, "/srv/project");
    let emitted = std::rc::Rc::new(std::cell::Cell::new(0));
    let counter = emitted.clone();
    let _subscription = cx.update(|_, cx| cx.subscribe(&p, move |_, _: &PickerEvent, _| counter.set(counter.get() + 1)));
    cx.simulate_keystrokes("home right shift-end left end");
    assert_eq!(emitted.get(), 0, "navigation alone must not requery a remote host or change a preview");
    cx.simulate_input("x");
    assert!(emitted.get() > 0, "editing still updates results and live preview");
}

#[gpui::test]
fn a_partial_secret_selection_stays_masked_and_cannot_be_copied_or_cut(cx: &mut gpui::TestAppContext) {
    let (p, cx) = setup(cx, Source::Secret { verb: "Send" }, "İ🦀secret");
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("untouched".into()));
    cx.simulate_keystrokes("home shift-right shift-right");
    cx.simulate_keystrokes(&format!("{} {}", shortcut("c"), shortcut("x")));
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("untouched"));
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "İ🦀secret");
    assert_eq!(p.read_with(cx, |p, _| p.shown_query()), "••••••••");
    cx.simulate_input("x");
    assert_eq!(p.read_with(cx, |p, _| p.query.clone()), "xsecret");
    assert_eq!(p.read_with(cx, |p, _| p.shown_query()), "•••••••");
}
