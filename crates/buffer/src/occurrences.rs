//! Selecting occurrences of the selection or the word at the cursor: the next, the previous and all of them (part of
//! `impl Buffer`; accesses the private fields of `lib.rs`).

use super::*;

impl Buffer {
    /// ⌘D: with no selection, select the word at the cursor; otherwise also add a selection at the next
    /// occurrence of the selected text (wrapping around at the end, skipping already-selected ones).
    /// `true` if a new selection was added.
    pub fn select_next_occurrence(&mut self) -> bool {
        let sel = *self.sel();
        let Some(range) = sel.selection() else {
            let Some(word) = self.word_at(sel.cursor) else { return false };
            self.sels[self.primary] = Sel { cursor: word.end, anchor: Some(word.start), goal: None };
            self.normalize();
            self.word_search = true;
            return true;
        };
        let needle = self.rope.slice(range).to_string();
        let all = self.occurrences(&needle, self.word_search);
        let from = self.sels.iter().map(Sel::end).max().unwrap_or(0);
        // First unselected match after the cursor; wraps around at the end.
        // Selections are sorted and non-overlapping: binary search looks at one neighbor at most (previously all selections were
        // scanned for every candidate; with tens of thousands of matches ⌘D took seconds).
        let free = |r: &Range<usize>| {
            let i = self.sels.partition_point(|s| s.end() <= r.start);
            !self.sels.get(i).is_some_and(|s| s.start() < r.end)
        };
        let next = all.iter().find(|r| r.start >= from && free(r)).or_else(|| all.iter().find(|r| free(r))).cloned();
        match next {
            Some(r) => {
                let word = self.word_search;
                self.add_sel(Sel { cursor: r.end, anchor: Some(r.start), goal: None });
                self.word_search = word;
                true
            }
            None => false,
        }
    }

    /// ⌘K ⌘D: also select the previous occurrence of the selected text (continuing from the end when reaching the start).
    pub fn select_previous_occurrence(&mut self) -> bool {
        let sel = *self.sel();
        let Some(range) = sel.selection() else {
            let Some(word) = self.word_at(sel.cursor) else { return false };
            self.sels[self.primary] = Sel { cursor: word.end, anchor: Some(word.start), goal: None };
            self.normalize();
            self.word_search = true;
            return true;
        };
        let needle = self.rope.slice(range).to_string();
        let all = self.occurrences(&needle, self.word_search);
        let from = self.sels.iter().map(Sel::start).min().unwrap_or(0);
        let free = |r: &Range<usize>| !self.sels.iter().any(|s| s.start() < r.end && r.start < s.end());
        let prev = all.iter().rev().find(|r| r.end <= from && free(r)).or_else(|| all.iter().rev().find(|r| free(r))).cloned();
        match prev {
            Some(r) => {
                let word = self.word_search;
                self.add_sel(Sel { cursor: r.start, anchor: Some(r.end), goal: None });
                self.word_search = word;
                true
            }
            None => false,
        }
    }

    /// ⇧⌘L: select all occurrences of the selected text (the word at the cursor if no selection).
    pub fn select_all_occurrences(&mut self) {
        let sel = *self.sel();
        let (needle, whole) = match sel.selection() {
            Some(r) => (self.rope.slice(r).to_string(), self.word_search),
            None => match self.word_at(sel.cursor) {
                Some(w) => (self.rope.slice(w).to_string(), true),
                None => return,
            },
        };
        let all = self.occurrences(&needle, whole);
        if all.is_empty() {
            return;
        }
        self.cursor_history.push((self.sels.clone(), self.primary));
        if self.sels.len() == 1 {
            self.origin = Some(sel.cursor);
        }
        self.sels = all.into_iter().map(|r| Sel { cursor: r.end, anchor: Some(r.start), goal: None }).collect();
        self.primary = self.sels.len() - 1;
        self.word_search = whole;
        self.normalize();
    }
}
