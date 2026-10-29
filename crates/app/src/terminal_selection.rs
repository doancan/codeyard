//! The selection across output, and copying it when it is made.
//!
//! Programs that redraw in place (Claude Code, Codex, spinners, status lines) erase and rewrite the selected lines on
//! every frame, and the parser drops a selection an erase touches. The selected rows' text is noted before the output,
//! and looked for afterwards where it was and above it (the output may have scrolled): found, the selection comes back on
//! the same text; not found, the text really changed and it stays dropped.
//!
//! `terminal.copyOnSelection` copies a finished selection: "agents" only while Claude Code or Codex runs in the terminal
//! (they take Ctrl+C for themselves), "always", or "off".

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::selection::Selection;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use gpui::{ClipboardItem, Context};

use super::TerminalView;

/// Most lines the selection is followed up by one chunk of output.
const MAX_FOLLOW: usize = 10_000;
/// How long "Hold Shift to select" stays, and how long before it shows again.
const HINT_SHOWN: std::time::Duration = std::time::Duration::from_secs(3);
const HINT_AGAIN: std::time::Duration = std::time::Duration::from_secs(30);

/// A selection as it was before some output: its rows' text, and the screen it was on.
pub(super) struct Kept {
    selection: Selection,
    top: Line,
    rows: Vec<String>,
    alt_screen: bool,
}

impl TerminalView {
    fn row_text(&self, line: Line) -> String {
        let row = &self.term.grid()[line];
        let text: String = (0..self.term.columns()).map(|c| &row[Column(c)]).filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER)).map(|c| c.c).collect();
        text.trim_end().to_string()
    }

    /// Note the selection before output is parsed (nothing to note without one, or when its rows are all blank: blank
    /// rows would be found anywhere).
    pub(super) fn keep_selection(&self) -> Option<Kept> {
        let selection = self.term.selection.clone().filter(|s| !s.is_empty())?;
        let range = selection.to_range(&self.term)?;
        let rows: Vec<String> = (range.start.line.0..=range.end.line.0).map(|l| self.row_text(Line(l))).collect();
        if rows.iter().all(|r| r.is_empty()) {
            return None;
        }
        Some(Kept { selection, top: range.start.line, rows, alt_screen: self.term.mode().contains(TermMode::ALT_SCREEN) })
    }

    /// After the output: bring a dropped selection back onto its text, `scrolled` lines up at most.
    pub(super) fn restore_selection(&mut self, kept: Option<Kept>, scrolled: usize) {
        let Some(kept) = kept else { return };
        if self.term.selection.is_some() || kept.alt_screen != self.term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let topmost = self.term.topmost_line();
        let bottom = self.term.bottommost_line();
        let fits = |shift: i32| {
            let top = kept.top - shift;
            top >= topmost && top + (kept.rows.len() as i32 - 1) <= bottom
        };
        let same = |shift: i32| kept.rows.iter().enumerate().all(|(i, row)| self.row_text(kept.top - shift + i as i32) == *row);
        let found = (0..=scrolled.min(MAX_FOLLOW) as i32).take_while(|&s| fits(s)).find(|&s| same(s));
        if let Some(shift) = found {
            let screen = Line(self.term.screen_lines() as i32);
            self.term.selection = kept.selection.rotate(&self.term, &(Line(0)..screen), shift);
        }
    }

    /// A drag went to the program (it reads the mouse) although the user may have meant to select: say Shift selects.
    pub(super) fn show_shift_hint(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        if self.shift_hint || self.shift_hint_last.is_some_and(|t| now - t < HINT_AGAIN) {
            return;
        }
        self.shift_hint = true;
        self.shift_hint_last = Some(now);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HINT_SHOWN).await;
            this.update(cx, |this, cx| {
                this.shift_hint = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The mouse let go of a selection: copy it when `terminal.copyOnSelection` says so.
    pub(super) fn copy_on_select(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.term.selection_to_string().filter(|s| !s.is_empty()) else { return };
        match cx.global::<crate::settings::Settings>().terminal_copy_on_selection.as_str() {
            "always" => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            "agents" if self.runs_agent_here() => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            // Without shell integration or a title to go by, ask the host what holds the terminal.
            "agents" => {
                let Some((host, id)) = self.pty_handle() else { return };
                let ask = cx.background_executor().spawn(async move { host.pty_status(id).name });
                cx.spawn(async move |this, cx| {
                    let name = ask.await;
                    this.update(cx, |this, cx| {
                        let still = this.term.selection_to_string().is_some_and(|s| s == text);
                        if still && name.as_deref().is_some_and(is_agent) {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                    })
                    .ok();
                })
                .detach();
            }
            _ => {}
        }
    }

    /// Claude Code or Codex runs here, by what the terminal already knows: the command typed (shell integration) or the
    /// title the program set.
    fn runs_agent_here(&self) -> bool {
        let typed = match self.activity() {
            super::Activity::Running(typed) => typed,
            _ => String::new(),
        };
        typed.split(|c: char| c.is_whitespace() || c == '/' || c == '\\').any(is_agent) || {
            let title = self.title.to_lowercase();
            title.contains("claude code") || title.contains("codex")
        }
    }
}

/// `claude`, `codex` (with `.exe` or `.cmd` on Windows).
pub(super) fn is_agent(word: &str) -> bool {
    let word = word.to_ascii_lowercase();
    let stem = word.strip_suffix(".exe").or_else(|| word.strip_suffix(".cmd")).unwrap_or(&word);
    stem == "claude" || stem == "codex"
}

/// How many lines `data` may have scrolled a terminal of `screen_lines`: one per line feed, plus a screen for scroll
/// escapes (`CSI n S`, index) that move lines without one.
pub(super) fn scroll_bound(data: &[u8], screen_lines: usize) -> usize {
    data.iter().filter(|&&b| matches!(b, b'\n' | 0x0b | 0x0c)).count() + screen_lines
}
