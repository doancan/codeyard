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

/// A dirty buffer whose file was then rewritten by someone else, with the watcher's notice delivered.
type Conflicted<'a> = (gpui::Entity<Workbench>, &'a mut VisualTestContext, PathBuf, PathBuf, gpui::Entity<crate::editor::EditorView>);

fn conflicted<'a>(cx: &'a mut TestAppContext, name: &str) -> Conflicted<'a> {
    let (data, dir) = (home(name), project(name));
    let (wb, cx) = setup_persistent(cx, &data);
    open(&wb, &dir, cx);
    let file = dir.join("src/main.rs");
    open_in(&wb, &file, cx);
    cx.simulate_input("// mine\n");
    std::fs::write(&file, "fn main() { println!(\"theirs\"); }\n").unwrap();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.on_disk_change(proto::ChangeKind::Modified, cx));
    cx.run_until_parked();
    assert!(ed.read_with(cx, |e, _| e.disk_state().0), "the watcher should have raised the conflict");
    (wb, cx, data, file, ed)
}

#[gpui::test]
fn a_forced_save_keeps_the_version_it_overwrites(cx: &mut TestAppContext) {
    let (wb, cx, data, file, ed) = conflicted(cx, "force");
    ed.update(cx, |e, _| e.expire_conflict_grace());
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "// mine\nfn main() {}\n");
    let path = ed.read_with(cx, |e, _| e.path.clone());
    let copies = crate::history::list(&crate::history::dir_for(&data, "", &path));
    let kept: Vec<String> = copies.iter().map(|c| crate::history::read(c).unwrap()).collect();
    assert!(kept.contains(&"fn main() { println!(\"theirs\"); }\n".to_string()), "the other version must be recoverable: {kept:?}");
    assert!(wb.read_with(cx, |wb, _| wb.notice_for_test()).is_some());
}

#[gpui::test]
fn a_save_right_after_the_conflict_appears_does_not_overwrite(cx: &mut TestAppContext) {
    let (_wb, cx, data, file, ed) = conflicted(cx, "rapid");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fn main() { println!(\"theirs\"); }\n");
    assert!(ed.read_with(cx, |e, _| e.disk_state().0), "the banner stays until the user decides");
    let path = ed.read_with(cx, |e, _| e.path.clone());
    assert!(crate::history::list(&crate::history::dir_for(&data, "", &path)).is_empty());
}

#[gpui::test]
fn a_damaged_state_database_is_reported_set_aside_and_replaced(cx: &mut TestAppContext) {
    let dir = home("corrupt-db");
    let db = dir.join("state.db");
    std::fs::write(&db, "this is not a database, just text that happens to be long enough to look like a header").unwrap();
    globals(cx);
    let path = db.clone();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::new_at(Some(path), None, window, cx));
    let (error, text) = wb.read_with(cx, |wb, _| wb.notice_for_test()).expect("the user must be told");
    assert!(error && text.contains("state.db.corrupt-"), "{text}");
    let aside = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("state.db.corrupt-")).count();
    assert_eq!(aside, 1);
    assert!(wb.read_with(cx, |wb, _| wb.has_state_for_test()));
    drop(wb);
    assert!(state::State::open(&db).is_ok(), "a second open must find a usable database");
}

#[gpui::test]
fn a_database_from_a_newer_version_is_left_in_place_but_reported(cx: &mut TestAppContext) {
    let dir = home("newer-db");
    let db = dir.join("state.db");
    drop(state::State::open(&db).unwrap());
    // `user_version` lives at byte 60 of the file header.
    let mut bytes = std::fs::read(&db).unwrap();
    bytes[60..64].copy_from_slice(&9999u32.to_be_bytes());
    std::fs::write(&db, bytes).unwrap();
    globals(cx);
    let path = db.clone();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::new_at(Some(path), None, window, cx));
    let (error, text) = wb.read_with(cx, |wb, _| wb.notice_for_test()).expect("the user must be told");
    assert!(error && text.contains("newer Codeyard"), "{text}");
    assert!(std::fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("corrupt")));
    assert!(!wb.read_with(cx, |wb, _| wb.has_state_for_test()));
}

#[gpui::test]
fn save_as_keeps_the_old_tab_when_typing_continued_during_the_write(cx: &mut TestAppContext) {
    let (dir, data) = (project("saveas"), home("saveas"));
    // A plain-text target: a Rust file would start a language server, whose "not installed" notice depends on the machine.
    let (wb, cx) = setup_persistent(cx, &data);
    open(&wb, &dir, cx);
    let main = dir.join("src/main.rs");
    open_in(&wb, &main, cx);
    cx.simulate_input("// before\n");
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let (from, copy) = (main.to_string_lossy().into_owned(), dir.join("src/copy.txt").to_string_lossy().into_owned());
    wb.update_in(cx, |wb, window, cx| wb.save_as_for_test(from.clone(), copy, window, cx));
    // The write has been handed to the background but has not finished: this is typed in the meantime.
    ed.update(cx, |e, cx| e.apply_buffer_edits(|_| vec![(0..0, "// during\n".to_string())], cx));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("src/copy.txt")).unwrap(), "// before\nfn main() {}\n");
    assert!(wb.read_with(cx, |wb, cx| wb.open_paths(cx)).contains(&from), "the tab holding the late edit must stay");
    assert!(ed.read_with(cx, |e, _| e.text().starts_with("// during\n")));
    assert!(wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test());
    assert!(notice.as_ref().is_some_and(|(_, m)| m.contains("kept typing")), "{notice:?}");
}

#[gpui::test]
fn save_as_closes_the_old_tab_when_nothing_changed_meanwhile(cx: &mut TestAppContext) {
    let (dir, data) = (project("saveas2"), home("saveas2"));
    let (wb, cx) = setup_persistent(cx, &data);
    open(&wb, &dir, cx);
    let main = dir.join("src/main.rs");
    open_in(&wb, &main, cx);
    cx.simulate_input("// before\n");
    let (from, copy) = (main.to_string_lossy().into_owned(), dir.join("src/copy.rs").to_string_lossy().into_owned());
    wb.update_in(cx, |wb, window, cx| wb.save_as_for_test(from.clone(), copy, window, cx));
    cx.run_until_parked();
    assert!(!wb.read_with(cx, |wb, cx| wb.open_paths(cx)).contains(&from));
    assert!(!wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
}
