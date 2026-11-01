//! Buffer lines <-> screen lines: line wrapping (a long line takes several screen
//! lines) and folding (hidden lines). With both off it's one to one: no table is kept
//! (not even 4 bytes per line wasted on a large file).

use std::ops::Range;

use buffer::Buffer;

/// Start offsets of a line's screen segments (characters within the line). Always starts with 0.
/// Splits after whitespace when possible; at the column limit if a single word doesn't fit.
pub fn wrap_points(text: &str, cols: usize, tab: usize) -> Vec<usize> {
    let mut points = vec![0];
    if cols == 0 {
        return points;
    }
    let chars: Vec<char> = text.chars().collect();
    let width = |c: char| if c == '\t' { tab.max(1) } else { 1 };
    let (mut start, mut col, mut last_break) = (0, 0, None);
    let mut i = 0;
    while i < chars.len() {
        let w = width(chars[i]);
        if col + w > cols && i > start {
            let at = match last_break {
                Some(b) if b > start => b,
                _ => i,
            };
            points.push(at);
            start = at;
            last_break = None;
            col = chars[at..i].iter().map(|&c| width(c)).sum();
            continue;
        }
        col += w;
        if chars[i] == ' ' || chars[i] == '\t' {
            last_break = Some(i + 1);
        }
        i += 1;
    }
    points
}

/// Segment range of line `line` (characters within the line), `points` = `wrap_points`.
pub fn segment(points: &[usize], seg: usize, line_len: usize) -> Range<usize> {
    let start = points.get(seg).copied().unwrap_or(line_len);
    let end = points.get(seg + 1).copied().unwrap_or(line_len);
    start..end
}

/// Segment a column within the line falls in (a position at a segment end is the next one's start).
pub fn segment_of(points: &[usize], col: usize) -> usize {
    points.partition_point(|&p| p <= col).saturating_sub(1)
}

pub struct DisplayMap {
    /// First screen line of each line (length = line count + 1); a hidden line takes 0 lines.
    /// `None`: one to one.
    row_start: Option<Vec<u32>>,
    lines: usize,
}

impl DisplayMap {
    pub fn identity(lines: usize) -> Self {
        DisplayMap { row_start: None, lines }
    }

    /// `hidden`: sorted, non-overlapping hidden line ranges.
    pub fn build(buffer: &Buffer, cols: Option<usize>, tab: usize, hidden: &[Range<usize>]) -> Self {
        let n = buffer.len_lines();
        if cols.is_none() && hidden.is_empty() {
            return Self::identity(n);
        }
        let mut starts = Vec::with_capacity(n + 1);
        let mut row = 0u32;
        let mut h = hidden.iter().peekable();
        // One pass over the lines: looking each line up anew (`line_len_chars`) costs a descent of the rope per line,
        // which on a 50 000-line file with wrapping on was most of a keystroke.
        for (line, text) in buffer.rope().lines().take(n).enumerate() {
            starts.push(row);
            while h.peek().is_some_and(|r| r.end <= line) {
                h.next();
            }
            if h.peek().is_some_and(|r| r.contains(&line)) {
                continue;
            }
            row += match cols {
                // Tab is the widest character: a line this short surely fits (without reading the text).
                Some(c) if text.len_chars().saturating_sub(1) * tab.max(1) > c => {
                    let len = text.len_chars();
                    let len = if len > 0 && text.char(len - 1) == '\n' { len - 1 } else { len };
                    if len * tab.max(1) > c { wrap_points(&text.slice(..len).to_string(), c, tab).len() as u32 } else { 1 }
                }
                _ => 1,
            };
        }
        starts.push(row);
        DisplayMap { row_start: Some(starts), lines: n }
    }

    pub fn is_identity(&self) -> bool {
        self.row_start.is_none()
    }

    pub fn row_count(&self) -> usize {
        self.row_start.as_ref().map_or(self.lines, |s| *s.last().unwrap_or(&0) as usize)
    }

    pub fn first_row(&self, line: usize) -> usize {
        match &self.row_start {
            None => line,
            Some(s) => s.get(line).copied().unwrap_or(*s.last().unwrap_or(&0)) as usize,
        }
    }

    /// Screen lines a line takes (0 if hidden).
    pub fn row_span(&self, line: usize) -> usize {
        match &self.row_start {
            None => 1,
            Some(s) => match (s.get(line), s.get(line + 1)) {
                (Some(&a), Some(&b)) => (b - a) as usize,
                _ => 0,
            },
        }
    }

