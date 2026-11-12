//! The buttons of a notification: each kind of action does its work, a toast carries the action of its entry, and an action is
//! only data (it can be cloned and compared, so nothing is held alive by the history).

use gpui::{Entity, TestAppContext, VisualTestContext};

#[cfg(unix)]
use super::super::locks::{lock_dir, lock_file};
use super::model::{NoticeAction, RetryKind};
use super::*;
use crate::qa_workbench_tests::{fresh, open, project};

fn setup(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    crate::qa_workbench_tests::setup(cx)
}

fn press(wb: &Entity<Workbench>, cx: &mut VisualTestContext, serial: u64) {
    wb.update_in(cx, |wb, window, cx| wb.run_notification_action(serial, window, cx));
    cx.run_until_parked();
}

fn newest_serial(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> u64 {
    wb.read_with(cx, |wb, _| wb.notices.log.newest_first().next().expect("an entry").serial)
}

#[test]
fn reveal_shows_the_nearest_folder_that_exists() {
    let dir = fresh("reveal-target");
    let file = dir.join("a.txt");
    std::fs::write(&file, "x").unwrap();
    assert_eq!(reveal_target(&file), file, "a file that exists is selected itself");
    assert_eq!(reveal_target(&dir.join("missing.txt")), dir, "a file that is not there: its folder");
    assert_eq!(reveal_target(&dir.join("gone").join("deeper").join("x.db")), dir, "folders that are not there: the nearest one that is");
    assert_eq!(reveal_target(Path::new("")), PathBuf::from(""), "nothing to reveal stays as it is");
}

#[gpui::test]
fn the_card_carries_the_action_of_its_entry_and_a_flash_carries_none(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let target = PathBuf::from("/some/where");
    wb.update(cx, |wb, _| {
        wb.notify_with(Notification::new(Severity::Error, NoticeSource::Files, "could not write").action(NoticeAction::Reveal(target.clone())))
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.toast_action().map(|(_, a)| a)), Some(NoticeAction::Reveal(target)));
    wb.update(cx, |wb, _| wb.notify(NoticeSource::Files, Notice::Error("plain".into())));
    assert_eq!(wb.read_with(cx, |wb, _| wb.toast_action()), None, "an entry without an action shows no button");
    wb.update(cx, |wb, _| wb.flash("Copied".into()));
    assert_eq!(wb.read_with(cx, |wb, _| wb.toast_action()), None);
}

#[gpui::test]
fn open_file_opens_the_file_in_a_tab(cx: &mut TestAppContext) {
    let dir = project("action-open");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let file = dir.join("src/main.rs");
    wb.update(cx, |wb, _| {
        wb.notify_with(Notification::new(Severity::Warning, NoticeSource::Files, "look here").action(NoticeAction::OpenFile(file.clone(), Some(1))))
    });
    let serial = newest_serial(&wb, cx);
    press(&wb, cx, serial);
    let open_now = wb.read_with(cx, |wb, cx| wb.open_paths(cx));
    assert!(open_now.iter().any(|p| p.ends_with("src/main.rs")), "{open_now:?}");
}

#[gpui::test]
fn retry_of_a_file_that_is_not_open_says_so(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| {
        wb.notify_with(
            Notification::new(Severity::Error, NoticeSource::Files, "could not save")
                .action(NoticeAction::Retry(RetryKind::SaveFile { host: String::new(), path: PathBuf::from("/not/open.txt") })),
        )
    });
    let serial = newest_serial(&wb, cx);
    press(&wb, cx, serial);
    let note = wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default();
    assert!(note.contains("/not/open.txt"), "{note:?}");
}

