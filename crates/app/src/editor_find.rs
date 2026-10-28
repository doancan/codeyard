//! Editor: find / replace.

use super::*;
use crate::i18n::{t, tf};

impl EditorView {
    pub(super) fn find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(false, window, cx);
    }

    pub(super) fn find_replace(&mut self, _: &FindReplace, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(true, window, cx);
    }

    pub(super) fn find_next(&mut self, _: &FindNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            return self.open_find(false, window, cx);
        }
        self.jump(true, cx);
    }

    pub(super) fn find_prev(&mut self, _: &find::FindPrev, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            return self.open_find(false, window, cx);
        }
        self.jump(false, cx);
    }

    /// Open the bar (focus it if open). A single-line selection or the word at the cursor is searched.
    pub fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Scope candidate for find in selection: what was selected when the bar opened (as you type the selection
        // moves to the match). With no selection, the cursor's line.
        if self.find.is_none() {
            let sel = self.buffer.selection_or_cursor();
            let scope = if sel.is_empty() {
                let line = self.buffer.char_to_line(sel.start);
                self.buffer.line_to_char(line)..self.buffer.line_to_char(line) + self.buffer.line_len_chars(line)
            } else {
                sel
            };
            self.find_origin = Some(scope);
        }
        let seed =
            self.buffer.selected_text().filter(|t| !t.contains('\n')).or_else(|| self.buffer.word_at(self.buffer.cursor()).map(|r| self.buffer.text_range(r)));
        // If the bar is open and the selection already is a match, leave the query alone.
        let seed = seed.filter(|_| self.find.is_none() || self.current_match().is_none());
        let bar = match &self.find {
            Some((bar, _)) => bar.clone(),
            None => {
                let bar = cx.new(FindBar::new);
                let sub = cx.subscribe_in(&bar, window, |this, _, ev: &FindEvent, window, cx| this.on_find_event(ev, window, cx));
                self.find = Some((bar.clone(), sub));
                bar
            }
        };
        bar.update(cx, |b, cx| {
            if replace {
                b.show_replace = true;
            }
            if let Some(seed) = &seed {
                let seed = if b.regex { regex::escape(seed) } else { seed.clone() };
                b.set_query(&seed, cx);
            }
        });
        // `Changed` arrives when the query text changes; if it didn't change, refresh by hand.
        self.refresh_matches(cx);
        if replace && !bar.read(cx).query(cx).is_empty() {
            bar.read(cx).focus_replacement(window, cx);
        } else {
            bar.read(cx).focus_query(window, cx);
        }
        cx.notify();
    }

    pub(super) fn on_find_event(&mut self, ev: &FindEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            FindEvent::Changed => {
                self.refresh_matches(cx);
                // While typing: go to the first match from the cursor.
                let from = self.buffer.selection_or_cursor().start;
                if let Some(m) = self.matches.iter().find(|m| m.start >= from).or(self.matches.first()).cloned() {
                    self.select_match(m, cx);
                }
            }
            FindEvent::Next => self.jump(true, cx),
            FindEvent::Prev => self.jump(false, cx),
            FindEvent::ReplaceOne => self.replace_one(cx),
            FindEvent::ReplaceAll => self.replace_every(cx),
            FindEvent::SelectAll => {
                if !self.matches.is_empty() {
                    let all = self.matches.clone();
                    self.buffer.set_selections(&all);
                    window.focus(&self.focus_handle);
                    self.after_move(cx);
                }
            }
            FindEvent::Close => {
                self.find = None;
                self.find_origin = None;
                self.buffer.set_marks(FIND_SCOPE_MARKS + self.slot, Vec::new());
                self.matches.clear();
                window.focus(&self.focus_handle);
                cx.notify();
            }
        }
    }

    /// Pattern or text changed: find the matches again, write the counter.
    pub(super) fn refresh_matches(&mut self, cx: &mut Context<Self>) {
        let Some((bar, _)) = &self.find else { return };
        let bar = bar.clone();
        let compiled = bar.read(cx).compile(cx);
        // Find in selection: mark the scope when it opens (shifts with edits), drop it when it closes.
        let key = FIND_SCOPE_MARKS + self.slot;
        match (bar.read(cx).in_selection, self.buffer.marks(key).is_empty()) {
            (true, true) => {
                if let Some(r) = self.find_origin.clone() {
                    self.buffer.set_marks(key, vec![r.start, r.end]);
                }
            }
            (false, false) => self.buffer.set_marks(key, Vec::new()),
            _ => {}
        }
        let status = match compiled {
            Ok(Some(re)) => {
                self.matches = find::find_all(&self.buffer, &re);
                if let Some(scope) = self.find_scope() {
                    self.matches.retain(|m| m.start >= scope.start && m.end <= scope.end);
                }
                self.find_re = Some(re);
                None
            }
            Ok(None) => {
                self.matches.clear();
                self.find_re = None;
                Some(None)
            }
            Err(e) => {
                self.matches.clear();
                self.find_re = None;
                Some(Some((SharedString::from(e.lines().last().unwrap_or(t("Invalid pattern")).trim().to_string()), true)))
            }
        };
        let status = status.unwrap_or_else(|| Some((self.match_status().into(), false)));
        bar.update(cx, |b, cx| b.set_status(status, cx));
        cx.notify();
    }

    /// Like an external edit (cursors shift with the text).
    #[cfg(test)]
    pub fn insert_at_for_test(&mut self, at: usize, text: &str, cx: &mut Context<Self>) {
        self.buffer.apply_edits(vec![(at..at, text.to_string())]);
        self.after_edit(cx);
    }

    /// Scope if find in selection is on.
    pub(super) fn find_scope(&self) -> Option<Range<usize>> {
        match self.buffer.marks(FIND_SCOPE_MARKS + self.slot) {
            [a, b] => Some(*a..*b),
            _ => None,
        }
    }

    pub(super) fn current_match(&self) -> Option<usize> {
        let sel = self.buffer.selection_or_cursor();
        self.matches.iter().position(|m| *m == sel)
    }

    pub(super) fn match_status(&self) -> String {
        let n = self.matches.len();
        let more = if n >= find::MAX_MATCHES { "+" } else { "" };
        match (n, self.current_match()) {
            (0, _) => t("No results").into(),
            (_, Some(i)) => tf("{0} of {1}{2}", &[&(i + 1), &n, &more]),
            _ => tf("{0}{1} found", &[&n, &more]),
        }
    }

    pub(super) fn select_match(&mut self, m: Range<usize>, cx: &mut Context<Self>) {
        let start = m.start;
        self.buffer.set_selection(m, false);
        self.unfold_at_cursors();
        let row = self.row_of_char(start);
        self.scroll.scroll_to_item(row, ScrollStrategy::Center);
        let status = self.match_status();
        if let Some((bar, _)) = &self.find {
            bar.update(cx, |b, cx| b.set_status(Some((status.into(), false)), cx));
        }
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    /// Next/previous match (wraps around).
    pub(super) fn jump(&mut self, forward: bool, cx: &mut Context<Self>) {
        let sel = self.buffer.selection_or_cursor();
        let m = if forward {
            self.matches.iter().find(|m| m.start >= sel.end).or(self.matches.first())
        } else {
            self.matches.iter().rev().find(|m| m.end <= sel.start).or(self.matches.last())
        };
        if let Some(m) = m.cloned() {
            self.select_match(m, cx);
        }
    }

    pub(super) fn replace_one(&mut self, cx: &mut Context<Self>) {
        let (Some((bar, _)), Some(re)) = (&self.find, &self.find_re) else { return };
        let Some(i) = self.current_match() else { return self.jump(true, cx) };
        let bar = bar.read(cx);
        let m = self.matches[i].clone();
        let mut text = find::replacement_for(&self.buffer, re, &m, &bar.replacement(cx), bar.regex);
        if bar.preserve_case {
            text = find::preserve_case(&self.buffer.text_range(m.clone()), &text);
        }
        self.buffer.replace(m.clone(), &text, false);
        self.after_edit(cx);
        // Move on to the match after the replaced one.
        let after = m.start + text.chars().count();
        if let Some(next) = self.matches.iter().find(|n| n.start >= after).or(self.matches.first()).cloned() {
            self.select_match(next, cx);
        }
    }

    pub(super) fn replace_every(&mut self, cx: &mut Context<Self>) {
        let (Some((bar, _)), Some(re)) = (&self.find, &self.find_re) else { return };
        if self.matches.is_empty() {
            return;
        }
        let bar = bar.read(cx);
        let (template, regex, preserve) = (bar.replacement(cx), bar.regex, bar.preserve_case);
        // The displayed match list is capped (10,000); replace covers all of them.
        let mut parts = find::replace_all_parts(&self.buffer, re, &template, regex);
        if preserve {
            for (range, new) in &mut parts {
                *new = find::preserve_case(&self.buffer.text_range(range.clone()), new);
            }
        }
        let n = parts.len();
        self.buffer.replace_all(parts);
        self.after_edit(cx);
        if let Some((bar, _)) = &self.find {
            bar.update(cx, |b, cx| b.set_status(Some((tf("Replaced {}", &[&n]).into(), false)), cx));
        }
    }

    pub fn find_bar(&self) -> Option<Entity<FindBar>> {
        self.find.as_ref().map(|(b, _)| b.clone())
    }
}
