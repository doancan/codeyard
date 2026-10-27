//! Selection and line transformations: change case, join, sort, reverse, remove duplicate lines, duplicate the selection,
//! transpose characters (part of `impl Buffer`; accesses the private fields of `lib.rs`).

use super::*;

impl Buffer {
    /// Transform the selected text (the word at the cursor if no selection); the result stays selected.
    pub fn transform_selections(&mut self, f: impl Fn(&str) -> String) {
        let mut parts: Vec<(Range<usize>, String)> = Vec::new();
        let mut changed = false;
        for s in &self.sels {
            let r = if s.selection().is_some() { s.range() } else { self.word_at(s.cursor).unwrap_or(s.cursor..s.cursor) };
            if r.is_empty() || parts.last().is_some_and(|(p, _)| r.start < p.end) {
                continue;
            }
            let before = self.text_range(r.clone());
            let text = f(&before);
            changed |= text != before;
            parts.push((r, text));
        }
        // If nothing changed (already uppercase, single line...) no undo step or "changed" marker is created.
        if parts.is_empty() || !changed {
            return;
        }
        let lens: Vec<usize> = parts.iter().map(|(_, t)| t.chars().count()).collect();
        self.edit_parts(parts, Place::Custom(&|i, start| Sel { cursor: start + lens[i], anchor: Some(start), goal: None }), false);
    }

    /// ⌃J: join the selected lines (with the next one if no selection) into one line; the intervening
    /// indentation and trailing whitespace collapse to a single space.
    pub fn join_lines(&mut self) {
        let mut parts = Vec::new();
        for (first, last) in self.line_blocks() {
            let last = if first == last { first + 1 } else { last };
            for l in first..last.min(self.len_lines().saturating_sub(1)) {
                let (this, next) = (self.line(l), self.line(l + 1));
                let trailing = this.chars().rev().take_while(|c| *c == ' ' || *c == '\t').count();
                let leading = next.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                let start = self.rope.line_to_char(l) + self.line_len_chars(l) - trailing;
                let stop = self.rope.line_to_char(l + 1) + leading;
                let sep = if this.trim().is_empty() || next.trim().is_empty() { "" } else { " " };
                parts.push((start..stop, sep.to_string()));
            }
        }
        if !parts.is_empty() {
            self.apply_edits(parts);
        }
    }

    /// Sort the selected lines (ascending or descending); each block stays selected.
    pub fn sort_lines(&mut self, descending: bool) {
        let mut parts: Vec<(Range<usize>, String)> = Vec::new();
        let mut changed = false;
        for (first, last) in self.line_blocks() {
            if first == last {
                continue;
            }
            let mut lines: Vec<String> = (first..=last).map(|l| self.line(l)).collect();
            lines.sort_by_cached_key(|l| (collation_key(l), l.clone()));
            if descending {
                lines.reverse();
            }
            let range = self.rope.line_to_char(first)..self.rope.line_to_char(last) + self.line_len_chars(last);
            let sorted = lines.join("\n");
            changed |= self.rope.slice(range.clone()) != sorted.as_str();
            parts.push((range, sorted));
        }
        // If already sorted, no undo step or "changed" marker should be created.
        if parts.is_empty() || !changed {
            return;
        }
        let lens: Vec<usize> = parts.iter().map(|(_, t)| t.chars().count()).collect();
        self.edit_parts(parts, Place::Custom(&|i, start| Sel { cursor: start + lens[i], anchor: Some(start), goal: None }), false);
    }

    /// Reverse the order of the selected lines (does nothing without a selection).
    pub fn reverse_lines(&mut self) {
        self.rewrite_blocks(true, |lines| lines.into_iter().rev().collect());
    }

    /// Remove duplicates among the selected lines (the first stays). Whole file if no selection.
    pub fn delete_duplicate_lines(&mut self) {
        self.rewrite_blocks(false, |lines| {
            let mut seen = std::collections::HashSet::new();
            lines.into_iter().filter(|l| seen.insert(l.clone())).collect()
        });
    }

    /// Duplicate the selection right after itself (the copy stays selected); if no selection, copy the line downward.
    pub fn duplicate_selection(&mut self) {
        if self.sels.iter().all(|s| s.selection().is_none()) {
            return self.copy_lines(true);
        }
        let parts: Vec<(Range<usize>, String)> = self
            .sels
            .iter()
            .filter(|s| s.selection().is_some())
            .map(|s| {
                let r = s.range();
                (r.end..r.end, self.text_range(r))
            })
            .collect();
        let lens: Vec<usize> = parts.iter().map(|(_, t)| t.chars().count()).collect();
        self.edit_parts(parts, Place::Custom(&|i, start| Sel { cursor: start + lens[i], anchor: Some(start), goal: None }), false);
    }

    /// ⌃T: swap the two characters at the cursor (the previous two at the end of a line).
    pub fn transpose_chars(&mut self) {
        let mut parts: Vec<(Range<usize>, String)> = Vec::new();
        let mut cursors: Vec<usize> = Vec::new();
        for s in &self.sels {
            if s.selection().is_some() {
                continue;
            }
            let c = s.cursor;
            let line = self.rope.char_to_line(c);
            let (ls, le) = (self.rope.line_to_char(line), self.rope.line_to_char(line) + self.line_len_chars(line));
            // At line end, the previous two; at line start, nothing to do.
            let at = if c >= le { le.saturating_sub(1) } else { c };
            if at <= ls || at >= le {
                continue;
            }
            let (a, b) = (self.rope.char(at - 1), self.rope.char(at));
            if parts.last().is_some_and(|(p, _)| at - 1 < p.end) {
                continue;
            }
            parts.push((at - 1..at + 1, format!("{b}{a}")));
            cursors.push(at + 1);
        }
        if parts.is_empty() {
            return;
        }
        self.edit_parts(parts, Place::Custom(&|i, _| Sel { cursor: cursors[i], anchor: None, goal: None }), false);
    }
}
