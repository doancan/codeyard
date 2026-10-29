//! Editor: edit commands: delete, line ops, transforms, comment, clipboard, undo.

use super::*;
use crate::i18n::{t, tf};

impl EditorView {
    pub(super) fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let spaces = self.indent != "\t";
        if spaces && self.buffer.backspace_to_tab_stop(self.indent_unit()) {
            // in indentation: to the previous tab stop
        } else if cx.global::<Settings>().auto_close_brackets {
            self.buffer.backspace_pair();
        } else {
            self.buffer.backspace();
        }
        let open = self.completion.take();
        self.after_edit(cx);
        // Deleting with the list open shortens the prefix: refilter (closes once it passes the word start).
        if open.is_some() {
            self.completion = open;
            self.refilter(cx);
        }
    }

    pub(super) fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete();
        self.after_edit(cx);
    }

    pub(super) fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        let indent = self.indent.clone();
        // In Python, after `:` also indent one level.
        let colon = matches!(self.path.rsplit_once('.').map(|(_, e)| e), Some("py" | "pyi"));
        self.buffer.smart_newline(&indent, colon);
        self.after_edit(cx);
    }

    /// Indent if the selection spans several lines, otherwise insert an indent.
    pub(super) fn tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        if self.snippet_jump(true) {
            return self.after_move(cx);
        }
        if self.snippet_stop.is_none() && (self.try_emmet(cx) || self.try_expand_prefix(cx)) {
            return;
        }
        let indent = self.indent.clone();
        let multiline = self.buffer.selections().iter().any(|r| self.buffer.char_to_line(r.start) != self.buffer.char_to_line(r.end));
        if multiline {
            self.buffer.indent_lines(&indent);
        } else if indent == "\t" {
            self.buffer.insert(&indent);
        } else {
            self.buffer.insert_to_tab_stop(indent.len());
        }
        self.after_edit(cx);
    }

    pub(super) fn outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        if self.snippet_jump(false) {
            return self.after_move(cx);
        }
        let unit = self.indent_unit();
        self.buffer.outdent_lines(unit);
        self.after_edit(cx);
    }

    pub(super) fn delete_to_line_start(&mut self, _: &DeleteToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete_to_line_start();
        self.after_edit(cx);
    }

    pub(super) fn insert_line_below(&mut self, _: &InsertLineBelow, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.insert_line(true);
        self.after_edit(cx);
    }

    pub(super) fn insert_line_above(&mut self, _: &InsertLineAbove, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.insert_line(false);
        self.after_edit(cx);
    }

    pub(super) fn indent_line(&mut self, _: &IndentLine, _: &mut Window, cx: &mut Context<Self>) {
        let unit = self.indent.clone();
        self.buffer.indent_lines(&unit);
        self.after_edit(cx);
    }

    pub(super) fn outdent_line(&mut self, _: &OutdentLine, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.outdent_lines(self.indent_unit());
        self.after_edit(cx);
    }

    pub(super) fn select_page_up(&mut self, _: &SelectPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(-self.page_lines(), true);
        self.after_move(cx);
    }

    pub(super) fn select_page_down(&mut self, _: &SelectPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(self.page_lines(), true);
        self.after_move(cx);
    }

    /// Shift+Cmd+\\: jump to the matching bracket of the one at the cursor.
    pub(super) fn jump_to_bracket(&mut self, _: &JumpToBracket, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.buffer.cursor();
        let Some((a, b)) = self.buffer.matching_bracket(cursor) else { return };
        let to = if cursor == a || cursor == a + 1 { b } else { a };
        self.buffer.set_cursor(to, false);
        self.after_move(cx);
    }

    pub(super) fn cursor_undo(&mut self, _: &CursorUndo, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.undo_cursor() {
            self.after_move(cx);
        }
    }

    pub(super) fn cursors_at_line_ends(&mut self, _: &CursorsAtLineEnds, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.cursors_at_line_ends();
        self.after_move(cx);
    }

    pub(super) fn select_all_occurrences(&mut self, _: &SelectAllOccurrences, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.select_all_occurrences();
        self.after_move(cx);
    }

    /// Ctrl+Shift+Cmd+Right: grow the selection to the parent syntax node (none: word, then line, then document).
    pub(super) fn expand_selection(&mut self, _: &ExpandSelection, _: &mut Window, cx: &mut Context<Self>) {
        let before = self.buffer.selections();
        let after: Vec<Range<usize>> = before.iter().map(|r| self.expanded(r.clone())).collect();
        if after == before {
            return;
        }
        // After any other movement the old history is invalid.
        if self.expand_stack.last().is_some_and(|(_, a)| *a != before) {
            self.expand_stack.clear();
        }
        self.buffer.set_selections(&after);
        self.expand_stack.push((before, self.buffer.selections()));
        self.after_move(cx);
    }

    pub(super) fn shrink_selection(&mut self, _: &ShrinkSelection, _: &mut Window, cx: &mut Context<Self>) {
        let now = self.buffer.selections();
        match self.expand_stack.pop() {
            Some((before, after)) if after == now => {
                self.buffer.set_selections(&before);
                self.after_move(cx);
            }
            _ => self.expand_stack.clear(),
        }
    }

    pub(super) fn expanded(&self, r: Range<usize>) -> Range<usize> {
        if r.is_empty()
            && let Some(w) = self.buffer.word_at(r.start)
        {
            return w;
        }
        if let Some(syntax) = self.syntax.as_ref().filter(|s| s.is_loaded()) {
            let rope = self.buffer.rope();
            if let Some(n) = syntax.enclosing_node(rope.char_to_byte(r.start)..rope.char_to_byte(r.end)) {
                return rope.byte_to_char(n.start)..rope.byte_to_char(n.end);
            }
        }
        let (l0, l1) = (self.buffer.char_to_line(r.start), self.buffer.char_to_line(r.end));
        let lines = self.buffer.line_to_char(l0)..self.buffer.line_to_char(l1) + self.buffer.line_len_chars(l1);
        if lines != r && lines.start <= r.start && lines.end >= r.end { lines } else { 0..self.buffer.len_chars() }
    }

    pub(super) fn join_lines(&mut self, _: &JoinLines, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.join_lines();
        self.after_edit(cx);
    }

    pub(super) fn sort_lines_ascending(&mut self, _: &SortLinesAscending, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.sort_lines(false);
        self.after_edit(cx);
    }

    pub(super) fn sort_lines_descending(&mut self, _: &SortLinesDescending, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.sort_lines(true);
        self.after_edit(cx);
    }

    pub(super) fn transform_to_uppercase(&mut self, _: &TransformToUppercase, _: &mut Window, cx: &mut Context<Self>) {
        let loc = crate::textcase::CaseLocale::from_setting(&cx.global::<Settings>().case_locale);
        self.buffer.transform_selections(move |t| crate::textcase::upper(t, loc));
        self.after_edit(cx);
    }

    pub(super) fn transform_to_lowercase(&mut self, _: &TransformToLowercase, _: &mut Window, cx: &mut Context<Self>) {
        let loc = crate::textcase::CaseLocale::from_setting(&cx.global::<Settings>().case_locale);
        self.buffer.transform_selections(move |t| crate::textcase::lower_locale(t, loc));
        self.after_edit(cx);
    }

    pub(super) fn transform_to_title_case(&mut self, _: &TransformToTitleCase, _: &mut Window, cx: &mut Context<Self>) {
        let loc = crate::textcase::CaseLocale::from_setting(&cx.global::<Settings>().case_locale);
        self.buffer.transform_selections(move |t| crate::textcase::title(t, loc));
        self.after_edit(cx);
    }

    pub(super) fn transform_to_camel_case(&mut self, _: &TransformToCamelCase, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.transform_selections(crate::textcase::to_camel);
        self.after_edit(cx);
    }

    pub(super) fn transform_to_pascal_case(&mut self, _: &TransformToPascalCase, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.transform_selections(crate::textcase::to_pascal);
        self.after_edit(cx);
    }

    pub(super) fn transform_to_snake_case(&mut self, _: &TransformToSnakeCase, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.transform_selections(crate::textcase::to_snake);
        self.after_edit(cx);
    }

    pub(super) fn transform_to_kebab_case(&mut self, _: &TransformToKebabCase, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.transform_selections(crate::textcase::to_kebab);
        self.after_edit(cx);
    }

    pub(super) fn transform_to_constant_case(&mut self, _: &TransformToConstantCase, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.transform_selections(crate::textcase::to_constant);
        self.after_edit(cx);
    }

    /// Shift+Alt+A: wrap the selected text in a block comment / unwrap it (no selection: the word under the cursor).
    pub(super) fn toggle_block_comment(&mut self, _: &ToggleBlockComment, _: &mut Window, cx: &mut Context<Self>) {
        let Some((open, close)) = block_delims(&self.path) else {
            cx.emit(EditorEvent::Message(t("No block comment syntax for this file type").into()));
            return;
        };
        self.buffer.transform_selections(|t| crate::textcase::toggle_wrap(t, open, close));
        self.after_edit(cx);
    }

    /// Convert the selected text (no selection: the word under the cursor); if the conversion fails, leave it alone
    /// and say so (`what`: e.g. "Base64").
    pub(super) fn transform_text(&mut self, what: &str, f: impl Fn(&str) -> Option<String>, cx: &mut Context<Self>) {
        let failed = std::cell::Cell::new(false);
        self.buffer.transform_selections(|t| {
            f(t).unwrap_or_else(|| {
                failed.set(true);
                t.to_string()
            })
        });
        if failed.get() {
            cx.emit(EditorEvent::Message(match what {
                "Base64" => t("Not valid Base64: left unchanged").to_string(),
                "URL encoding" => t("Not valid URL encoding: left unchanged").to_string(),
                _ => tf("Not valid {}: left unchanged", &[&what]),
            }));
        }
        self.after_edit(cx);
    }

    /// Apply to the selected lines (no selection: the whole document).
    pub(super) fn map_lines(&mut self, f: impl Fn(&[String]) -> Vec<String>, cx: &mut Context<Self>) {
        let n = self.buffer.len_lines();
        let (first, last) = match self.buffer.selections().first() {
            Some(r) if !r.is_empty() => {
                let end = if self.buffer.char_to_line(r.end) > self.buffer.char_to_line(r.start)
                    && self.buffer.line_to_char(self.buffer.char_to_line(r.end)) == r.end
                {
                    r.end - 1
                } else {
                    r.end
                };
                (self.buffer.char_to_line(r.start), self.buffer.char_to_line(end))
            }
            // Whole document: the empty "last line" after a trailing newline is not processed.
            _ => {
                let last = n.saturating_sub(1);
                (0, if last > 0 && self.buffer.line(last).is_empty() { last - 1 } else { last })
            }
        };
        let lines: Vec<String> = (first..=last).map(|l| self.buffer.line(l).to_string()).collect();
        let new = f(&lines).join("\n");
        let start = self.buffer.line_to_char(first);
        let end = self.buffer.line_to_char(last) + self.buffer.line(last).chars().count();
        if self.buffer.text_range(start..end) == new {
            return;
        }
        self.buffer.replace_all(vec![(start..end, new)]);
        self.after_edit(cx);
    }

    pub(super) fn transform_base64_encode(&mut self, _: &TransformBase64Encode, _: &mut Window, cx: &mut Context<Self>) {
        self.transform_text("text", |t| Some(crate::texttools::base64_encode(t)), cx);
    }

    pub(super) fn transform_base64_decode(&mut self, _: &TransformBase64Decode, _: &mut Window, cx: &mut Context<Self>) {
        self.transform_text("Base64", crate::texttools::base64_decode, cx);
    }

    pub(super) fn transform_url_encode(&mut self, _: &TransformUrlEncode, _: &mut Window, cx: &mut Context<Self>) {
        self.transform_text("text", |t| Some(crate::texttools::url_encode(t)), cx);
    }

    pub(super) fn transform_url_decode(&mut self, _: &TransformUrlDecode, _: &mut Window, cx: &mut Context<Self>) {
        self.transform_text("URL encoding", crate::texttools::url_decode, cx);
    }

    pub(super) fn transform_html_escape(&mut self, _: &TransformHtmlEscape, _: &mut Window, cx: &mut Context<Self>) {
        self.transform_text("text", |t| Some(crate::texttools::html_escape(t)), cx);
    }

    pub(super) fn transform_html_unescape(&mut self, _: &TransformHtmlUnescape, _: &mut Window, cx: &mut Context<Self>) {
        self.transform_text("HTML", |t| Some(crate::texttools::html_unescape(t)), cx);
    }

    pub(super) fn sort_lines_insensitive(&mut self, _: &SortLinesInsensitive, _: &mut Window, cx: &mut Context<Self>) {
        self.map_lines(crate::texttools::sort_case_insensitive, cx);
    }

    pub(super) fn sort_lines_natural(&mut self, _: &SortLinesNatural, _: &mut Window, cx: &mut Context<Self>) {
        self.map_lines(crate::texttools::sort_natural, cx);
    }

    pub(super) fn remove_blank_lines(&mut self, _: &RemoveBlankLines, _: &mut Window, cx: &mut Context<Self>) {
        self.map_lines(crate::texttools::remove_blank_lines, cx);
    }

    pub(super) fn collapse_blank_lines(&mut self, _: &CollapseBlankLines, _: &mut Window, cx: &mut Context<Self>) {
        self.map_lines(crate::texttools::collapse_blank_lines, cx);
    }

    pub(super) fn insert_date(&mut self, _: &InsertDate, _: &mut Window, cx: &mut Context<Self>) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.buffer.insert(&crate::texttools::iso_date(now));
        self.after_edit(cx);
    }

    pub(super) fn insert_date_time(&mut self, _: &InsertDateTime, _: &mut Window, cx: &mut Context<Self>) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.buffer.insert(&crate::texttools::iso_datetime(now));
        self.after_edit(cx);
    }

    pub(super) fn insert_uuid(&mut self, _: &InsertUuid, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.insert(&crate::texttools::uuid_v4());
        self.after_edit(cx);
    }

    pub(super) fn duplicate_selection(&mut self, _: &DuplicateSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.duplicate_selection();
        self.after_edit(cx);
    }

    // --- Block (hunk) operations ---

    /// Lines touched by the cursor (or selection), 0-based, closed range.
    pub(super) fn cursor_lines(&self) -> (usize, usize) {
        let r = self.buffer.selection_or_cursor();
        let lo = self.buffer.char_to_line(r.start);
        // If the selection ends at a line start, that last line is not part of it.
        let end = if r.end > r.start && self.buffer.line_to_char(self.buffer.char_to_line(r.end)) == r.end { r.end - 1 } else { r.end };
        (lo, self.buffer.char_to_line(end).max(lo))
    }

    pub(super) fn delete_all_right(&mut self, _: &DeleteAllRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete_to_line_end();
        self.after_edit(cx);
    }

    pub(super) fn select_to_bracket(&mut self, _: &SelectToBracket, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.select_to_bracket() {
            self.after_move(cx);
        } else {
            cx.emit(EditorEvent::Message(t("No enclosing brackets").into()));
        }
    }

    pub(super) fn add_selection_to_previous_find_match(&mut self, _: &AddSelectionToPreviousFindMatch, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.select_previous_occurrence() {
            self.after_move(cx);
        }
    }

    /// Cmd+K Cmd+C / Cmd+K Cmd+U: add / remove line comment (not a toggle).
    pub(super) fn comment_lines_cmd(&mut self, add: bool, cx: &mut Context<Self>) {
        let Some(token) = comment_token(&self.path) else {
            return cx.emit(EditorEvent::Message(t("No line comment syntax for this file type").into()));
        };
        self.buffer.comment_lines(token, Some(add));
        self.after_edit(cx);
    }

    pub(super) fn add_line_comment(&mut self, _: &AddLineComment, _: &mut Window, cx: &mut Context<Self>) {
        self.comment_lines_cmd(true, cx);
    }

    pub(super) fn remove_line_comment(&mut self, _: &RemoveLineComment, _: &mut Window, cx: &mut Context<Self>) {
        self.comment_lines_cmd(false, cx);
    }

    pub(super) fn trim_trailing_whitespace_cmd(&mut self, _: &TrimTrailingWhitespace, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.trim_trailing_whitespace();
        self.after_edit(cx);
    }

    pub(super) fn transpose_characters(&mut self, _: &TransposeCharacters, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.transpose_chars();
        self.after_edit(cx);
    }

    pub(super) fn reverse_lines(&mut self, _: &ReverseLines, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.reverse_lines();
        self.after_edit(cx);
    }

    pub(super) fn delete_duplicate_lines(&mut self, _: &DeleteDuplicateLines, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete_duplicate_lines();
        self.after_edit(cx);
    }

    pub(super) fn toggle_comment(&mut self, _: &ToggleComment, _: &mut Window, cx: &mut Context<Self>) {
        match (comment_token(&self.path), block_comment(&self.path)) {
            (Some(token), _) => self.buffer.toggle_comment(token),
            (None, Some((open, close))) => self.buffer.toggle_block_comment_lines(open, close),
            (None, None) => {
                cx.emit(EditorEvent::Message(t("No comment syntax for this file type").into()));
                return;
            }
        }
        self.after_edit(cx);
    }

    pub(super) fn move_line_up(&mut self, _: &MoveLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_lines(true);
        self.after_edit(cx);
    }

    pub(super) fn move_line_down(&mut self, _: &MoveLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_lines(false);
        self.after_edit(cx);
    }

    pub(super) fn copy_line_up(&mut self, _: &CopyLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.copy_lines(false);
        self.after_edit(cx);
    }

    pub(super) fn copy_line_down(&mut self, _: &CopyLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.copy_lines(true);
        self.after_edit(cx);
    }

    pub(super) fn delete_line(&mut self, _: &DeleteLine, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete_lines();
        self.after_edit(cx);
    }

    /// Cmd+L: select the cursor line; pressing again adds the next line.
    pub(super) fn select_line(&mut self, _: &SelectLine, _: &mut Window, cx: &mut Context<Self>) {
        let r = self.buffer.selection_or_cursor();
        let (first, last) = (self.buffer.char_to_line(r.start), self.buffer.char_to_line(r.end));
        let end = if last + 1 < self.buffer.len_lines() { self.buffer.line_to_char(last + 1) } else { self.buffer.len_chars() };
        self.buffer.set_selection(self.buffer.line_to_char(first)..end, false);
        self.after_move(cx);
    }

    /// Undo / redo from the merge editor.
    pub fn undo_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.undo(&Undo, window, cx);
    }

    pub fn redo_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.redo(&Redo, window, cx);
    }

    pub(super) fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.undo() {
            // Keep the undo from re-reflecting the linked tag (the chained step already undid both).
            self.buffer.set_marks(LINK_MARKS, Vec::new());
            self.after_edit(cx);
        }
    }

    /// Undo a replacement's single step, only if the text is still exactly what it left (`expected` is the version right
    /// after it). Anything typed since makes this refuse instead of undoing the wrong step.
    pub fn undo_replace_step(&mut self, expected: u64, cx: &mut Context<Self>) -> bool {
        if self.buffer.version() != expected || !self.buffer.undo() {
            return false;
        }
        self.buffer.set_marks(LINK_MARKS, Vec::new());
        self.after_edit(cx);
        true
    }

    pub(super) fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.redo() {
            self.buffer.set_marks(LINK_MARKS, Vec::new());
            self.after_edit(cx);
        }
    }

    /// No selection: the whole line (as in VS Code). With multiple cursors each
    /// selection (or line) becomes its own line, redistributed on paste.
    pub(super) fn copy_text(&self) -> String {
        if self.buffer.cursor_count() > 1 {
            let texts = self.buffer.selected_texts();
            if texts.iter().any(|t| !t.is_empty()) {
                return texts.join("\n");
            }
            let lines: Vec<String> = self.buffer.cursors().iter().map(|&c| self.buffer.line(self.buffer.char_to_line(c))).collect();
            return format!("{}\n", lines.join("\n"));
        }
        self.buffer.selected_text().unwrap_or_else(|| {
            let (line, _) = self.buffer.cursor_line_col();
            format!("{}\n", self.buffer.line(line))
        })
    }

    /// Whole line(s) copied with no selection: pasted as lines.
    pub(super) fn remember_line_copy(&self, text: &str) {
        let whole_lines = self.buffer.selected_texts().iter().all(|t| t.is_empty());
        LINE_CLIP.with(|c| *c.borrow_mut() = whole_lines.then(|| text.to_string()));
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.copy_text();
        self.remember_line_copy(&text);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    pub(super) fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.copy_text();
        self.remember_line_copy(&text);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if self.buffer.selected_texts().iter().any(|t| !t.is_empty()) {
            self.buffer.insert(""); // only selections are deleted, empty cursors stay in place
        } else {
            // Delete the whole lines directly; no fake selection, or undo would bring that selection back.
            self.buffer.delete_lines();
        }
        self.after_edit(cx);
    }

    pub(super) fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Line copied without a selection and no selection now: the whole line goes above the cursor's line
            // (VS Code); the cursor stays on its own line.
            let line_copy = LINE_CLIP.with(|c| c.borrow().as_deref() == Some(text.as_str()));
            if line_copy && self.buffer.cursor_count() == 1 && self.buffer.selection().is_none() {
                let line = self.buffer.char_to_line(self.buffer.cursor());
                let at = self.buffer.line_to_char(line);
                self.buffer.apply_edits(vec![(at..at, text.clone())]);
                self.after_edit(cx);
                return;
            }
            // As many lines as cursors: one line per cursor.
            let n = self.buffer.cursor_count();
            let lines: Vec<String> = text.strip_suffix('\n').unwrap_or(&text).split('\n').map(str::to_string).collect();
            let mut pasted = None;
            if n > 1 && lines.len() == n {
                self.buffer.insert_each(&lines);
            } else {
                let from = self.buffer.selection_or_cursor().start;
                self.buffer.insert(&text);
                pasted = (n == 1).then(|| from..from + text.chars().count());
            }
            self.after_edit(cx);
            // after_edit invalidates leftover requests: formatting is requested after it.
            if let Some(range) = pasted.filter(|_| self.scoped(|f| f.format_on_paste, cx.global::<Settings>().format_on_paste)) {
                self.format_pasted(range, cx);
            }
        }
    }

    /// `editor.formatOnPaste`: have the server format the pasted range. Does nothing without a server or
    /// if the document changed before the response.
    pub(super) fn format_pasted(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.sync_lsp();
        let Some(client) = self
            .lsp
            .clone()
            .filter(|c| !c.capabilities["documentRangeFormattingProvider"].is_null() && c.capabilities["documentRangeFormattingProvider"] != false)
        else {
            return;
        };
        let (a, b) = (self.buffer.lsp_position(range.start), self.buffer.lsp_position(range.end));
        let params = serde_json::json!({
            "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) },
            "range": { "start": { "line": a.0, "character": a.1 }, "end": { "line": b.0, "character": b.1 } },
            "options": { "tabSize": self.indent_unit(), "insertSpaces": self.indent != "\t" },
        });
        self.apply_lsp_format("textDocument/rangeFormatting", client, params, cx);
    }

    /// `editor.formatOnType`: `textDocument/onTypeFormatting` when a trigger character is typed.
    pub(super) fn format_typed(&mut self, ch: char, client: Arc<crate::lsp::Client>, cx: &mut Context<Self>) {
        let caps = &client.capabilities["documentOnTypeFormattingProvider"];
        let is_trigger = caps["firstTriggerCharacter"].as_str().is_some_and(|t| t.starts_with(ch))
            || caps["moreTriggerCharacter"].as_array().is_some_and(|a| a.iter().any(|t| t.as_str().is_some_and(|t| t.starts_with(ch))));
        if !is_trigger {
            return;
        }
        self.sync_lsp();
        let pos = self.buffer.lsp_position(self.buffer.cursor());
        let params = serde_json::json!({
            "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) },
            "position": { "line": pos.0, "character": pos.1 },
            "ch": ch.to_string(),
            "options": { "tabSize": self.indent_unit(), "insertSpaces": self.indent != "\t" },
        });
        self.apply_lsp_format("textDocument/onTypeFormatting", client, params, cx);
    }
}
