//! Terminals saved at exit come back as new shells at the next start: same folder, title and order, never the old
//! commands; their history only with `terminal.persistentScrollback`.

use super::*;
use crate::qa_integration_tests::wait_ticking;

fn with_history(cx: &mut gpui::VisualTestContext, lines: usize) {
    cx.update(|_, cx| {
        let mut s = cx.global::<Settings>().clone();
        s.terminal_persistent_scrollback = lines;
        s.restore_session = true;
        cx.set_global(s);
    });
}

fn terminal_text(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext, ix: usize) -> String {
    wb.read_with(cx, |wb, cx| wb.terminals.get(ix).map(|t| t.read(cx).scrollback_text()).unwrap_or_default())
}

#[gpui::test]
fn terminals_come_back_as_new_shells_in_their_folders_without_running_anything_again(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("term-restore-home");
    let dir = crate::qa_workbench_tests::project("term-restore");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::create_dir_all(dir.join("gone")).unwrap();
    let sub = dir.join("sub").to_string_lossy().into_owned();
    let gone = dir.join("gone").to_string_lossy().into_owned();
    {
        let (wb, cx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
        with_history(cx, 50);
        crate::qa_workbench_tests::open(&wb, &dir, cx);
        for cwd in [&sub, &gone] {
            wb.update_in(cx, |wb, window, cx| wb.new_terminal_launch(Some(cwd.clone()), crate::terminal::Launch::default(), window, cx));
        }
        wait_ticking(cx, "both shells running", |cx| wb.read_with(cx, |wb, cx| wb.terminals.iter().all(|t| t.read(cx).pty_handle().is_some())));
        wb.update(cx, |wb, cx| {
            wb.terminals[0].update(cx, |t, _| t.set_fixed_title("build".into()));
            wb.terminals[0].update(cx, |t, _| t.send("echo restore-marker; touch ran-once\n"));
        });
        wait_ticking(cx, "the command ran", |_| dir.join("sub/ran-once").exists());
        wait_ticking(cx, "its output", |cx| terminal_text(&wb, cx, 0).contains("restore-marker"));
        wb.update(cx, |wb, cx| wb.persist_session(cx));
    }
    std::fs::remove_file(dir.join("sub/ran-once")).unwrap();
    std::fs::remove_dir_all(dir.join("gone")).unwrap();

    let (wb, cx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
    with_history(cx, 50);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    wait_ticking(cx, "the terminals restored", |cx| {
        wb.read_with(cx, |wb, cx| wb.terminals.len() == 2 && wb.terminals.iter().all(|t| t.read(cx).pty_handle().is_some()))
    });
    let (title, cwd) = wb.read_with(cx, |wb, cx| {
        let t = wb.terminals[0].read(cx);
        (t.title.to_string(), t.start_dir())
    });
    assert_eq!((title.as_str(), cwd.as_deref()), ("build", Some(sub.as_str())));
    let shown = terminal_text(&wb, cx, 0);
    assert!(shown.contains("restore-marker") && shown.contains("Previous session ended; started a new shell"), "{shown}");
    // The missing folder falls back to the workspace folder, and says so.
    assert!(terminal_text(&wb, cx, 1).contains("no longer exists"), "{}", terminal_text(&wb, cx, 1));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.terminals[1].read(cx).start_dir()), Some(dir.to_string_lossy().into_owned()));
    // Nothing was sent to the new shells: the old command does not run again.
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(!dir.join("sub/ran-once").exists(), "a restored terminal never replays commands");
}

