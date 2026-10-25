//! Text buffer: rope, cursor, selection, undo. Pure and independent of the UI.
//!
//! Positions are rope character (Unicode scalar) indices. Cursor movement
//! respects grapheme cluster boundaries. Internally the line ending is always `\n`;
//! the file's original format (CRLF, BOM) is written back on save.

use std::ops::Range;

pub use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
    /// Old Mac format (`\r` only): kept as `\n` internally, restored on save.
    Cr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotUtf8;

/// Edit in bytes (for incremental parsing). Points are (line, byte within line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: (usize, usize),
    pub old_end_point: (usize, usize),
    pub new_end_point: (usize, usize),
}

/// Incremental change destined for the language server: (line, UTF-16 column) positions
/// relative to the document before the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChange {
    pub start: (u32, u32),
    pub end: (u32, u32),
    pub text: String,
}

/// A single selection: cursor + anchor (if any). `goal`: the column preserved during vertical movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sel {
    cursor: usize,
    anchor: Option<usize>,
    goal: Option<usize>,
}

impl Sel {
    fn at(cursor: usize) -> Self {
        Sel { cursor, anchor: None, goal: None }
    }

    fn start(&self) -> usize {
        self.anchor.map_or(self.cursor, |a| a.min(self.cursor))
    }

    fn end(&self) -> usize {
        self.anchor.map_or(self.cursor, |a| a.max(self.cursor))
    }

    fn selection(&self) -> Option<Range<usize>> {
        (self.start() != self.end()).then(|| self.start()..self.end())
    }

    fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }
}

/// Where cursors go after an edit: to the end of the inserted text, to their old positions
/// (shifted), or custom per piece (piece index, its start in the final document).
enum Place<'a> {
    End,
    Keep(&'a dyn Fn(usize) -> usize),
    Custom(&'a dyn Fn(usize, usize) -> Sel),
}

/// Upper limits of the undo history (see `trim_history`).
pub const MAX_UNDO_STEPS: usize = 10_000;
pub const MAX_UNDO_BYTES: usize = 64 * 1024 * 1024;
/// Once the step limit is exceeded, this many extra are tolerated, then trimmed in bulk.
const UNDO_SLACK: usize = 500;

/// One piece of an edit group; `at` is the coordinate in the document before the edit.
#[derive(Debug, Clone)]
struct Part {
    at: usize,
    removed: String,
    inserted: String,
}

/// A single undo step: the changes made at the same time (at every cursor).
#[derive(Debug, Clone)]
struct Edit {
    /// Ascending by `at`, non-overlapping.
    parts: Vec<Part>,
    sels_before: Vec<Sel>,
    primary_before: usize,
    sels_after: Vec<Sel>,
    primary_after: usize,
    /// Identity of the content after this step (see `version`).
    id: u64,
    /// Adjacent single-letter typing merges into one undo step.
    typing: bool,
    /// Single-cursor Backspace: consecutive ones merge into one undo step.
    deleting: bool,
    /// Undone / redone together with the previous step (like a linked-tag mirror).
    chain: bool,
}

pub struct Buffer {
    rope: Rope,
    line_ending: LineEnding,
    bom: bool,
    /// Selections sorted by start, non-overlapping; at least one.
    sels: Vec<Sel>,
    /// Primary selection (the most recently added): single-cursor APIs look at this.
    primary: usize,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// Identity to give to new steps (never repeats).
    next_id: u64,
    /// `None`: tracking is off (so nothing accumulates when nobody consumes it).
    edits: Option<Vec<ByteEdit>>,
    /// For the language server (if enabled).
    lsp: Option<Vec<TextChange>>,
    /// Cursors of other views of the same document (slot → selections, primary).
    /// Shifted on every edit; restored when the view comes back.
    parked: Vec<(usize, Vec<Sel>, usize)>,
    /// Markers of the view layer (key → character positions; e.g. the starts of
    /// folds). They shift on every edit, like selections.
    marks: Vec<(usize, Vec<usize>)>,
    /// Cursor before multi-cursor began: Esc returns to it (like VS Code).
    origin: Option<usize>,
    /// ⌘U: selections before the cursor was added (the last one added is undone).
    cursor_history: Vec<(Vec<Sel>, usize)>,
    /// ⌘D started by selecting a word from a bare cursor: subsequent matches must be whole words.
    word_search: bool,
    /// The next edit is a Backspace (marker for merging).
    next_is_delete: bool,
    /// The cursor has moved since the last edit: the typing group ends (like VS Code).
    moved_since_edit: bool,
    /// IME/dead-key composition in progress: the undo stack length at the moment it began.
    compose_start: Option<usize>,
    /// Line endings were mixed when the file was opened (LF + CRLF, or a lone CR): on save, all revert to the first line's format.
    mixed_endings: bool,
}

const BOM: &str = "\u{feff}";

/// Sort key (close to VS Code's locale comparison): accents and upper/lower
/// case are ignored at the first level (çay < dal, İstanbul ~ istanbul); ties fall back to the original text.
fn collation_key(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            let base = match c {
                'ç' | 'Ç' => 'c',
                'ğ' | 'Ğ' => 'g',
                'ı' | 'I' | 'İ' | 'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Î' => 'i',
                'ö' | 'Ö' | 'ó' | 'ò' | 'ô' | 'õ' | 'Ó' | 'Ô' => 'o',
                'ş' | 'Ş' | 'ß' => 's',
                'ü' | 'Ü' | 'ú' | 'ù' | 'û' | 'Ú' | 'Û' => 'u',
                'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'Á' | 'Â' | 'Ä' => 'a',
                'é' | 'è' | 'ê' | 'ë' | 'É' | 'Ê' => 'e',
                'ñ' | 'Ñ' => 'n',
                c => c,
            };
            base.to_lowercase()
        })
        .collect()
}

/// Marker key: positions of automatically inserted closing characters.
const AUTO_CLOSERS: usize = 99;

