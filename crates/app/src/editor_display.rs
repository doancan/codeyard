//! Editor: display: line wrapping, line mapping, folding.

use super::*;

impl EditorView {
    /// Wrap column (`None` if wrapping is off or the width isn't known yet).
    pub(super) fn wrap_cols(&self) -> Option<usize> {
        let (wrap, _, char_w) = self.layout_params;
        (wrap && self.text_width > px(0.) && char_w > px(0.)).then(|| ((self.text_width - px(8.)) / char_w).floor().max(10.) as usize)
    }

    pub(super) fn tab_width(&self) -> usize {
        self.layout_params.1
    }

    #[cfg(test)]
    pub fn sticky_for_test(&self) -> Vec<usize> {
        self.sticky_lines.clone()
    }

    #[cfg(test)]
    pub fn bracket_depths_for_test(&self) -> Vec<i32> {
        self.bracket_cache.1.clone()
    }

    /// Bracket depth at the start of each line; recomputed only when the document changes.
    pub(super) fn ensure_bracket_cache(&mut self) {
        let v = self.buffer.version();
        if self.bracket_cache.0 == v {
            return;
        }
        self.bracket_cache = if self.buffer.len_chars() > BRACKET_COLOR_MAX_CHARS {
            (v, Vec::new())
        } else {
            let rope = self.buffer.rope();
            let lines: Vec<String> = (0..self.buffer.len_lines()).map(|i| rope.line(i).to_string()).collect();
            (v, editor_decor::bracket_depths(lines.iter().map(String::as_str)))
        };
    }

