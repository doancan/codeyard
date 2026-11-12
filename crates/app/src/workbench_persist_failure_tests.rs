//! A state database that stops accepting writes: the window says so once per area, with the folder to look at, and keeps working.

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::model::{NoticeAction, Severity};
use super::*;
use crate::qa_workbench_tests::{fresh, open, project, setup_persistent};

fn break_state(home: &std::path::Path) {
    // Another connection takes the table away: every later write of the window fails with "no such table".
    rusqlite::Connection::open(home.join("state.db")).unwrap().execute_batch("drop table kv").unwrap();
}

fn warnings(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<(String, String, Option<NoticeAction>, u32)> {
    wb.read_with(cx, |wb, _| {
        wb.notices
            .log
            .newest_first()
            .filter(|e| e.severity == Severity::Warning)
            .map(|e| (e.id.clone(), e.detail.clone().unwrap_or_default(), e.action.clone(), e.count))
            .collect()
    })
}

#[gpui::test]
fn a_failing_state_database_is_reported_once_per_area_with_the_state_folder(cx: &mut TestAppContext) {
    let home = fresh("persist-failure");
    let dir = project("persist-failure-project");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    break_state(&home);
    for _ in 0..4 {
        wb.update(cx, |wb, cx| {
            wb.persist_tree(cx);
            wb.persist_sections(cx);
        });
    }
    wb.update(cx, |wb, cx| wb.track_notice(cx));
    let all = warnings(&wb, cx);
    assert_eq!(all.len(), 1, "one warning for the Explorer however often it saves: {all:?}");
    let (id, detail, action, count) = &all[0];
    assert_eq!(id, "persist:explorer");
    assert_eq!(*count, 1, "reported once per session, not counted again");
    assert!(detail.contains("no such table"), "the cause is kept: {detail}");
    assert_eq!(action, &Some(NoticeAction::Reveal(home.clone())), "the button shows the state folder");
    let message = wb.read_with(cx, |wb, _| wb.notices.log.newest_first().next().unwrap().message.clone());
    assert!(message.contains("will not be restored next time"), "{message}");
    assert!(wb.read_with(cx, |wb, _| wb.toast_is_none() || wb.notice_text().is_some()), "the window is still alive");
}

#[gpui::test]
fn a_failing_session_save_is_reported_once(cx: &mut TestAppContext) {
    let home = fresh("persist-failure-session");
    let dir = project("persist-failure-session-project");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("src/main.rs"), cx);
    rusqlite::Connection::open(home.join("state.db")).unwrap().execute_batch("drop table open_editors; drop table kv").unwrap();
    wb.update(cx, |wb, cx| wb.persist_session(cx));
    wb.update(cx, |wb, cx| wb.persist_session(cx));
    wb.update(cx, |wb, cx| wb.track_notice(cx));
    let ids: Vec<String> = warnings(&wb, cx).into_iter().map(|w| w.0).collect();
    assert!(ids.contains(&"persist:tabs".to_string()), "{ids:?}");
    assert_eq!(ids.iter().filter(|i| *i == "persist:tabs").count(), 1);
}

#[gpui::test]
fn a_healthy_state_database_says_nothing(cx: &mut TestAppContext) {
    let home = fresh("persist-healthy");
    let dir = project("persist-healthy-project");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, cx| {
        wb.persist_session(cx);
        wb.persist_tree(cx);
        wb.persist_sections(cx);
        wb.track_notice(cx);
    });
    assert!(warnings(&wb, cx).is_empty());
}
