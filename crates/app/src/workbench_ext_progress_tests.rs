//! An extension's progress: the ticks are toasts, the run is one entry in the notification history.

use ext_api::value::Notify;
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::workbench::notifications_glue::model::Severity;

fn notify(wb: &Entity<Workbench>, cx: &mut VisualTestContext, n: Notify) {
    wb.update_in(cx, |wb, window, cx| wb.on_extension_notify("you.indexer", n, window, cx));
}

fn history(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<(Severity, String, Option<String>)> {
    wb.read_with(cx, |wb, _| wb.notice_history_with_detail())
}

fn start(id: u64, title: &str) -> Notify {
    Notify::StartProgress { id, title: title.into(), cancellable: false }
}

#[gpui::test]
fn a_finished_run_leaves_one_done_entry_and_its_ticks_leave_none(cx: &mut TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    notify(&wb, cx, start(1, "Indexing"));
    for percent in [10, 50, 90] {
        notify(&wb, cx, Notify::ReportProgress { id: 1, message: Some("files".into()), percent: Some(percent) });
    }
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).is_some_and(|t| t.contains("90%")), "the tick is on screen");
    assert!(history(&wb, cx).is_empty(), "ticks are not recorded");
    notify(&wb, cx, Notify::EndProgress { id: 1 });
    assert_eq!(history(&wb, cx), [(Severity::Info, "you.indexer: Indexing — done".to_string(), None)]);
    // An end that arrives twice, or for a run that never started, adds nothing.
    notify(&wb, cx, Notify::EndProgress { id: 1 });
    notify(&wb, cx, Notify::EndProgress { id: 99 });
    assert_eq!(history(&wb, cx).len(), 1);
}

#[gpui::test]
fn the_done_entry_is_recorded_under_do_not_disturb_too(cx: &mut TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Notifications: Toggle Do Not Disturb", window, cx));
    notify(&wb, cx, start(1, "Indexing"));
    notify(&wb, cx, Notify::EndProgress { id: 1 });
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()), None, "no toast");
    assert_eq!(history(&wb, cx).len(), 1, "but the history has it");
}

#[gpui::test]
fn runs_are_told_apart(cx: &mut TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    notify(&wb, cx, start(1, "Indexing"));
    notify(&wb, cx, start(2, "Linting"));
    notify(&wb, cx, Notify::EndProgress { id: 2 });
    notify(&wb, cx, Notify::EndProgress { id: 1 });
    let messages: Vec<String> = history(&wb, cx).into_iter().map(|(_, m, _)| m).collect();
    assert_eq!(messages, ["you.indexer: Indexing — done", "you.indexer: Linting — done"]);
}

#[gpui::test]
fn a_run_cut_short_by_the_extension_stopping_is_an_error_with_the_reason(cx: &mut TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    notify(&wb, cx, start(1, "Indexing"));
    notify(&wb, cx, start(2, "Linting"));
    wb.update_in(cx, |wb, window, cx| {
        wb.on_extension_event("you.indexer", Event::Exited { crashed: true, reason: "killed by signal 9".into() }, window, cx);
    });
    let entries = history(&wb, cx);
    let runs: Vec<_> = entries.iter().filter(|(_, m, _)| m.contains("did not finish")).collect();
    assert_eq!(runs.len(), 2, "{entries:?}");
    assert!(runs.iter().all(|(severity, _, detail)| *severity == Severity::Error && detail.as_deref() == Some("killed by signal 9")), "{runs:?}");
    // The runs are over: a late end changes nothing.
    notify(&wb, cx, Notify::EndProgress { id: 1 });
    assert!(!history(&wb, cx).iter().any(|(_, m, _)| m.ends_with("— done")));
    // Other extensions' runs are left alone.
    wb.update_in(cx, |wb, window, cx| wb.on_extension_notify("you.other", start(1, "Other"), window, cx));
    wb.update_in(cx, |wb, window, cx| {
        wb.on_extension_event("you.indexer", Event::Exited { crashed: false, reason: "stopped".into() }, window, cx);
    });
    assert!(!history(&wb, cx).iter().any(|(_, m, _)| m.contains("Other")));
}
