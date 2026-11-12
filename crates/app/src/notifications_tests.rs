use super::*;

fn err(msg: &str) -> Notification {
    Notification::new(Severity::Error, NoticeSource::Git, msg)
}

#[test]
fn identical_notifications_collapse_into_one_entry_with_a_counter() {
    let mut log = Log::default();
    let first = log.post(err("push failed"), 10);
    let second = log.post(err("push failed"), 20);
    assert!(!first.repeated && second.repeated);
    assert_eq!(log.len(), 1);
    let e = log.newest_first().next().unwrap();
    assert_eq!((e.count, e.at), (2, 20));
    assert!(second.serial > first.serial);
}

#[test]
fn a_repeat_moves_to_the_top_and_is_unread_again() {
    let mut log = Log::default();
    log.post(err("a"), 1);
    log.post(err("b"), 2);
    log.mark_all_read();
    assert_eq!(log.unread(), 0);
    log.post(err("a"), 3);
    let order: Vec<_> = log.newest_first().map(|e| e.message.as_str()).collect();
    assert_eq!(order, ["a", "b"]);
    assert_eq!(log.unread(), 1);
}

#[test]
fn an_explicit_id_collapses_different_messages() {
    let mut log = Log::default();
    log.post(err("server crashed (1)").id("lsp:rust"), 1);
    let p = log.post(err("server crashed (2)").id("lsp:rust"), 2);
    assert!(p.repeated);
    assert_eq!(log.newest_first().next().unwrap().message, "server crashed (2)");
}

#[test]
fn same_message_from_another_source_or_severity_is_a_separate_entry() {
    let mut log = Log::default();
    log.post(Notification::new(Severity::Error, NoticeSource::Git, "x"), 1);
    log.post(Notification::new(Severity::Error, NoticeSource::Tasks, "x"), 1);
    log.post(Notification::new(Severity::Warning, NoticeSource::Git, "x"), 1);
    assert_eq!(log.len(), 3);
}

#[test]
fn the_history_is_bounded_and_drops_the_oldest() {
    let mut log = Log::default();
    for i in 0..CAPACITY + 50 {
        log.post(err(&format!("m{i}")), i as u64);
    }
    assert_eq!(log.len(), CAPACITY);
    assert_eq!(log.newest_first().last().unwrap().message, "m50");
}

#[test]
fn long_text_is_cut() {
    let mut log = Log::default();
    log.post(err(&"é".repeat(MAX_MESSAGE_CHARS + 10)).detail("ü".repeat(MAX_DETAIL_CHARS + 10)), 0);
    let e = log.newest_first().next().unwrap();
    assert_eq!(e.message.chars().count(), MAX_MESSAGE_CHARS + 1);
    assert_eq!(e.detail.as_ref().unwrap().chars().count(), MAX_DETAIL_CHARS + 1);
}

#[test]
fn only_warnings_and_errors_are_unread_and_errors_are_counted_apart() {
    let mut log = Log::default();
    log.post(Notification::new(Severity::Info, NoticeSource::App, "copied"), 0);
    log.post(Notification::new(Severity::Warning, NoticeSource::App, "slow"), 0);
    log.post(err("broken"), 0);
    assert_eq!((log.unread(), log.unread_errors()), (2, 1));
    log.mark_all_read();
    assert_eq!(log.unread(), 0);
}

#[test]
fn dismiss_and_clear() {
    let mut log = Log::default();
    let a = log.post(err("a"), 0);
    log.post(err("b"), 0);
    assert!(log.dismiss(a.serial));
    assert!(!log.dismiss(a.serial));
    assert_eq!(log.len(), 1);
    log.clear();
    assert!(log.is_empty());
}

#[test]
fn ages_read_naturally() {
    assert_eq!(age_label(100, 100), "just now");
    assert_eq!(age_label(100, 200), "just now", "a clock that went back is not negative");
    assert_eq!(age_label(160, 100), "1 minute ago");
    assert_eq!(age_label(100 + 3 * 3600, 100), "3 hours ago");
    assert_eq!(age_label(100 + 86_400, 100), "1 day ago");
}

#[test]
fn copy_text_has_the_detail_after_a_blank_line() {
    let mut log = Log::default();
    log.post(err("failed").detail("fatal: no remote"), 0);
    assert_eq!(copy_text(log.newest_first().next().unwrap()), "failed\n\nfatal: no remote");
}

#[test]
fn a_quiet_entry_is_kept_but_never_unread() {
    let mut log = Log::default();
    let offline = |severity| Notification::new(severity, NoticeSource::Update, "Couldn't check for updates").id("update-check").quiet();
    log.post(offline(Severity::Info), 1);
    log.post(offline(Severity::Info), 2);
    assert_eq!((log.len(), log.unread(), log.unread_errors()), (1, 0, 0));
    log.post(offline(Severity::Warning), 3);
    assert_eq!((log.len(), log.unread()), (1, 0), "quiet means quiet, whatever the severity");
    // The same id posted loudly later is unread again.
    log.post(Notification::new(Severity::Error, NoticeSource::Update, "bad signature").id("update-check"), 4);
    assert_eq!((log.unread(), log.unread_errors()), (1, 1));
}

#[test]
fn an_entry_can_be_removed_by_its_id() {
    let mut log = Log::default();
    log.post(err("a").id("one"), 1);
    assert!(log.dismiss_id("one"));
    assert!(!log.dismiss_id("one"));
    assert!(log.is_empty());
}
