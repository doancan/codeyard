//! Local History keeps no secret files and asks before keeping a server's files (#272): the acceptance criteria, through
//! real saves in a window.

use super::*;
use crate::qa_integration_tests::wait_ticking;
use crate::qa_workbench_tests::{open, project, setup};
use gpui::{TestAppContext, VisualTestContext};
use std::path::{Path, PathBuf};

fn data_dir(name: &str) -> PathBuf {
    let data = std::env::temp_dir().join(format!("codeyard-history-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    data
}

fn start<'a>(cx: &'a mut TestAppContext, name: &str) -> (Entity<Workbench>, &'a mut VisualTestContext, PathBuf, PathBuf) {
    let dir = project(name);
    let data = data_dir(name);
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| wb.set_data_dir(data.clone()));
    open(&wb, &dir, cx);
    (wb, cx, dir, data)
}

/// Type a line at the top of `file` and save it.
fn save(wb: &Entity<Workbench>, cx: &mut VisualTestContext, file: &Path, line: &str) {
    open(wb, file, cx);
    cx.simulate_input(&format!("{line}\n"));
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
}

fn copies(data: &Path, host: &str, file: &Path) -> usize {
    crate::history::list(&crate::history::dir_for(data, host, &crate::ui::local_path(file))).len()
}

/// Saving `main.rs` after `other` and seeing its copy proves the earlier save had its chance to write too.
fn control(wb: &Entity<Workbench>, cx: &mut VisualTestContext, dir: &Path, data: &Path) {
    let main = dir.join("src/main.rs");
    save(wb, cx, &main, "// control");
    wait_ticking(cx, "the control copy", |_| copies(data, "", &main) == 1);
}

fn set_settings(cx: &mut VisualTestContext, change: impl FnOnce(&mut crate::settings::Settings)) {
    cx.update(|_, cx| change(cx.global_mut::<crate::settings::Settings>()));
}

fn connect(wb: &Entity<Workbench>, cx: &mut VisualTestContext, alias: &str) {
    wb.update(cx, |wb, _| wb.remote.conn = Conn::Connected { alias: alias.into() });
}

#[gpui::test]
fn ac1_a_saved_env_file_leaves_no_copy(cx: &mut TestAppContext) {
    let (wb, cx, dir, data) = start(cx, "history-env");
    let env = dir.join(".env");
    std::fs::write(&env, "TOKEN=1\n").unwrap();
    save(&wb, cx, &env, "TOKEN=2");
    control(&wb, cx, &dir, &data);
    assert_eq!(copies(&data, "", &env), 0);
    assert_eq!(std::fs::read_dir(data.join("history")).unwrap().count(), 1, "only the control file has a folder");
}

#[gpui::test]
fn ac2_a_user_pattern_keeps_a_file_out(cx: &mut TestAppContext) {
    let (wb, cx, dir, data) = start(cx, "history-pattern");
    set_settings(cx, |s| s.local_history_exclude = vec!["*.secret.yaml".into()]);
    let file = dir.join("db.secret.yaml");
    std::fs::write(&file, "password: a\n").unwrap();
    save(&wb, cx, &file, "password: b");
    control(&wb, cx, &dir, &data);
    assert_eq!(copies(&data, "", &file), 0);
}

#[gpui::test]
fn copies_of_a_file_that_is_excluded_later_are_deleted_on_its_next_save(cx: &mut TestAppContext) {
    let (wb, cx, dir, data) = start(cx, "history-forget");
    let file = dir.join("db.secret.yaml");
    std::fs::write(&file, "password: a\n").unwrap();
    save(&wb, cx, &file, "password: b");
    wait_ticking(cx, "the copy", |_| copies(&data, "", &file) == 1);
    set_settings(cx, |s| s.local_history_exclude = vec!["*.secret.yaml".into()]);
    save(&wb, cx, &file, "password: c");
    assert_eq!(copies(&data, "", &file), 0, "the earlier copy is gone");
    assert!(!crate::history::dir_for(&data, "", &crate::ui::local_path(&file)).exists());
}

#[gpui::test]
fn disabled_history_writes_nothing(cx: &mut TestAppContext) {
    let (wb, cx, dir, data) = start(cx, "history-off");
    set_settings(cx, |s| s.local_history = false);
    let main = dir.join("src/main.rs");
    save(&wb, cx, &main, "// one");
    cx.run_until_parked();
    assert!(!data.join("history").exists());
}

#[gpui::test]
fn the_overwritten_version_of_an_excluded_file_is_not_kept_either(cx: &mut TestAppContext) {
    let (wb, cx, dir, data) = start(cx, "history-overwrite");
    let (env, main) = (dir.join(".env"), dir.join("src/main.rs"));
    std::fs::write(&env, "TOKEN=1\n").unwrap();
    for file in [&env, &main] {
        open(&wb, file, cx);
        let ed = wb.read_with(cx, |wb, _| wb.active_editor().cloned().unwrap());
        wb.update(cx, |wb, cx| wb.keep_overwritten(&ed, "on disk\n", cx));
    }
    assert_eq!(copies(&data, "", &env), 0);
    assert_eq!(copies(&data, "", &main), 1, "an ordinary file's overwritten version is kept");
}