    /// Screen line -> (line, segment). Hidden lines are skipped.
    pub fn line_at_row(&self, row: usize) -> (usize, usize) {
        match &self.row_start {
            None => (row.min(self.lines.saturating_sub(1)), 0),
            Some(s) => {
                let row = row.min(self.row_count().saturating_sub(1)) as u32;
                // Latest starting line: hidden ones start at the same place as the visible one before them.
                let line = s[..self.lines].partition_point(|&r| r <= row).saturating_sub(1);
                (line, (row - s[line]) as usize)
            }
        }
    }
}

/// Indent width of a line; `None` for an empty (whitespace-only) line.
fn indent_of(text: &str, tab: usize) -> Option<usize> {
    let mut w = 0;
    for c in text.chars() {
        match c {
            ' ' => w += 1,
            '\t' => w += tab.max(1),
            _ => return Some(w),
        }
    }
    None
}

/// Region foldable by indentation: the last of the lines indented deeper after `line`
/// (blank lines in between included, trailing ones excluded). The closing line (`}`,
/// at the same indent) stays visible. `None` if it can't be folded.
/// (Without server ranges; the editor calls `fold_end_with`. Tests use this as the reference for the single-pass version.)
#[cfg(test)]
pub fn fold_end(buffer: &Buffer, line: usize, tab: usize) -> Option<usize> {
    fold_end_with(buffer, line, tab, &[])
}

/// If the language server's folding range (`start`, `end`; sorted) makes this line a header, use that, else indentation.
fn lsp_end(lsp: &[(usize, usize)], buffer: &Buffer, line: usize) -> Option<usize> {
    let i = lsp.partition_point(|&(s, _)| s < line);
    let last = buffer.len_lines().saturating_sub(1);
    lsp.get(i..)?.iter().take_while(|&&(s, _)| s == line).map(|&(_, e)| e.min(last)).filter(|&e| e > line).max()
}

pub fn fold_end_with(buffer: &Buffer, line: usize, tab: usize, lsp: &[(usize, usize)]) -> Option<usize> {
    if let Some(e) = lsp_end(lsp, buffer, line) {
        return Some(e);
    }
    let base = indent_of(&buffer.line(line), tab)?;
    let mut last = None;
    for l in line + 1..buffer.len_lines() {
        match indent_of(&buffer.line(l), tab) {
            None => continue,
            Some(i) if i > base => last = Some(l),
            Some(_) => break,
        }
    }
    last
}

/// Cheap check for the gutter arrow: is the next non-blank line indented deeper.
/// (Test-only counterpart of `is_foldable_with` without server ranges.)
#[cfg(test)]
pub fn is_foldable(buffer: &Buffer, line: usize, tab: usize) -> bool {
    is_foldable_with(buffer, line, tab, &[])
}

pub fn is_foldable_with(buffer: &Buffer, line: usize, tab: usize, lsp: &[(usize, usize)]) -> bool {
    if lsp_end(lsp, buffer, line).is_some() {
        return true;
    }
    let Some(base) = indent_of(&buffer.line(line), tab) else { return false };
    (line + 1..buffer.len_lines()).find_map(|l| indent_of(&buffer.line(l), tab)).is_some_and(|i| i > base)
}

/// Indent of a line (without splitting; `None` if blank).
pub fn line_indent_of(buffer: &Buffer, line: usize, tab: usize) -> Option<usize> {
    line_indent(buffer, line, tab)
}

fn line_indent(buffer: &Buffer, line: usize, tab: usize) -> Option<usize> {
    chars_indent(buffer.rope().line(line).chars(), tab)
}

/// Indentation width of a line's text; `None` for a blank line.
fn chars_indent(chars: impl Iterator<Item = char>, tab: usize) -> Option<usize> {
    let mut w = 0;
    for c in chars {
        match c {
            ' ' => w += 1,
            '\t' => w += tab.max(1),
            '\n' | '\r' => return None,
            _ => return Some(w),
        }
    }
    None
}

/// Fold ends of all headers in a single pass (with a stack): for each header the last
/// non-blank line indented deeper than it. `headers` sorted.
/// (Test-only counterpart of `fold_ends_with` without server ranges.)
#[cfg(test)]
pub fn fold_ends(buffer: &Buffer, headers: &[usize], tab: usize) -> Vec<Option<usize>> {
    fold_ends_with(buffer, headers, tab, &[])
}

pub fn fold_ends_with(buffer: &Buffer, headers: &[usize], tab: usize, lsp: &[(usize, usize)]) -> Vec<Option<usize>> {
    let mut ends = fold_ends_indent(buffer, headers, tab);
    if !lsp.is_empty() {
        for (e, &h) in ends.iter_mut().zip(headers) {
            if let Some(l) = lsp_end(lsp, buffer, h) {
                *e = Some(l);
            }
        }
    }
    ends
}

