//! Editors in windows of their own: one document shared with the tabs, closing and returning, restarts.

use super::*;
use crate::qa_workbench_tests::{fresh, open, open_in, setup, setup_persistent};
use gpui::VisualTestContext;

fn window_of(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> (gpui::WindowHandle<EditorWindow>, usize) {
    wb.read_with(cx, |wb, _| {
        assert_eq!(wb.chrome.editor_windows.len(), 1, "one editor window");
        (wb.chrome.editor_windows[0].window, wb.chrome.editor_windows[0].slot)
    })
}

fn window_count(cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| cx.windows().len())
}

/// AC3: a copy in its own window is the same document: typing there changes the tab's text and dirty state, one undo
/// in the tab takes it back, and each view keeps its own cursor.
#[gpui::test]
fn a_copied_editor_shares_text_undo_and_dirty_state_with_its_tab(cx: &mut gpui::TestAppContext) {
    let dir = fresh("edwin-shared");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let (wb, vcx) = setup(cx);
    open(&wb, &dir, vcx);
    open_in(&wb, &file, vcx);
    let ed = wb.read_with(vcx, |wb, _| wb.active_editor().cloned()).expect("an editor");
    wb.update_in(vcx, |wb, window, cx| wb.editor_into_new_window(true, window, cx));
    vcx.run_until_parked();
    let (handle, slot) = window_of(&wb, vcx);
    assert!(slot >= MAX_GROUPS, "a view after the groups' views");
    wb.read_with(vcx, |wb, _| {
        assert_eq!(wb.tabs.editors, vec![ed.clone()], "the tab stays");
        assert_eq!(wb.all_editors().len(), 1, "no second copy of the document");
    });
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.simulate_keystrokes("down down");
    win.simulate_input("new");
    win.run_until_parked();
    assert_eq!(ed.read_with(vcx, |e, _| e.text()), "one\ntwo\nnewthree\n");
    assert!(wb.read_with(vcx, |wb, cx| wb.any_dirty(cx)), "the workbench sees the change");
    let (tab_view, window_view) = ed.update(vcx, |e, _| (e.view_for_slot(0), e.view_for_slot(slot)));
    assert_ne!(tab_view.line, window_view.line, "each view keeps its own cursor");
    // One undo history: undoing in the tab takes back what was typed in the window.
    wb.update_in(vcx, |wb, window, cx| wb.focus_group_key(wb.tabs.current_key(), window, cx));
    vcx.simulate_keystrokes("cmd-z");
    vcx.run_until_parked();
    assert_eq!(ed.read_with(vcx, |e, _| e.text()), "one\ntwo\nthree\n");
    assert!(!ed.read_with(vcx, |e, _| e.is_dirty()));
    // ⌘W in the window closes that view only; the tab keeps the document, and no question is asked.
    let windows = window_count(vcx);
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.simulate_keystrokes("cmd-w");
    vcx.run_until_parked();
    assert!(!vcx.has_pending_prompt());
    wb.read_with(vcx, |wb, _| assert!(wb.chrome.editor_windows.is_empty() && wb.tabs.editors == vec![ed.clone()]));
    assert_eq!(window_count(vcx), windows - 1, "its window closed");
    assert!(!ed.read_with(vcx, |e, _| e.has_slot(slot)), "its view is gone");
}

/// Moving takes the tab away; closing the window brings the same editor back as a tab, unsaved changes and all.
#[gpui::test]
fn closing_a_moved_editors_window_returns_it_as_a_tab_with_its_changes(cx: &mut gpui::TestAppContext) {
    let dir = fresh("edwin-return");
    let file = dir.join("a.txt");
    std::fs::write(&file, "alpha\n").unwrap();
    let (wb, vcx) = setup(cx);
    open(&wb, &dir, vcx);
    open_in(&wb, &file, vcx);
    let ed = wb.read_with(vcx, |wb, _| wb.active_editor().cloned()).expect("an editor");
    wb.update_in(vcx, |wb, window, cx| wb.editor_into_new_window(false, window, cx));
    vcx.run_until_parked();
    let (handle, slot) = window_of(&wb, vcx);
    wb.read_with(vcx, |wb, _| {
        assert!(wb.tabs.editors.is_empty(), "the tab moved");
        assert_eq!(wb.all_editors(), vec![ed.clone()], "the workbench still owns the document");
    });
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.simulate_input("x");
    assert!(win.simulate_close(), "the window closes");
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(wb.chrome.editor_windows.is_empty());
        assert_eq!(wb.tabs.editors, vec![ed.clone()], "back as a tab of the focused group");
        assert_eq!(wb.active_editor(), Some(&ed));
    });
    assert!(ed.read_with(vcx, |e, _| e.is_dirty() && e.text() == "xalpha\n"), "nothing was lost");
    assert!(!ed.read_with(vcx, |e, _| e.has_slot(slot)));
}