#[cfg(unix)]
#[gpui::test]
fn retry_save_succeeds_once_the_folder_is_writable_again(cx: &mut TestAppContext) {
    let dir = project("action-retry-save");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("src/main.rs"), cx);
    let ed = wb.read_with(cx, |w, _| w.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "// edit\n", cx));
    let Some(file_lock) = lock_file(&dir.join("src/main.rs")) else { return };
    let Some(dir_lock) = lock_dir(&dir.join("src")) else { return };
    ed.update(cx, |e, cx| e.save_now(cx));
    cx.run_until_parked();
    assert!(ed.read_with(cx, |e, _| e.is_dirty()), "the failed save left the edit unsaved");
    // Exactly one error: with the path, the cause in plain words, a next step, the raw text as detail and a Retry button.
    let entries: Vec<_> = wb.read_with(cx, |wb, _| wb.notices.log.newest_first().cloned().collect());
    assert_eq!(entries.len(), 1, "{entries:?}");
    let e = &entries[0];
    let path = ed.read_with(cx, |e, _| e.path.clone());
    assert_eq!((e.severity, e.source), (Severity::Error, NoticeSource::Files));
    assert!(e.message.starts_with("Could not save ") && e.message.contains("main.rs"), "{}", e.message);
    assert!(e.message.contains("permission") && e.message.contains("try again"), "{}", e.message);
    assert!(e.detail.as_deref().is_some_and(|d| d.contains("main.rs") && d.to_lowercase().contains("permission denied")), "{:?}", e.detail);
    assert_eq!(e.action, Some(NoticeAction::Retry(RetryKind::SaveFile { host: String::new(), path: PathBuf::from(&path) })));
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|n| n.0), Some(true), "the card shows");
    // Saving again while it still fails does not pile up entries: one row with a counter.
    ed.update(cx, |e, cx| e.save_now(cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| (wb.notices.log.len(), wb.notices.log.newest_first().next().unwrap().count)), (1, 2));
    // The folder is fixed: the button saves, and the failure is gone from the list and from the card.
    drop(dir_lock);
    drop(file_lock);
    let serial = newest_serial(&wb, cx);
    press(&wb, cx, serial);
    assert!(!ed.read_with(cx, |e, _| e.is_dirty()), "the retry saved the file");
    assert!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap().starts_with("// edit"));
    assert!(wb.read_with(cx, |wb, _| wb.notices.log.is_empty()), "the failure is out of date once the file is saved");
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()), None, "and so is its card");
}

/// A save fails, then another program edits the file and the conflict banner comes up: the button must not be the way the other
/// version gets replaced.
#[cfg(unix)]
#[gpui::test]
fn retry_save_never_overwrites_an_edit_made_by_another_program(cx: &mut TestAppContext) {
    let dir = project("action-retry-conflict");
    let file = dir.join("src/main.rs");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &file, cx);
    let ed = wb.read_with(cx, |w, _| w.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "// mine\n", cx));
    {
        let Some(_file_lock) = lock_file(&file) else { return };
        let Some(_dir_lock) = lock_dir(&dir.join("src")) else { return };
        ed.update(cx, |e, cx| e.save_now(cx));
        cx.run_until_parked();
    }
    assert_eq!(wb.read_with(cx, |wb, _| wb.notices.log.len()), 1, "the failed save is in the list");
    std::fs::write(&file, "fn main() { println!(\"theirs\"); }\n").unwrap();
    ed.update(cx, |e, cx| {
        e.on_disk_change(proto::ChangeKind::Modified, cx);
        e.expire_conflict_grace();
    });
    cx.run_until_parked();
    assert!(ed.read_with(cx, |e, _| e.disk_state().0), "the banner is up");
    let serial = newest_serial(&wb, cx);
    press(&wb, cx, serial);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fn main() { println!(\"theirs\"); }\n", "their version is still on disk");
    assert!(ed.read_with(cx, |e, _| e.is_dirty() && e.disk_state().0), "the edit is kept and the banner stays");
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()).as_deref(), Some("The file changed on disk; resolve the conflict first."));
    assert_eq!(wb.read_with(cx, |wb, _| wb.notices.log.len()), 2, "the failed save stays in the list until the file is saved");
}

