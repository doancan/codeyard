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

#[gpui::test]
fn a_terminal_tab_dragged_to_another_group_moves_there_with_the_same_process(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.new_terminal(&NewTerminal, window, cx);
        wb.move_terminal_into_editor(window, cx);
    });
    vcx.run_until_parked();
    let term = wb.read_with(vcx, |wb, _| wb.editor_terminals_for_test()[0].clone());
    let pty = vcx.update(|_, cx| term.read(cx).pty_handle().map(|(_, id)| id));
    let from = wb.read_with(vcx, |wb, _| wb.tabs.current_key());
    wb.update_in(vcx, |wb, window, cx| wb.split_editor(&SplitEditor, window, cx));
    vcx.run_until_parked();
    let to = wb.read_with(vcx, |wb, _| wb.tabs.current_key());
    assert_ne!(from, to);
    wb.update_in(vcx, |wb, window, cx| wb.drop_terminal_tab(term.clone(), from, to, window, cx));
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert_eq!(wb.tabs.current_key(), to);
        assert_eq!(wb.tabs.group(to.id).unwrap().terminals, vec![term.clone()], "the destination group holds it");
        assert_eq!(wb.editor_terminals_for_test(), vec![term.clone()], "and only that group");
        assert_eq!(wb.front_editor_terminal(), Some(term.clone()));
    });
    assert_eq!(vcx.update(|_, cx| term.read(cx).pty_handle().map(|(_, id)| id)), pty, "the same process");
    // A drag that started in a group that is gone (or no longer holds the tab) is stale: nothing moves.
    wb.update_in(vcx, |wb, window, cx| wb.drop_terminal_tab(term.clone(), from, to, window, cx));
    wb.read_with(vcx, |wb, _| assert_eq!(wb.editor_terminals_for_test(), vec![term.clone()]));
}

fn restart_with_saved(wb: Entity<Workbench>, cx: &mut gpui::VisualTestContext, home: &std::path::Path, saved: Option<&str>) -> Entity<Workbench> {
    assert!(wb.update(cx, |wb, cx| wb.prepare_quit(cx)));
    let ws = wb.read_with(cx, |wb, _| wb.persist.workspace_id).expect("a workspace");
    drop(wb);
    let db = home.join("state.db");
    if let Some(text) = saved {
        state::State::open(&db).unwrap().set(&format!("moved_terminals:{ws}"), text).unwrap();
    }
    let data = home.to_path_buf();
    let new = cx.update(|window, cx| {
        window.replace_root(cx, |window, cx| {
            let mut w = Workbench::with_state(state::State::open(&db).ok(), None, window, cx);
            w.set_data_dir(data);
            w
        })
    });
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().startup = crate::settings::Startup::Continue);
    new.update_in(cx, |w, window, cx| {
        window.focus(&w.focus_handle(cx));
        w.restore_last(window, cx);
    });
    cx.run_until_parked();
    new
}

