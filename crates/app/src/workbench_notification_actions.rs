//! Workbench: what the buttons of a notification do. Actions are plain data (see `notifications.rs`); this is the one place that
//! turns them into work, so a notification never holds an entity or a closure.

use super::*;
use model::{NoticeAction, RetryKind};
use model::{Notification, Severity};
use std::path::{Path, PathBuf};

/// What to show in the file manager for `path`: the path itself, or its nearest folder that still exists (a file that was never
/// created cannot be selected, but its folder can be opened).
pub(super) fn reveal_target(path: &Path) -> PathBuf {
    path.ancestors().find(|p| !p.as_os_str().is_empty() && p.exists()).map_or_else(|| path.to_path_buf(), Path::to_path_buf)
}

/// The same id for a path however its separators are written (the editor keeps `C:/x`, the config folder `C:\\x`).
fn settings_file_failure_id(path: &Path) -> String {
    format!("settings-file-failed:{}", path.display().to_string().replace('\\', "/"))
}

fn save_failure_id(host: &str, path: &str) -> String {
    format!("save-failed:{host}:{path}")
}

/// The value of `key` in the settings file at `path` as JSON; none when the file cannot be read or has no such key.
fn value_in_file(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&theme::themes::strip_jsonc(&text)).ok()?;
    value.get(key).map(|v| v.to_string())
}

impl Workbench {
    /// Run the button of the notification `serial`; the list closes first when it is open.
    pub(super) fn run_notification_action(&mut self, serial: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.notices.log.get(serial).and_then(|e| e.action.clone()) else { return };
        self.close_notifications(window, cx);
        self.run_notice_action(action, window, cx);
    }

