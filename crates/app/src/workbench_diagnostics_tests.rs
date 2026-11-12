//! The crash notification and the diagnostics report in a real workbench.

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;

fn setup(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    crate::qa_workbench_tests::setup(cx)
}

#[gpui::test]
fn an_unfinished_run_is_announced_with_details_and_a_button(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let previous = crate::crash::PreviousCrash { location: Some("src/x.rs:4:2".into()), message: Some("index out of bounds".into()) };
    wb.update(cx, |wb, cx| wb.announce_crash(previous, cx));
    let entries: Vec<_> = wb.read_with(cx, |wb, _| wb.notices.log.newest_first().cloned().collect());
    assert_eq!(entries.len(), 1, "{entries:?}");
    let e = &entries[0];
    assert_eq!(e.severity, model::Severity::Warning);
    assert_eq!(e.message, "Codeyard quit unexpectedly last time");
    let detail = e.detail.as_deref().unwrap();
    assert!(detail.starts_with("src/x.rs:4:2\nindex out of bounds"), "{detail}");
    assert!(matches!(&e.action, Some(model::NoticeAction::Command(a)) if a.command == "Help: Open Crash Reports Folder"), "{:?}", e.action);
    assert!(wb.read_with(cx, |wb, _| wb.commands().iter().any(|c| c.label == "Help: Open Crash Reports Folder")), "the button's command exists");
}

#[gpui::test]
fn a_kill_without_a_report_still_explains_itself(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| wb.announce_crash(crate::crash::PreviousCrash::default(), cx));
    let detail = wb.read_with(cx, |wb, _| wb.notices.log.newest_first().next().and_then(|e| e.detail.clone())).unwrap();
    assert!(detail.starts_with("The last session ended without closing normally"), "{detail}");
}

#[gpui::test]
fn nothing_pending_means_no_notification(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| wb.announce_previous_crash(cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.notices.log.newest_first().count()), 0);
}

fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(command, window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn copy_diagnostics_puts_a_redacted_report_on_the_clipboard(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).filter(|h| h.len() > 3);
    let leaky = format!("push to https://bob:hunter2@github.com/o/r failed in {} with token=abc123", home.as_deref().unwrap_or("/Users/nobody"));
    wb.update(cx, |wb, _| wb.notify(NoticeSource::Git, Notice::Error(leaky.into())));
    run(&wb, cx, "Help: Copy Diagnostics");
    let text = cx.read_from_clipboard().and_then(|item| item.text()).expect("text on the clipboard");
    assert!(text.starts_with("# Codeyard diagnostics"), "{text}");
    assert!(text.contains(&format!("Version: {}", env!("CARGO_PKG_VERSION"))), "{text}");
    assert!(text.contains("[Error] Git: push to https://***@github.com/o/r failed"), "{text}");
    for secret in ["hunter2", "abc123"].into_iter().chain(home.as_deref()) {
        assert!(!text.contains(secret), "{secret:?} leaked:\n{text}");
    }
    let history = wb.read_with(cx, |wb, _| wb.notice_history());
    assert!(history.iter().any(|(_, m)| m.starts_with("Diagnostics copied")), "the user is told: {history:?}");
}

#[gpui::test]
fn the_log_commands_are_in_the_palette(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let labels = wb.read_with(cx, |wb, _| wb.commands().into_iter().map(|c| c.label.to_string()).collect::<Vec<_>>());
    for label in ["Help: Copy Diagnostics", "Help: Open Log File", "Help: Reveal Log Folder"] {
        assert!(labels.iter().any(|l| l == label), "{label}");
    }
}

#[test]
fn every_way_into_the_history_is_written_to_the_log() {
    // A message from before the window existed (the state database that could not be opened, a theme that was skipped).
    let _center = super::super::NoticeCenter::with_startup(Some(Notice::Error("startup trouble 1".into())));
    assert!(crate::diagnostics::log::tail(200).iter().any(|l| l.contains("ERROR") && l.contains("startup trouble 1")));
}
