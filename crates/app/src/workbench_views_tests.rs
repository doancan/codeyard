//! Moving views between side bars, and a live terminal into a window of its own and back.

use super::*;

#[gpui::test]
fn a_view_moves_to_the_secondary_side_bar_and_opens_there_when_asked_for(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.show_scm(&ShowScm, window, cx);
        wb.move_view_to_secondary(cx);
        assert_eq!((wb.chrome.panel, wb.chrome.secondary), (None, Some(Panel::Scm)));
        wb.show_explorer(&ShowExplorer, window, cx);
        wb.place_views();
        assert!(wb.view_shown(Panel::Explorer) && wb.view_shown(Panel::Scm), "both side bars show a view");
        // Its activity bar icon or command opens it in the secondary side bar; the primary keeps its view.
        wb.chrome.secondary = None;
        wb.show_scm(&ShowScm, window, cx);
        wb.place_views();
        assert_eq!((wb.chrome.panel, wb.chrome.secondary), (Some(Panel::Explorer), Some(Panel::Scm)));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("secondary-side-bar").is_some(), "the secondary side bar is drawn");
    wb.update(cx, |wb, cx| {
        wb.move_view_to_primary(cx);
        assert_eq!((wb.chrome.panel, wb.chrome.secondary), (Some(Panel::Scm), None));
        assert!(wb.chrome.in_secondary.is_empty());
    });
}

#[test]
fn a_damaged_saved_arrangement_restores_what_it_can() {
    assert_eq!(views_glue::parse_secondary("scm,nope,,scm,search"), vec![Panel::Scm, Panel::Search]);
    assert!(views_glue::parse_secondary("").is_empty());
}

#[gpui::test]
fn a_terminal_keeps_its_process_in_its_own_window_and_comes_back_when_that_closes(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(vcx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    vcx.run_until_parked();
    let term = wb.read_with(vcx, |wb, _| wb.terminals.last().cloned()).expect("a terminal");
    let pty = vcx.update(|_, cx| term.read(cx).pty_handle().map(|(_, id)| id));
    wb.update_in(vcx, |wb, window, cx| wb.detach_terminal(window, cx));
    vcx.run_until_parked();
    let handle = wb.read_with(vcx, |wb, _| {
        assert!(!wb.terminals.contains(&term), "the panel let go of it");
        assert_eq!(wb.chrome.detached.len(), 1);
        wb.chrome.detached[0].window
    });
    // The same terminal, the same process, in the new window.
    assert_eq!(vcx.update(|_, cx| term.read(cx).pty_handle().map(|(_, id)| id)), pty);
    assert!(!vcx.update(|_, cx| term.read(cx).has_exited()));
    let mut detached = gpui::VisualTestContext::from_window(handle.into(), vcx);
    assert!(detached.simulate_close(), "the window closes");
    wb.read_with(vcx, |wb, _| {
        assert!(wb.terminals.contains(&term), "closing the window returns the terminal");
        assert!(wb.chrome.detached.is_empty());
    });
    assert_eq!(vcx.update(|_, cx| term.read(cx).pty_handle().map(|(_, id)| id)), pty, "still the same process");
    assert!(!vcx.update(|_, cx| term.read(cx).has_exited()), "and still running");
}

#[gpui::test]
fn detached_terminals_end_with_their_connection(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(vcx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    vcx.run_until_parked();
    let term = wb.read_with(vcx, |wb, _| wb.terminals.last().cloned()).unwrap();
    wb.update_in(vcx, |wb, window, cx| wb.detach_terminal(window, cx));
    vcx.run_until_parked();
    wb.update(vcx, |wb, cx| wb.close_all_terminals(cx));
    assert!(wb.read_with(vcx, |wb, _| wb.chrome.detached.is_empty() && !wb.terminals.contains(&term)));
}

#[gpui::test]
fn a_terminal_moves_into_the_editor_area_and_back_with_the_same_process(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(vcx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    vcx.run_until_parked();
    let term = wb.read_with(vcx, |wb, _| wb.terminals.last().cloned()).expect("a terminal");
    let pty = vcx.update(|_, cx| term.read(cx).pty_handle().map(|(_, id)| id));
    assert!(pty.is_some(), "the shell runs");
    wb.update_in(vcx, |wb, window, cx| wb.move_terminal_into_editor(window, cx));
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(!wb.terminals.contains(&term), "the panel let go of it");
        assert_eq!(wb.editor_terminals_for_test(), vec![term.clone()], "one group holds it");
        assert_eq!(wb.front_editor_terminal(), Some(term.clone()), "in front of the focused group");
    });
    assert!(vcx.debug_bounds("tab-terminal-0-0").is_some(), "its tab is drawn");
    assert!(vcx.update(|window, cx| term.read(cx).focus_handle(cx).contains_focused(window, cx)), "the keyboard is in it");
    assert_eq!(vcx.update(|_, cx| term.read(cx).pty_handle().map(|(_, id)| id)), pty, "the same process");
    wb.update_in(vcx, |wb, window, cx| wb.move_terminal_back_to_panel(window, cx));
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(wb.terminals.contains(&term) && wb.editor_terminals_for_test().is_empty(), "back in the panel only");
        assert!(wb.chrome.panel_open && wb.front_editor_terminal().is_none());
    });
    assert_eq!(vcx.update(|_, cx| term.read(cx).pty_handle().map(|(_, id)| id)), pty, "still the same process");
    assert!(!vcx.update(|_, cx| term.read(cx).has_exited()), "and still running");
}

