//! Workbench: View toggles for settings that have no command of their own (breadcrumbs, line numbers, indent guides,
//! inlay hints, auto save), and the Help commands that open a page or a folder (release notes, report an issue, crash
//! reports). A toggle changes the running session's setting like the other View toggles; the settings file stays as it is.

use super::*;
use crate::settings::AutoSave;

gpui::actions!(
    workbench,
    [ToggleBreadcrumbs, ToggleLineNumbers, ToggleIndentGuides, ToggleInlayHints, ToggleAutoSave, ShowReleaseNotes, ReportIssue, OpenCrashReportsFolder]
);

/// What a toggle changes to when `files.autoSave` is flipped: off becomes after a second, anything else off.
const AUTO_SAVE_DELAY_MS: u64 = 1000;

/// `editor.lineNumbers` after a toggle: on and relative both turn off, off turns on.
fn toggled_line_numbers(current: &str) -> &'static str {
    if current == "off" { "on" } else { "off" }
}

fn toggled_auto_save(current: &AutoSave) -> AutoSave {
    match current {
        AutoSave::Off => AutoSave::AfterDelay(AUTO_SAVE_DELAY_MS),
        _ => AutoSave::Off,
    }
}

/// The new-issue page of the repository, with the version and system already in the text.
fn report_issue_url() -> String {
    let body = format!("\n\n---\nCodeyard {} on {} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    format!("{}/issues/new?body={}", env!("CARGO_PKG_REPOSITORY"), crate::texttools::url_encode(&body))
}

fn release_notes_url() -> String {
    format!("{}/releases/tag/v{}", env!("CARGO_PKG_REPOSITORY"), env!("CARGO_PKG_VERSION"))
}

impl Workbench {
    pub(super) fn toggle_breadcrumbs(&mut self, _: &ToggleBreadcrumbs, _: &mut Window, cx: &mut Context<Self>) {
        let s = cx.global_mut::<Settings>();
        s.breadcrumbs = !s.breadcrumbs;
        cx.refresh_windows();
    }

    pub(super) fn toggle_line_numbers(&mut self, _: &ToggleLineNumbers, _: &mut Window, cx: &mut Context<Self>) {
        let s = cx.global_mut::<Settings>();
        s.line_numbers = toggled_line_numbers(&s.line_numbers).into();
        cx.refresh_windows();
    }

    pub(super) fn toggle_indent_guides(&mut self, _: &ToggleIndentGuides, _: &mut Window, cx: &mut Context<Self>) {
        let s = cx.global_mut::<Settings>();
        s.indent_guides = !s.indent_guides;
        cx.refresh_windows();
    }

    pub(super) fn toggle_inlay_hints(&mut self, _: &ToggleInlayHints, _: &mut Window, cx: &mut Context<Self>) {
        let s = cx.global_mut::<Settings>();
        s.inlay_hints = !s.inlay_hints;
        cx.refresh_windows();
    }

    pub(super) fn toggle_auto_save(&mut self, _: &ToggleAutoSave, _: &mut Window, cx: &mut Context<Self>) {
        let s = cx.global_mut::<Settings>();
        s.auto_save = toggled_auto_save(&s.auto_save);
        let message = if s.auto_save == AutoSave::Off { crate::i18n::t("Auto Save is off.") } else { crate::i18n::t("Auto Save is on.") };
        self.notice = Some(Notice::Info(message.into()));
        cx.refresh_windows();
    }

    pub(super) fn show_release_notes(&mut self, _: &ShowReleaseNotes, _: &mut Window, cx: &mut Context<Self>) {
        cx.open_url(&release_notes_url());
    }

    pub(super) fn report_issue(&mut self, _: &ReportIssue, _: &mut Window, cx: &mut Context<Self>) {
        cx.open_url(&report_issue_url());
    }

    /// Shows the folder in the system's file manager, or says there are no reports yet.
    pub(super) fn open_crash_reports_folder(&mut self, _: &OpenCrashReportsFolder, _: &mut Window, cx: &mut Context<Self>) {
        match crash_dir().filter(|d| d.is_dir()) {
            Some(dir) => cx.reveal_path(&dir),
            None => {
                self.notice = Some(Notice::Info(crate::i18n::t("No crash reports: Codeyard has not crashed.").into()));
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_flip_between_two_states() {
        assert_eq!(toggled_line_numbers("on"), "off");
        assert_eq!(toggled_line_numbers("relative"), "off");
        assert_eq!(toggled_line_numbers("off"), "on");
        assert_eq!(toggled_auto_save(&AutoSave::Off), AutoSave::AfterDelay(AUTO_SAVE_DELAY_MS));
        assert_eq!(toggled_auto_save(&AutoSave::OnFocusChange), AutoSave::Off);
        assert_eq!(toggled_auto_save(&AutoSave::AfterDelay(300)), AutoSave::Off);
    }

    #[test]
    fn the_issue_page_carries_the_version_and_encodes_the_text() {
        let url = report_issue_url();
        assert!(url.starts_with(env!("CARGO_PKG_REPOSITORY")) && url.contains("/issues/new?body="), "{url}");
        assert!(url.contains(&crate::texttools::url_encode(env!("CARGO_PKG_VERSION"))) && !url.contains(' ') && !url.contains('\n'), "{url}");
        assert!(release_notes_url().ends_with(&format!("/releases/tag/v{}", env!("CARGO_PKG_VERSION"))));
    }
}
