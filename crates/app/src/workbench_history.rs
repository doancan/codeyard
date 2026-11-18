//! Workbench: what Local History may keep. `history.rs` holds the rules (secret files, the user's patterns, deleting); this
//! decides for each save whether a copy is written, asked about first or left out, and deletes copies on request.

use super::notifications_glue::model::{NoticeAction, Notification, Severity};
use super::*;

/// What a save of a file does to Local History.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Gate {
    Keep,
    /// A server's file and the server has not been decided yet (`localHistory.remote: "ask"`): nothing is written until it is.
    Ask,
    Skip(Skip),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Skip {
    /// `localHistory.enabled` is off.
    Off,
    /// A secret file of the built-in list, or one the user's `localHistory.exclude` names.
    Excluded,
    /// A server's file, and copies of those are not kept (`"off"`, or the answer "Don't Keep").
    Server,
}

const ANSWER_KEEP: &str = "keep";
const ANSWER_OFF: &str = "off";

/// What *Local History: Delete…* removes.
enum Scope {
    File,
    All,
}

impl Workbench {
    /// The stored answer for a server, kept in the state database like other one-time choices.
    fn history_answer_key(host: &str) -> String {
        format!("localHistory.remote:{host}")
    }

    /// What a save of `path` does, from the settings and, for a server's file under `"ask"`, the stored answer for that server.
    pub(super) fn history_gate(&self, path: &str, cx: &App) -> Gate {
        let settings = cx.global::<crate::settings::Settings>();
        if !settings.local_history {
            return Gate::Skip(Skip::Off);
        }
        if !crate::history::keeps(path, &settings.local_history_exclude) {
            return Gate::Skip(Skip::Excluded);
        }
        let host = self.history_host();
        if host.is_empty() {
            return Gate::Keep;
        }
        match settings.local_history_remote {
            crate::history::Remote::Keep => Gate::Keep,
            crate::history::Remote::Off => Gate::Skip(Skip::Server),
            crate::history::Remote::Ask => match self.persist.state.as_ref().and_then(|s| s.get(&Self::history_answer_key(&host))).as_deref() {
                Some(ANSWER_KEEP) => Gate::Keep,
                Some(_) => Gate::Skip(Skip::Server),
                None => Gate::Ask,
            },
        }
    }

    /// After a save: keep a copy of the file (in the background), refresh TIMELINE. A file Local History does not keep loses
    /// the copies it has from before.
    pub(super) fn record_history(&mut self, ed: &Entity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        let (path, lines) = {
            let e = ed.read(cx);
            (e.path.clone(), e.line_count())
        };
        let gate = self.history_gate(&path, cx);
        match gate {
            Gate::Skip(Skip::Excluded) => return self.forget_history_of(&path, cx),
            Gate::Skip(_) => return,
            Gate::Keep | Gate::Ask => {}
        }
        if self.persist.data_dir.is_none() || lines > 60_000 {
            return;
        }
        let text = ed.read(cx).text_lines().join("\n");
        match gate {
            Gate::Ask => self.ask_remote_history(path, text, window, cx),
            _ => self.write_history(&path, text, cx),
        }
    }