#[gpui::test]
fn closing_a_terminal_tab_returns_it_to_the_panel_still_running(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(vcx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    vcx.run_until_parked();
    let term = wb.read_with(vcx, |wb, _| wb.terminals.last().cloned()).expect("a terminal");
    let pty = vcx.update(|_, cx| term.read(cx).pty_handle().map(|(_, id)| id));
    wb.update_in(vcx, |wb, window, cx| {
        wb.move_terminal_into_editor(window, cx);
        wb.close_editor(&CloseEditor, window, cx);
    });
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(wb.terminals.contains(&term), "closing the tab returns the terminal");
        assert!(wb.editor_terminals_for_test().is_empty());
    });
    assert_eq!(vcx.update(|_, cx| term.read(cx).pty_handle().map(|(_, id)| id)), pty);
    assert!(!vcx.update(|_, cx| term.read(cx).has_exited()), "it was not ended");
    wb.update_in(vcx, |wb, window, cx| {
        wb.move_terminal_back_to_panel(window, cx);
        assert!(wb.notice.is_some(), "no terminal is in front: the command says so");
    });
}

#[gpui::test]
fn changing_or_closing_the_folder_returns_editor_area_terminals_to_the_panel(cx: &mut gpui::TestAppContext) {
    let base = std::env::temp_dir().join(format!("codeyard-term-tabs-{}", std::process::id()));
    let (one, two) = (base.join("one"), base.join("two"));
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    let (wb, vcx) = crate::qa_workbench_tests::setup(cx);
    wb.update(vcx, |wb, cx| wb.set_root(one.to_string_lossy().into_owned(), cx));
    vcx.run_until_parked();
    wb.update_in(vcx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    vcx.run_until_parked();
    let term = wb.read_with(vcx, |wb, _| wb.terminals.last().cloned()).unwrap();
    let back_in_panel =
        |vcx: &mut gpui::VisualTestContext| wb.read_with(vcx, |wb, _| wb.terminals.contains(&term) && wb.editor_terminals_for_test().is_empty());
    wb.update_in(vcx, |wb, window, cx| wb.move_terminal_into_editor(window, cx));
    wb.update(vcx, |wb, cx| wb.set_root(two.to_string_lossy().into_owned(), cx));
    vcx.run_until_parked();
    assert!(back_in_panel(vcx), "another folder replaced the tabs, not the terminal");
    wb.update_in(vcx, |wb, window, cx| wb.move_terminal_into_editor(window, cx));
    wb.update(vcx, |wb, cx| wb.close_folder(cx));
    vcx.run_until_parked();
    assert!(back_in_panel(vcx), "closing the folder did not end it either");
    assert!(!vcx.update(|_, cx| term.read(cx).has_exited()));
    std::fs::remove_dir_all(&base).ok();
}

#[gpui::test]
fn terminals_in_the_editor_area_end_with_their_connection(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(vcx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    vcx.run_until_parked();
    let term = wb.read_with(vcx, |wb, _| wb.terminals.last().cloned()).unwrap();
    wb.update_in(vcx, |wb, window, cx| wb.move_terminal_into_editor(window, cx));
    vcx.run_until_parked();
    wb.update(vcx, |wb, cx| wb.close_all_terminals(cx));
    wb.read_with(vcx, |wb, _| {
        assert!(wb.editor_terminals_for_test().is_empty() && !wb.terminals.contains(&term));
        assert!(wb.front_editor_terminal().is_none(), "no tab is left for it");
    });
}
