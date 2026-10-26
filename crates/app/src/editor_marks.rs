//! Editor: bookmarks, breakpoints and the debug line.

use super::*;
use crate::i18n::t;

impl EditorView {
    /// Bookmarked lines (0-based, sorted).
    pub fn bookmark_lines(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.buffer.marks(BOOKMARK_MARKS).iter().map(|&c| self.buffer.char_to_line(c)).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn set_bookmark_lines(&mut self, lines: &[usize], cx: &mut Context<Self>) {
        let n = self.buffer.len_lines();
        let marks = lines.iter().filter(|&&l| l < n).map(|&l| self.buffer.line_to_char(l)).collect();
        self.buffer.set_marks(BOOKMARK_MARKS, marks);
        cx.notify();
    }

    /// Breakpoints: (0-based line, meta), sorted by line, one per line.
    /// Meta is kept in the same order as the marks (marks aren't deleted on edit, they only shift).
    pub fn breakpoints(&self) -> Vec<(usize, BpMeta)> {
        let mut v: Vec<(usize, BpMeta)> = self
            .buffer
            .marks(BREAKPOINT_MARKS)
            .iter()
            .enumerate()
            .map(|(i, &c)| (self.buffer.char_to_line(c), self.bp_meta.get(i).cloned().unwrap_or_default()))
            .collect();
        v.sort_by_key(|(l, _)| *l);
        v.dedup_by_key(|(l, _)| *l);
        v
    }

    #[cfg(test)]
    pub fn breakpoint_lines(&self) -> Vec<usize> {
        self.breakpoints().into_iter().map(|(l, _)| l).collect()
    }

    pub fn set_breakpoints(&mut self, list: &[(usize, BpMeta)], cx: &mut Context<Self>) {
        let n = self.buffer.len_lines();
        let kept: Vec<&(usize, BpMeta)> = list.iter().filter(|(l, _)| *l < n).collect();
        self.buffer.set_marks(BREAKPOINT_MARKS, kept.iter().map(|(l, _)| self.buffer.line_to_char(*l)).collect());
        self.bp_meta = kept.into_iter().map(|(_, m)| m.clone()).collect();
        cx.notify();
    }

    /// Toggle a breakpoint on a line (F9 and gutter click).
    pub fn toggle_breakpoint_at(&mut self, line: usize, cx: &mut Context<Self>) {
        let mut list = self.breakpoints();
        match list.iter().position(|(l, _)| *l == line) {
            Some(i) => {
                list.remove(i);
            }
            None => list.push((line, BpMeta::default())),
        }
        self.set_breakpoints(&list, cx);
        cx.emit(EditorEvent::BreakpointsChanged);
    }

    pub(super) fn toggle_breakpoint(&mut self, _: &ToggleBreakpoint, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.buffer.char_to_line(self.buffer.cursor());
        self.toggle_breakpoint_at(line, cx);
    }

    #[cfg(test)]
    pub fn has_debug_eval(&self) -> bool {
        self.debug_eval.is_some()
    }

    #[cfg(test)]
    pub fn debug_values(&self) -> std::collections::HashMap<usize, String> {
        self.debug_values.clone()
    }

    #[cfg(test)]
    pub fn toggle_bookmark_for_test(&mut self, line: usize, cx: &mut Context<Self>) {
        let mut lines = self.bookmark_lines();
        lines.push(line);
        self.set_bookmark_lines(&lines, cx);
        cx.emit(EditorEvent::BookmarksChanged);
    }

    #[cfg(test)]
    pub fn debug_line(&self) -> Option<usize> {
        self.debug_line
    }

    /// Program stopped (or resumed): session and frame for showing values on hover.
    pub fn set_debug_eval(&mut self, eval: Option<(Arc<crate::dap::Session>, i64)>) {
        self.debug_eval = eval;
    }

    /// Expression under the mouse: an `a.b.c` chain (words joined by dots and `->`); none if just a number.
    pub(super) fn debug_expression_at(&self, ix: usize) -> Option<String> {
        let line = self.buffer.char_to_line(ix);
        let chars: Vec<char> = self.buffer.line(line).chars().collect();
        expression_in_line(&chars, ix.checked_sub(self.buffer.line_to_char(line))?)
    }

    /// Show the stopped frame's locals at line ends (empty: clear).
    pub fn set_debug_values(&mut self, values: std::collections::HashMap<usize, String>, cx: &mut Context<Self>) {
        if self.debug_values != values {
            self.debug_values = values;
            cx.notify();
        }
    }

    /// The debugger stopped at this line (`None`: running or finished).
    pub fn set_debug_line(&mut self, line: Option<usize>, cx: &mut Context<Self>) {
        if self.debug_line != line {
            self.debug_line = line;
            cx.notify();
        }
    }

    /// Alt+Cmd+K: toggle a bookmark on the cursor's line.
    pub(super) fn toggle_bookmark(&mut self, _: &ToggleBookmark, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let mut lines = self.bookmark_lines();
        match lines.iter().position(|&l| l == line) {
            Some(i) => {
                lines.remove(i);
            }
            None => lines.push(line),
        }
        self.set_bookmark_lines(&lines, cx);
        cx.emit(EditorEvent::BookmarksChanged);
    }

    /// Alt+Cmd+L / Alt+Cmd+J: next/previous bookmark in the file (wraps).
    pub(super) fn jump_bookmark(&mut self, forward: bool, cx: &mut Context<Self>) {
        let lines = self.bookmark_lines();
        let cur = self.buffer.char_to_line(self.buffer.cursor());
        let target = if forward {
            lines.iter().copied().find(|&l| l > cur).or(lines.first().copied())
        } else {
            lines.iter().rev().copied().find(|&l| l < cur).or(lines.last().copied())
        };
        let Some(line) = target else {
            cx.emit(EditorEvent::Message(t("No bookmarks in this file (⌥⌘K adds one)").into()));
            return;
        };
        self.buffer.set_cursor(self.buffer.line_to_char(line), false);
        self.after_move(cx);
    }

    pub(super) fn next_bookmark(&mut self, _: &NextBookmark, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_bookmark(true, cx);
    }

    pub(super) fn previous_bookmark(&mut self, _: &PreviousBookmark, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_bookmark(false, cx);
    }

    pub(super) fn clear_bookmarks(&mut self, _: &ClearBookmarks, _: &mut Window, cx: &mut Context<Self>) {
        self.set_bookmark_lines(&[], cx);
        cx.emit(EditorEvent::BookmarksChanged);
    }
}
