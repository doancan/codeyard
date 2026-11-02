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