/// AC5: ⌘W on the last view of a changed file asks like closing its last tab; Cancel keeps it, Don't Save closes it.
#[gpui::test]
fn closing_the_last_view_of_a_changed_file_in_its_window_asks_first(cx: &mut gpui::TestAppContext) {
    let dir = fresh("edwin-ask");
    let file = dir.join("b.txt");
    std::fs::write(&file, "beta\n").unwrap();
    let (wb, vcx) = setup(cx);
    open(&wb, &dir, vcx);
    open_in(&wb, &file, vcx);
    wb.update_in(vcx, |wb, window, cx| wb.editor_into_new_window(false, window, cx));
    vcx.run_until_parked();
    let (handle, _) = window_of(&wb, vcx);
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.simulate_input("y");
    win.simulate_keystrokes("cmd-w");
    win.run_until_parked();
    assert!(vcx.has_pending_prompt(), "asks to save");
    vcx.simulate_prompt_answer("Cancel");
    vcx.run_until_parked();
    assert_eq!(wb.read_with(vcx, |wb, _| wb.chrome.editor_windows.len()), 1, "Cancel keeps it");
    let windows = window_count(vcx);
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.simulate_keystrokes("cmd-w");
    win.run_until_parked();
    vcx.simulate_prompt_answer("Don't Save");
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(wb.chrome.editor_windows.is_empty() && wb.all_editors().is_empty(), "closed");
        assert!(wb.nav.closed.iter().any(|c| c.0.ends_with("b.txt")), "offered by Reopen Closed Editor");
    });
    assert_eq!(window_count(vcx), windows - 1);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "beta\n", "nothing was written");
}

/// Opening the file again while it is only in its window shows the same document in a tab (no second copy); closing
/// the folder closes the window with the tabs.
#[gpui::test]
fn reopening_shares_the_windowed_document_and_closing_the_folder_closes_its_window(cx: &mut gpui::TestAppContext) {
    let dir = fresh("edwin-reopen");
    let file = dir.join("c.txt");
    std::fs::write(&file, "gamma\n").unwrap();
    let (wb, vcx) = setup(cx);
    open(&wb, &dir, vcx);
    open_in(&wb, &file, vcx);
    let ed = wb.read_with(vcx, |wb, _| wb.active_editor().cloned()).expect("an editor");
    let windows = window_count(vcx);
    wb.update_in(vcx, |wb, window, cx| wb.editor_into_new_window(false, window, cx));
    vcx.run_until_parked();
    open_in(&wb, &file, vcx);
    wb.read_with(vcx, |wb, _| {
        assert_eq!(wb.tabs.editors, vec![ed.clone()], "the same editor is the tab");
        assert_eq!(wb.chrome.editor_windows.len(), 1, "and still in its window");
    });
    wb.update(vcx, |wb, cx| wb.close_folder(cx));
    vcx.run_until_parked();
    assert!(wb.read_with(vcx, |wb, _| wb.chrome.editor_windows.is_empty()));
    assert_eq!(window_count(vcx), windows, "its window closed with the folder");
}