#[gpui::test]
fn ac3_a_server_is_asked_once_and_nothing_is_written_before_the_answer(cx: &mut TestAppContext) {
    let (wb, cx, dir, data) = start(cx, "history-ask");
    let main = dir.join("src/main.rs");
    connect(&wb, cx, "srv");
    save(&wb, cx, &main, "// one");
    assert!(cx.has_pending_prompt(), "the first save on a server asks");
    assert_eq!(copies(&data, "srv", &main), 0, "nothing is written before the answer");
    cx.simulate_prompt_answer("Keep");
    wait_ticking(cx, "the copy of the save that asked", |_| copies(&data, "srv", &main) == 1);
    save(&wb, cx, &main, "// two");
    assert!(!cx.has_pending_prompt(), "the answer is remembered");
    wait_ticking(cx, "the second copy", |_| copies(&data, "srv", &main) == 2);
    connect(&wb, cx, "other");
    save(&wb, cx, &main, "// three");
    assert!(cx.has_pending_prompt(), "another server does not share the approval");
    cx.simulate_prompt_answer("Don't Keep");
    cx.run_until_parked();
    assert_eq!(copies(&data, "other", &main), 0);
    save(&wb, cx, &main, "// four");
    assert!(!cx.has_pending_prompt(), "Don't Keep is remembered too");
    cx.run_until_parked();
    assert_eq!(copies(&data, "other", &main), 0, "and writes nothing for that server afterwards");
}

#[gpui::test]
fn a_server_policy_of_off_or_keep_never_asks(cx: &mut TestAppContext) {
    let (wb, cx, dir, data) = start(cx, "history-policy");
    let main = dir.join("src/main.rs");
    connect(&wb, cx, "srv");
    set_settings(cx, |s| s.local_history_remote = crate::history::Remote::Off);
    save(&wb, cx, &main, "// one");
    assert!(!cx.has_pending_prompt());
    cx.run_until_parked();
    assert_eq!(copies(&data, "srv", &main), 0);
    set_settings(cx, |s| s.local_history_remote = crate::history::Remote::Keep);
    save(&wb, cx, &main, "// two");
    assert!(!cx.has_pending_prompt());
    wait_ticking(cx, "the copy", |_| copies(&data, "srv", &main) == 1);
}

#[gpui::test]
fn a_secret_file_on_a_server_is_not_asked_about(cx: &mut TestAppContext) {
    let (wb, cx, dir, data) = start(cx, "history-remote-env");
    let env = dir.join(".env");
    std::fs::write(&env, "TOKEN=1\n").unwrap();
    connect(&wb, cx, "srv");
    save(&wb, cx, &env, "TOKEN=2");
    assert!(!cx.has_pending_prompt(), "a file Local History never keeps is not worth a question");
    cx.run_until_parked();
    assert_eq!(copies(&data, "srv", &env), 0);
}

#[gpui::test]
fn ac4_deleting_local_history_removes_one_file_or_all(cx: &mut TestAppContext) {
    let (wb, cx, dir, data) = start(cx, "history-delete");
    let (main, lib) = (dir.join("src/main.rs"), dir.join("src/lib.rs"));
    save(&wb, cx, &main, "// one");
    save(&wb, cx, &lib, "// two");
    wait_ticking(cx, "both copies", |_| copies(&data, "", &main) == 1 && copies(&data, "", &lib) == 1);
    wb.update_in(cx, |wb, window, cx| wb.delete_local_history(&DeleteLocalHistory, window, cx));
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(copies(&data, "", &lib), 1, "Cancel deletes nothing");
    wb.update_in(cx, |wb, window, cx| wb.delete_local_history(&DeleteLocalHistory, window, cx));
    cx.simulate_prompt_answer("Delete History of “lib.rs”");
    wait_ticking(cx, "the file's copies gone", |_| copies(&data, "", &lib) == 0);
    assert_eq!(copies(&data, "", &main), 1, "another file's copies stay");
    wb.update_in(cx, |wb, window, cx| wb.delete_local_history(&DeleteLocalHistory, window, cx));
    cx.simulate_prompt_answer("Delete All Local History");
    wait_ticking(cx, "the history folder gone", |_| !data.join("history").exists());
}

/// A server's file over a real SSH connection (the throwaway `sshd`): the first save asks, nothing is written before the
/// answer, *Keep* writes the copy on this computer under the server's name, and a secret file on the server is never copied.
#[gpui::test]
fn a_real_ssh_server_is_asked_once_and_secrets_on_it_are_not_copied(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("history-ssh") else { return };
    let _en = crate::i18n::LangGuard::set("en");
    let dir = project("history-ssh");
    let data = data_dir("history-ssh");
    std::fs::write(dir.join(".env"), "TOKEN=1\n").unwrap();
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| wb.set_data_dir(data.clone()));
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 30, "connected", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    open(&wb, &dir, cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.remote_host_label_for_test()), Some(format!("ssh://{}", crate::test_sshd::ALIAS)));
    let alias = crate::test_sshd::ALIAS;
    let main = dir.join("src/main.rs");
    // A secret file on the server: no question and no copy.
    save(&wb, cx, &dir.join(".env"), "TOKEN=2");
    assert!(!cx.has_pending_prompt(), "a file Local History never keeps is not worth a question");
    // An ordinary file: the question, nothing before the answer, then the copy.
    save(&wb, cx, &main, "// one");
    assert!(cx.has_pending_prompt(), "the first save on a server asks");
    assert_eq!(copies(&data, alias, &main), 0);
    cx.simulate_prompt_answer("Keep");
    wait_ticking(cx, "the copy", |_| copies(&data, alias, &main) == 1);
    save(&wb, cx, &main, "// two");
    assert!(!cx.has_pending_prompt(), "the answer is remembered");
    wait_ticking(cx, "the second copy", |_| copies(&data, alias, &main) == 2);
    assert_eq!(copies(&data, alias, &dir.join(".env")), 0);
}