fn fold_ends_indent(buffer: &Buffer, headers: &[usize], tab: usize) -> Vec<Option<usize>> {
    let mut ends = vec![None; headers.len()];
    let Some(&first) = headers.first() else { return ends };
    // Stack: (header index, base indent).
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut next = 0;
    // One pass over the lines with the rope's line iterator: looking each line up again is a tree walk per line.
    let lines = buffer.rope().lines_at(first).take(buffer.len_lines().saturating_sub(first));
    for (line, text) in (first..).zip(lines) {
        let Some(indent) = chars_indent(text.chars(), tab) else { continue };
        while stack.last().is_some_and(|&(_, base)| base >= indent) {
            stack.pop();
        }
        for &(h, _) in &stack {
            ends[h] = Some(line);
        }
        while next < headers.len() && headers[next] < line {
            next += 1; // blank-line headers can't be folded
        }
        if next < headers.len() && headers[next] == line {
            stack.push((next, indent));
            next += 1;
        }
        if stack.is_empty() && next >= headers.len() {
            break;
        }
    }
    ends
}

/// Hidden ranges from header lines: sorted, nested ones merged into the outer one.
/// (Test-only counterpart of `hidden_ranges_with` without server ranges.)
#[cfg(test)]
pub fn hidden_ranges(buffer: &Buffer, headers: &[usize], tab: usize) -> Vec<Range<usize>> {
    hidden_ranges_with(buffer, headers, tab, &[])
}

pub fn hidden_ranges_with(buffer: &Buffer, headers: &[usize], tab: usize, lsp: &[(usize, usize)]) -> Vec<Range<usize>> {
    let mut sorted = headers.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let ends = fold_ends_with(buffer, &sorted, tab, lsp);
    let mut ranges: Vec<Range<usize>> = sorted.iter().zip(ends).filter_map(|(&h, end)| end.map(|end| h + 1..end + 1)).collect();
    ranges.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    let mut out: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_spaces_then_hard() {
        assert_eq!(wrap_points("kısa", 10, 4), vec![0]);
        // 6 columns: "bir " | "iki üç" (fits exactly)
        assert_eq!(wrap_points("bir iki üç", 6, 4), vec![0, 4]);
        // "bir " | "iki " | "üç " | "dört"
        assert_eq!(wrap_points("bir iki üç dört", 6, 4), vec![0, 4, 8, 11]);
        // A long word without spaces splits at the column limit.
        assert_eq!(wrap_points("abcdefghij", 4, 4), vec![0, 4, 8]);
        // Tab is 4 columns.
        assert_eq!(wrap_points("\t\tx", 8, 4), vec![0, 2]);
        assert_eq!(segment(&[0, 4, 8], 1, 10), 4..8);
        assert_eq!(segment(&[0, 4, 8], 2, 10), 8..10);
        assert_eq!(segment_of(&[0, 4, 8], 4), 1, "segment end is the start of the next");
        assert_eq!(segment_of(&[0, 4, 8], 3), 0);
    }

    #[test]
    fn folds_by_indentation() {
        let b = Buffer::from_str("fn a() {\n    x;\n\n    y;\n}\n\nfn b() {\n    z;\n}\n");
        assert_eq!(fold_end(&b, 0, 4), Some(3), "blank line is inside, closing line is outside");
        assert_eq!(fold_end(&b, 1, 4), None);
        assert_eq!(fold_end(&b, 6, 4), Some(7));
        assert_eq!(hidden_ranges(&b, &[6, 0, 0], 4), vec![1..4, 7..8]);
        // Single pass, same result as fold_end (nested and with blank lines).
        let n = Buffer::from_str("a\n  b\n    c\n\n    d\n  e\nf\n  g\n");
        let headers: Vec<usize> = (0..n.len_lines()).collect();
        let fast = fold_ends(&n, &headers, 4);
        let slow: Vec<Option<usize>> = headers.iter().map(|&h| fold_end(&n, h, 4)).collect();
        assert_eq!(fast, slow);
        assert!(is_foldable(&b, 0, 4) && !is_foldable(&b, 1, 4) && !is_foldable(&b, 2, 4));
    }

    #[test]
    fn rows_with_wrap_and_folds() {
        let b = Buffer::from_str("fn a() {\n    uzun uzun uzun\n}\nson");
        let m = DisplayMap::build(&b, None, 4, &[1..2]);
        assert_eq!((m.row_count(), m.first_row(2), m.row_span(1)), (3, 1, 0));
        assert_eq!(m.line_at_row(1), (2, 0), "hidden line is skipped");
        let m = DisplayMap::build(&b, Some(10), 4, &[]);
        // "    uzun uzun uzun" (18): "    uzun " | "uzun uzun"
        assert_eq!(m.row_span(1), 2);
        assert_eq!(m.line_at_row(2), (1, 1));
        assert_eq!(m.row_count(), 5);
        assert!(DisplayMap::build(&b, None, 4, &[]).is_identity());
    }
}
