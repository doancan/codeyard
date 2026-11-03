//! What a terminal contributes to the saved session (see `terminal_session`).

use super::TerminalView;
use crate::terminal_session::SavedTerminal;

impl TerminalView {
    /// What a restart needs to start this terminal again as a new shell; output-only terminals have no shell (`None`).
    pub fn saved(&self, history_lines: usize) -> Option<SavedTerminal> {
        let title = self.title_locked.then(|| self.title.to_string());
        // The whole history is read only when some of it is kept.
        let history = if history_lines > 0 { self.scrollback_text() } else { String::new() };
        self.has_shell.then(|| SavedTerminal {
            token: self.token.clone(),
            ..SavedTerminal::of(self.cwd.clone(), self.launched.clone(), self.color.clone(), title, &history, history_lines)
        })
    }
}