    /// Write a copy in the background.
    fn write_history(&mut self, path: &str, text: String, cx: &mut Context<Self>) {
        let Some(data) = self.persist.data_dir.clone() else { return };
        let dir = crate::history::dir_for(&data, &self.history_host(), path);
        let folder = dir.clone();
        let task = cx.background_executor().spawn(async move { crate::history::record(&dir, &text) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(true) => this.timeline.rev += 1,
                    Ok(false) => {}
                    Err(e) => this.report_history_failed(&folder, &e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The copies a file already has are removed once Local History stops keeping it.
    fn forget_history_of(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(data) = self.persist.data_dir.clone() else { return };
        let dir = crate::history::dir_for(&data, &self.history_host(), path);
        if !dir.exists() {
            return;
        }
        let deleted = crate::history::delete_file(&dir);
        if deleted.copies > 0 {
            self.timeline.rev += 1;
        }
        if deleted.failed > 0 {
            let message = crate::i18n::tn(
                "{} earlier copy of this file could not be deleted from Local History, which does not keep it any more.",
                "{} earlier copies of this file could not be deleted from Local History, which does not keep it any more.",
                deleted.failed,
                &[],
            );
            self.notify_with(Notification::new(Severity::Warning, NoticeSource::Files, message).id("history-forget-failed").action(NoticeAction::Reveal(dir)));
        }
        cx.notify();
    }

    /// Ask once per server whether its files' copies may be kept on this computer. Nothing is written before the answer; the
    /// save that raised the question is written after a *Keep*, if the settings and the server still allow it by then.
    fn ask_remote_history(&mut self, path: String, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.history_host();
        if !self.persist.history_asking.insert(host.clone()) {
            return;
        }
        let answer = crate::questions::remote_history(&host).ask(window, cx);
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| {
                this.persist.history_asking.remove(&host);
                // A dialog that went away unanswered decides nothing: the next save asks again.
                let Some(choice) = answer else { return };
                let keep = choice == 0;
                let key = Self::history_answer_key(&host);
                if let Some(Err(e)) = this.persist.state.as_ref().map(|s| s.set(&key, if keep { ANSWER_KEEP } else { ANSWER_OFF })) {
                    eprintln!("could not remember {key}: {e}");
                }
                if keep && this.history_host() == host && this.history_gate(&path, cx) == Gate::Keep {
                    this.write_history(&path, text, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A forced save just replaced `text` on disk: put it in Local History so the other version can be brought back, if Local
    /// History keeps this file. Under the same rules as a save: a file it leaves out leaves its overwritten version out too.
    pub(super) fn keep_overwritten(&mut self, ed: &Entity<EditorView>, text: &str, cx: &mut Context<Self>) {
        let Some(data) = self.persist.data_dir.clone() else { return };
        let path = ed.read(cx).path.clone();
        match self.history_gate(&path, cx) {
            Gate::Keep => {}
            Gate::Skip(Skip::Off) => return,
            Gate::Skip(Skip::Excluded) => {
                self.notify(
                    NoticeSource::App,
                    Notice::Info(crate::i18n::t("The version that was on disk was not kept: Local History does not keep this file.").into()),
                );
                return cx.notify();
            }
            Gate::Skip(Skip::Server) | Gate::Ask => {
                self.notify(
                    NoticeSource::App,
                    Notice::Info(crate::i18n::t("The version that was on disk was not kept: Local History does not keep this server's files.").into()),
                );
                return cx.notify();
            }
        }
        let dir = crate::history::dir_for(&data, &self.history_host(), &path);
        // Written here rather than in the background: the save that follows records its own copy, and two writers could pick the same
        // millisecond for the file name and silently replace one another.
        match crate::history::record(&dir, text) {
            Ok(true) => {
                self.timeline.rev += 1;
                self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("The version that was on disk has been kept in Local History.").into()));
            }
            Ok(false) => {}
            Err(e) => {
                let text = super::notifications_glue::errors::describe_io(&e, &dir, super::notifications_glue::errors::Verb::Write);
                let message = crate::i18n::tf("The version that was on disk could not be kept in Local History. {}", &[&text.full()]);
                self.notify_with(Notification::new(Severity::Error, NoticeSource::App, message).detail(text.detail).action(NoticeAction::Reveal(dir)));
            }
        }
        cx.notify();
    }

    /// Whether TIMELINE should say that Local History is not kept for the active file.
    pub(super) fn history_off_for(&self, path: &str, cx: &App) -> bool {
        matches!(self.history_gate(path, cx), Gate::Skip(_))
    }

    /// *Local History: Delete…*: this file's copies or all of them, after a question.
    pub(super) fn delete_local_history(&mut self, _: &DeleteLocalHistory, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.persist.data_dir.clone() else {
            self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("Local History is not available in this window.").into()));
            return cx.notify();
        };
        let file = self.active_editor().map(|ed| ed.read(cx).path.clone());
        let name = file.as_deref().map(ui::file_name);
        let answer = crate::questions::delete_history(name).ask(window, cx);
        let host = self.history_host();
        cx.spawn(async move |this, cx| {
            let scope = match (answer.await, &file) {
                (Some(0), Some(_)) => Scope::File,
                (Some(0), None) | (Some(1), Some(_)) => Scope::All,
                _ => return,
            };
            let deleted = cx
                .background_executor()
                .spawn(async move {
                    match (scope, file) {
                        (Scope::File, Some(path)) => crate::history::delete_file(&crate::history::dir_for(&data, &host, &path)),
                        _ => crate::history::delete_all(&data),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.timeline.rev += 1;
                this.report_history_deleted(&deleted, cx);
            })
            .ok();
        })
        .detach();
    }

    fn report_history_deleted(&mut self, deleted: &crate::history::Deleted, cx: &mut Context<Self>) {
        if deleted.failed > 0 {
            let message = crate::i18n::tf("Local History: {0} copies deleted, {1} could not be deleted.", &[&deleted.copies, &deleted.failed]);
            self.notify_with(Notification::new(Severity::Warning, NoticeSource::App, message));
        } else if deleted.copies == 0 {
            self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("Local History has nothing to delete.").into()));
        } else {
            let message = crate::i18n::tn("Deleted {} copy from Local History.", "Deleted {} copies from Local History.", deleted.copies, &[]);
            self.notify(NoticeSource::App, Notice::Info(message.into()));
        }
        cx.notify();
    }
}

#[cfg(test)]
#[path = "workbench_history_tests.rs"]
mod tests;
