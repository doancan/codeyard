//! The notification centre in a real workbench: every message is recorded, Do Not Disturb keeps toasts away but not the
//! history, repeats collapse, and the list opens, clears and closes from the palette and the keyboard.

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::model::Action;
use super::*;

fn setup(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    crate::qa_workbench_tests::setup(cx)
}

fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(command, window, cx));
    cx.run_until_parked();
}

fn log_messages(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    wb.read_with(cx, |wb, _| wb.notices.log.newest_first().map(|e| e.message.clone()).collect())
}

#[gpui::test]
fn two_messages_in_one_frame_are_both_kept(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| {
        wb.notify(NoticeSource::Git, Notice::Error("push failed".into()));
        wb.notify(NoticeSource::Language, Notice::Error("server did not start".into()));
    });
    assert_eq!(log_messages(&wb, cx), ["server did not start", "push failed"]);
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()).as_deref(), Some("server did not start"), "the toast shows the latest");
}

#[gpui::test]
fn repeated_messages_collapse_and_the_toast_text_does_not_change(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| wb.notify(NoticeSource::Git, Notice::Error("push failed".into())));
    let tick = wb.read_with(cx, |wb, _| wb.notices.toast.as_ref().map(|t| t.tick));
    wb.update(cx, |wb, _| wb.notify(NoticeSource::Git, Notice::Error("push failed".into())));
    assert_eq!(wb.read_with(cx, |wb, _| (wb.notices.log.len(), wb.notices.log.newest_first().next().unwrap().count)), (1, 2));
    // The timer restarts (a new tick) but the text is the same string, so the accessibility tree has nothing new to say.
    assert_ne!(tick, wb.read_with(cx, |wb, _| wb.notices.toast.as_ref().map(|t| t.tick)));
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_for_test()), Some((true, "push failed".to_string())));
}

#[gpui::test]
fn do_not_disturb_hides_information_and_warnings_but_errors_still_show(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    run(&wb, cx, "Notifications: Toggle Do Not Disturb");
    assert!(wb.read_with(cx, |wb, _| wb.do_not_disturb()));
    wb.update(cx, |wb, _| {
        wb.notify(NoticeSource::App, Notice::Info("copied".into()));
        wb.notify(NoticeSource::Tasks, Notice::Warning("slow".into()));
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()), None, "no toast for information and warnings");
    wb.update(cx, |wb, _| wb.notify(NoticeSource::Update, Notice::Error("update failed".into())));
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_for_test()), Some((true, "update failed".to_string())), "an error still shows its card");
    assert_eq!(log_messages(&wb, cx), ["update failed", "slow", "copied"], "everything is recorded");
    assert_eq!(wb.read_with(cx, |wb, _| wb.unread_notifications()), (2, 1));
    run(&wb, cx, "Notifications: Toggle Do Not Disturb");
    wb.update(cx, |wb, _| wb.notify(NoticeSource::App, Notice::Info("visible again".into())));
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()).as_deref(), Some("visible again"));
}

#[gpui::test]
fn turning_do_not_disturb_on_keeps_a_visible_error_card_and_hides_an_info_toast(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| wb.notify(NoticeSource::Git, Notice::Error("boom".into())));
    run(&wb, cx, "Notifications: Toggle Do Not Disturb");
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()).as_deref(), Some("boom"), "the card stays");
    run(&wb, cx, "Notifications: Toggle Do Not Disturb");
    wb.update(cx, |wb, _| wb.notify(NoticeSource::Git, Notice::Info("pushed".into())));
    run(&wb, cx, "Notifications: Toggle Do Not Disturb");
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()), None, "an information toast goes");
    assert_eq!(wb.read_with(cx, |wb, _| wb.unread_notifications()), (1, 1));
}

#[gpui::test]
fn extension_progress_is_shown_but_never_fills_the_history(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| {
        wb.cfg.exts.code.progress.insert(("acme.lint".into(), 1), ("Indexing".into(), None, None));
        for pc in 0..=100u8 {
            wb.cfg.exts.code.progress.insert(("acme.lint".into(), 1), ("Indexing".into(), Some("files".into()), Some(pc)));
            wb.show_extension_progress("acme.lint", 1, cx);
        }
    });
    assert!(log_messages(&wb, cx).is_empty(), "a hundred ticks left nothing behind");
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()).as_deref(), Some("acme.lint: Indexing — files (100%)"));
}