/// The same path on another machine is another file: its retry must not save the local one.
#[gpui::test]
fn retry_save_of_another_machines_file_does_not_save_the_local_one(cx: &mut TestAppContext) {
    let dir = project("action-retry-host");
    let file = dir.join("src/main.rs");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &file, cx);
    let ed = wb.read_with(cx, |w, _| w.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "// local\n", cx));
    let path = PathBuf::from(ed.read_with(cx, |e, _| e.path.clone()));
    wb.update_in(cx, |wb, window, cx| {
        wb.run_notice_action(NoticeAction::Retry(RetryKind::SaveFile { host: "build-server".into(), path: path.clone() }), window, cx)
    });
    cx.run_until_parked();
    assert!(ed.read_with(cx, |e, _| e.is_dirty()), "nothing was saved");
    assert!(std::fs::read_to_string(&file).unwrap().starts_with("fn main"), "the local file is untouched");
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default().contains("not open any more"));
    // The same retry for this machine does save it.
    wb.update_in(cx, |wb, window, cx| wb.run_notice_action(NoticeAction::Retry(RetryKind::SaveFile { host: String::new(), path }), window, cx));
    cx.run_until_parked();
    assert!(std::fs::read_to_string(&file).unwrap().starts_with("// local"));
}

/// Creating a file in a folder that cannot be written says which path, why, what to do and offers to show the folder.
#[cfg(unix)]
#[gpui::test]
fn a_failed_create_names_the_path_and_offers_the_folder(cx: &mut TestAppContext) {
    let dir = project("action-create");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let Some(src_lock) = lock_dir(&dir.join("src")) else { return };
    let src = crate::ui::local_path(&dir.join("src"));
    wb.update_in(cx, |wb, window, cx| {
        wb.finish_input(crate::workbench::types::InputPurpose::Create { dir: src.clone(), is_dir: false }, "Çalışma planı.md".into(), window, cx)
    });
    cx.run_until_parked();
    drop(src_lock);
    let entries: Vec<_> = wb.read_with(cx, |wb, _| wb.notices.log.newest_first().cloned().collect());
    assert_eq!(entries.len(), 1, "{entries:?}");
    let e = &entries[0];
    assert_eq!((e.severity, e.source), (Severity::Error, NoticeSource::Files));
    assert!(e.message.starts_with("Could not create ") && e.message.contains("Çalışma planı.md"), "{}", e.message);
    assert!(e.message.contains("permission"), "{}", e.message);
    assert_eq!(e.action, Some(NoticeAction::Reveal(PathBuf::from(format!("{src}/Çalışma planı.md")))));
    assert!(!dir.join("src/Çalışma planı.md").exists());
}

/// A settings file that does not parse: one error, from Settings, with a button that opens the file at the mistake.
#[gpui::test]
fn a_broken_settings_file_offers_to_open_it_at_the_line(cx: &mut TestAppContext) {
    let cfg = fresh("action-broken-settings");
    std::fs::write(cfg.join("settings.jsonc"), "{\n  /* a note\n     over two lines */\n  \"editor.fontSize\": oops\n}\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    for _ in 0..2 {
        wb.update_in(cx, |w, window, cx| w.reload_config_for_test(window, cx));
        cx.run_until_parked();
    }
    let entries: Vec<_> = wb.read_with(cx, |wb, _| wb.notices.log.newest_first().cloned().collect());
    assert_eq!(entries.len(), 1, "{entries:?}");
    let e = &entries[0];
    assert_eq!((e.severity, e.source), (Severity::Error, NoticeSource::Settings));
    assert!(e.count >= 2, "the repeats collapse into one row with a counter: {}", e.count);
    assert!(e.message.contains("line 4 column"), "{}", e.message);
    assert_eq!(e.action, Some(NoticeAction::OpenFile(cfg.join("settings.jsonc"), Some(4))));
    let serial = e.serial;
    press(&wb, cx, serial);
    let open_now = wb.read_with(cx, |wb, cx| wb.open_paths(cx));
    assert!(open_now.iter().any(|p| p.ends_with("settings.jsonc")), "{open_now:?}");
}

#[gpui::test]
fn retry_of_a_setting_writes_it_and_a_damaged_value_reports_itself(cx: &mut TestAppContext) {
    let cfg = fresh("action-retry-setting");
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    let retry = |json: &str, before: Option<&str>| {
        NoticeAction::Retry(RetryKind::WriteSetting {
            path: cfg.join("settings.jsonc"),
            key: "editor.fontSize".into(),
            json: json.into(),
            before: before.map(str::to_string),
        })
    };
    wb.update(cx, |wb, _| wb.notify_with(Notification::new(Severity::Error, NoticeSource::Settings, "could not write").action(retry("17", None))));
    let serial = newest_serial(&wb, cx);
    press(&wb, cx, serial);
    let written = std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap_or_default();
    assert!(written.contains("\"editor.fontSize\": 17"), "{written}");
    wb.update(cx, |wb, _| wb.notify_with(Notification::new(Severity::Error, NoticeSource::Settings, "again").action(retry("{not json", Some("17")))));
    let serial = newest_serial(&wb, cx);
    press(&wb, cx, serial);
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default().contains("Could not retry"));
}

