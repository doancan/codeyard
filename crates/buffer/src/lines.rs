//! Line operations: toggle comment, move, duplicate (part of `impl Buffer`; accesses the private fields of `lib.rs`).

use std::ops::Range;

use super::*;

impl Buffer {
    /// Position mapping by line pieces (cursors after inserting/deleting at line starts).
    pub(super) fn mapper(parts: &[(Range<usize>, String)]) -> impl Fn(usize) -> usize + '_ {
        move |p: usize| {
            let mut out = p as isize;
            for (r, t) in parts {
                // Counted the way `edit_parts` inserts it: a `\r\n` in the text becomes one character.
                let len = if t.contains('\r') { unify_breaks(t).chars().count() } else { t.chars().count() };
                let ins = len as isize;
                if p >= r.end {
                    out += ins - r.len() as isize;
                } else if p > r.start {
                    out += (r.start as isize + (p - r.start).min(len) as isize) - p as isize;
                    break;
                }
            }
            out.max(0) as usize
        }
    }

    /// ⌘/: toggle line comment. Removes it if all non-empty lines in the block are commented,
    /// otherwise inserts `token ` at the least-indented column.
    pub fn toggle_comment(&mut self, token: &str) {
        self.comment_lines(token, None);
    }

    /// Add a line comment (`Some(true)`, also on already-commented lines: ⌘K ⌘C) or remove it
    /// (`Some(false)`: leaves uncommented lines alone, ⌘K ⌘U).
    pub fn comment_lines(&mut self, token: &str, force: Option<bool>) {
        let mut parts: Vec<(Range<usize>, String)> = Vec::new();
        for (a, b) in self.line_blocks() {
            let lines: Vec<(usize, String)> = (a..=b).map(|l| (l, self.line(l))).filter(|(_, t)| !t.trim().is_empty()).collect();
            if lines.is_empty() {
                continue;
            }
            let all = lines.iter().all(|(_, t)| t.trim_start().starts_with(token));
            let remove = force.map_or(all, |add| !add);
            let col = lines.iter().map(|(_, t)| t.chars().take_while(|c| c.is_whitespace()).count()).min().unwrap_or(0);
            for (l, t) in lines {
                let start = self.line_to_char(l);
                if remove {
                    if !t.trim_start().starts_with(token) {
                        continue;
                    }
                    let lead = t.chars().take_while(|c| c.is_whitespace()).count();
                    let after = &t.chars().skip(lead + token.chars().count()).collect::<String>();
                    let n = token.chars().count() + usize::from(after.starts_with(' '));
                    parts.push((start + lead..start + lead + n, String::new()));
                } else {
                    parts.push((start + col..start + col, format!("{token} ")));
                }
            }
        }
        let keep = Self::mapper(&parts);
        self.edit_parts(parts.clone(), Place::Keep(&keep), false);
    }

    /// In block-comment languages (CSS `/* */`, HTML `<!-- -->`), wrap or unwrap each line.
    pub fn toggle_block_comment_lines(&mut self, open: &str, close: &str) {
        let mut parts: Vec<(Range<usize>, String)> = Vec::new();
        for (a, b) in self.line_blocks() {
            let lines: Vec<(usize, String)> = (a..=b).map(|l| (l, self.line(l))).filter(|(_, t)| !t.trim().is_empty()).collect();
            // A line where the opening and closing markers overlap, such as `/*/` or `<!-->`, does not count as wrapped (the inner part would be negative).
            let wrapped = |t: &str| {
                let t = t.trim();
                t.starts_with(open) && t.ends_with(close) && t.len() >= open.len() + close.len()
            };
            let all = !lines.is_empty() && lines.iter().all(|(_, t)| wrapped(t));
            for (l, t) in lines {
                let start = self.line_to_char(l);
                let lead = t.chars().take_while(|c| c.is_whitespace()).count();
                let body: String = t.chars().skip(lead).collect();
                let body = body.trim_end();
                let new = if all {
                    let inner = &body[open.len()..body.len() - close.len()];
                    let inner = inner.strip_prefix(' ').unwrap_or(inner);
                    inner.strip_suffix(' ').unwrap_or(inner).to_string()
                } else {
                    format!("{open} {body} {close}")
                };
                let end = start + t.chars().count();
                parts.push((start + lead..end, new));
            }
        }
        let keep = Self::mapper(&parts);
        self.edit_parts(parts.clone(), Place::Keep(&keep), false);
    }

    /// Indent the selected lines by one level.
    pub fn indent_lines(&mut self, unit: &str) {
        let mut parts = Vec::new();
        for (a, b) in self.line_blocks() {
            for l in a..=b {
                if !self.line(l).is_empty() {
                    let at = self.line_to_char(l);
                    parts.push((at..at, unit.to_string()));
                }
            }
        }
        let keep = Self::mapper(&parts);
        self.edit_parts(parts.clone(), Place::Keep(&keep), false);
    }

    /// ⇧Tab: outdent the selected (or cursor) lines by one level.
    pub fn outdent_lines(&mut self, unit_cols: usize) {
        let mut parts = Vec::new();
        for (a, b) in self.line_blocks() {
            for l in a..=b {
                let t = self.line(l);
                let n = if t.starts_with('\t') { 1 } else { t.chars().take(unit_cols.max(1)).take_while(|c| *c == ' ').count() };
                if n > 0 {
                    let at = self.line_to_char(l);
                    parts.push((at..at + n, String::new()));
                }
            }
        }
        let keep = Self::mapper(&parts);
        self.edit_parts(parts.clone(), Place::Keep(&keep), false);
    }

    /// Text of the block (joined without line endings, no trailing line ending).
    fn block_text(&self, a: usize, b: usize) -> String {
        (a..=b).map(|l| self.line(l)).collect::<Vec<_>>().join("\n")
    }

    /// ⌥↑/↓: move the line(s) up/down by one line; cursors travel with the lines.
    pub fn move_lines(&mut self, up: bool) {
        let blocks = self.line_blocks();
        let last_line = self.len_lines() - 1;
        // In a file with a trailing line ending ("a\nb\n"), the last "line" is empty and not a real line: swapping with it
        // would add an empty line and swallow the end-of-file line ending (`"a\n\nb"`). No line moves onto it, and it does not move.
        let phantom_last = last_line > 0 && self.line_len_chars(last_line) == 0;
        let usable_last = if phantom_last { last_line - 1 } else { last_line };
        if blocks.iter().any(|&(a, b)| (up && a == 0) || (!up && b >= usable_last) || (phantom_last && b > usable_last)) {
            return;
        }
        let mut parts = Vec::new();
        // (block start, block end, opposite line length + 1, block length + 1)
        let mut moves: Vec<(usize, usize, isize, isize)> = Vec::new();
        for &(a, b) in &blocks {
            let (other, block) = if up { (a - 1, self.block_text(a, b)) } else { (b + 1, self.block_text(a, b)) };
            let other_text = self.line(other);
            let (lo, hi) = if up { (other, b) } else { (a, other) };
            let start = self.line_to_char(lo);
            let end = self.line_to_char(hi) + self.line_len_chars(hi);
            let text = if up { format!("{block}\n{other_text}") } else { format!("{other_text}\n{block}") };
            parts.push((start..end, text));
            let (bs, be) = (self.line_to_char(a), self.line_to_char(b) + self.line_len_chars(b));
            moves.push((bs, be, other_text.chars().count() as isize + 1, block.chars().count() as isize + 1));
        }
        let other_start = |i: usize| if up { self.line_to_char(blocks[i].0 - 1) } else { self.line_to_char(blocks[i].1 + 1) };
        let others: Vec<usize> = (0..blocks.len()).map(other_start).collect();
        let map = |p: usize| -> usize {
            for (i, &(bs, be, other_len, block_len)) in moves.iter().enumerate() {
                if (bs..=be).contains(&p) {
                    return (p as isize + if up { -other_len } else { other_len }) as usize;
                }
                let os = others[i];
                let oe = os + other_len as usize - 1;
                if (os..=oe).contains(&p) {
                    return (p as isize + if up { block_len } else { -block_len }) as usize;
                }
            }
            p
        };
        self.edit_parts(parts, Place::Keep(&map), false);
    }

    /// ⇧⌥↑/↓: duplicate the line(s); the cursor lands on the lower (↓) or upper (↑) copy.
    pub fn copy_lines(&mut self, down: bool) {
        let blocks = self.line_blocks();
        let mut parts = Vec::new();
        let mut shifts: Vec<(usize, usize, usize)> = Vec::new();
        for &(a, b) in &blocks {
            let block = self.block_text(a, b);
            let at = self.line_to_char(a);
            parts.push((at..at, format!("{block}\n")));
            shifts.push((at, self.line_to_char(b) + self.line_len_chars(b), block.chars().count() + 1));
        }
        // The copy is inserted at the start of the block: what follows shifts; on ↓ the cursor
        // moves to the lower (original) copy, on ↑ it stays on the upper new copy.
        let map = |p: usize| -> usize { p + shifts.iter().map(|&(start, end, len)| if p > end || (p >= start && down) { len } else { 0 }).sum::<usize>() };
        self.edit_parts(parts, Place::Keep(&map), false);
    }
}