    /// This view's fold headers (line, sorted). A keystroke asks several times (moving the cursors out of folds, the
    /// display map, painting), and with every region of a large file folded there are thousands: the lines are kept
    /// until the text or the fold marks change.
    pub fn fold_headers(&self) -> Vec<usize> {
        let marks = self.buffer.marks(FOLD_MARKS + self.slot);
        if marks.is_empty() {
            return Vec::new();
        }
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            marks.hash(&mut h);
            // The version alone stays the same while typing joins one undo step: the length and line count change.
            (self.slot, self.buffer.version(), self.buffer.len_chars(), self.buffer.len_lines(), h.finish())
        };
        if let Some((k, v)) = &*self.fold_lines.borrow()
            && *k == key
        {
            return v.clone();
        }
        let mut v: Vec<usize> = marks.iter().map(|&c| self.buffer.char_to_line(c)).collect();
        v.sort_unstable();
        v.dedup();
        *self.fold_lines.borrow_mut() = Some((key, v.clone()));
        v
    }

    pub(super) fn set_fold_headers(&mut self, lines: Vec<usize>) {
        let marks = lines.into_iter().map(|l| self.buffer.line_to_char(l)).collect();
        self.buffer.set_marks(FOLD_MARKS + self.slot, marks);
        self.refresh_display();
    }

    pub(super) fn hidden_ranges(&self) -> Vec<Range<usize>> {
        let headers = self.fold_headers();
        if headers.is_empty() {
            return Vec::new();
        }
        display::hidden_ranges_with(&self.buffer, &headers, self.tab_width(), &self.lsp_folds)
    }

    /// Rebuild the mapping if needed (text, width or folds changed).
    pub(super) fn refresh_display(&mut self) {
        let cols = self.wrap_cols();
        let headers = self.fold_headers();
        let key: DisplayKey = (self.buffer.version(), self.buffer.len_chars(), cols, headers);
        if self.display_key.as_ref() == Some(&key) {
            return;
        }
        let hidden = if key.3.is_empty() { Vec::new() } else { display::hidden_ranges_with(&self.buffer, &key.3, self.tab_width(), &self.lsp_folds) };
        self.display = DisplayMap::build(&self.buffer, cols, self.tab_width(), &hidden);
        self.hidden = hidden;
        self.display_key = Some(key);
    }

    /// Segment starts of a line (only 0 if wrapping is off).
    pub(super) fn wrap_points_of(&self, line: usize) -> Vec<usize> {
        match self.wrap_cols() {
            Some(c) if self.display.row_span(line) > 1 => display::wrap_points(&self.buffer.line(line), c, self.tab_width()),
            _ => vec![0],
        }
    }

    /// Screen line of a character.
    pub(super) fn row_of_char(&mut self, ix: usize) -> usize {
        self.refresh_display();
        let line = self.buffer.char_to_line(ix);
        let points = self.wrap_points_of(line);
        self.display.first_row(line) + display::segment_of(&points, ix - self.buffer.line_to_char(line))
    }

    pub(super) fn row_of_line(&mut self, line: usize) -> usize {
        self.refresh_display();
        self.display.first_row(line)
    }

    /// Vertical movement of the cursors by screen line (wrapped segment, skipping folds).
    pub(super) fn move_rows(&mut self, delta: isize, select: bool) {
        self.refresh_display();
        if self.display.is_identity() {
            self.buffer.move_vertical_visual(delta, select, self.indent_unit());
            return;
        }
        // Target per cursor: document edge or (target segment start, max column, current column).
        let plan: Vec<(usize, RowTarget)> = self.buffer.cursors().into_iter().map(|c| (c, self.row_target(c, delta))).collect();
        self.buffer.move_cursors(select, |c, goal| match plan.iter().find(|(p, _)| *p == c).map(|(_, t)| *t) {
            Some(Ok(edge)) => (edge, goal),
            Some(Err((base, max, col))) => {
                let goal = goal.unwrap_or(col);
                (base + goal.min(max), Some(goal))
            }
            None => (c, goal),
        });
    }

    /// `delta` screen lines away from `c`: `Ok` document edge, `Err((segment start, max
    /// column, current column within the segment))`.
    pub(super) fn row_target(&self, c: usize, delta: isize) -> RowTarget {
        let line = self.buffer.char_to_line(c);
        let points = self.wrap_points_of(line);
        let col = c - self.buffer.line_to_char(line);
        let seg = display::segment_of(&points, col);
        let row = self.display.first_row(line) + seg;
        let target = (row as isize + delta).clamp(0, self.display.row_count() as isize - 1) as usize;
        if target == row {
            return Ok(if delta < 0 { 0 } else { self.buffer.len_chars() });
        }
        let (tline, tseg) = self.display.line_at_row(target);
        let len = self.buffer.line_len_chars(tline);
        let r = display::segment(&self.wrap_points_of(tline), tseg, len);
        let max = if r.end < len { r.end - r.start - 1 } else { r.end - r.start };
        Err((self.buffer.line_to_char(tline) + r.start, max, col - points[seg]))
    }

    /// Alt+Cmd+Up/Down: add a cursor one screen line beyond the topmost/bottommost cursor.
    pub(super) fn add_cursor_rows(&mut self, delta: isize) {
        self.refresh_display();
        if self.display.is_identity() {
            self.buffer.add_cursor_vertical(delta);
            return;
        }
        let (first, last) = self.buffer.edge_cursors();
        let (c, goal) = if delta < 0 { first } else { last };
        if let Err((base, max, col)) = self.row_target(c, delta) {
            let goal = goal.unwrap_or(col);
            self.buffer.add_cursor_at(base + goal.min(max), Some(goal));
        }
    }

    /// If a cursor entered a folded region, unfold it (as VS Code does).
    pub(super) fn unfold_at_cursors(&mut self) {
        if self.buffer.marks(FOLD_MARKS + self.slot).is_empty() {
            return;
        }
        // Fast path: no cursor is on a hidden line (most movements).
        self.refresh_display();
        let hidden = &self.hidden;
        let inside = self.buffer.cursors().into_iter().any(|c| {
            let l = self.buffer.char_to_line(c);
            let i = hidden.partition_point(|r| r.end <= l);
            hidden.get(i).is_some_and(|r| r.contains(&l))
        });
        if !inside {
            return;
        }
        let headers = self.fold_headers();
        let tab = self.tab_width();
        let lines: Vec<usize> = self.buffer.cursors().into_iter().map(|c| self.buffer.char_to_line(c)).collect();
        let keep: Vec<usize> = headers
            .iter()
            .copied()
            .filter(|&h| display::fold_end_with(&self.buffer, h, tab, &self.lsp_folds).is_some_and(|end| !lines.iter().any(|&l| l > h && l <= end)))
            .collect();
        if keep.len() != headers.len() {
            self.set_fold_headers(keep);
        }
    }

    /// Fold the cursor's line (or the innermost region containing it).
    pub(super) fn fold_at_cursor(&mut self) {
        let tab = self.tab_width();
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let header = (0..=line).rev().find(|&h| display::fold_end_with(&self.buffer, h, tab, &self.lsp_folds).is_some_and(|end| end >= line));
        let Some(header) = header else { return };
        let mut headers = self.fold_headers();
        if !headers.contains(&header) {
            headers.push(header);
        }
        self.set_fold_headers(headers);
        // A cursor on a now-hidden line goes to the end of the header.
        let end = display::fold_end_with(&self.buffer, header, tab, &self.lsp_folds).unwrap_or(header);
        let header_end = self.buffer.line_to_char(header) + self.buffer.line_len_chars(header);
        let (a, b) = (self.buffer.line_to_char(header + 1), self.buffer.line_to_char(end) + self.buffer.line_len_chars(end));
        self.buffer.move_cursors(false, |c, goal| if c >= a && c <= b { (header_end, None) } else { (c, goal) });
    }

    /// Unfold the folds on the cursor's line or containing it.
    pub(super) fn unfold_at_cursor(&mut self) {
        let tab = self.tab_width();
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let keep: Vec<usize> = self
            .fold_headers()
            .into_iter()
            .filter(|&h| h != line && !display::fold_end_with(&self.buffer, h, tab, &self.lsp_folds).is_some_and(|end| h < line && line <= end))
            .collect();
        self.set_fold_headers(keep);
    }

    /// Toggle the fold of a line (gutter arrow).
    pub(super) fn toggle_fold_line(&mut self, line: usize) {
        let mut headers = self.fold_headers();
        if let Some(i) = headers.iter().position(|&h| h == line) {
            headers.remove(i);
        } else if display::fold_end_with(&self.buffer, line, self.tab_width(), &self.lsp_folds).is_some() {
            headers.push(line);
        }
        self.set_fold_headers(headers);
        self.move_cursors_out_of_folds();
    }

    /// If folding from the gutter left the cursor hidden, move it to the header (instead of unfolding).
    pub(super) fn move_cursors_out_of_folds(&mut self) {
        let hidden = self.hidden_ranges();
        if hidden.is_empty() {
            return;
        }
        let spans: Vec<(usize, usize, usize)> = hidden
            .iter()
            .map(|r| {
                let header = r.start - 1;
                let header_end = self.buffer.line_to_char(header) + self.buffer.line_len_chars(header);
                let last = r.end - 1;
                (self.buffer.line_to_char(r.start), self.buffer.line_to_char(last) + self.buffer.line_len_chars(last), header_end)
            })
            .collect();
        self.buffer.move_cursors(false, |c, goal| match spans.iter().find(|&&(a, b, _)| c >= a && c <= b) {
            Some(&(_, _, to)) => (to, None),
            None => (c, goal),
        });
    }

    /// Fold all foldable regions (nested ones included) or unfold all.
    pub(super) fn set_all_folds(&mut self, fold: bool) {
        let headers = if fold {
            let all: Vec<usize> = (0..self.buffer.len_lines()).collect();
            let ends = display::fold_ends_with(&self.buffer, &all, self.tab_width(), &self.lsp_folds);
            all.into_iter().zip(ends).filter_map(|(l, e)| e.map(|_| l)).collect()
        } else {
            Vec::new()
        };
        self.set_fold_headers(headers);
        if fold {
            self.move_cursors_out_of_folds();
        }
    }

    /// Line segment a character is drawn in, and its x (per the last paint).
    pub(super) fn screen_pos(&self, ix: usize) -> Option<(&VisibleLine, Pixels)> {
        let mut found = None;
        for v in &self.visible {
            let len = v.text.chars().count();
            if ix >= v.start && ix <= v.start + len {
                found = Some(v);
                if ix < v.start + len {
                    break; // a position at the segment end is the start of the next segment
                }
            }
        }
        let v = found?;
        Some((v, v.text_left + v.layout.x_for_index(byte_of(&v.text, ix - v.start))))
    }

    /// Ranges that change under composition: with one cursor the marked range (else the selection), with
    /// several cursors the marked length before each cursor (else the selections).
    pub(super) fn compose_ranges(&self) -> Vec<Range<usize>> {
        let marked_len = self.marked.as_ref().map_or(0, |m| m.len());
        if self.buffer.cursor_count() == 1 {
            return vec![self.marked.clone().unwrap_or_else(|| self.buffer.selection_or_cursor())];
        }
        self.buffer.selections().into_iter().map(|r| if marked_len > 0 { r.end.saturating_sub(marked_len)..r.end } else { r }).collect()
    }

    pub(super) fn fold_region(&mut self, _: &FoldRegion, _: &mut Window, cx: &mut Context<Self>) {
        self.fold_at_cursor();
        self.after_move(cx);
    }

    pub(super) fn unfold_region(&mut self, _: &UnfoldRegion, _: &mut Window, cx: &mut Context<Self>) {
        self.unfold_at_cursor();
        self.after_move(cx);
    }

    pub(super) fn fold_level_1(&mut self, _: &FoldLevel1, _: &mut Window, cx: &mut Context<Self>) {
        self.fold_level(1, cx)
    }

    pub(super) fn fold_level_2(&mut self, _: &FoldLevel2, _: &mut Window, cx: &mut Context<Self>) {
        self.fold_level(2, cx)
    }

    pub(super) fn fold_level_3(&mut self, _: &FoldLevel3, _: &mut Window, cx: &mut Context<Self>) {
        self.fold_level(3, cx)
    }

    pub(super) fn fold_level_4(&mut self, _: &FoldLevel4, _: &mut Window, cx: &mut Context<Self>) {
        self.fold_level(4, cx)
    }

    pub(super) fn fold_level_5(&mut self, _: &FoldLevel5, _: &mut Window, cx: &mut Context<Self>) {
        self.fold_level(5, cx)
    }

    pub(super) fn fold_level_6(&mut self, _: &FoldLevel6, _: &mut Window, cx: &mut Context<Self>) {
        self.fold_level(6, cx)
    }

    pub(super) fn fold_level_7(&mut self, _: &FoldLevel7, _: &mut Window, cx: &mut Context<Self>) {
        self.fold_level(7, cx)
    }

    pub(super) fn fold_all(&mut self, _: &FoldAll, _: &mut Window, cx: &mut Context<Self>) {
        self.set_all_folds(true);
        self.after_move(cx);
    }

    /// All foldable regions: (header, last line), sorted by header.
    pub(super) fn fold_regions(&self) -> Vec<(usize, usize)> {
        let all: Vec<usize> = (0..self.buffer.len_lines()).collect();
        let ends = display::fold_ends_with(&self.buffer, &all, self.tab_width(), &self.lsp_folds);
        all.into_iter().zip(ends).filter_map(|(h, e)| e.map(|e| (h, e))).collect()
    }

    /// Cmd+K Cmd+N: fold regions at nesting level `level` (1 = outermost), except the region the
    /// cursor is in (as VS Code does). Other folds stay as they are.
    pub(super) fn fold_level(&mut self, level: usize, cx: &mut Context<Self>) {
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let mut headers = self.fold_headers();
        let mut open: Vec<usize> = Vec::new(); // last lines of the enclosing regions
        for (h, end) in self.fold_regions() {
            while open.last().is_some_and(|&e| e < h) {
                open.pop();
            }
            if open.len() + 1 == level && !(h <= line && line <= end) && !headers.contains(&h) {
                headers.push(h);
            }
            open.push(end);
        }
        self.set_fold_headers(headers);
        self.move_cursors_out_of_folds();
        self.after_move(cx);
    }

    /// Header and last line of the region at the cursor (innermost).
    pub(super) fn region_at_cursor(&self) -> Option<(usize, usize)> {
        let tab = self.tab_width();
        let line = self.buffer.char_to_line(self.buffer.cursor());
        (0..=line).rev().find_map(|h| display::fold_end_with(&self.buffer, h, tab, &self.lsp_folds).filter(|&end| end >= line).map(|end| (h, end)))
    }

    /// Cmd+K Cmd+[: fold the region at the cursor and everything inside it.
    pub(super) fn fold_recursively(&mut self, _: &FoldRecursively, _: &mut Window, cx: &mut Context<Self>) {
        let Some((top, end)) = self.region_at_cursor() else { return };
        let mut headers = self.fold_headers();
        for (h, _) in self.fold_regions().into_iter().filter(|&(h, _)| h >= top && h <= end) {
            if !headers.contains(&h) {
                headers.push(h);
            }
        }
        self.set_fold_headers(headers);
        self.move_cursors_out_of_folds();
        self.after_move(cx);
    }

    /// Cmd+K Cmd+]: unfold the region at the cursor and all folds inside it.
    pub(super) fn unfold_recursively(&mut self, _: &UnfoldRecursively, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let tab = self.tab_width();
        // No cursor on a hidden line; on a folded header that header, otherwise the enclosing region.
        let top = if self.fold_headers().contains(&line) {
            Some((line, display::fold_end_with(&self.buffer, line, tab, &self.lsp_folds).unwrap_or(line)))
        } else {
            self.region_at_cursor()
        };
        let Some((top, end)) = top else { return };
        let keep: Vec<usize> = self.fold_headers().into_iter().filter(|&h| h < top || h > end).collect();
        self.set_fold_headers(keep);
        self.after_move(cx);
    }

    pub(super) fn unfold_all(&mut self, _: &UnfoldAll, _: &mut Window, cx: &mut Context<Self>) {
        self.set_all_folds(false);
        self.after_move(cx);
    }
}
