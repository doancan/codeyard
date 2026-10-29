//! What a terminal contributes to the saved session (see `terminal_session`).

use super::TerminalView;
use crate::terminal_session::SavedTerminal;

impl TerminalView {
    /// What a restart needs to start this terminal again as a new shell; output-only terminals have no shell (`None`).
    pub fn saved(&self, history_lines: usize) -> Option<SavedTerminal> {
        let title = self.title_locked.then(|| self.title.to_string());
        self.has_shell.then(|| SavedTerminal::of(self.cwd.clone(), self.launched.clone(), self.color.clone(), title, &self.scrollback_text(), history_lines))
    }
}
