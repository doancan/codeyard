//! Workbench: what the command line asks for once the window is up.

use super::*;

impl Workbench {
    /// `codeyard -g FILE:LINE:COLUMN`: opens `path` like `open_path` and puts the cursor at `at` (line and column from 1).
    pub fn open_path_at(&mut self, path: &str, at: Option<(usize, usize)>, window: &mut Window, cx: &mut Context<Self>) {
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.into());
        self.open_path(path, window, cx);
        // After the open: moving to the file's folder starts the window's tabs over, and with them any wish kept in them.
        if let Some((line, col)) = at.filter(|_| abs.is_file()) {
            self.tabs.pending_reveal = Some((ui::local_path(&abs), line as u32, col.saturating_sub(1) as u32, 0));
        }
    }
}
