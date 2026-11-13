//! Collapsing unchanged regions of a diff: which rows hide behind a "N unchanged lines" row, and the list the view
//! draws once some folds are open. Pure, so the row indices staging and F7 use never depend on what is folded.

use std::collections::BTreeSet;
use std::ops::Range;

/// Unchanged lines kept around each change.
pub const CONTEXT: usize = 3;
/// A fold hides at least this many rows; a fold row standing in for one row would not save anything.
const MIN_HIDDEN: usize = 2;

/// One entry of the virtualized list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// A row of the current view (side-by-side row or inline row).
    Row(usize),
    /// `len` unchanged side-by-side rows from `start`, folded.
    Fold { start: usize, len: usize },
}

/// The side-by-side row ranges that can fold: the unchanged rows farther than `context` from every change. A file
/// without changes has none (there is nothing to look at but the text).
pub fn fold_ranges(blocks: &[Range<usize>], rows: usize, context: usize) -> Vec<Range<usize>> {
    let (Some(first), Some(last)) = (blocks.first(), blocks.last()) else { return Vec::new() };
    let mut out = Vec::new();
    let mut fold = |start: usize, end: usize| {
        if end >= start + MIN_HIDDEN {
            out.push(start..end);
        }
    };
    fold(0, first.start.saturating_sub(context));
    for pair in blocks.windows(2) {
        fold(pair[0].end + context, pair[1].start.saturating_sub(context));
    }
    fold(last.end + context, rows);
    out
}

/// The list for a view of `len` rows, where row `ix` shows side-by-side row `src_of(ix)`: folds replace their rows
/// unless their start is in `expanded`. `folds` is sorted, as `fold_ranges` returns it.
pub fn items(len: usize, src_of: impl Fn(usize) -> usize, folds: &[Range<usize>], expanded: &BTreeSet<usize>) -> Vec<Item> {
    let mut out = Vec::with_capacity(len);
    let mut f = 0;
    for ix in 0..len {
        let src = src_of(ix);
        while f < folds.len() && folds[f].end <= src {
            f += 1;
        }
        match folds.get(f) {
            Some(r) if r.contains(&src) && !expanded.contains(&r.start) => {
                if src == r.start {
                    out.push(Item::Fold { start: r.start, len: r.len() });
                }
            }
            _ => out.push(Item::Row(ix)),
        }
    }
    out
}

/// How far up a scope is looked for.
const SCOPE_LOOKBACK: usize = 2000;

/// The line that opens the scope line `at` sits in (`fn area(&self) {`): the nearest line above it that is indented less
/// and reads like code, not a comment or a closing bracket. Shown on a fold so the change below it has its context.
pub fn scope_of(lines: &[String], at: usize) -> Option<&str> {
    let indent = |l: &str| (!l.trim().is_empty()).then(|| l.len() - l.trim_start().len());
    // A blank line counts as indented like the code after it.
    let depth = lines.get(at..)?.iter().take(50).find_map(|l| indent(l))?;
    lines[..at]
        .iter()
        .rev()
        .take(SCOPE_LOOKBACK)
        .map(String::as_str)
        .find(|l| {
            let t = l.trim_start();
            indent(l).is_some_and(|i| i < depth) && t.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == '$') && !t.starts_with("else")
        })
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_folds_without_changes() {
        assert!(fold_ranges(&[], 100, CONTEXT).is_empty());
    }

    #[test]
    fn three_rows_of_context_stay_around_each_change() {
        // 30 rows, changes at 10..12 and 20..21.
        let folds = fold_ranges(&[10..12, 20..21], 30, 3);
        // Above: 0..7 (rows 7, 8, 9 stay). Between: 15..17 (12, 13, 14 and 17, 18, 19 stay). Below: 24..30.
        assert_eq!(folds, vec![0..7, 15..17, 24..30]);
    }

    #[test]
    fn a_gap_too_small_to_save_a_row_stays_open() {
        // Change at 5..6, 6 rows apart from the next: 3 + 3 context leaves nothing between them.
        assert!(fold_ranges(&[5..6, 12..13], 18, 3).iter().all(|r| r.start != 9));
        assert_eq!(fold_ranges(&[5..6, 12..13], 18, 3), vec![0..2, 16..18]);
        // A single hidden row is not folded.
        assert_eq!(fold_ranges(&[4..5], 9, 3), Vec::<Range<usize>>::new());
    }

    #[test]
    fn a_change_at_the_edges_keeps_its_context_inside_the_file() {
        assert_eq!(fold_ranges(&[0..1], 20, 3), vec![4..20]);
        assert_eq!(fold_ranges(&[19..20], 20, 3), vec![0..16]);
    }

    #[test]
    fn the_list_replaces_folded_rows_and_keeps_expanded_ones() {
        let folds = vec![0..7, 15..17];
        let side = items(20, |ix| ix, &folds, &BTreeSet::new());
        assert_eq!(side[0], Item::Fold { start: 0, len: 7 });
        assert_eq!(side[1], Item::Row(7));
        assert_eq!(side.len(), 1 + 8 + 1 + 3);
        assert!(side.contains(&Item::Fold { start: 15, len: 2 }));
        let open = items(20, |ix| ix, &folds, &BTreeSet::from([0]));
        assert_eq!(open.len(), 20 - 2 + 1);
        assert_eq!(open[0], Item::Row(0));
        assert!(!open.contains(&Item::Fold { start: 0, len: 7 }));
    }

    #[test]
    fn a_fold_names_the_scope_the_next_change_sits_in() {
        let file: Vec<String> = [
            "use std::fmt;",        // 0
            "",                     // 1
            "impl Shape {",         // 2
            "    // Area in m².",   // 3
            "    fn area(&self) {", // 4
            "        let w = 1;",   // 5
            "",                     // 6
            "        let h = 2;",   // 7
            "    }",                // 8
            "}",                    // 9
            "fn main() {}",         // 10
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(scope_of(&file, 7), Some("fn area(&self) {"), "comments and blank lines are not scopes");
        assert_eq!(scope_of(&file, 6), Some("fn area(&self) {"), "a blank line takes the indent of the code after it");
        assert_eq!(scope_of(&file, 4), Some("impl Shape {"));
        assert_eq!(scope_of(&file, 8), Some("impl Shape {"), "a closing brace belongs to the scope it closes");
        assert_eq!(scope_of(&file, 10), None, "nothing encloses top-level code");
        assert_eq!(scope_of(&file, 99), None);
    }

    #[test]
    fn inline_rows_fold_by_the_side_by_side_row_they_show() {
        // Inline view: rows 0..4 show side rows 0..4, then a block shows side row 4 twice (removed, then added).
        let src = [0, 1, 2, 3, 4, 4, 5, 6];
        let list = items(src.len(), |ix| src[ix], &[0..3], &BTreeSet::new());
        assert_eq!(list[0], Item::Fold { start: 0, len: 3 });
        assert_eq!(list[1], Item::Row(3));
        assert_eq!(list.len(), 1 + 5);
    }
}
