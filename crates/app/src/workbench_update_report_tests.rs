use super::*;

fn failure(e: UpdateError) -> Failure {
    Failure::from_error(&e, None, Phase::Check)
}

fn report(e: UpdateError) -> Option<(Severity, bool)> {
    report_for(&failure(e)).map(|r| (r.severity, r.quiet))
}

#[test]
fn being_offline_or_the_server_being_down_is_only_information() {
    assert_eq!(report(UpdateError::Network("curl: (28) Operation timed out".into())), Some((Severity::Info, true)));
    assert_eq!(report(UpdateError::Http { file: "SHA256SUMS.txt".into(), status: 503 }), Some((Severity::Info, true)));
    assert_eq!(report(UpdateError::NoRelease), Some((Severity::Info, true)));
}

#[test]
fn what_the_user_can_fix_or_should_know_is_not_quiet() {
    assert_eq!(report(UpdateError::Tls("clock".into())), Some((Severity::Warning, false)));
    assert_eq!(report(UpdateError::NoSpace), Some((Severity::Warning, false)));
    assert_eq!(report(UpdateError::BadSignature), Some((Severity::Error, false)));
    assert_eq!(report(UpdateError::Unsigned), Some((Severity::Error, false)));
    assert_eq!(report(UpdateError::HashMismatch("x.dmg".into())), Some((Severity::Error, false)));
}

#[test]
fn a_cancelled_check_says_nothing() {
    assert_eq!(report(UpdateError::Cancelled), None);
    assert!(notification_for(&failure(UpdateError::Cancelled)).is_none());
}

#[test]
fn only_a_failure_that_is_not_quiet_offers_to_check_again() {
    let quiet = notification_for(&failure(UpdateError::Network("x".into()))).unwrap();
    assert!(quiet.quiet && quiet.action.is_none());
    let loud = notification_for(&failure(UpdateError::BadSignature)).unwrap();
    assert!(!loud.quiet);
    assert_eq!(loud.action, Some(NoticeAction::Retry(RetryKind::UpdateCheck)));
    assert_eq!(loud.id.as_deref(), Some("update-check-failed"));
    assert!(loud.detail.as_deref().is_some_and(|d| d.contains("Nothing was installed")));
}
