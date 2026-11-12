//! Workbench: the Help commands for finding out why something failed: copy the diagnostics report, open the log file, show the
//! log folder. What the report holds is described in `diagnostics_report.rs`; nothing is sent anywhere.

use super::*;
use crate::diagnostics::{log, report};

gpui::actions!(workbench, [CopyDiagnostics, OpenLogFile, RevealLogFolder]);

impl Workbench {
    /// Severity, message and detail of every notification in the history, newest first.
    #[cfg(test)]
    pub(crate) fn notice_history_with_detail(&self) -> Vec<(model::Severity, String, Option<String>)> {
        self.notices.log.newest_first().map(|e| (e.severity, e.message.clone(), e.detail.clone())).collect()
    }

    /// Severity and message of every notification in the history, newest first (tests of other topics read it).
    #[cfg(test)]
    pub(crate) fn notice_history(&self) -> Vec<(model::Severity, String)> {
        self.notices.log.newest_first().map(|e| (e.severity, e.message.clone())).collect()
    }

    /// At start: when the last run did not finish (it was killed or crashed; a panic the app survived does not count), say so once,
    /// with where it happened when a report says, and a button to the folder. Only the first window gets it.
    pub(crate) fn announce_previous_crash(&mut self, cx: &mut Context<Self>) {
        if let Some(previous) = crate::crash::session::take_previous() {
            self.announce_crash(previous, cx);
        }
    }

    fn announce_crash(&mut self, previous: crate::crash::PreviousCrash, cx: &mut Context<Self>) {
        let advice = crate::i18n::t(
            "The full report is in the crash reports folder and stays on this computer. Help: Report Issue… opens a new issue with the version filled in.",
        );
        let what = match (previous.location, previous.message) {
            (Some(location), Some(message)) => format!("{location}\n{message}"),
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => crate::i18n::t("The last session ended without closing normally, and no crash report explains why.").to_string(),
        };
        self.notify_with(
            model::Notification::new(model::Severity::Warning, NoticeSource::App, crate::i18n::t("Codeyard quit unexpectedly last time"))
                .detail(format!("{what}\n\n{advice}"))
                .action(model::NoticeAction::Command(model::Action {
                    label: crate::i18n::tr("Open Crash Reports Folder"),
                    command: "Help: Open Crash Reports Folder",
                }))
                .id("previous-crash"),
        );
        cx.notify();
    }

    /// The report as it would be copied now.
    pub(crate) fn diagnostics_report(&self, cx: &App) -> String {
        let notifications = self.notices.log.newest_first().take(report::NOTIFICATIONS).map(|e| report::NoticeLine {
            severity: match e.severity {
                model::Severity::Info => "Info",
                model::Severity::Warning => "Warning",
                model::Severity::Error => "Error",
            },
            source: format!("{:?}", e.source),
            message: e.message.clone(),
        });
        log::flush();
        report::build(&report::Inputs {
            version: env!("CARGO_PKG_VERSION"),
            commit: option_env!("CODEYARD_BUILD_ID"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            language: crate::i18n::LANGUAGES[crate::i18n::current()].code.to_string(),
            locales: crate::i18n::system_locales(),
            settings: report::changed_settings(cx.global::<Settings>()),
            notifications: notifications.collect(),
            log: log::tail(report::LOG_LINES),
            home: dirs::home_dir().map(|h| h.to_string_lossy().into_owned()),
        })
    }

    pub(in crate::workbench) fn copy_diagnostics(&mut self, _: &CopyDiagnostics, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.diagnostics_report(cx);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.notify(
            NoticeSource::App,
            Notice::Info(crate::i18n::t("Diagnostics copied. Nothing was sent anywhere: paste them where you want to share them.").into()),
        );
        cx.notify();
    }

    pub(in crate::workbench) fn open_log_file(&mut self, _: &OpenLogFile, window: &mut Window, cx: &mut Context<Self>) {
        log::flush();
        match log::log_dir().map(|d| d.join(log::FILE_NAME)).filter(|p| p.is_file()) {
            Some(path) => match host::try_local() {
                Ok(local) => self.open_file_on(Arc::new(local), path.to_string_lossy().into_owned(), window, cx),
                Err(e) => {
                    log::error("files", format_args!("could not open the log file: {e}"));
                    self.notify(NoticeSource::App, Notice::Error(e.to_string().into()));
                    cx.notify();
                }
            },
            None => {
                self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("There is no log file yet.").into()));
                cx.notify();
            }
        }
    }

    pub(in crate::workbench) fn reveal_log_folder(&mut self, _: &RevealLogFolder, _: &mut Window, cx: &mut Context<Self>) {
        log::flush();
        match log::log_dir().filter(|d| d.is_dir()) {
            Some(dir) => cx.reveal_path(&dir),
            None => {
                self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("There is no log file yet.").into()));
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
#[path = "workbench_diagnostics_tests.rs"]
mod tests;