/// Go to definition inside the same file, asked in the editor's window, moves that window's view (not the tab's).
#[gpui::test]
fn a_location_in_the_same_file_is_revealed_in_the_editors_own_window(cx: &mut gpui::TestAppContext) {
    let dir = fresh("edwin-reveal");
    let file = dir.join("d.txt");
    std::fs::write(&file, "1\n2\n3\n4\n5\n").unwrap();
    let (wb, vcx) = setup(cx);
    open(&wb, &dir, vcx);
    open_in(&wb, &file, vcx);
    let ed = wb.read_with(vcx, |wb, _| wb.active_editor().cloned()).expect("an editor");
    wb.update_in(vcx, |wb, window, cx| wb.editor_into_new_window(true, window, cx));
    vcx.run_until_parked();
    let (handle, slot) = window_of(&wb, vcx);
    vcx.update(|_, cx| handle.update(cx, |_, window, _| window.activate_window()).unwrap());
    vcx.run_until_parked();
    let path = ed.read_with(vcx, |e, _| e.path.clone());
    assert!(wb.update(vcx, |wb, cx| wb.reveal_in_editor_window(&ed, &path, 3, 0, cx)));
    vcx.run_until_parked();
    let (tab_view, window_view) = ed.update(vcx, |e, _| (e.view_for_slot(0), e.view_for_slot(slot)));
    assert_eq!((tab_view.line, window_view.line), (0, 3), "only the window's view moved");
    assert!(!wb.update(vcx, |wb, cx| wb.reveal_in_editor_window(&ed, "/elsewhere.txt", 0, 0, cx)), "another file opens in a tab");
}

/// AC6: after a restart the window comes back on its file (the tab it was moved out of does not), on a display.
#[gpui::test]
fn a_restart_brings_editor_windows_back_on_a_display(cx: &mut gpui::TestAppContext) {
    let home = fresh("edwin-restart-home");
    let dir = fresh("edwin-restart-project");
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    std::fs::write(dir.join("b.txt"), "b\n").unwrap();
    let (wb, vcx) = setup_persistent(cx, &home);
    open(&wb, &dir, vcx);
    open_in(&wb, &dir.join("a.txt"), vcx);
    open_in(&wb, &dir.join("b.txt"), vcx);
    wb.update_in(vcx, |wb, window, cx| wb.editor_into_new_window(false, window, cx));
    vcx.run_until_parked();
    let wb = crate::qa_workbench_tests::restart_with(wb, vcx, &home, crate::settings::Startup::Continue);
    let start = std::time::Instant::now();
    while wb.read_with(vcx, |wb, _| wb.chrome.editor_windows.is_empty()) && start.elapsed() < std::time::Duration::from_secs(10) {
        vcx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let (handle, _) = window_of(&wb, vcx);
    wb.read_with(vcx, |wb, cx| {
        let name = |e: &Entity<EditorView>| ui::file_name(&e.read(cx).path).to_string();
        assert_eq!(name(&wb.chrome.editor_windows[0].editor), "b.txt");
        assert_eq!(wb.tabs.editors.iter().map(name).collect::<Vec<_>>(), vec!["a.txt"], "b.txt is only in its window");
    });
    let display = vcx.update(|_, cx| cx.primary_display().unwrap().bounds());
    let at = vcx.update(|_, cx| handle.update(cx, |_, window, _| window.bounds()).unwrap());
    assert!(display.contains(&at.origin) && at.size.width <= display.size.width, "{at:?} on {display:?}");
}

#[test]
fn saved_editor_windows_are_versioned_and_bounded() {
    let ok = r#"{"version":1,"windows":[{"path":"src/a.rs","only_here":true,"view":[3,1,0],"at":[10,20,800,600]},{"path":"b.txt"}]}"#;
    let saved = SavedEditorWindows::parse(ok).expect("valid");
    assert_eq!(saved.only_here().collect::<Vec<_>>(), vec!["src/a.rs"]);
    let many = format!(r#"{{"version":1,"windows":[{}]}}"#, [r#"{"path":"a"}"#; MAX_DETACHED + 1].join(","));
    for bad in ["", "{", r#"{"version":2,"windows":[]}"#, r#"{"version":1,"windows":[{"path":""}]}"#, r#"{"version":1,"windows":[{"path":"a","x":1}]}"#, &many]
    {
        assert!(SavedEditorWindows::parse(bad).is_none(), "{bad}");
    }
    assert!(SavedEditorWindows::parse(&" ".repeat(MAX_TEXT + 1)).is_none());
}
