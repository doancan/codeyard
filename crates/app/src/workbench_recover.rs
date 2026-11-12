//! Workbench: keeping what would otherwise be lost (versions overwritten by a forced save) and recovering from a damaged state database.

use super::notifications_glue::errors;
use super::notifications_glue::model::{NoticeAction, Notification, Severity};
use super::*;

impl Workbench {
    /// A window on the state database at `path` (none when it is unavailable: the cause is shown as a notice, not just logged).
    pub fn new_at(path: Option<std::path::PathBuf>, config_dir: Option<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (state, problem) = path.as_deref().map_or((None, None), open_state_recovering);
        let mut wb = Self::with_state(state, config_dir, window, cx);
        wb.persist.data_dir = path.and_then(|p| p.parent().map(|d| d.to_path_buf()));
        if let Some(problem) = problem {
            wb.notify_with(problem.into_notification());
        }
        wb
    }

    /// A forced save just replaced `text` on disk: put it in local history so the other version can be brought back.
    pub(super) fn keep_overwritten(&mut self, ed: &Entity<EditorView>, text: &str, cx: &mut Context<Self>) {
        let Some(data) = self.persist.data_dir.clone() else { return };
        let dir = crate::history::dir_for(&data, &self.history_host(), &ed.read(cx).path);
        // Written here rather than in the background: the save that follows records its own copy, and two writers could pick the same
        // millisecond for the file name and silently replace one another.
        match crate::history::record(&dir, text) {
            Ok(true) => {
                self.timeline.rev += 1;
                self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("The version that was on disk has been kept in Local History.").into()));
            }
            Ok(false) => {}
            Err(e) => {
                let text = errors::describe_io(&e, &dir, errors::Verb::Write);
                let message = crate::i18n::tf("The version that was on disk could not be kept in Local History. {}", &[&text.full()]);
                self.notify_with(Notification::new(Severity::Error, NoticeSource::App, message).detail(text.detail).action(NoticeAction::Reveal(dir)));
            }
        }
        cx.notify();
    }
}

/// What went wrong with the state database, ready to be shown: the message names the file, the detail has the raw cause and the
/// button shows the folder it lives in.
pub(super) struct StateProblem {
    message: String,
    detail: String,
    folder: std::path::PathBuf,
}

impl StateProblem {
    fn new(path: &std::path::Path, message: String, cause: String) -> Self {
        let folder = path.parent().map(std::path::Path::to_path_buf).unwrap_or_else(|| path.to_path_buf());
        Self { message, detail: format!("{}\n{cause}", path.display()), folder }
    }

    fn into_notification(self) -> Notification {
        Notification::new(Severity::Error, NoticeSource::App, self.message).detail(self.detail).action(NoticeAction::Reveal(self.folder)).id("state-database")
    }
}

/// Open the state database. A file that is not a database (damaged, overwritten) is moved aside as `state.db.corrupt-<unix time>` and
/// replaced by a fresh one, so the app does not run without saving anything for good. Anything else, such as a database written by a newer
/// version, is left alone. The text explains what happened when the user should hear about it.
pub(super) fn open_state_recovering(path: &std::path::Path) -> (Option<State>, Option<StateProblem>) {
    let failed = |e: &dyn std::fmt::Display| {
        eprintln!("could not open state database ({}): {e}", path.display());
        let cause = e.to_string().replace(&format!(": {}", path.display()), "");
        StateProblem::new(
            path,
            crate::i18n::tf(
                "Could not open the state database {0}: {1}. Tabs, recent folders and backups will not be remembered this session.",
                &[&path.display(), &cause],
            ),
            cause,
        )
    };
    let e = match State::open(path) {
        Ok(s) => return (Some(s), None),
        Err(e) => e,
    };
    if !state::is_corrupt(&e) {
        return (None, Some(failed(&e)));
    }
    let to = match move_aside(path) {
        Ok(to) => to,
        Err(why) => {
            let text = errors::describe_io(&why, path, errors::Verb::Rename);
            let message = crate::i18n::tf(
                "The state database {0} is damaged ({1}) and could not be moved aside. {2} Tabs, recent folders and backups will not be remembered this session.",
                &[&path.display(), &e, &text.full()],
            );
            return (None, Some(StateProblem::new(path, message, format!("{e}\n{}", text.detail))));
        }
    };
    match State::open(path) {
        Ok(s) => {
            let note = crate::i18n::tf(
                "The state database was damaged ({0}). It was moved to {1} and a new one was started; recent folders and open tabs were forgotten.",
                &[&e, &to.display()],
            );
            (Some(s), Some(StateProblem::new(path, note, e.to_string())))
        }
        Err(e) => (None, Some(failed(&e))),
    }
}

/// Rename the database and its write-ahead files so a half-written journal cannot be applied to the new file.
fn move_aside(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let to = path.with_file_name(format!("{name}.corrupt-{at}"));
    std::fs::rename(path, &to)?;
    for ext in ["-wal", "-shm"] {
        let _ = std::fs::rename(path.with_file_name(format!("{name}{ext}")), path.with_file_name(format!("{name}.corrupt-{at}{ext}")));
    }
    Ok(to)
}
