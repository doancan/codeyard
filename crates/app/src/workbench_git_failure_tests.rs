//! Git and Local History failures are said, not swallowed: a history diff never shows an empty side because git failed, and a
//! Local History folder that cannot be written is reported once per session.

use gpui::TestAppContext;

use super::model::{NoticeAction, Severity};
use super::*;
use crate::qa_workbench_tests::{fresh, open, open_in, project, setup, setup_persistent};

#[gpui::test]
fn a_history_diff_that_git_cannot_read_shows_the_error_not_an_empty_side(cx: &mut TestAppContext) {
    let not_a_repo = fresh("history-diff-not-a-repo");
    let (wb, cx) = setup(cx);
    let entry = crate::git::LogEntry {
        hash: "0123456789abcdef0123456789abcdef01234567".into(),
        short: "0123456".into(),
        author: "Test".into(),
        when: "now".into(),
        subject: "a change".into(),
        path: "Çalışma.txt".into(),
    };
    wb.update_in(cx, |wb, window, cx| wb.open_commit_diff(crate::ui::local_path(&not_a_repo), entry, window, cx));
    cx.run_until_parked();
    let history = wb.read_with(cx, |wb, _| wb.history_for_test());
    assert_eq!(history.len(), 1, "{history:?}");
    let e = &history[0];
    assert_eq!((e.severity, e.source), (Severity::Error, NoticeSource::Git));
    assert!(e.message.starts_with("Could not read Çalışma.txt at 0123456: "), "{}", e.message);
    assert!(e.detail.as_deref().is_some_and(|d| d.to_lowercase().contains("not a git repository")), "{:?}", e.detail);
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|n| n.0), Some(true));
}

/// The oldest commit of a shallow clone has no parent here: its history diff says so instead of showing the whole file as added.
#[cfg(unix)]
#[gpui::test]
fn a_history_diff_at_the_end_of_a_shallow_clone_says_so(cx: &mut TestAppContext) {
    let Some((origin, run)) = crate::git::repo_for_test("shallow-diff-origin") else { return };
    std::fs::write(origin.join("f.txt"), "two\n").unwrap();
    run(&["commit", "-q", "-am", "second"]).unwrap();
    let copy = origin.with_file_name(format!("codeyard-shallow-diff-copy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&copy);
    let cloned = crate::test_git::git().args(["clone", "-q", "--depth", "1", &format!("file://{}", origin.display())]).arg(&copy).output().unwrap();
    assert!(cloned.status.success());
    let hash = String::from_utf8_lossy(&crate::test_git::git().args(["rev-parse", "HEAD"]).current_dir(&copy).output().unwrap().stdout).trim().to_string();
    let (wb, cx) = setup(cx);
    let entry =
        crate::git::LogEntry { short: hash[..7].into(), hash, author: "Test".into(), when: "now".into(), subject: "second".into(), path: "f.txt".into() };
    wb.update_in(cx, |wb, window, cx| wb.open_commit_diff(crate::ui::local_path(&copy), entry, window, cx));
    cx.run_until_parked();
    let history = wb.read_with(cx, |wb, _| wb.history_for_test());
    let _ = std::fs::remove_dir_all(&copy);
    assert_eq!(history.len(), 1, "{history:?}");
    assert!(history[0].message.contains("shallow clone"), "{}", history[0].message);
}

#[cfg(unix)]
#[gpui::test]
fn local_history_that_cannot_write_is_reported_once(cx: &mut TestAppContext) {
    let (home, dir) = (fresh("history-failure-home"), project("history-failure-project"));
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    let ed = wb.read_with(cx, |w, _| w.active_editor_entity()).unwrap();
    let Some(lock) = super::locks::lock_dir(&home) else { return };
    for round in 0..3 {
        ed.update(cx, |e, cx| e.insert_at_for_test(0, &format!("// edit {round}\n"), cx));
        ed.update(cx, |e, cx| e.save_now(cx));
        cx.run_until_parked();
    }
    drop(lock);
    let warnings: Vec<_> = wb.read_with(cx, |wb, _| wb.history_for_test()).into_iter().filter(|e| e.id == "history-record-failed").collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let e = &warnings[0];
    assert_eq!((e.severity, e.count), (Severity::Warning, 1), "three failing saves, one warning");
    assert!(e.message.starts_with("Local History could not keep a copy of the file you saved."), "{}", e.message);
    assert!(e.message.contains("permission"), "{}", e.message);
    assert!(matches!(&e.action, Some(NoticeAction::Reveal(p)) if p.starts_with(&home)), "{:?}", e.action);
}