impl Buffer {
    pub fn empty() -> Self {
        Self::from_str("")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NotUtf8> {
        let text = std::str::from_utf8(bytes).map_err(|_| NotUtf8)?;
        Ok(Self::from_str(text))
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Self {
        let (bom, text) = match text.strip_prefix(BOM) {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        // The first line ending determines the file's format.
        let line_ending = match text.find('\n') {
            Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => LineEnding::Crlf,
            Some(_) => LineEnding::Lf,
            // No `\n` at all but there is `\r`: a CR-only file.
            None if text.contains('\r') => LineEnding::Cr,
            None => LineEnding::Lf,
        };
        let mixed_endings = has_mixed_endings(text);
        let normalized = match line_ending {
            LineEnding::Cr => text.replace('\r', "\n"),
            _ => text.replace("\r\n", "\n"),
        };
        Buffer {
            rope: Rope::from_str(&normalized),
            line_ending,
            bom,
            sels: vec![Sel::at(0)],
            primary: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
            edits: None,
            lsp: None,
            parked: Vec::new(),
            marks: Vec::new(),
            origin: None,
            cursor_history: Vec::new(),
            word_search: false,
            next_is_delete: false,
            moved_since_edit: false,
            compose_start: None,
            mixed_endings,
        }
    }

    /// Line endings were mixed when opened; saving normalizes to `line_ending()`.
    pub fn has_mixed_line_endings(&self) -> bool {
        self.mixed_endings
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::with_capacity(self.rope.len_bytes() + 3);
        if self.bom {
            out.push_str(BOM);
        }
        for chunk in self.rope.chunks() {
            match self.line_ending {
                LineEnding::Lf => out.push_str(chunk),
                LineEnding::Crlf => out.push_str(&chunk.replace('\n', "\r\n")),
                LineEnding::Cr => out.push_str(&chunk.replace('\n', "\r")),
            }
        }
        out.into_bytes()
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Start recording byte edits (for the syntax tree).
    pub fn track_edits(&mut self) {
        self.edits.get_or_insert_with(Vec::new);
    }

    pub fn take_edits(&mut self) -> Vec<ByteEdit> {
        self.edits.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Start recording language server changes.
    pub fn track_lsp(&mut self) {
        self.lsp.get_or_insert_with(Vec::new);
    }

    pub fn take_lsp_changes(&mut self) -> Vec<TextChange> {
        self.lsp.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Character position → (line, UTF-16 column): LSP's default position unit.
    pub fn lsp_position(&self, idx: usize) -> (u32, u32) {
        let idx = idx.min(self.len_chars());
        let line = self.rope.char_to_line(idx);
        let col = self.rope.char_to_utf16_cu(idx) - self.rope.char_to_utf16_cu(self.rope.line_to_char(line));
        (line as u32, col as u32)
    }

    /// (line, UTF-16 column) → character position; clamped if line/column overflow.
    pub fn from_lsp_position(&self, line: u32, col: u32) -> usize {
        let line = (line as usize).min(self.len_lines().saturating_sub(1));
        let start = self.rope.line_to_char(line);
        let base = self.rope.char_to_utf16_cu(start);
        let end = start + self.line_len_chars(line);
        let target = (base + col as usize).min(self.rope.char_to_utf16_cu(end));
        self.rope.utf16_cu_to_char(target).clamp(start, end)
    }

    fn point(&self, byte: usize) -> (usize, usize) {
        let line = self.rope.byte_to_line(byte);
        (line, byte - self.rope.line_to_byte(line))
    }

    /// Single choke point for changes: updates the rope and records the byte edit if tracking is on.
    fn splice(&mut self, at: usize, remove_chars: usize, insert: &str) {
        if self.lsp.is_some() {
            let (start, end) = (self.lsp_position(at), self.lsp_position(at + remove_chars));
            if let Some(list) = self.lsp.as_mut() {
                list.push(TextChange { start, end, text: insert.to_string() });
            }
        }
        let tracked = self.edits.is_some().then(|| {
            let start_byte = self.rope.char_to_byte(at);
            let old_end_byte = self.rope.char_to_byte(at + remove_chars);
            (start_byte, old_end_byte, self.point(start_byte), self.point(old_end_byte))
        });
        self.rope.remove(at..at + remove_chars);
        self.rope.insert(at, insert);
        if let Some(o) = self.origin {
            let inserted = insert.chars().count();
            self.origin = Some(if o <= at {
                o
            } else if o >= at + remove_chars {
                o - remove_chars + inserted
            } else {
                at + inserted
            });
        }
        if !self.parked.is_empty() || !self.marks.is_empty() {
            let inserted = insert.chars().count();
            // Inside the deleted range goes to its start; what follows shifts.
            let map = |p: usize| {
                if p <= at {
                    p
                } else if p >= at + remove_chars {
                    p - remove_chars + inserted
                } else {
                    at
                }
            };
            // Markers are tied to line starts: text inserted exactly at one pushes it forward
            // (when a line is inserted before a heading, the fold moves with the heading).
            let map_mark = |p: usize| if p == at && remove_chars == 0 { p + inserted } else { map(p) };
            for (_, list) in &mut self.marks {
                for p in list.iter_mut() {
                    *p = map_mark(*p);
                }
            }
            for (_, sels, _) in &mut self.parked {
                for s in sels.iter_mut() {
                    s.cursor = map(s.cursor);
                    s.anchor = s.anchor.map(map);
                    s.goal = None;
                }
            }
        }
        if let Some((start_byte, old_end_byte, start_point, old_end_point)) = tracked {
            let new_end_byte = start_byte + insert.len();
            let edit = ByteEdit { start_byte, old_end_byte, new_end_byte, start_point, old_end_point, new_end_point: self.point(new_end_byte) };
            self.edits.as_mut().unwrap().push(edit);
        }
    }

    /// View changed: save the current selections into slot `from` and restore those of slot `to`
    /// (if none, the current ones stay: the new view starts at the same place).
    pub fn switch_selections(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        let restored = self.parked.iter().position(|(slot, ..)| *slot == to).map(|i| self.parked.swap_remove(i));
        self.parked.retain(|(slot, ..)| *slot != from);
        self.parked.push((from, self.sels.clone(), self.primary));
        if let Some((_, sels, primary)) = restored {
            // Let merged/overlapping selections normalize.
            self.sels = sels;
            self.primary = primary.min(self.sels.len() - 1);
            self.normalize();
        }
    }

    /// Markers (empty if none).
    pub fn marks(&self, key: usize) -> &[usize] {
        self.marks.iter().find(|(k, _)| *k == key).map_or(&[], |(_, v)| v)
    }

    pub fn set_marks(&mut self, key: usize, list: Vec<usize>) {
        self.marks.retain(|(k, _)| *k != key);
        if !list.is_empty() {
            self.marks.push((key, list));
        }
    }

    /// View closed: forget the saved selections.
    pub fn forget_selections(&mut self, slot: usize) {
        self.parked.retain(|(s, ..)| *s != slot);
    }

    /// Line ending to use when saving (content is kept with `\n`).
    pub fn set_line_ending(&mut self, le: LineEnding) {
        self.line_ending = le;
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// Content identity: the step at the top of the undo stack. When undo returns to the
    /// saved state the same value comes back, so the file does not look "dirty".
    pub fn version(&self) -> u64 {
        self.undo.last().map_or(0, |e| e.id)
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line content, without the line ending.
    pub fn line(&self, idx: usize) -> String {
        let mut s = self.rope.line(idx).to_string();
        if s.ends_with('\n') {
            s.pop();
        }
        s
    }

    pub fn line_to_char(&self, line: usize) -> usize {
        self.rope.line_to_char(line)
    }

    pub fn char_to_line(&self, idx: usize) -> usize {
        self.rope.char_to_line(idx)
    }

    pub fn line_len_chars(&self, line: usize) -> usize {
        let l = self.rope.line(line);
        let n = l.len_chars();
        if n > 0 && l.char(n - 1) == '\n' { n - 1 } else { n }
    }

    fn sel(&self) -> &Sel {
        &self.sels[self.primary]
    }

    pub fn cursor(&self) -> usize {
        self.sel().cursor
    }

    /// (line, column) 0-based; column in characters.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let cursor = self.cursor();
        let line = self.rope.char_to_line(cursor);
        (line, cursor - self.rope.line_to_char(line))
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        self.sel().selection()
    }

    /// Empty range at the cursor position if there is no selection.
    pub fn selection_or_cursor(&self) -> Range<usize> {
        self.sel().range()
    }

    pub fn selection_reversed(&self) -> bool {
        self.sel().anchor.is_some_and(|a| a > self.cursor())
    }

    /// Text of the primary selection.
    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|r| self.rope.slice(r).to_string())
    }

    // --- Multiple cursors ---

    pub fn cursor_count(&self) -> usize {
        self.sels.len()
    }

    /// All cursors, in document order.
    pub fn cursors(&self) -> Vec<usize> {
        self.sels.iter().map(|s| s.cursor).collect()
    }

    /// All selections (including empty ones), in document order.
    pub fn selections(&self) -> Vec<Range<usize>> {
        self.sels.iter().map(Sel::range).collect()
    }

    /// Text of each selection (empty selections give empty text), in document order.
    pub fn selected_texts(&self) -> Vec<String> {
        self.sels.iter().map(|s| self.rope.slice(s.range()).to_string()).collect()
    }

    /// Return to a single cursor (the primary stays). `true` if there was more than one.
    pub fn collapse_cursors(&mut self) -> bool {
        if self.sels.len() == 1 {
            return false;
        }
        let keep = match self.origin.take() {
            Some(o) => *self.sels.iter().min_by_key(|s| s.cursor.abs_diff(o)).expect("at least one selection"),
            None => self.sels[self.primary],
        };
        self.sels = vec![keep];
        self.primary = 0;
        true
    }

    /// Alt+click: add a cursor; if there is already a cursor at that position (and it is not the only one), remove it.
    pub fn toggle_cursor(&mut self, idx: usize) {
        let idx = idx.min(self.len_chars());
        if self.sels.len() > 1
            && let Some(i) = self.sels.iter().position(|s| s.cursor == idx || s.selection().is_some_and(|r| r.contains(&idx)))
        {
            self.sels.remove(i);
            self.primary = self.sels.len() - 1;
            return;
        }
        self.add_sel(Sel::at(idx));
    }

    /// Visual column (tabs align to `tab` columns) → character index within the line. A column
    /// falling in the middle of a tab is rounded to the nearer edge; if beyond the line, the line end.
    pub fn char_at_visual_col(&self, line: usize, vcol: usize, tab: usize) -> usize {
        let tab = tab.max(1);
        let mut w = 0usize;
        for (i, ch) in self.line(line).chars().enumerate() {
            if ch == '\n' || ch == '\r' {
                return i;
            }
            let width = if ch == '\t' { tab - w % tab } else { 1 };
            if w >= vcol {
                return i;
            }
            if w + width > vcol {
                return if vcol - w >= width.div_ceil(2) { i + 1 } else { i };
            }
            w += width;
        }
        self.line_len_chars(line)
    }

    /// Visual column of a character index in a line (tabs aligned to `tab` columns).
    pub fn visual_col(&self, line: usize, col: usize, tab: usize) -> usize {
        let tab = tab.max(1);
        let mut w = 0usize;
        for ch in self.line(line).chars().take(col) {
            w += if ch == '\t' { tab - w % tab } else { 1 };
        }
        w
    }

    /// Column (box) selection: in every line between `anchor` and `head` (line, **visual** column), select the same
    /// visual column range; tabs align to `tab` columns, and on a short line the range is clamped to the line
    /// end. The primary cursor is on `head`'s line. If the columns are equal, an empty cursor on every line.
    pub fn select_columns(&mut self, anchor: (usize, usize), head: (usize, usize), tab: usize) {
        let last = self.len_lines().saturating_sub(1);
        let (top, bottom) = (anchor.0.min(head.0).min(last), anchor.0.max(head.0).min(last));
        let (left, right) = (anchor.1.min(head.1), anchor.1.max(head.1));
        let head_right = head.1 >= anchor.1;
        let mut sels = Vec::with_capacity(bottom - top + 1);
        let mut primary = 0;
        for line in top..=bottom {
            let start = self.rope.line_to_char(line);
            let (l, r) = (start + self.char_at_visual_col(line, left, tab), start + self.char_at_visual_col(line, right, tab));
            let goal = Some(head.1);
            sels.push(if l == r {
                Sel { cursor: l, anchor: None, goal }
            } else if head_right {
                Sel { cursor: r, anchor: Some(l), goal }
            } else {
                Sel { cursor: l, anchor: Some(r), goal }
            });
            if line == head.0.min(last) {
                primary = sels.len() - 1;
            }
        }
        self.cursor_history.push((self.sels.clone(), self.primary));
        if self.cursor_history.len() > 100 {
            self.cursor_history.remove(0);
        }
        self.sels = sels;
        self.primary = primary;
        self.word_search = false;
        self.origin = None;
    }

    /// Alt+⌘+↑/↓: add a cursor one line beyond the topmost (or bottommost) cursor.
    pub fn add_cursor_vertical(&mut self, delta: isize) {
        let edge = if delta < 0 { self.sels[0] } else { *self.sels.last().unwrap() };
        let line = self.rope.char_to_line(edge.cursor);
        let col = edge.cursor - self.rope.line_to_char(line);
        let goal = edge.goal.unwrap_or(col);
        let target = line as isize + delta;
        if target < 0 || target >= self.len_lines() as isize {
            return;
        }
        let target = target as usize;
        let at = self.rope.line_to_char(target) + goal.min(self.line_len_chars(target));
        self.add_sel(Sel { cursor: at, anchor: None, goal: Some(goal) });
    }

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

    /// Occurrences of the text (character ranges, non-overlapping); `whole_word`: on word boundaries.
    fn occurrences(&self, needle: &str, whole_word: bool) -> Vec<Range<usize>> {
        if needle.is_empty() {
            return Vec::new();
        }
        let text = self.rope.to_string();
        let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        text.match_indices(needle)
            .filter(|(b, _)| !whole_word || (!is_word(text[..*b].chars().next_back()) && !is_word(text[b + needle.len()..].chars().next())))
            .map(|(b, _)| {
                let start = self.rope.byte_to_char(b);
                start..start + needle.chars().count()
            })
            .collect()
    }

    fn add_sel(&mut self, sel: Sel) {
        self.cursor_history.push((self.sels.clone(), self.primary));
        if self.cursor_history.len() > 100 {
            self.cursor_history.remove(0);
        }
        if self.sels.len() == 1 {
            self.origin = Some(self.sels[0].cursor);
        }
        self.sels.push(sel);
        self.primary = self.sels.len() - 1;
        self.normalize();
    }

    /// Sort, merge overlapping ones; track the primary selection.
    fn normalize(&mut self) {
        let primary = self.sels[self.primary];
        let mut v = std::mem::take(&mut self.sels);
        v.sort_by_key(|s| (s.start(), s.end()));
        let mut out: Vec<Sel> = Vec::with_capacity(v.len());
        let mut primary_ix = 0;
        for s in v {
            let is_primary = s == primary;
            if let Some(last) = out.last_mut() {
                let touching = s.start() == last.end() && (s.selection().is_none() || last.selection().is_none());
                if s.start() < last.end() || s.start() == last.start() || touching {
                    let (start, end) = (last.start(), last.end().max(s.end()));
                    *last = if start == end {
                        Sel::at(start)
                    } else if last.anchor.is_some_and(|a| a > last.cursor) {
                        Sel { cursor: start, anchor: Some(end), goal: None }
                    } else {
                        Sel { cursor: end, anchor: Some(start), goal: None }
                    };
                    if is_primary {
                        primary_ix = out.len() - 1;
                    }
                    continue;
                }
            }
            if is_primary {
                primary_ix = out.len();
            }
            out.push(s);
        }
        self.sels = out;
        self.primary = primary_ix;
    }

    pub fn text_range(&self, range: Range<usize>) -> String {
        let end = range.end.min(self.len_chars());
        self.rope.slice(range.start.min(end)..end).to_string()
    }

    /// Match of the bracket right before or on the cursor: (bracket, its match).
    /// At most 20,000 characters are scanned; no distinction is made for strings/comments.
    pub fn matching_bracket(&self, cursor: usize) -> Option<(usize, usize)> {
        const LIMIT: usize = 20_000;
        let n = self.len_chars();
        let candidates = [cursor.checked_sub(1), (cursor < n).then_some(cursor)];
        for at in candidates.into_iter().flatten() {
            let ch = self.rope.char(at);
            let (open, close, forward) = match ch {
                '(' => ('(', ')', true),
                '[' => ('[', ']', true),
                '{' => ('{', '}', true),
                ')' => ('(', ')', false),
                ']' => ('[', ']', false),
                '}' => ('{', '}', false),
                _ => continue,
            };
            let mut depth = 0usize;
            if forward {
                for (i, c) in self.rope.chars_at(at).enumerate().take(LIMIT) {
                    if c == open {
                        depth += 1;
                    } else if c == close {
                        depth -= 1;
                        if depth == 0 {
                            return Some((at, at + i));
                        }
                    }
                }
            } else {
                let mut it = self.rope.chars_at(at + 1);
                for i in 0..LIMIT.min(at + 1) {
                    let Some(c) = it.prev() else { break };
                    if c == close {
                        depth += 1;
                    } else if c == open {
                        depth -= 1;
                        if depth == 0 {
                            return Some((at, at - i));
                        }
                    }
                }
            }
            return None;
        }
        None
    }

    /// The word (letters, digits, `_`) the cursor is inside or immediately to the right of.
    pub fn word_at(&self, cursor: usize) -> Option<Range<usize>> {
        let line = self.rope.char_to_line(cursor);
        let start = self.rope.line_to_char(line);
        let chars: Vec<char> = self.line(line).chars().collect();
        let is_word = |c: char| c.is_alphanumeric() || c == '_' || is_combining_mark(c);
        let col = cursor - start;
        let anchor = if col < chars.len() && is_word(chars[col]) {
            col
        } else if col > 0 && is_word(chars[col - 1]) {
            col - 1
        } else {
            return None;
        };
        let a = (0..=anchor).rev().take_while(|&i| is_word(chars[i])).last()?;
        let b = (anchor..chars.len()).take_while(|&i| is_word(chars[i])).last()? + 1;
        Some(start + a..start + b)
    }

    // --- UTF-16 (platform text input / IME uses this unit) ---

    pub fn char_to_utf16(&self, idx: usize) -> usize {
        self.rope.char_to_utf16_cu(idx.min(self.len_chars()))
    }

    pub fn utf16_to_char(&self, cu: usize) -> usize {
        self.rope.utf16_cu_to_char(cu.min(self.rope.len_utf16_cu()))
    }

    // --- Cursor ---

    /// Collapse to a single cursor at the position (`select`: extend the primary selection).
    pub fn set_cursor(&mut self, idx: usize, select: bool) {
        self.moved_since_edit = true;
        self.word_search = false;
        let idx = idx.min(self.len_chars());
        let s = *self.sel();
        let anchor = if select { Some(s.anchor.unwrap_or(s.cursor)) } else { None };
        self.sels = vec![Sel { cursor: idx, anchor, goal: None }];
        self.primary = 0;
    }

    pub fn set_selection(&mut self, range: Range<usize>, reversed: bool) {
        self.moved_since_edit = true;
        self.word_search = false;
        let n = self.len_chars();
        let (a, c) = if reversed { (range.end, range.start) } else { (range.start, range.end) };
        self.sels = vec![Sel { cursor: c.min(n), anchor: Some(a.min(n)), goal: None }];
        self.primary = 0;
    }

    pub fn select_all(&mut self) {
        self.set_selection(0..self.len_chars(), false);
    }

    /// Move every selection with `f` (`select`: keep the anchor).
    fn move_each(&mut self, select: bool, f: impl Fn(&Self, &Sel) -> (usize, Option<usize>)) {
        self.moved_since_edit = true;
        self.word_search = false;
        let n = self.len_chars();
        let moved: Vec<Sel> = self
            .sels
            .iter()
            .map(|s| {
                let (to, goal) = f(self, s);
                let anchor = if select { Some(s.anchor.unwrap_or(s.cursor)) } else { None };
                Sel { cursor: to.min(n), anchor, goal }
            })
            .collect();
        self.sels = moved;
        self.normalize();
    }

    pub fn move_left(&mut self, select: bool) {
        self.move_each(select, |b, s| match s.selection() {
            Some(r) if !select => (r.start, None),
            _ => (b.prev_boundary(s.cursor), None),
        });
    }

    pub fn move_right(&mut self, select: bool) {
        self.move_each(select, |b, s| match s.selection() {
            Some(r) if !select => (r.end, None),
            _ => (b.next_boundary(s.cursor), None),
        });
    }

    /// Move every cursor with `f(cursor, target column)`; the returned target column is kept (so the
    /// column is not forgotten when passing through a short line during vertical movement). For the view layer's
    /// (line wrapping, folding) own vertical movement.
    pub fn move_cursors(&mut self, select: bool, f: impl Fn(usize, Option<usize>) -> (usize, Option<usize>)) {
        self.move_each(select, |_, s| f(s.cursor, s.goal));
    }

    /// A new cursor below/above each cursor at the given position (the view layer computes it).
    pub fn add_cursor_at(&mut self, at: usize, goal: Option<usize>) {
        let at = at.min(self.len_chars());
        if self.sels.iter().any(|s| s.cursor == at) {
            return;
        }
        self.add_sel(Sel { cursor: at, anchor: None, goal });
    }

    /// The topmost and bottommost cursors (and their target columns).
    pub fn edge_cursors(&self) -> ((usize, Option<usize>), (usize, Option<usize>)) {
        let (first, last) = (self.sels[0], *self.sels.last().unwrap());
        ((first.cursor, first.goal), (last.cursor, last.goal))
    }

    pub fn move_vertical(&mut self, delta: isize, select: bool) {
        self.move_each(select, |b, s| {
            let line = b.rope.char_to_line(s.cursor);
            let col = s.cursor - b.rope.line_to_char(line);
            let goal = s.goal.unwrap_or(col);
            let target = (line as isize + delta).clamp(0, b.len_lines() as isize - 1) as usize;
            let idx = if target == line && delta < 0 {
                0
            } else if target == line && delta > 0 {
                b.len_chars()
            } else {
                b.rope.line_to_char(target) + goal.min(b.line_len_chars(target))
            };
            (idx, Some(goal))
        });
    }

    /// ↑/↓ (visual column): the target column treats tabs as aligned to `tab` columns, so when going from tab-indented lines to space-indented
    /// ones the cursor stays on the same vertical line on screen (`move_vertical` preserves the character column and would drift).
    pub fn move_vertical_visual(&mut self, delta: isize, select: bool, tab: usize) {
        self.move_each(select, |b, s| {
            let line = b.rope.char_to_line(s.cursor);
            let col = s.cursor - b.rope.line_to_char(line);
            let goal = s.goal.unwrap_or_else(|| b.visual_col(line, col, tab));
            let target = (line as isize + delta).clamp(0, b.len_lines() as isize - 1) as usize;
            let idx = if target == line && delta < 0 {
                0
            } else if target == line && delta > 0 {
                b.len_chars()
            } else {
                b.rope.line_to_char(target) + b.char_at_visual_col(target, goal, tab).min(b.line_len_chars(target))
            };
            (idx, Some(goal))
        });
    }

    /// First press: the first non-whitespace character of the line; again: line start.
    pub fn move_home(&mut self, select: bool) {
        self.move_each(select, |b, s| {
            let line = b.rope.char_to_line(s.cursor);
            let start = b.rope.line_to_char(line);
            let indent = b.line(line).chars().take_while(|c| c.is_whitespace()).count();
            (start + if s.cursor - start == indent { 0 } else { indent }, None)
        });
    }

    pub fn move_end(&mut self, select: bool) {
        self.move_each(select, |b, s| {
            let line = b.rope.char_to_line(s.cursor);
            (b.rope.line_to_char(line) + b.line_len_chars(line), None)
        });
    }

    /// ⌥←: start of the previous word (skip whitespace, then characters of the same kind).
    pub fn move_word_left(&mut self, select: bool) {
        self.move_each(select, |b, s| (b.word_start_before(s.cursor), None));
    }

    /// ⌥→: end of the next word.
    pub fn move_word_right(&mut self, select: bool) {
        self.move_each(select, |b, s| (b.word_end_after(s.cursor), None));
    }

    /// ⌥⌫ / ⌥⌦: delete the selection if any, otherwise from the cursor to the word boundary.
    pub fn delete_word(&mut self, forward: bool) {
        self.edit_each(
            |b, _, s| match s.selection() {
                Some(r) => (r, String::new()),
                None if forward => (s.cursor..b.word_end_after(s.cursor), String::new()),
                None => (b.word_start_before(s.cursor)..s.cursor, String::new()),
            },
            false,
        );
    }

    /// Extend the selection to whole line(s), including the line ending (triple click).
    pub fn select_line_at(&mut self, idx: usize) {
        let line = self.rope.char_to_line(idx.min(self.len_chars()));
        let start = self.rope.line_to_char(line);
        let end = if line + 1 < self.len_lines() { self.rope.line_to_char(line + 1) } else { self.len_chars() };
        self.set_selection(start..end, false);
    }

    fn word_start_before(&self, idx: usize) -> usize {
        let mut i = idx;
        while i > 0 && char_class(self.rope.char(i - 1)) == 0 {
            i -= 1;
        }
        if i == 0 {
            return 0;
        }
        let class = char_class(self.rope.char(i - 1));
        while i > 0 && char_class(self.rope.char(i - 1)) == class {
            i -= 1;
        }
        i
    }

    fn word_end_after(&self, idx: usize) -> usize {
        let n = self.len_chars();
        let mut i = idx;
        while i < n && char_class(self.rope.char(i)) == 0 {
            i += 1;
        }
        if i == n {
            return n;
        }
        let class = char_class(self.rope.char(i));
        while i < n && char_class(self.rope.char(i)) == class {
            i += 1;
        }
        i
    }

    fn prev_boundary(&self, idx: usize) -> usize {
        if idx == 0 {
            return 0;
        }
        let line = self.rope.char_to_line(idx);
        let start = self.rope.line_to_char(line);
        if idx == start {
            return idx - 1; // to the end of the previous line
        }
        let text = self.line(line);
        let byte = char_to_byte(&text, idx - start);
        let prev = text.grapheme_indices(true).map(|(b, _)| b).take_while(|&b| b < byte).last().unwrap_or(0);
        start + text[..prev].chars().count()
    }

    fn next_boundary(&self, idx: usize) -> usize {
        if idx >= self.len_chars() {
            return self.len_chars();
        }
        let line = self.rope.char_to_line(idx);
        let start = self.rope.line_to_char(line);
        let text = self.line(line);
        let col = idx - start;
        if col >= text.chars().count() {
            return idx + 1; // skip past the line ending
        }
        let byte = char_to_byte(&text, col);
        let next = text.grapheme_indices(true).map(|(b, _)| b).find(|&b| b > byte).unwrap_or(text.len());
        start + text[..next].chars().count()
    }

    // --- Editing ---

    /// Replace each selection (or cursor if none) with the text.
    pub fn insert(&mut self, text: &str) {
        // A letter replacing a selection also starts the typing group (following letters join it).
        let typing = text.chars().count() == 1 && text != "\n";
        self.edit_each(|_, _, s| (s.range(), text.to_string()), typing);
    }

    /// Each selection gets its own text (line-by-line paste with multiple cursors).
    /// `texts` must have the same length as the number of selections; otherwise their concatenation is written to all.
    pub fn insert_each(&mut self, texts: &[String]) {
        if texts.len() != self.sels.len() {
            return self.insert(&texts.join("\n"));
        }
        self.edit_each(|_, i, s| (s.range(), texts[i].clone()), false);
    }

    /// Replace the given range (returns to a single cursor); the cursor goes to the end of the inserted text.
    pub fn replace(&mut self, range: Range<usize>, text: &str, typing: bool) {
        let end = range.end.min(self.len_chars());
        let start = range.start.min(end);
        self.collapse_cursors();
        self.edit_parts(vec![(start..end, text.to_string())], Place::End, typing);
    }

    /// Replace multiple ranges in a single undo step (replace all). Ranges
    /// must be ascending and non-overlapping; the single cursor goes to the end of the last change.
    pub fn replace_all(&mut self, parts: Vec<(Range<usize>, String)>) {
        self.collapse_cursors();
        self.edit_parts(parts, Place::End, false);
    }

    /// For each selection `f(index, selection) → (range, new text)`; cursors go to the end of the inserted texts.
    fn edit_each(&mut self, f: impl Fn(&Self, usize, &Sel) -> (Range<usize>, String), typing: bool) {
        let parts = self.sels.iter().enumerate().map(|(i, s)| f(self, i, s)).collect();
        self.edit_parts(parts, Place::End, typing);
    }

    /// Common path: apply the pieces, record as a single undo step. `keep`:
    /// cursors are placed by shifting with this function instead of at the end of the inserted text.
    fn edit_parts(&mut self, raw: Vec<(Range<usize>, String)>, place: Place, typing: bool) {
        self.cursor_history.clear();
        self.word_search = false;
        let n = self.len_chars();
        let mut parts: Vec<Part> = Vec::with_capacity(raw.len());
        // Each piece's own cursor (same order as the selections): positioned afterwards.
        let mut owners: Vec<usize> = Vec::with_capacity(raw.len());
        let mut prev_end = 0;
        for (i, (range, text)) in raw.into_iter().enumerate() {
            let end = range.end.min(n);
            let start = range.start.min(end).max(prev_end);
            let end = end.max(start);
            prev_end = end;
            // Typing and most pastes have no `\r`: keep the string instead of copying it twice.
            let text = if text.contains('\r') { unify_breaks(&text) } else { text };
            parts.push(Part { at: start, removed: self.rope.slice(start..end).to_string(), inserted: text });
            owners.push(i);
        }
        if parts.iter().all(|p| p.removed.is_empty() && p.inserted.is_empty()) {
            return;
        }
        let (sels_before, primary_before) = (self.sels.clone(), self.primary);
        self.apply_parts(&parts);
        // New cursors: the piece's start in the final document + inserted length.
        let mut shift: isize = 0;
        let mut after = Vec::with_capacity(parts.len());
        for p in &parts {
            let start = (p.at as isize + shift) as usize;
            let ins = p.inserted.chars().count();
            after.push(Sel::at(start + ins));
            shift += ins as isize - p.removed.chars().count() as isize;
        }
        let sels_after = match place {
            Place::Keep(f) => sels_before.iter().map(|s| Sel { cursor: f(s.cursor), anchor: s.anchor.map(f), goal: None }).collect(),
            Place::Custom(f) => {
                let mut shift: isize = 0;
                parts
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let start = (p.at as isize + shift) as usize;
                        shift += p.inserted.chars().count() as isize - p.removed.chars().count() as isize;
                        f(i, start)
                    })
                    .collect()
            }
            Place::End if owners.len() == self.sels.len() => after,
            Place::End => vec![*after.last().unwrap()],
        };
        // A placement counted on the text as written (`\r\n` is two chars there, one here) must not leave a cursor past the end.
        let end = self.len_chars();
        self.sels = sels_after.into_iter().map(|s| Sel { cursor: s.cursor.min(end), anchor: s.anchor.map(|a| a.min(end)), goal: s.goal }).collect();
        self.primary = primary_before.min(self.sels.len() - 1);
        self.normalize();
        let edit = Edit {
            parts,
            sels_before,
            primary_before,
            sels_after: self.sels.clone(),
            primary_after: self.primary,
            id: self.next_id,
            typing,
            deleting: std::mem::take(&mut self.next_is_delete),
            chain: false,
        };
        self.next_id += 1;
        self.push_undo(edit);
        self.redo.clear();
    }

    /// Last to first, so earlier pieces keep their positions.
    fn apply_parts(&mut self, parts: &[Part]) {
        for p in parts.iter().rev() {
            self.splice(p.at, p.removed.chars().count(), &p.inserted);
        }
    }

    /// Automatic edit (like trimming whitespace on save): cursors and
    /// selections stay in place, or shift if after the range. Enters undo as a single step.
    pub fn edit_keep_cursor(&mut self, range: Range<usize>, text: &str) {
        let end = range.end.min(self.len_chars());
        let start = range.start.min(end);
        let inserted = text.chars().count();
        let shift = move |p: usize| -> usize {
            if p >= end {
                p - (end - start) + inserted
            } else if p > start {
                start + (p - start).min(inserted)
            } else {
                p
            }
        };
        self.edit_parts(vec![(start..end, text.to_string())], Place::Keep(&shift), false);
    }

    /// Delete trailing whitespace and tabs at line ends. A single undo step (so that if 100 thousand lines are trimmed on save, undoing is not 100 thousand
    /// key presses); cursors stay on their own lines.
    pub fn trim_trailing_whitespace(&mut self) {
        let mut parts: Vec<(Range<usize>, String)> = Vec::new();
        for line in 0..self.len_lines() {
            let text = self.line(line);
            let kept = text.trim_end_matches([' ', '\t']).chars().count();
            let total = text.chars().count();
            if kept < total {
                let start = self.line_to_char(line);
                parts.push((start + kept..start + total, String::new()));
            }
        }
        if !parts.is_empty() {
            let keep = Self::mapper(&parts);
            self.edit_parts(parts.clone(), Place::Keep(&keep), false);
        }
    }

    /// Append a line ending if the file is non-empty and does not end with one. If the cursor is at the end of the file (`abc|`) it stays where it is: saving
    /// must not throw the cursor onto the new empty line.
    pub fn ensure_final_newline(&mut self) {
        let n = self.len_chars();
        if n > 0 && self.rope.char(n - 1) != '\n' {
            let stay = |p: usize| p;
            self.edit_parts(vec![(n..n, "\n".to_string())], Place::Keep(&stay), false);
        }
    }

    pub fn backspace(&mut self) {
        self.next_is_delete = self.sels.len() == 1 && self.sels[0].selection().is_none();
        self.edit_each(
            |b, _, s| match s.selection() {
                Some(r) => (r, String::new()),
                None => (b.prev_boundary(s.cursor)..s.cursor, String::new()),
            },
            false,
        );
    }

    pub fn delete(&mut self) {
        self.edit_each(
            |b, _, s| match s.selection() {
                Some(r) => (r, String::new()),
                None => (s.cursor..b.next_boundary(s.cursor), String::new()),
            },
            false,
        );
    }

    /// Delete the whole line of each cursor (with its line ending): cut without a selection.
    pub fn delete_lines(&mut self) {
        self.edit_each(
            |b, _, s| {
                let (first, last) = (b.rope.char_to_line(s.start()), b.rope.char_to_line(s.end()));
                let mut start = b.rope.line_to_char(first);
                let line_end = b.rope.line_to_char(last) + b.line_len_chars(last);
                // If the last line has no line ending, take the preceding one (so no empty line is left).
                if line_end == b.len_chars() && first > 0 {
                    start -= 1;
                }
                let end = (line_end + 1).min(b.len_chars());
                (start..end, String::new())
            },
            false,
        );
    }

    /// IME composition: the range at each cursor (`ranges`, in cursor order) is replaced
    /// with the same text; cursors end up at the end of the text. Intermediate steps become a single undo step in `end_compose`.
    pub fn compose(&mut self, ranges: Vec<Range<usize>>, text: &str) {
        if self.compose_start.is_none() {
            self.compose_start = Some(self.undo.len());
        }
        let parts = ranges.into_iter().map(|r| (r, text.to_string())).collect();
        self.edit_parts(parts, Place::End, false);
    }

    /// Composition ended: merge the intermediate steps (what the first deleted → what the last inserted).
    pub fn end_compose(&mut self) {
        let Some(start) = self.compose_start.take() else { return };
        if self.undo.len() <= start + 1 {
            return;
        }
        let edits: Vec<Edit> = self.undo.drain(start..).collect();
        let n = edits[0].parts.len();
        if edits.iter().any(|e| e.parts.len() != n) {
            self.undo.extend(edits); // unexpected shape: leave as is
            return;
        }
        let (first, last) = (&edits[0], &edits[edits.len() - 1]);
        let merged = Edit {
            parts: first.parts.iter().zip(&last.parts).map(|(f, l)| Part { at: f.at, removed: f.removed.clone(), inserted: l.inserted.clone() }).collect(),
            sels_before: first.sels_before.clone(),
            primary_before: first.primary_before,
            sels_after: last.sels_after.clone(),
            primary_after: last.primary_after,
            id: last.id,
            typing: false,
            deleting: false,
            chain: false,
        };
        self.undo.push(merged);
    }

    pub fn is_composing(&self) -> bool {
        self.compose_start.is_some()
    }

    /// ⌘U: undo the last cursor addition.
    pub fn undo_cursor(&mut self) -> bool {
        let Some((sels, primary)) = self.cursor_history.pop() else { return false };
        let n = self.len_chars();
        self.sels = sels.into_iter().map(|s| Sel { cursor: s.cursor.min(n), anchor: s.anchor.map(|a| a.min(n)), goal: None }).collect();
        self.primary = primary.min(self.sels.len() - 1);
        self.normalize();
        true
    }

    /// ⇧⌥I: a cursor at the end of each selected line.
    pub fn cursors_at_line_ends(&mut self) {
        let mut ends = Vec::new();
        for s in &self.sels {
            let (first, mut last) = (self.rope.char_to_line(s.start()), self.rope.char_to_line(s.end()));
            if last > first && s.end() == self.rope.line_to_char(last) {
                last -= 1;
            }
            for l in first..=last {
                ends.push(self.rope.line_to_char(l) + self.line_len_chars(l));
            }
        }
        if ends.is_empty() {
            return;
        }
        self.cursor_history.push((self.sels.clone(), self.primary));
        self.sels = ends.into_iter().map(Sel::at).collect();
        self.primary = self.sels.len() - 1;
        self.normalize();
    }

    /// Tab (indent with spaces): spaces up to the next tab stop (like VS Code).
    pub fn insert_to_tab_stop(&mut self, width: usize) {
        let width = width.max(1);
        let parts = self
            .sels
            .iter()
            .map(|s| {
                let line = self.rope.char_to_line(s.start());
                let col = s.start() - self.rope.line_to_char(line);
                (s.range(), " ".repeat(width - col % width))
            })
            .collect();
        self.edit_parts(parts, Place::End, false);
    }

    /// Backspace inside indentation: delete back to the previous tab stop. `false` if any of the
    /// cursors is not in indentation (let normal Backspace run).
    pub fn backspace_to_tab_stop(&mut self, width: usize) -> bool {
        let width = width.max(1);
        let mut parts = Vec::new();
        for s in &self.sels {
            if s.selection().is_some() {
                return false;
            }
            let line = self.rope.char_to_line(s.cursor);
            let start = self.rope.line_to_char(line);
            let before = self.text_range(start..s.cursor);
            if before.is_empty() || !before.chars().all(|c| c == ' ') {
                return false;
            }
            let col = before.chars().count();
            let back = (col - 1) % width + 1;
            parts.push((s.cursor - back..s.cursor, String::new()));
        }
        self.next_is_delete = self.sels.len() == 1;
        self.edit_parts(parts, Place::End, false);
        true
    }

    /// ⌘K ⌘K: delete from the cursor to the end of the line (the selection if there is one). At the line end,
    /// joins with the next line (like Delete).
    pub fn delete_to_line_end(&mut self) {
        let n = self.len_chars();
        let parts = self
            .sels
            .iter()
            .map(|s| {
                if s.selection().is_some() {
                    return (s.range(), String::new());
                }
                let line = self.rope.char_to_line(s.cursor);
                let end = self.rope.line_to_char(line) + self.line_len_chars(line);
                (s.cursor..if end == s.cursor { (end + 1).min(n) } else { end }, String::new())
            })
            .collect();
        self.edit_parts(parts, Place::End, false);
    }

    /// Select the inside of the bracket pair surrounding the cursor (excluding the brackets). If the cursor is adjacent to a bracket,
    /// that one; otherwise the nearest pair enclosing it. `false` if no pair is found.
    pub fn select_to_bracket(&mut self) -> bool {
        let cursor = self.cursor();
        let pair = self.matching_bracket(cursor).or_else(|| {
            // Enclosing opener: backwards, the first unbalanced opener.
            let mut depth = 0usize;
            let mut it = self.rope.chars_at(cursor);
            for i in 0..cursor.min(20_000) {
                let c = it.prev()?;
                match c {
                    ')' | ']' | '}' => depth += 1,
                    '(' | '[' | '{' if depth > 0 => depth -= 1,
                    '(' | '[' | '{' => return self.matching_bracket(cursor - i - 1 + 1).filter(|&(a, _)| a == cursor - i - 1),
                    _ => {}
                }
            }
            None
        });
        let Some((a, b)) = pair else { return false };
        let (lo, hi) = (a.min(b) + 1, a.max(b));
        self.sels = vec![Sel { cursor: hi, anchor: Some(lo), goal: None }];
        self.primary = 0;
        true
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

    /// ⌘⌫: delete from the cursor to the line start (the selection if there is one).
    pub fn delete_to_line_start(&mut self) {
        let parts = self
            .sels
            .iter()
            .map(|s| {
                if s.selection().is_some() {
                    return (s.range(), String::new());
                }
                let start = self.rope.line_to_char(self.rope.char_to_line(s.cursor));
                // At line start, join with the previous line (like Backspace).
                (if start == s.cursor { s.cursor.saturating_sub(1) } else { start }..s.cursor, String::new())
            })
            .collect();
        self.edit_parts(parts, Place::End, false);
    }

    /// ⌘↵ / ⇧⌘↵: an empty line below/above with the same indentation, cursor there.
    pub fn insert_line(&mut self, below: bool) {
        let parts: Vec<(Range<usize>, String)> = self
            .sels
            .iter()
            .map(|s| {
                let line = self.rope.char_to_line(s.cursor);
                let text = self.line(line);
                let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                if below {
                    let end = self.rope.line_to_char(line) + self.line_len_chars(line);
                    (end..end, format!("\n{indent}"))
                } else {
                    let start = self.rope.line_to_char(line);
                    (start..start, format!("{indent}\n"))
                }
            })
            .collect();
        let lens: Vec<usize> = parts.iter().map(|(_, t)| t.chars().count()).collect();
        // Below: end of the inserted text; above: end of the indentation (on the new line).
        self.edit_parts(parts, Place::Custom(&|i, start| Sel::at(if below { start + lens[i] } else { start + lens[i] - 1 })), false);
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

    /// Rewrite line blocks; `need_selection`: a bare cursor counts as a single line and
    /// nothing is done. Without a selection (`false`) the whole file.
    fn rewrite_blocks(&mut self, need_selection: bool, f: impl Fn(Vec<String>) -> Vec<String>) {
        let has_selection = self.sels.iter().any(|s| s.selection().is_some());
        let blocks = if has_selection {
            self.line_blocks()
        } else if need_selection {
            return;
        } else {
            let last = self.len_lines().saturating_sub(1);
            // The trailing empty "line" (file ends with \n) is not part of the block.
            let last = if last > 0 && self.line(last).is_empty() { last - 1 } else { last };
            vec![(0, last)]
        };
        let mut parts: Vec<(Range<usize>, String)> = Vec::new();
        for (first, last) in blocks {
            if first == last {
                continue;
            }
            let old: Vec<String> = (first..=last).map(|l| self.line(l)).collect();
            let new = f(old.clone());
            if new == old {
                continue;
            }
            let range = self.rope.line_to_char(first)..self.rope.line_to_char(last) + self.line_len_chars(last);
            parts.push((range, new.join("\n")));
        }
        if parts.is_empty() {
            return;
        }
        let lens: Vec<usize> = parts.iter().map(|(_, t)| t.chars().count()).collect();
        self.edit_parts(parts, Place::Custom(&|i, start| Sel { cursor: start + lens[i], anchor: Some(start), goal: None }), false);
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

    /// Set the selections to the given ranges (for extending/shrinking the selection).
    pub fn set_selections(&mut self, ranges: &[Range<usize>]) {
        if ranges.is_empty() {
            return;
        }
        let n = self.len_chars();
        self.sels = ranges.iter().map(|r| Sel { cursor: r.end.min(n), anchor: Some(r.start.min(n)), goal: None }).collect();
        self.primary = self.sels.len() - 1;
        self.normalize();
    }

    /// External edits (formatting, renaming): a single undo
    /// step, cursors shift along with the text. Ranges must not overlap.
    pub fn apply_edits(&mut self, mut edits: Vec<(Range<usize>, String)>) {
        edits.sort_by_key(|(r, _)| (r.start, r.end));
        let keep = Self::mapper(&edits);
        self.edit_parts(edits.clone(), Place::Keep(&keep), false);
    }

    // --- Line commands ---

    /// Line blocks covered by the selections (in order, adjacent ones merged). If a selection
    /// ends at the start of a line, that line is not included (like VS Code).
    fn line_blocks(&self) -> Vec<(usize, usize)> {
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        for s in &self.sels {
            let first = self.rope.char_to_line(s.start());
            let mut last = self.rope.char_to_line(s.end());
            if last > first && s.end() == self.rope.line_to_char(last) {
                last -= 1;
            }
            match blocks.last_mut() {
                Some(b) if first <= b.1 + 1 => b.1 = b.1.max(last),
                _ => blocks.push((first, last)),
            }
        }
        blocks
    }

    /// Typed character: auto-closing pairs. Wraps if there is a selection; if on top of a
    /// closing character, steps over it; otherwise inserts the pair where appropriate, leaving the cursor between.
    pub fn type_char(&mut self, ch: char, auto_close: bool) {
        const PAIRS: [(char, char); 6] = [('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\''), ('`', '`')];
        let pair = PAIRS.iter().find(|(o, _)| *o == ch).map(|&(_, c)| c);
        let closer = PAIRS.iter().any(|&(_, c)| c == ch);
        if !auto_close || (pair.is_none() && !closer) {
            return self.insert(&ch.to_string());
        }
        let n = self.len_chars();
        let next = |i: usize| (i < n).then(|| self.rope.char(i));
        let prev = |i: usize| (i > 0).then(|| self.rope.char(i - 1));
        // Step over the closer: only if the editor inserted it itself (VS Code
        // "autoClosingOvertype"); before a user-typed ")" a new ")" is inserted.
        let auto = self.marks(AUTO_CLOSERS).to_vec();
        if closer && self.sels.iter().all(|s| s.selection().is_none() && next(s.cursor) == Some(ch) && auto.contains(&s.cursor)) {
            let skipped: Vec<usize> = self.sels.iter().map(|s| s.cursor).collect();
            let moved: Vec<Sel> = self.sels.iter().map(|s| Sel::at(s.cursor + 1)).collect();
            self.sels = moved;
            self.set_marks(AUTO_CLOSERS, auto.into_iter().filter(|p| !skipped.contains(p)).collect());
            return;
        }
        let Some(close) = pair else { return self.insert(&ch.to_string()) };
        #[derive(Clone, Copy)]
        enum Kind {
            Wrap(usize),
            Pair,
            Plain,
        }
        let kinds: Vec<Kind> = self
            .sels
            .iter()
            .map(|s| match s.selection() {
                Some(r) => Kind::Wrap(r.len()),
                None => {
                    let after_ok = next(s.cursor).is_none_or(|c| c.is_whitespace() || ")]}>,;:".contains(c));
                    // Quote: no pair is opened inside a word (don't) or after an escape.
                    let quote_ok = ch != close || prev(s.cursor).is_none_or(|c| !(c.is_alphanumeric() || c == '\\' || c == ch));
                    if after_ok && quote_ok { Kind::Pair } else { Kind::Plain }
                }
            })
            .collect();
        let parts: Vec<(Range<usize>, String)> = self
            .sels
            .iter()
            .zip(&kinds)
            .map(|(s, k)| match k {
                Kind::Wrap(_) => (s.range(), format!("{ch}{}{close}", self.rope.slice(s.range()))),
                Kind::Pair => (s.range(), format!("{ch}{close}")),
                Kind::Plain => (s.range(), ch.to_string()),
            })
            .collect();
        let place = |i: usize, start: usize| match kinds[i] {
            Kind::Wrap(len) => Sel { cursor: start + 1 + len, anchor: Some(start + 1), goal: None },
            Kind::Pair | Kind::Plain => Sel::at(start + 1),
        };
        self.edit_parts(parts, Place::Custom(&place), true);
        // Positions of the inserted closers (immediately right of the cursor).
        let mut auto = self.marks(AUTO_CLOSERS).to_vec();
        for (s, k) in self.sels.iter().zip(&kinds) {
            if matches!(k, Kind::Pair) {
                auto.push(s.cursor);
            }
        }
        self.set_marks(AUTO_CLOSERS, auto);
    }

    /// Delete backwards; inside an empty pair (`(|)`) both at once.
    pub fn backspace_pair(&mut self) {
        const PAIRS: [(char, char); 6] = [('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\''), ('`', '`')];
        let n = self.len_chars();
        let in_pair = |c: usize| c > 0 && c < n && PAIRS.iter().any(|&(o, cl)| self.rope.char(c - 1) == o && self.rope.char(c) == cl);
        if !self.sels.iter().any(|s| s.selection().is_none() && in_pair(s.cursor)) {
            return self.backspace();
        }
        let pairs: Vec<bool> = self.sels.iter().map(|s| s.selection().is_none() && in_pair(s.cursor)).collect();
        self.edit_each(
            |b, i, s| match s.selection() {
                Some(r) => (r, String::new()),
                None if pairs[i] => (s.cursor - 1..s.cursor + 1, String::new()),
                None => (b.prev_boundary(s.cursor)..s.cursor, String::new()),
            },
            false,
        );
    }

    /// Smart Enter: after an opening bracket (or `:` if `colon`) indent one level
    /// deeper; if the cursor is between a pair (`{|}`) the closer drops to its own line.
    pub fn smart_newline(&mut self, unit: &str, colon: bool) {
        let n = self.len_chars();
        let parts: Vec<(Range<usize>, String)> = self
            .sels
            .iter()
            .map(|s| {
                let line = self.rope.char_to_line(s.start());
                let line_text = self.line(line);
                let indent: String = line_text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                // Enter on a whitespace-only line: that whitespace (auto-indent) is deleted,
                // the new line starts with the same indentation (VS Code "trimAutoWhitespace").
                if s.selection().is_none()
                    && !line_text.is_empty()
                    && line_text.trim().is_empty()
                    && s.cursor == self.line_to_char(line) + line_text.chars().count()
                {
                    return (self.line_to_char(line)..s.cursor, format!("\n{indent}"));
                }
                let before: String = self.text_range(self.line_to_char(line)..s.start());
                let last = before.trim_end().chars().last();
                let opens = last.is_some_and(|c| "([{".contains(c)) || (colon && last == Some(':'));
                let next = (s.end() < n).then(|| self.rope.char(s.end()));
                let closes = opens && next.is_some_and(|c| ")]}".contains(c));
                let text = match (opens, closes) {
                    (true, true) => format!("\n{indent}{unit}\n{indent}"),
                    (true, false) => format!("\n{indent}{unit}"),
                    _ => format!("\n{indent}"),
                };
                (s.range(), text)
            })
            .collect();
        let lens: Vec<(usize, usize)> = self
            .sels
            .iter()
            .map(|s| {
                let line = self.rope.char_to_line(s.start());
                let indent = self.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').count();
                (indent, unit.chars().count())
            })
            .collect();
        let texts: Vec<String> = parts.iter().map(|(_, t)| t.clone()).collect();
        let place = |i: usize, start: usize| {
            let t = &texts[i];
            // Between a pair: the cursor is at the end of the middle (indented) line.
            if t.matches('\n').count() == 2 { Sel::at(start + 1 + lens[i].0 + lens[i].1) } else { Sel::at(start + t.chars().count()) }
        };
        self.edit_parts(parts, Place::Custom(&place), false);
    }

    /// New line, keeping the indentation of each cursor's line.
    pub fn newline(&mut self) {
        self.edit_each(
            |b, _, s| {
                let line = b.rope.char_to_line(s.start());
                let indent: String = b.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                (s.range(), format!("\n{indent}"))
            },
            false,
        );
    }

    fn push_undo(&mut self, edit: Edit) {
        let moved = std::mem::take(&mut self.moved_since_edit);
        // Consecutive Backspaces (single cursor): the deleted text is prepended.
        if !moved
            && edit.deleting
            && let Some(last) = self.undo.last_mut()
            && last.deleting
            && let ([l], [e]) = (last.parts.as_mut_slice(), edit.parts.as_slice())
            && l.inserted.is_empty()
            && e.inserted.is_empty()
            && e.at + e.removed.chars().count() == l.at
        {
            l.removed = format!("{}{}", e.removed, l.removed);
            l.at = e.at;
            last.sels_after = edit.sels_after;
            last.primary_after = edit.primary_after;
            last.id = edit.id;
            return;
        }
        if let Some(last) = self.undo.last_mut()
            && !moved
            && edit.typing
            && last.typing
            && last.parts.len() == edit.parts.len()
            && edit.parts.iter().all(|p| p.removed.is_empty() && !p.inserted.starts_with(char::is_whitespace))
        {
            // Was each piece written right after the previous one? (in final document coordinates)
            let mut shift: isize = 0;
            let adjacent = last.parts.iter().zip(&edit.parts).all(|(l, e)| {
                let len = l.inserted.chars().count() as isize;
                let end = l.at as isize + shift + len;
                shift += len - l.removed.chars().count() as isize;
                e.at as isize == end
            });
            if adjacent {
                // Pieces are relative to the document before the previous group; only the text grows.
                for (l, e) in last.parts.iter_mut().zip(&edit.parts) {
                    l.inserted.push_str(&e.inserted);
                }
                last.sels_after = edit.sels_after;
                last.primary_after = edit.primary_after;
                last.id = edit.id; // content changed: new identity
                return;
            }
        }
        self.undo.push(edit);
        self.trim_history();
    }

    /// Limit the undo history: the oldest steps are dropped (at most `MAX_UNDO_STEPS` steps and `MAX_UNDO_BYTES` of text). In a large
    /// file, "replace all" holds file-sized text at every step; without a limit memory would balloon over a session. Not touched
    /// while a composition (`compose`) is in progress: its start index depends on the front of the history.
    fn trim_history(&mut self) {
        if self.compose_start.is_some() {
            return;
        }
        // Step limit: drop in bulk once slightly over, so we don't shift the front on every key.
        if self.undo.len() > MAX_UNDO_STEPS + UNDO_SLACK {
            let drop = self.undo.len() - MAX_UNDO_STEPS;
            self.undo.drain(..drop);
        }
        // Byte limit: measured every 64 steps, not on every key.
        if !self.undo.len().is_multiple_of(64) {
            return;
        }
        let size = |e: &Edit| e.parts.iter().map(|p| p.removed.len() + p.inserted.len()).sum::<usize>();
        let mut keep = 0usize; // number of steps to keep, counted from the end
        let mut bytes = 0usize;
        for e in self.undo.iter().rev() {
            bytes += size(e);
            if keep > 0 && bytes > MAX_UNDO_BYTES {
                break;
            }
            keep += 1;
        }
        if keep < self.undo.len() {
            let drop = self.undo.len() - keep;
            self.undo.drain(..drop);
        }
    }

    /// Link the last undo step to the previous one: both are undone and redone together
    /// (the typed letter + the mirrored letter at its pair in a linked-tag edit).
    pub fn chain_last_undo(&mut self) {
        if self.undo.len() >= 2
            && let Some(last) = self.undo.last_mut()
        {
            last.chain = true;
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(e) = self.undo.pop() else { return false };
        // Front to back: everything before each piece is already restored, so `at` is valid.
        for p in &e.parts {
            self.splice(p.at, p.inserted.chars().count(), &p.removed);
        }
        self.restore_sels(&e.sels_before, e.primary_before);
        let chained = e.chain;
        self.redo.push(e);
        if chained {
            self.undo();
        }
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(e) = self.redo.pop() else { return false };
        self.apply_parts(&e.parts);
        self.restore_sels(&e.sels_after, e.primary_after);
        self.undo.push(e);
        if self.redo.last().is_some_and(|n| n.chain) {
            self.redo();
        }
        true
    }

    fn restore_sels(&mut self, sels: &[Sel], primary: usize) {
        let n = self.len_chars();
        self.sels = sels.iter().map(|s| Sel { cursor: s.cursor.min(n), anchor: s.anchor.map(|a| a.min(n)), goal: None }).collect();
        self.primary = primary.min(self.sels.len() - 1);
        self.normalize();
    }
}

/// 0: whitespace, 1: word (letters, digits, `_`), 2: punctuation.
fn char_class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' || is_combining_mark(c) {
        1
    } else {
        2
    }
}

fn char_to_byte(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map_or(s.len(), |(b, _)| b)
}

mod edit_bytes;
mod lines;
use edit_bytes::unify_breaks;
mod textutil;
use textutil::{has_mixed_endings, is_combining_mark};
#[cfg(test)]
mod tests;