#[gpui::test]
fn without_persistent_scrollback_no_history_is_saved(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("term-restore-nohistory-home");
    let dir = crate::qa_workbench_tests::project("term-restore-nohistory");
    let (wb, cx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
    with_history(cx, 0);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.new_terminal_launch(None, crate::terminal::Launch::default(), window, cx));
    wait_ticking(cx, "the shell running", |cx| wb.read_with(cx, |wb, cx| wb.terminals.first().is_some_and(|t| t.read(cx).pty_handle().is_some())));
    wb.update(cx, |wb, cx| wb.terminals[0].update(cx, |t, _| t.send("echo private-output\n")));
    wait_ticking(cx, "its output", |cx| terminal_text(&wb, cx, 0).contains("private-output"));
    wb.update(cx, |wb, cx| wb.persist_session(cx));
    let saved = wb.read_with(cx, |wb, _| wb.persist.state.as_ref().and_then(|s| s.get(&saved_terminals_key(wb.persist.workspace_id.unwrap())))).unwrap();
    assert!(saved.contains("\"version\":1") && !saved.contains("private-output"), "{saved}");
}

/// Opening a terminal saves it at once: a killed or crashed app still brings it back.
#[gpui::test]
fn a_terminal_is_saved_as_soon_as_it_opens(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("term-restore-early-home");
    let dir = crate::qa_workbench_tests::project("term-restore-early");
    let (wb, cx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
    with_history(cx, 0);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.new_terminal_launch(None, crate::terminal::Launch::default(), window, cx));
    // No session save happened: only opening the terminal.
    let saved = wb.read_with(cx, |wb, _| wb.persist.state.as_ref().and_then(|s| s.get(&saved_terminals_key(wb.persist.workspace_id.unwrap())))).unwrap();
    let parsed = crate::terminal_session::SavedTerminals::parse(&saved, 0).unwrap();
    assert_eq!(parsed.terminals.len(), 1, "{saved}");
}

/// `--terminal` (or a new terminal asked for by hand) while the folder's terminals are being restored: they still come
/// back, and the request waits for them instead of making the restore give up.
#[gpui::test]
fn a_terminal_asked_for_during_the_restore_does_not_drop_it(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("term-restore-asked-home");
    let dir = crate::qa_workbench_tests::project("term-restore-asked");
    {
        let (wb, cx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
        with_history(cx, 0);
        crate::qa_workbench_tests::open(&wb, &dir, cx);
        wb.update_in(cx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
        wait_ticking(cx, "the shell running", |cx| wb.read_with(cx, |wb, cx| wb.terminals.first().is_some_and(|t| t.read(cx).pty_handle().is_some())));
        wb.update(cx, |wb, cx| {
            wb.terminals[0].update(cx, |t, _| t.set_fixed_title("build".into()));
            wb.persist_session(cx);
        });
    }

    let (wb, cx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
    with_history(cx, 0);
    let p = dir.to_string_lossy().into_owned();
    // As `codeyard FOLDER --terminal` does: the terminal is asked for right after the folder starts opening.
    wb.update_in(cx, |wb, window, cx| {
        wb.open_path(&p, window, cx);
        wb.open_terminal(window, cx);
    });
    wait_ticking(cx, "the saved terminal back", |cx| {
        wb.read_with(cx, |wb, cx| wb.terminals.first().is_some_and(|t| t.read(cx).title.as_ref() == "build" && t.read(cx).pty_handle().is_some()))
    });
    wb.read_with(cx, |wb, _| {
        assert_eq!(wb.terminals.len(), 1, "the restored terminal answers the request; no second shell");
        assert!(wb.chrome.panel_open && wb.chrome.bottom == Bottom::Terminal, "the terminal is shown");
        assert!(!wb.persist.terminal_wanted);
    });
}

/// With nothing to restore, a terminal asked for during the (empty) restore still opens once it is done.
#[gpui::test]
fn a_terminal_asked_for_during_an_empty_restore_opens(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("term-restore-empty-home");
    let dir = crate::qa_workbench_tests::project("term-restore-empty");
    let (wb, cx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
    with_history(cx, 0);
    let p = dir.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| {
        wb.open_path(&p, window, cx);
        wb.open_terminal(window, cx);
    });
    wait_ticking(cx, "a new terminal", |cx| wb.read_with(cx, |wb, cx| wb.terminals.len() == 1 && wb.terminals[0].read(cx).pty_handle().is_some()));
    wb.update(cx, |wb, cx| wb.close_all_terminals(cx));
}
