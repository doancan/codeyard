//! Data-loss guards: what hot exit cannot back up, forced saves after a conflict, a damaged state database and
//! Save As while typing continues.

use std::path::PathBuf;

use gpui::{Focusable, TestAppContext, VisualTestContext};

use crate::qa_workbench_tests::{fresh, globals, open, open_in, project, setup_persistent};
use crate::workbench::Workbench;

fn home(name: &str) -> PathBuf {
    fresh(&format!("home-{name}"))
}

/// Type into the active editor and let the 1 s backup timer fire.
fn type_and_settle(cx: &mut VisualTestContext, text: &str) {
    cx.simulate_input(text);
    cx.executor().advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
}

#[gpui::test]
fn quit_asks_when_a_dirty_file_outside_the_folder_cannot_be_backed_up(cx: &mut TestAppContext) {
    let (data, dir) = (home("outside"), project("outside"));
    let stray = fresh("outside-stray").join("settings.jsonc");
    std::fs::write(&stray, "{}\n").unwrap();
    let (wb, cx) = setup_persistent(cx, &data);
    open(&wb, &dir, cx);
    open_in(&wb, &stray, cx);
    type_and_settle(cx, "// edited\n");
    assert!(wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
    assert!(!wb.update(cx, |wb, cx| wb.prepare_quit(cx)), "the edit has no backup, so quitting must ask");
}

#[gpui::test]
fn quit_stays_silent_when_every_dirty_file_is_backed_up(cx: &mut TestAppContext) {
    let (data, dir) = (home("inside"), project("inside"));
    let (wb, cx) = setup_persistent(cx, &data);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    type_and_settle(cx, "// edited\n");
    assert!(wb.update(cx, |wb, cx| wb.prepare_quit(cx)));
}

#[gpui::test]
fn quit_asks_without_a_state_database(cx: &mut TestAppContext) {
    let dir = project("nostate");
    globals(cx);
    let (wb, cx) = cx.add_window_view(|window, cx| Workbench::with_state(None, None, window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    type_and_settle(cx, "// edited\n");
    assert!(!wb.update(cx, |wb, cx| wb.prepare_quit(cx)));
}

#[gpui::test]
fn quit_asks_when_the_backup_cannot_be_written(cx: &mut TestAppContext) {
    let (data, dir) = (home("unwritable"), project("unwritable"));
    let (wb, cx) = setup_persistent(cx, &data);
    // A regular file where the data folder should be: neither `backups/` nor its blobs can be created.
    let blocker = data.join("blocker");
    std::fs::write(&blocker, "").unwrap();
    wb.update(cx, |wb, _| wb.set_data_dir(blocker.join("data")));
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    type_and_settle(cx, "// edited\n");
    assert!(!wb.update(cx, |wb, cx| wb.prepare_quit(cx)));
}

#[gpui::test]
fn switching_folders_keeps_an_unprotected_edit_open(cx: &mut TestAppContext) {
    let (data, dir, other) = (home("switch"), project("switch"), project("switch-other"));
    let stray = fresh("switch-stray").join("scratch.txt");
    std::fs::write(&stray, "x\n").unwrap();
    let (wb, cx) = setup_persistent(cx, &data);
    open(&wb, &dir, cx);
    open_in(&wb, &stray, cx);
    type_and_settle(cx, "keep me");
    let other = other.to_string_lossy().into_owned();
    wb.update(cx, |wb, cx| wb.set_root(other, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.root_for_test(cx)).as_deref(), dir.to_str(), "the folder must not change under a dirty buffer");
    assert!(wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
    assert!(wb.read_with(cx, |wb, _| wb.notice_for_test()).is_some_and(|(error, _)| error));
}