#[gpui::test]
fn a_failure_while_the_list_has_lost_the_focus_stays_unread_and_the_list_closes(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let project = crate::qa_workbench_tests::project("notices-focus");
    crate::qa_workbench_tests::open_in(&wb, &project.join("src/main.rs"), cx);
    run(&wb, cx, "Notifications: Show");
    assert!(wb.read_with(cx, |wb, _| wb.notices.open));
    // An error that comes while the list has the keyboard is in front of the user: it counts as seen.
    wb.update_in(cx, |wb, w, cx| {
        wb.notify(NoticeSource::Git, Notice::Error("seen".into()));
        wb.settle_notifications(w, cx);
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.unread_notifications()), (0, 0));
    // The user goes to the editor; then a failure arrives.
    wb.update_in(cx, |wb, w, cx| wb.focus_active_editor(w, cx));
    wb.update_in(cx, |wb, w, cx| {
        wb.notify(NoticeSource::Language, Notice::Error("server did not start".into()));
        wb.settle_notifications(w, cx);
    });
    assert!(!wb.read_with(cx, |wb, _| wb.notices.open), "the list closes when the focus leaves it");
    assert_eq!(wb.read_with(cx, |wb, _| wb.unread_notifications()), (1, 1), "the bell shows the failure");
}

#[gpui::test]
fn closing_the_list_returns_the_focus_or_falls_back_to_the_editor(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let project = crate::qa_workbench_tests::project("notices-restore");
    crate::qa_workbench_tests::open_in(&wb, &project.join("src/main.rs"), cx);
    let editor_focused = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| {
        wb.update_in(cx, |wb, w, cx| wb.active_editor().is_some_and(|e| e.read(cx).focus_handle(cx).is_focused(w)))
    };
    wb.update_in(cx, |wb, w, cx| wb.focus_active_editor(w, cx));
    run(&wb, cx, "Notifications: Show");
    assert!(!editor_focused(&wb, cx), "the list has the keyboard");
    run(&wb, cx, "Notifications: Show");
    assert!(editor_focused(&wb, cx), "closing gives it back");
    // The element that had the focus is gone by the time the list closes.
    run(&wb, cx, "Notifications: Show");
    wb.update_in(cx, |wb, w, cx| {
        let gone = cx.focus_handle();
        wb.notices.return_focus = Some(gone.downgrade());
        drop(gone);
        wb.close_notifications(w, cx);
    });
    assert!(editor_focused(&wb, cx), "the editor takes it when the old element is gone");
}

#[gpui::test]
fn tab_walks_the_buttons_and_keys_the_list_does_not_use_are_left_alone(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| {
        wb.notify_with(
            Notification::new(Severity::Error, NoticeSource::Language, "server did not start")
                .action(Action { label: "Language Services", command: "Notifications: Clear" }),
        )
    });
    run(&wb, cx, "Notifications: Show");
    let serial = wb.read_with(cx, |wb, _| wb.notices.selected).unwrap();
    let key = |cx: &mut VisualTestContext, k: &'static str, shift: bool| wb.update_in(cx, |wb, w, cx| wb.notifications_key(k, false, shift, w, cx));
    assert!(!key(cx, "a", false), "a letter is not the list's");
    assert!(!key(cx, "f5", false), "nor a function key");
    let mut seen = Vec::new();
    for _ in 0..12 {
        assert!(key(cx, "tab", false));
        cx.run_until_parked();
        let focused = wb.update_in(cx, |wb, w, _| wb.focused_button(w));
        seen.extend(focused);
    }
    for want in [(serial, "action"), (serial, "copy"), (serial, "dismiss"), (0, "notifications-dnd"), (0, "notifications-clear"), (0, "notifications-close")] {
        assert!(seen.contains(&want), "Tab reaches {want:?}: {seen:?}");
    }
    // Enter on the button that has the keyboard presses it: Copy puts the message on the clipboard.
    for _ in 0..12 {
        if wb.update_in(cx, |wb, w, _| wb.focused_button(w)) == Some((serial, "copy")) {
            break;
        }
        key(cx, "tab", false);
        cx.run_until_parked();
    }
    assert!(key(cx, "enter", false));
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("server did not start"));
}
#[gpui::test]
fn dismissing_the_toast_keeps_the_history(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| wb.notify(NoticeSource::Git, Notice::Error("boom".into())));
    wb.update(cx, |wb, _| wb.dismiss_notice());
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()), None);
    assert_eq!(log_messages(&wb, cx), ["boom"]);
}

