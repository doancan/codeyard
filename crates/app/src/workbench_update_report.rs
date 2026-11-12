//! Workbench: what a failed background update check tells the user. Being offline is expected: it goes into the history as
//! information and no toast. A failure that needs attention (a bad signature, a broken secure connection, a full disk) is a
//! warning or an error with the details and a button to check again. The id collapses repeats, and the schedule (at most three
//! failed checks a day, see `retry_allowed`) keeps the rest in bounds.

use super::notifications_glue::model::{NoticeAction, Notification, RetryKind, Severity};
use super::*;

/// The id of the entry a failed background check leaves in the history (a later good check removes it).
pub(super) const CHECK_FAILED_ID: &str = "update-check-failed";

/// How a failed check is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Report {
    pub(super) severity: Severity,
    /// Only in the history: no toast, because nobody is waiting for the check and nothing is wrong on this side.
    pub(super) quiet: bool,
}

/// `None`: nothing to say (the user cancelled).
pub(super) fn report_for(failure: &Failure) -> Option<Report> {
    let (severity, quiet) = match (failure.kind, &failure.cause) {
        (ErrorKind::Cancelled, _) => return None,
        (ErrorKind::Network | ErrorKind::NoRelease, Cause::Other) => (Severity::Info, true),
        (ErrorKind::Untrusted, _) => (Severity::Error, false),
        _ => (Severity::Warning, false),
    };
    Some(Report { severity, quiet })
}

pub(super) fn notification_for(failure: &Failure) -> Option<Notification> {
    let report = report_for(failure)?;
    let detail = format!("{}\n\n{}", failure.lead(), failure.detail);
    let mut n = Notification::new(report.severity, NoticeSource::Update, failure.title()).detail(detail).id(CHECK_FAILED_ID);
    if report.quiet {
        n = n.quiet();
    } else {
        n = n.action(NoticeAction::Retry(RetryKind::UpdateCheck));
    }
    Some(n)
}

impl Workbench {
    /// A check nobody was waiting for failed.
    pub(super) fn report_background_check_failure(&mut self, failure: &Failure) {
        if let Some(n) = notification_for(failure) {
            self.notify_with(n);
        }
    }
}

#[cfg(test)]
#[path = "workbench_update_report_tests.rs"]
mod tests;
