//! What the terminal does on its own account, outside the shell's output: getting out of a mode a program left on, and
//! typing the paths of files dropped on it.

use super::*;

impl TerminalView {
    /// Get out of a mode a program left on: a dropped SSH connection or a killed program (Claude Code, tmux, vim) cannot
    /// switch off the mouse reporting, the alternate screen or the application keys it had turned on, and the terminal
    /// then types `^[[<0;12;5M` at every click. Text and history stay; the shell is not told anything.
    pub(super) fn reset(&mut self, _: &TermReset, _: &mut Window, cx: &mut Context<Self>) {
        self.reset_modes();
        cx.notify();
    }

    pub(super) fn reset_modes(&mut self) {
        // Mouse reporting (X10, button, any-motion, focus, UTF-8, SGR, urxvt), alternate screen, application cursor keys
        // and keypad, shown cursor, default colours and attributes.
        self.feed(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1049l\x1b[?1l\x1b>\x1b[?25h\x1b[0m");
        self.wheel = Default::default();
        self.selecting = false;
        self.term.scroll_display(Scroll::Bottom);
    }

    /// Files dropped on the terminal: their paths are typed at the cursor, quoted, separated by spaces. A remote shell cannot
    /// see this machine's files, so there the drop does nothing.
    pub(super) fn drop_paths(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        if self.host.is_remote() || self.exit.is_some() || self.error.is_some() {
            return;
        }
        let quoted: Vec<String> = paths.iter().filter_map(|p| crate::terminal_paste::quote_path(&p.to_string_lossy(), cfg!(windows))).collect();
        if quoted.is_empty() {
            return;
        }
        let bracketed = self.term.mode().contains(TermMode::BRACKETED_PASTE);
        let text = format!("{} ", quoted.join(" "));
        self.paste_confirmed(crate::terminal_paste::prepare(&text, bracketed, crate::terminal_paste::Sanitize::Always).data);
        cx.notify();
    }
}
