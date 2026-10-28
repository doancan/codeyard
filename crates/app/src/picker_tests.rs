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