#[gpui::test]
fn show_marks_everything_read_and_clear_empties_the_list(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| {
        wb.notify(NoticeSource::Git, Notice::Error("one".into()));
        wb.notify(NoticeSource::Git, Notice::Warning("two".into()));
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.unread_notifications()), (2, 1));
    run(&wb, cx, "Notifications: Show");
    assert!(wb.read_with(cx, |wb, _| wb.notices.open));
    assert_eq!(wb.read_with(cx, |wb, _| wb.unread_notifications()), (0, 0));
    // A message that arrives while the list is open is already seen.
    wb.update_in(cx, |wb, w, cx| {
        wb.notify(NoticeSource::Git, Notice::Error("three".into()));
        wb.settle_notifications(w, cx);
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.unread_notifications()), (0, 0));
    run(&wb, cx, "Notifications: Clear");
    assert!(log_messages(&wb, cx).is_empty());
}

#[gpui::test]
fn the_keyboard_moves_through_the_list_dismisses_and_escape_closes(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| {
        for m in ["a", "b", "c"] {
            wb.notify(NoticeSource::Tasks, Notice::Error(m.into()));
        }
    });
    run(&wb, cx, "Notifications: Show");
    let key = |cx: &mut VisualTestContext, k: &'static str| wb.update_in(cx, |wb, window, cx| wb.notifications_key(k, false, false, window, cx));
    let selected = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.notices.selected.and_then(|s| wb.notices.log.get(s).map(|e| e.message.clone())));
    assert_eq!(selected(cx).as_deref(), Some("c"), "the newest is first");
    key(cx, "down");
    assert_eq!(selected(cx).as_deref(), Some("b"));
    key(cx, "delete");
    assert_eq!(log_messages(&wb, cx), ["c", "a"]);
    assert_eq!(selected(cx).as_deref(), Some("a"), "the row after the removed one is selected");
    key(cx, "up");
    key(cx, "up");
    assert_eq!(selected(cx).as_deref(), Some("c"), "up stops at the top");
    key(cx, "escape");
    assert!(!wb.read_with(cx, |wb, _| wb.notices.open));
}

#[gpui::test]
fn an_action_runs_its_palette_command_and_closes_the_list(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| {
        wb.notify_with(
            Notification::new(Severity::Error, NoticeSource::Language, "server did not start")
                .detail("stderr: no such file")
                .action(Action { label: "Clear Notifications", command: "Notifications: Toggle Do Not Disturb" }),
        )
    });
    run(&wb, cx, "Notifications: Show");
    let serial = wb.read_with(cx, |wb, _| wb.notices.selected).expect("the newest is selected");
    wb.update_in(cx, |wb, window, cx| wb.run_notification_action(serial, window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| !wb.notices.open && wb.do_not_disturb()));
}

#[gpui::test]
fn copying_puts_the_message_and_detail_on_the_clipboard(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| wb.notify_with(Notification::new(Severity::Error, NoticeSource::Git, "push failed").detail("fatal: no remote")));
    let serial = wb.read_with(cx, |wb, _| wb.notices.log.newest_first().next().unwrap().serial);
    wb.update(cx, |wb, cx| wb.copy_notification(serial, cx));
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("push failed\n\nfatal: no remote"));
    assert_eq!(log_messages(&wb, cx).len(), 1, "the confirmation is not itself recorded");
}

#[gpui::test]
fn the_bell_and_the_list_are_drawn(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| wb.notify(NoticeSource::Git, Notice::Error("boom".into())));
    cx.run_until_parked();
    assert!(cx.debug_bounds("sb-notifications").is_some(), "the bell is in the status bar");
    run(&wb, cx, "Notifications: Show");
    assert!(cx.debug_bounds("notifications-panel").is_some());
    let serial = wb.read_with(cx, |wb, _| wb.notices.log.newest_first().next().unwrap().serial);
    let row: &'static str = Box::leak(format!("notification-row-{serial}").into_boxed_str());
    assert!(cx.debug_bounds(row).is_some());
}