/// `wsl.exe` does not exist off Windows: listing fails, and the user hears why once, with the whole text in the detail.
#[cfg(not(windows))]
#[gpui::test]
fn a_failed_wsl_listing_posts_one_warning_with_the_whole_text(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    for _ in 0..2 {
        wb.update(cx, |wb, cx| wb.list_wsl_for_test(cx));
        cx.run_until_parked();
    }
    let entries: Vec<_> = wb.read_with(cx, |wb, _| wb.notices.log.newest_first().cloned().collect());
    assert_eq!(entries.len(), 1, "{entries:?}");
    let e = &entries[0];
    assert_eq!((e.severity, e.source, e.id.as_str()), (Severity::Warning, NoticeSource::Remote, "remote-list:true"));
    assert!(e.message.starts_with("Could not list the WSL distributions: "), "{}", e.message);
    assert!(e.detail.as_deref().is_some_and(|d| d.contains("WSL is not available")), "{:?}", e.detail);
}

/// Writing a setting into a folder that cannot be written: the path, the cause and a Retry that writes it once the folder is fixed.
#[cfg(unix)]
#[gpui::test]
fn a_failed_settings_write_offers_retry_and_the_retry_clears_it(cx: &mut TestAppContext) {
    let cfg = fresh("action-settings-write");
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    let Some(lock) = lock_dir(&cfg) else { return };
    wb.update_in(cx, |wb, window, cx| wb.write_setting("editor.fontSize", serde_json::json!(17), window, cx));
    cx.run_until_parked();
    drop(lock);
    let before = failures(&wb, cx);
    assert_eq!(before.len(), 1, "{before:?}");
    let e = &before[0];
    assert_eq!((e.severity, e.source), (Severity::Error, NoticeSource::Settings));
    assert!(e.message.starts_with("Could not write ") && e.message.contains("settings.jsonc"), "{}", e.message);
    assert!(e.message.contains("permission") && e.message.contains("try again"), "{}", e.message);
    let want = RetryKind::WriteSetting { path: cfg.join("settings.jsonc"), key: "editor.fontSize".into(), json: "17".into(), before: None };
    assert_eq!(e.action, Some(NoticeAction::Retry(want)));
    let serial = e.serial;
    press(&wb, cx, serial);
    let written = std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap_or_default();
    assert!(written.contains("\"editor.fontSize\": 17"), "{written}");
    assert!(failures(&wb, cx).is_empty(), "the failure is out of date once the setting is written");
}

fn failures(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<model::Entry> {
    wb.read_with(cx, |wb, _| wb.notices.log.newest_first().filter(|e| e.id.starts_with("settings-file-failed:")).cloned().collect())
}

/// Failure on profile A, switch to profile B, Retry: A's value must not land in B (nor anywhere).
#[cfg(unix)]
#[gpui::test]
fn retry_of_a_setting_refuses_when_another_profile_is_active(cx: &mut TestAppContext) {
    let (a, b) = (fresh("action-profile-a"), fresh("action-profile-b"));
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &a);
    let Some(lock) = lock_dir(&a) else { return };
    wb.update_in(cx, |wb, window, cx| wb.write_setting("editor.fontSize", serde_json::json!(17), window, cx));
    cx.run_until_parked();
    drop(lock);
    wb.update(cx, |wb, _| wb.set_config_dir_for_test(b.clone()));
    let serial = failures(&wb, cx)[0].serial;
    press(&wb, cx, serial);
    let note = wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default();
    assert!(note.contains("another profile") && note.contains("Nothing was written"), "{note}");
    for dir in [&a, &b] {
        let text = std::fs::read_to_string(dir.join("settings.jsonc")).unwrap_or_default();
        assert!(!text.contains("editor.fontSize"), "{}: {text}", dir.display());
    }
}

