//! Editor: cursor movement and selection commands.

use super::*;

impl EditorView {
    pub(super) fn after_move(&mut self, cx: &mut Context<Self>) {
        self.column = None;
        self.update_linked_tag(cx);
        self.schedule_symbol_highlight(cx);
        self.after_move_keep_column(cx);
    }

    pub(super) fn after_move_keep_column(&mut self, cx: &mut Context<Self>) {
        self.schedule_blame(cx);
        self.marked = None;
        self.reveal_cursor_x();
        // Movement or an edit closes popups (the typing path puts the list back).
        self.completion = None;
        self.signature = None;
        self.hover = None;
        self.peek = None;
        self.peek_seq += 1;
        self.hover_task = None;
        self.unfold_at_cursors();
        let row = self.row_of_char(self.buffer.cursor());
        self.scroll.scroll_to_item(row, ScrollStrategy::Top);
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    pub(super) fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.symbol_hl.clear();
        if self.lsp.is_some() {
            // In-flight requests belong to the old document; if the line count changed, the line-based data is stale.
            self.lsp_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.buffer.len_lines() != self.lsp_lines {
                (self.inlays, self.lsp_folds) = Default::default();
                (self.lenses, self.sem) = Default::default();
                self.display_key = None;
            }
        }
        if !self.buffer.marks(LINK_MARKS).is_empty() {
            self.sync_linked_tag();
        }
        self.update_linked_tag(cx);
        self.schedule_blame(cx);
        self.refresh_conflicts_soon(cx);
        self.sync_syntax(cx);
        if self.git_base.is_some() {
            self.schedule_git_marks(150, Some(cx));
        }
        if self.lsp.is_some() {
            self.schedule_lsp_extras(600, cx);
        }
        if self.find.is_some() {
            self.refresh_matches(cx);
        }
        // Keep the other view's open find bar up to date too.
        for v in self.parked.iter_mut().flatten() {
            if let Some(re) = &v.find_re {
                v.matches = find::find_all(&self.buffer, re);
            }
        }
        if let AutoSave::AfterDelay(ms) = cx.global::<Settings>().auto_save {
            self.autosave = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(ms)).await;
                this.update(cx, |e, cx| e.autosave_now(cx)).ok();
            }));
        }
        self.after_move(cx);
    }

    /// Keep the cursor visible horizontally (4 columns of slack at the edge).
    pub(super) fn reveal_cursor_x(&mut self) {
        if self.layout_params.0 {
            self.hscroll = px(0.); // no horizontal scroll while wrapping is on
            return;
        }
        let cursor = self.buffer.cursor();
        let line = self.buffer.char_to_line(cursor);
        let col = cursor - self.buffer.line_to_char(line);
        let Some(v) = self.visible.iter().find(|v| v.line == line).or(self.visible.first()) else { return };
        let width = v.bounds.size.width;
        let col_w = v.layout.width / (self.buffer.line(v.line).chars().count().max(1) as f32);
        let x = if v.line == line { v.layout.x_for_index(byte_of(&self.buffer.line(line), col)) } else { col_w * col as f32 };
        let margin = (col_w * 4.).min(width / 4.);
        if x < self.hscroll + margin {
            self.hscroll = (x - margin).max(px(0.));
        } else if x > self.hscroll + width - margin {
            self.hscroll = x - width + margin;
        }
    }

    /// Horizontal scroll (trackpad or Shift+wheel); up to the longest visible line.
    pub(super) fn scroll_x(&mut self, dx: Pixels, cx: &mut Context<Self>) {
        if self.layout_params.0 {
            return;
        }
        let width = self.visible.first().map_or(px(0.), |v| v.bounds.size.width);
        let longest = self.visible.iter().map(|v| v.layout.width).fold(px(0.), |a, b| a.max(b));
        let max = (longest - width + px(40.)).max(px(0.));
        let next = (self.hscroll - dx).clamp(px(0.), max);
        if next != self.hscroll {
            self.hscroll = next;
            cx.notify();
        }
    }

    pub(super) fn page_lines(&self) -> isize {
        (self.visible.len().saturating_sub(2)).max(10) as isize
    }

    pub(super) fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_left(false);
        self.after_move(cx);
    }

    pub(super) fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_right(false);
        self.after_move(cx);
    }

    pub(super) fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(-1, false);
        self.after_move(cx);
    }

    pub(super) fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(1, false);
        self.after_move(cx);
    }

    pub(super) fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_left(true);
        self.after_move(cx);
    }

    pub(super) fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_right(true);
        self.after_move(cx);
    }

    pub(super) fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(-1, true);
        self.after_move(cx);
    }

    pub(super) fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(1, true);
        self.after_move(cx);
    }

    pub(super) fn move_word_left(&mut self, _: &MoveWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_word_left(false);
        self.after_move(cx);
    }

    pub(super) fn move_word_right(&mut self, _: &MoveWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_word_right(false);
        self.after_move(cx);
    }

    pub(super) fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_word_left(true);
        self.after_move(cx);
    }

    pub(super) fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_word_right(true);
        self.after_move(cx);
    }

    pub(super) fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete_word(false);
        self.after_edit(cx);
    }

    pub(super) fn delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete_word(true);
        self.after_edit(cx);
    }

    pub(super) fn move_home(&mut self, _: &MoveHome, _: &mut Window, cx: &mut Context<Self>) {
        if !self.row_edge(false, false) {
            self.buffer.move_home(false);
        }
        self.after_move(cx);
    }

    pub(super) fn move_end(&mut self, _: &MoveEnd, _: &mut Window, cx: &mut Context<Self>) {
        if !self.row_edge(true, false) {
            self.buffer.move_end(false);
        }
        self.after_move(cx);
    }

    pub(super) fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        if !self.row_edge(false, true) {
            self.buffer.move_home(true);
        }
        self.after_move(cx);
    }

    pub(super) fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        if !self.row_edge(true, true) {
            self.buffer.move_end(true);
        }
        self.after_move(cx);
    }

    /// With wrapping on, Home/End go to the start/end of the screen line first (as VS Code does);
    /// if already there returns `false` and the logical line start/end applies.
    pub(super) fn row_edge(&mut self, end: bool, select: bool) -> bool {
        self.refresh_display();
        if self.display.is_identity() || self.buffer.cursor_count() > 1 {
            return false;
        }
        let c = self.buffer.cursor();
        let line = self.buffer.char_to_line(c);
        let points = self.wrap_points_of(line);
        if points.len() < 2 {
            return false;
        }
        let line_start = self.buffer.line_to_char(line);
        let len = self.buffer.line_len_chars(line);
        let col = c - line_start;
        let seg = display::segment_of(&points, col);
        let r = display::segment(&points, seg, len);
        // Segment end is the next segment's start: stop before the last character.
        let target = if end { if r.end < len { r.end - 1 } else { r.end } } else { r.start };
        if line_start + target == c {
            return false;
        }
        self.buffer.move_cursors(select, |_, _| (line_start + target, None));
        true
    }

    pub(super) fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.set_cursor(0, false);
        self.after_move(cx);
    }

    pub(super) fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.set_cursor(self.buffer.len_chars(), false);
        self.after_move(cx);
    }

    pub(super) fn select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.set_cursor(0, true);
        self.after_move(cx);
    }

    pub(super) fn select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.set_cursor(self.buffer.len_chars(), true);
        self.after_move(cx);
    }

    pub(super) fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(-self.page_lines(), false);
        self.after_move(cx);
    }

    pub(super) fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(self.page_lines(), false);
        self.after_move(cx);
    }

    pub(super) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.select_all();
        cx.notify();
    }

    /// First multi-cursor to single cursor, then clear the selection.
    pub(super) fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        if self.snippet_stop.is_some() {
            self.end_snippet();
        }
        if self.signature.take().is_some() || self.peek.take().is_some() || self.close_picker(cx) {
            cx.notify();
            return;
        }
        // VS Code order: first multi-cursor collapses to one, then the find bar closes (selection
        // stays), last the selection clears.
        if !self.buffer.collapse_cursors() {
            if self.find.is_some() {
                self.find = None;
                self.matches.clear();
            } else {
                let at = self.buffer.cursor();
                self.buffer.set_cursor(at, false);
            }
        }
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    pub(super) fn add_cursor_above(&mut self, _: &AddCursorAbove, _: &mut Window, cx: &mut Context<Self>) {
        self.add_cursor_rows(-1);
        self.after_move(cx);
    }

    pub(super) fn add_cursor_below(&mut self, _: &AddCursorBelow, _: &mut Window, cx: &mut Context<Self>) {
        self.add_cursor_rows(1);
        self.after_move(cx);
    }

    pub(super) fn select_next_occurrence(&mut self, _: &SelectNextOccurrence, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.select_next_occurrence() {
            self.after_move(cx);
        }
    }

    // --- Language server ---
}
