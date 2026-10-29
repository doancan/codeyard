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