    pub(crate) fn run_notice_action(&mut self, action: NoticeAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            NoticeAction::Command(model::Action { command, .. }) => {
                let Some(run) = self.commands().into_iter().find(|c| c.label == command).map(|c| c.run) else { return };
                self.run_command(run, window, cx);
            }
            NoticeAction::Reveal(path) => {
                // Looking for the nearest folder that exists touches the disk, and a network share that is gone can stall for a
                // long time: not on the thread that draws the window.
                let lookup = cx.background_executor().spawn(async move { reveal_target(&path) });
                cx.spawn(async move |_, cx| {
                    let target = lookup.await;
                    cx.update(|cx| cx.reveal_path(&target)).ok();
                })
                .detach();
            }
            NoticeAction::OpenFile(path, line) => {
                self.open_location(path.to_string_lossy().into_owned(), line.map_or(0, |l| l.saturating_sub(1)), 0, window, cx);
            }
            NoticeAction::Retry(kind) => self.retry_operation(kind, window, cx),
        }
    }

    fn retry_operation(&mut self, kind: RetryKind, window: &mut Window, cx: &mut Context<Self>) {
        match kind {
            RetryKind::SaveFile { host, path } => self.retry_save(&host, &path, window, cx),
            RetryKind::WriteSetting { path, key, json, before } => self.retry_write_setting(&path, &key, &json, before, window, cx),
            RetryKind::UpdateCheck => self.check_for_updates(&super::CheckForUpdates, window, cx),
        }
        cx.notify();
    }

    /// Save the file again, but only the file that failed (same machine, same path) and never over another program's edit.
    fn retry_save(&mut self, host: &str, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let shown = path.to_string_lossy().into_owned();
        let same_machine = host == self.history_host();
        let found = self.all_editors().into_iter().find(|ed| {
            let e = ed.read(cx);
            same_machine && e.path == shown && e.uses_host(&self.remote.host)
        });
        let Some(ed) = found else {
            return self.notify(NoticeSource::Files, Notice::Warning(crate::i18n::tf("{} is not open any more; nothing to save.", &[&shown]).into()));
        };
        if ed.update(cx, |e, cx| e.save_unless_changed_on_disk(cx)) {
            return;
        }
        if let Some(ix) = self.tabs.editors.iter().position(|e| *e == ed) {
            self.activate(ix, window, cx);
        }
        self.notify(NoticeSource::Files, Notice::Warning(crate::i18n::t("The file changed on disk; resolve the conflict first.").into()));
    }

    /// Write the setting again, unless the settings file is not the one that failed (another profile is active now) or the setting
    /// was changed in the file meanwhile: a stale value must not replace a newer one.
    fn retry_write_setting(&mut self, path: &Path, key: &str, json: &str, before: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let refuse = |this: &mut Self, message: String| this.notify(NoticeSource::Settings, Notice::Warning(message.into()));
        if self.config_paths().map(|(settings, _)| settings).as_deref() != Some(path) {
            return refuse(self, crate::i18n::tf("The settings file is not {} any more (another profile?). Nothing was written.", &[&path.display()]));
        }
        if value_in_file(path, key) != before {
            return refuse(self, crate::i18n::tf("{} was changed in the settings file since. Nothing was written.", &[&key]));
        }
        match serde_json::from_str(json) {
            Ok(value) => self.write_setting(key, value, window, cx),
            Err(e) => refuse(self, crate::i18n::tf("Could not retry: {}", &[&e])),
        }
    }

    /// What to run to write `key` = `value` into `path` again, remembering what the key was when the write failed.
    pub(in crate::workbench) fn write_setting_retry(path: &Path, key: &str, value: &serde_json::Value) -> RetryKind {
        RetryKind::WriteSetting { path: path.to_path_buf(), key: key.to_string(), json: value.to_string(), before: value_in_file(path, key) }
    }

    /// A write failed: say why in plain words, with the path, a next step and a button to try again.
    pub(in crate::workbench) fn report_save_failed(&mut self, ed: &Entity<EditorView>, error: &host::HostError, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        let host = self.history_host();
        let text = errors::describe_host(error, Path::new(&path), errors::Verb::Save);
        self.notify_with(
            Notification::new(Severity::Error, NoticeSource::Files, text.full())
                .detail(text.detail)
                .id(save_failure_id(&host, &path))
                .action(NoticeAction::Retry(RetryKind::SaveFile { host, path: PathBuf::from(&path) })),
        );
        cx.notify();
    }

    /// A settings file could not be read or written: the path, the cause and a next step, with a button to try the write again
    /// (`retry`) or, when there is nothing to repeat, to show the folder.
    pub(in crate::workbench) fn report_settings_file_failed(&mut self, path: &Path, error: &std::io::Error, verb: errors::Verb, retry: Option<RetryKind>) {
        let text = errors::describe_io(error, path, verb);
        let action = retry.map_or_else(|| NoticeAction::Reveal(path.to_path_buf()), NoticeAction::Retry);
        let id = settings_file_failure_id(path);
        self.notify_with(Notification::new(Severity::Error, NoticeSource::Settings, text.full()).detail(text.detail).id(id).action(action));
    }

    /// The settings file was written: a failure it left behind is out of date.
    pub(in crate::workbench) fn settings_file_written(&mut self, path: &Path) {
        self.dismiss_notice_id(&settings_file_failure_id(path));
    }

    /// Local History could not keep a copy: said once per session (every save would fail the same way).
    pub(in crate::workbench) fn report_history_failed(&mut self, folder: &Path, error: &std::io::Error) {
        let text = errors::describe_io(error, folder, errors::Verb::Write);
        let message = crate::i18n::tf("Local History could not keep a copy of the file you saved. {}", &[&text.full()]);
        let n = Notification::new(Severity::Warning, NoticeSource::Files, message)
            .detail(text.detail)
            .id("history-record-failed")
            .action(NoticeAction::Reveal(folder.to_path_buf()));
        if self.notices.deferred.defer_once("history-record-failed", n) {
            self.post_deferred_notices();
        }
    }

    /// The file was saved after all (a retry, or just another try): the failure it left behind is out of date.
    pub(in crate::workbench) fn save_failure_resolved(&mut self, ed: &Entity<EditorView>, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        let saved_here = self.dismiss_notice_id(&save_failure_id(&self.history_host(), &path));
        // Saving a settings file by hand in its tab puts right a failed write of the same file just as well.
        let saved_settings = self.dismiss_notice_id(&settings_file_failure_id(Path::new(&path)));
        if saved_here || saved_settings {
            cx.notify();
        }
    }

    /// Post what was queued from places that only had `&self` (see `persist_failures.rs`).
    pub(super) fn post_deferred_notices(&mut self) {
        for n in self.notices.deferred.take() {
            self.notify_with(n);
        }
    }

    /// Queue a failed save of `area`; it is posted at the next frame, once per session.
    pub(crate) fn persist_failed(&self, area: failures::Area, err: &dyn std::fmt::Display) {
        self.notices.deferred.persist_failed(area, err, self.persist.data_dir.as_deref());
    }
}

#[cfg(test)]
#[path = "workbench_notification_actions_tests.rs"]
mod tests;
