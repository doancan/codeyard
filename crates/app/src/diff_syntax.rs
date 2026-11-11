//! Syntax colors for the diff: each side's whole text is parsed once (tree-sitter, `crates/syntax`), and every line
//! keeps its color runs in the tab-expanded text the diff draws. Unknown languages and large texts get no colors.

use std::ops::Range;

use buffer::Rope;
use syntax::{Role, Syntax};

/// Larger texts are drawn plain: a parse of this size is still instant, a few megabytes would be felt.
pub const MAX_BYTES: usize = 512 * 1024;
/// Spaces a tab is drawn as (`diff::expand_tabs`).
const TAB_WIDTH: usize = 4;

/// A line's colors: `(bytes of the drawn line, role)` pieces. Empty when the whole line is plain.
pub type Runs = Vec<(usize, Option<Role>)>;

/// A piece of a drawn line with one color and one answer to "is it a changed word".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub range: Range<usize>,
    pub role: Option<Role>,
    pub marked: bool,
}

/// The color runs of each line of `text` (split on `\n`), or None when `path` has no known language or `text` is too
/// large to color.
pub fn highlight(path: &str, text: &str) -> Option<Vec<Runs>> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let lang = syntax::language_for_path(path)?;
    let rope = Rope::from_str(text);
    let syntax = Syntax::new(lang, &rope)?;
    let lines: Vec<&str> = text.split('\n').collect();
    let starts: Vec<usize> = lines
        .iter()
        .scan(0, |at, l| {
            let start = *at;
            *at += l.len() + 1;
            Some(start)
        })
        .collect();
    // Each span goes to the lines it covers, in the order `highlights` gives them (wide to narrow).
    let mut per_line: Vec<Vec<(Range<usize>, Role)>> = vec![Vec::new(); lines.len()];
    for (range, role) in syntax.highlights(&rope, 0..text.len()) {
        if range.is_empty() {
            continue;
        }
        let first = starts.partition_point(|&s| s <= range.start).saturating_sub(1);
        let last = starts.partition_point(|&s| s < range.end).saturating_sub(1);
        for spans in &mut per_line[first..=last.max(first).min(lines.len() - 1)] {
            spans.push((range.clone(), role));
        }
    }
    Some(lines.iter().zip(&starts).zip(&per_line).map(|((line, &start), spans)| expand(line, &syntax::line_runs(start, line.len(), spans))).collect())
}

/// `runs` of `line` as pieces of the line with its tabs drawn as spaces; a plain line gives none.
fn expand(line: &str, runs: &[(usize, Option<Role>)]) -> Runs {
    let mut out: Runs = Vec::new();
    let mut chars = line.chars();
    for &(len, role) in runs {
        let (mut taken, mut width) = (0, 0);
        while taken < len {
            let Some(ch) = chars.next() else { break };
            taken += ch.len_utf8();
            width += if ch == '\t' { TAB_WIDTH } else { ch.len_utf8() };
        }
        match out.last_mut() {
            Some((w, last)) if *last == role => *w += width,
            _ => out.push((width, role)),
        }
    }
    if out.iter().all(|(_, role)| role.is_none()) { Vec::new() } else { out }
}

/// A drawn line of `len` bytes cut where its color or its changed-word state changes. `runs` may be empty (plain).
pub fn segments(len: usize, runs: &[(usize, Option<Role>)], marks: &[Range<usize>]) -> Vec<Segment> {
    let mut cuts: Vec<usize> = vec![0, len];
    let mut at = 0;
    let mut run_ends = Vec::with_capacity(runs.len());
    for (width, _) in runs {
        at += width;
        run_ends.push(at);
        cuts.push(at.min(len));
    }
    for m in marks {
        cuts.extend([m.start.min(len), m.end.min(len)]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|w| {
            let run = run_ends.partition_point(|&end| end <= w[0]);
            Segment { range: w[0]..w[1], role: runs.get(run).and_then(|(_, role)| *role), marked: marks.iter().any(|m| m.start <= w[0] && w[0] < m.end) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role_at(lines: &[Runs], line: usize, byte: usize) -> Option<Role> {
        let mut at = 0;
        for (width, role) in &lines[line] {
            if byte < at + width {
                return *role;
            }
            at += width;
        }
        None
    }

    #[test]
    fn rust_keywords_and_strings_get_roles() {
        let lines = highlight("a.rs", "fn main() {\n    let s = \"hi\";\n}\n").unwrap();
        assert_eq!(role_at(&lines, 0, 0), Some(Role::Keyword));
        assert_eq!(role_at(&lines, 1, 4), Some(Role::Keyword));
        assert_eq!(role_at(&lines, 1, 13), Some(Role::String));
        assert_eq!(role_at(&lines, 1, 8), None, "the identifier stays plain");
    }

    #[test]
    fn a_multiline_span_colors_every_line_it_covers() {
        let lines = highlight("a.rs", "/* one\n   two */\nfn f() {}\n").unwrap();
        assert_eq!(role_at(&lines, 0, 3), Some(Role::Comment));
        assert_eq!(role_at(&lines, 1, 5), Some(Role::Comment));
        assert_eq!(role_at(&lines, 2, 0), Some(Role::Keyword));
    }

    #[test]
    fn tabs_widen_the_runs_to_the_drawn_text() {
        let lines = highlight("a.rs", "\tlet x = 1;\n").unwrap();
        // The tab draws as four spaces, so `let` starts at byte 4.
        assert_eq!(role_at(&lines, 0, 4), Some(Role::Keyword));
        assert_eq!(role_at(&lines, 0, 0), None);
        let total: usize = lines[0].iter().map(|(w, _)| w).sum();
        assert_eq!(total, "    let x = 1;".len());
    }

    #[test]
    fn unknown_languages_and_large_texts_are_left_plain() {
        assert!(highlight("notes.unknownext", "fn main() {}").is_none());
        assert!(highlight("a.rs", &"let x = 1;\n".repeat(MAX_BYTES / 10)).is_none());
    }

    #[test]
    fn a_line_is_cut_where_color_or_changed_words_change() {
        let runs = vec![(2, Some(Role::Keyword)), (1, None), (3, Some(Role::String))];
        let segs = segments(6, &runs, &[1..4]);
        assert_eq!(
            segs,
            vec![
                Segment { range: 0..1, role: Some(Role::Keyword), marked: false },
                Segment { range: 1..2, role: Some(Role::Keyword), marked: true },
                Segment { range: 2..3, role: None, marked: true },
                Segment { range: 3..4, role: Some(Role::String), marked: true },
                Segment { range: 4..6, role: Some(Role::String), marked: false },
            ]
        );
        // Plain line, no marks: one piece. Marks alone cut a plain line too.
        assert_eq!(segments(4, &[], &[]), vec![Segment { range: 0..4, role: None, marked: false }]);
        assert_eq!(segments(4, &[], &[2..9]).len(), 2);
        assert!(segments(0, &[], &[]).is_empty());
    }
}
