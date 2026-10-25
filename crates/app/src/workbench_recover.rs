//! Workbench: keeping what would otherwise be lost (versions overwritten by a forced save).

use super::*;

impl Workbench {
    /// A forced save just replaced `text` on disk: put it in local history so the other version can be brought back.
    pub(super) fn keep_overwritten(&mut self, ed: &Entity<EditorView>, text: &str, cx: &mut Context<Self>) {
        let Some(data) = self.persist.data_dir.clone() else { return };
        let dir = crate::history::dir_for(&data, &self.history_host(), &ed.read(cx).path);
        // Written here rather than in the background: the save that follows records its own copy, and two writers could pick the same
        // millisecond for the file name and silently replace one another.
        match crate::history::record(&dir, text) {
            Ok(true) => {
                self.timeline.rev += 1;
                self.notice = Some(Notice::Info(crate::i18n::t("The version that was on disk has been kept in Local History.").into()));
            }
            Ok(false) => {}
            Err(e) => self.notice = Some(Notice::Error(crate::i18n::tf("Could not keep a copy of the version that was on disk: {}", &[&e]).into())),
        }
        cx.notify();
    }
}
