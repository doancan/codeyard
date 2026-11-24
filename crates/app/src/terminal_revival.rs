//! A terminal started in place of one from the previous session: what is saved of it (a fixed title). Nothing of the
//! old session is run again.

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
}