fn wait(cx: &mut gpui::VisualTestContext, mut done: impl FnMut(&mut gpui::VisualTestContext) -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(10 * crate::test_sshd::timeout_scale()) {
        cx.run_until_parked();
        if done(cx) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

/// AC6: after a restart the terminal that was in the editor area is a NEW shell in its group, with its title, colour
/// and a notice; the command the old shell ran is not run again.
#[gpui::test]
fn a_restart_starts_a_new_shell_in_the_group_without_replaying_anything(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("moved-term-restore-home");
    let dir = crate::qa_workbench_tests::fresh("moved-term-restore-project");
    let marker = dir.join("marker");
    let (wb, vcx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
    crate::qa_workbench_tests::open(&wb, &dir, vcx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.new_terminal(&NewTerminal, window, cx);
        wb.move_terminal_into_editor(window, cx);
    });
    let old = wb.read_with(vcx, |wb, _| wb.editor_terminals_for_test()[0].clone());
    old.update(vcx, |t, _| {
        t.set_fixed_title("build".into());
        t.color = Some("success".into());
        // The default shell: `echo … > "…"` means the same in a POSIX shell, PowerShell and cmd (Windows), and `\r` is
        // the Enter key, which a Windows console needs (a Unix terminal turns it into `\n`).
        t.send(&format!("echo x > \"{}\"\r", marker.display()));
    });
    assert!(wait(vcx, |_| marker.exists()), "the first shell ran the command");
    std::fs::remove_file(&marker).unwrap();

    let wb = restart_with_saved(wb, vcx, &home, None);
    assert!(wait(vcx, |vcx| wb.read_with(vcx, |wb, _| wb.editor_terminal_count() == 1)), "a terminal is back in the editor area");
    let term = wb.read_with(vcx, |wb, _| {
        assert!(wb.terminals.is_empty(), "not in the panel");
        assert_eq!(wb.tabs.terminals.len(), 1, "in its group");
        wb.editor_terminals_for_test()[0].clone()
    });
    assert_ne!(term, old);
    let (title, color) = vcx.update(|_, cx| (term.read(cx).title.to_string(), term.read(cx).color.clone()));
    assert_eq!((title.as_str(), color.as_deref()), ("build", Some("success")));
    assert!(wait(vcx, |vcx| vcx.update(|_, cx| term.read(cx).pty_handle().is_some())), "a new shell runs");
    let text = vcx.update(|_, cx| term.read(cx).scrollback_text() + &term.read(cx).screen_text());
    assert!(text.contains("Previous session ended; started a new shell"), "the notice is shown: {text}");
    // The new shell runs what it is given in order: once a command typed now has run, anything replayed would have too.
    let ready = dir.join("ready");
    term.update(vcx, |t, _| t.send(&format!("echo x > \"{}\"\r", ready.display())));
    assert!(wait(vcx, |_| ready.exists()), "the new shell takes input");
    assert!(!marker.exists(), "no command of the previous session ran again");
    wb.update(vcx, |wb, cx| wb.close_all_terminals(cx));
}

/// One restart brings back both kinds: the panel's terminal in the panel and the moved one in its group. A task's
/// output terminal moved into the editor area has no shell, so it is not started again as one.
#[gpui::test]
fn a_restart_restores_panel_and_moved_terminals_but_not_task_output(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("term-both-home");
    let dir = crate::qa_workbench_tests::fresh("term-both-project");
    let (wb, vcx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
    crate::qa_workbench_tests::open(&wb, &dir, vcx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.new_terminal(&NewTerminal, window, cx);
        wb.move_terminal_into_editor(window, cx);
        let host = wb.remote.host.clone();
        let task = cx.new(|cx| crate::terminal::TerminalView::output_only(host, "build task".into(), cx));
        wb.terminals.push(task);
        wb.active_terminal = wb.terminals.len() - 1;
        wb.move_terminal_into_editor(window, cx);
        wb.new_terminal(&NewTerminal, window, cx);
    });
    wb.read_with(vcx, |wb, _| assert_eq!((wb.terminals.len(), wb.editor_terminal_count()), (1, 2)));

    let wb = restart_with_saved(wb, vcx, &home, None);
    let back = |vcx: &mut gpui::VisualTestContext| wb.read_with(vcx, |wb, _| (wb.terminals.len(), wb.editor_terminal_count()));
    assert!(wait(vcx, |vcx| back(vcx) == (1, 1)), "one terminal in the panel and one in the editor area: {:?}", back(vcx));
    let moved = wb.read_with(vcx, |wb, _| wb.editor_terminals_for_test()[0].clone());
    assert_ne!(vcx.update(|_, cx| moved.read(cx).title.to_string()), "build task", "the task output did not come back");
    assert!(wait(vcx, |vcx| vcx.update(|_, cx| moved.read(cx).pty_handle().is_some())), "the moved terminal is a new shell");
    wb.update(vcx, |wb, cx| wb.close_all_terminals(cx));
}

/// AC6: a damaged saved arrangement restores nothing and startup goes on; a group that is gone and a window off every
/// display fall back to the focused group and a window clamped onto a display.
#[gpui::test]
fn a_damaged_or_out_of_range_saved_arrangement_falls_back_safely(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("moved-term-restore-bad-home");
    let dir = crate::qa_workbench_tests::fresh("moved-term-restore-bad-project");
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    let (wb, vcx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
    crate::qa_workbench_tests::open(&wb, &dir, vcx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("a.txt"), vcx);

    let wb = restart_with_saved(wb, vcx, &home, Some("{\"version\":1,\"terminals\":[{\"place\""));
    assert!(wait(vcx, |vcx| wb.read_with(vcx, |wb, _| wb.active_editor().is_some())), "startup restored the files");
    wb.read_with(vcx, |wb, _| assert!(wb.terminals.is_empty() && wb.editor_terminal_count() == 0 && wb.chrome.detached.is_empty()));

    let saved = r#"{"version":1,"terminals":[{"place":{"group":9}},{"place":{"window":[-90000,-90000,99999,99999]}}]}"#;
    let wb = restart_with_saved(wb, vcx, &home, Some(saved));
    assert!(wait(vcx, |vcx| wb.read_with(vcx, |wb, _| wb.chrome.detached.len() == 1)));
    let handle = wb.read_with(vcx, |wb, _| {
        assert_eq!(wb.tabs.terminals.len(), 1, "group 9 is gone: the focused group took its terminal");
        wb.chrome.detached[0].window
    });
    let display = vcx.update(|_, cx| cx.primary_display().unwrap().bounds());
    let at = vcx.update(|_, cx| handle.update(cx, |_, window, _| window.bounds()).unwrap());
    assert!(display.contains(&at.origin) && at.size.width <= display.size.width && at.size.height <= display.size.height, "{at:?} on {display:?}");
    wb.update(vcx, |wb, cx| wb.close_all_terminals(cx));
}

#[test]
fn a_saved_arrangement_is_versioned_and_bounded() {
    use term_tabs_glue::restore::{Place, SavedTerminals};
    let ok = r#"{"version":1,"terminals":[{"place":{"group":1},"title":"build","color":"info","cwd":"/tmp"},{"place":{"window":null}}]}"#;
    assert_eq!(SavedTerminals::parse(ok).map(|s| s.places()), Some(vec![Place::Group(1), Place::Window(None)]));
    for bad in [
        "",
        "not json",
        r#"{"version":2,"terminals":[]}"#,
        r#"{"version":1,"terminals":[],"extra":1}"#,
        r#"{"version":1,"terminals":[{"place":{"group":1},"color":"chartreuse"}]}"#,
        r#"{"version":1,"terminals":[{"place":{"group":300}}]}"#,
        r#"{"version":1,"terminals":[{"place":{"tab":1}}]}"#,
    ] {
        assert!(SavedTerminals::parse(bad).is_none(), "{bad}");
    }
    let many = format!(r#"{{"version":1,"terminals":[{}]}}"#, [r#"{"place":{"group":0}}"#; MAX_TERMINALS + 1].join(","));
    assert!(SavedTerminals::parse(&many).is_none(), "more terminals than can be open");
    let long = format!(r#"{{"version":1,"terminals":[{{"place":{{"group":0}},"title":"{}"}}]}}"#, "x".repeat(300));
    assert!(SavedTerminals::parse(&long).is_none());
    assert!(SavedTerminals::parse(&" ".repeat(70 * 1024)).is_none());
}

#[test]
fn a_restored_window_lands_on_an_attached_display() {
    use gpui::{Bounds, point, size};
    use term_tabs_glue::restore::place_window;
    let primary = Bounds::new(point(px(0.), px(0.)), size(px(1440.), px(900.)));
    let second = Bounds::new(point(px(1440.), px(0.)), size(px(1920.), px(1080.)));
    // On a display that is still attached: kept.
    let kept = Bounds::new(point(px(1600.), px(100.)), size(px(800.), px(500.)));
    assert_eq!(place_window(Some([1600., 100., 800., 500.]), &[primary, second]), Some(kept));
    // Its display was removed: onto the primary one, whole.
    let moved = place_window(Some([1600., 100., 800., 500.]), &[primary]).unwrap();
    assert_eq!(moved, Bounds::new(point(px(640.), px(100.)), size(px(800.), px(500.))));
    // Negative coordinates partly on screen: pulled in.
    assert_eq!(place_window(Some([-300., -50., 800., 500.]), &[primary]).unwrap().origin, point(px(0.), px(0.)));
    // Larger than the display: shrunk to it.
    assert_eq!(place_window(Some([0., 0., 5000., 5000.]), &[primary]).unwrap().size, primary.size);
    // Unknown or damaged bounds: the default size, centred on the primary display.
    for bad in [None, Some([f32::NAN, 0., 800., 500.]), Some([0., 0., -1., 500.]), Some([f32::INFINITY; 4])] {
        let at = place_window(bad, &[primary]).unwrap();
        assert_eq!(at.size, views_glue::DETACHED_SIZE);
        assert!(primary.contains(&at.origin));
    }
    assert_eq!(place_window(Some([0., 0., 800., 500.]), &[]), None, "no display known");
}

/// A workbench that moved a terminal and an editor into windows of their own is released once its window closes: those
/// windows only point back at it weakly, and they close with it. (A workbench left alive would keep its editors,
/// language servers and terminals after the window is gone, since the app keeps running on macOS.)
#[gpui::test]
fn a_workbench_is_released_when_its_window_closes_with_detached_windows_open(cx: &mut gpui::TestAppContext) {
    let dir = crate::qa_workbench_tests::project("release-detached");
    let (wb, vcx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, vcx);
    crate::qa_workbench_tests::open(&wb, &dir.join("README.md"), vcx);
    wb.update_in(vcx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    vcx.run_until_parked();
    wb.update_in(vcx, |wb, window, cx| {
        wb.detach_terminal(window, cx);
        wb.editor_into_new_window(false, window, cx);
    });
    vcx.run_until_parked();
    let windows = vcx.cx.windows().len();
    assert_eq!(windows, 3, "the main window, the terminal's and the editor's");
    let weak = wb.downgrade();
    drop(wb);
    vcx.update(|window, _| window.remove_window());
    vcx.run_until_parked();
    assert!(weak.upgrade().is_none(), "the workbench is released with its window");
    assert_eq!(vcx.cx.windows().len(), 0, "its terminal and editor windows closed with it");
}

/// Closing the detached windows first, then the main window, releases it too.
#[gpui::test]
fn a_workbench_is_released_after_its_detached_windows_and_then_its_window_close(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(vcx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    vcx.run_until_parked();
    wb.update_in(vcx, |wb, window, cx| wb.detach_terminal(window, cx));
    vcx.run_until_parked();
    let handle = wb.read_with(vcx, |wb, _| wb.chrome.detached[0].window);
    let mut detached = gpui::VisualTestContext::from_window(handle.into(), vcx);
    assert!(detached.simulate_close(), "the terminal's window closes");
    vcx.run_until_parked();
    let weak = wb.downgrade();
    drop(wb);
    vcx.update(|window, _| window.remove_window());
    vcx.run_until_parked();
    assert!(weak.upgrade().is_none(), "the workbench is released with its window");
}