/// The setting was changed in the file since the write failed: the stale value is not written over it.
#[cfg(unix)]
#[gpui::test]
fn retry_of_a_setting_refuses_when_the_value_changed_meanwhile(cx: &mut TestAppContext) {
    let cfg = fresh("action-stale-setting");
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    let Some(lock) = lock_dir(&cfg) else { return };
    wb.update_in(cx, |wb, window, cx| wb.write_setting("editor.fontSize", serde_json::json!(17), window, cx));
    cx.run_until_parked();
    drop(lock);
    std::fs::write(cfg.join("settings.jsonc"), "{ \"editor.fontSize\": 20 }\n").unwrap();
    let serial = failures(&wb, cx)[0].serial;
    press(&wb, cx, serial);
    let note = wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default();
    assert!(note.contains("editor.fontSize was changed in the settings file since"), "{note}");
    assert_eq!(std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap(), "{ \"editor.fontSize\": 20 }\n");
}

/// Saving the settings file by hand in its own tab puts right the failed write: the entry goes away.
#[cfg(unix)]
#[gpui::test]
fn saving_the_settings_file_by_hand_clears_a_failed_write(cx: &mut TestAppContext) {
    let cfg = fresh("action-hand-saved");
    std::fs::write(cfg.join("settings.jsonc"), "{}\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    let Some(lock) = lock_dir(&cfg) else { return };
    wb.update_in(cx, |wb, window, cx| wb.write_setting("editor.fontSize", serde_json::json!(17), window, cx));
    cx.run_until_parked();
    drop(lock);
    assert_eq!(failures(&wb, cx).len(), 1);
    crate::qa_workbench_tests::open_in(&wb, &cfg.join("settings.jsonc"), cx);
    let ed = wb.read_with(cx, |w, _| w.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.insert_at_for_test(1, " \"editor.fontSize\": 17 ", cx));
    ed.update(cx, |e, cx| e.save_now(cx));
    cx.run_until_parked();
    assert!(failures(&wb, cx).is_empty(), "the hand-made save settled it");
}

/// The real profiles: a write fails in profile "work", the default profile becomes active, and Retry must not write into either
/// file; back in "work" the same Retry works.
#[cfg(unix)]
#[gpui::test]
fn retry_of_a_setting_follows_the_real_active_profile(cx: &mut TestAppContext) {
    use crate::workbench::settings_ui::profiles_glue::{ProfileAction, store};
    let cfg = fresh("action-real-profile");
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Profiles: Create…", window, cx));
    cx.run_until_parked();
    cx.simulate_input("work");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.active_profile().map(str::to_string)).as_deref(), Some("work"));
    let work = store::files_dir(&cfg, Some("work")).join("settings.jsonc");
    {
        let Some(_lock) = lock_dir(work.parent().unwrap()) else { return };
        wb.update_in(cx, |wb, window, cx| wb.write_setting("editor.fontSize", serde_json::json!(17), window, cx));
        cx.run_until_parked();
    }
    let failed = failures(&wb, cx);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(matches!(&failed[0].action, Some(NoticeAction::Retry(RetryKind::WriteSetting { path, .. })) if *path == work), "{:?}", failed[0].action);
    let serial = failed[0].serial;
    wb.update_in(cx, |wb, window, cx| wb.profile_picked(ProfileAction::Switch, None, window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.active_profile().map(str::to_string)), None, "the default profile is active");
    press(&wb, cx, serial);
    let note = wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default();
    assert!(note.contains("another profile") && note.contains("Nothing was written"), "{note}");
    for file in [cfg.join("settings.jsonc"), work.clone()] {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(!text.contains("\"editor.fontSize\": 17"), "{}: {text}", file.display());
    }
    // Back in the profile that failed, the same button writes.
    wb.update_in(cx, |wb, window, cx| wb.profile_picked(ProfileAction::Switch, Some("work".into()), window, cx));
    cx.run_until_parked();
    press(&wb, cx, serial);
    assert!(std::fs::read_to_string(&work).unwrap().contains("\"editor.fontSize\": 17"));
    assert!(failures(&wb, cx).is_empty());
}
