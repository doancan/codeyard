//! A terminal started in place of one from the previous session: what is saved of it (a fixed title) and the notice it
//! shows. Nothing of the old session is run again; the notice is output of the view, never input to the shell.

use super::*;

impl TerminalView {
    /// Whether a shell runs in it (a task's output terminal has none).
    pub fn has_shell(&self) -> bool {
        self.has_shell
    }

    /// The title, when it was set on purpose (a rename or a task) rather than by the shell.
    pub fn fixed_title(&self) -> Option<String> {
        self.title_locked.then(|| self.title.to_string())
    }

    /// Show, dimmed, that this shell replaces one of the previous session.
    pub fn show_restored_notice(&mut self, cx: &mut Context<Self>) {
        let line = crate::i18n::t("Previous session ended; started a new shell");
        self.feed(format!("\x1b[2m{line}\x1b[0m\r\n").as_bytes());
        cx.notify();
    }
}
