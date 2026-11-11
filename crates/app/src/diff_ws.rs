//! Diffing while ignoring whitespace: lines are matched on their text without trailing (or any) whitespace, and the
//! rows still show each side's own text.

use crate::git::{self, DiffRow};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Whitespace {
    /// Every difference counts.
    #[default]
    Exact,
    /// Whitespace at the end of a line (and a missing final newline) does not count.
    Trailing,
    /// Whitespace anywhere in a line does not count.
    All,
}

impl Whitespace {
    pub fn next(self) -> Self {
        match self {
            Whitespace::Exact => Whitespace::Trailing,
            Whitespace::Trailing => Whitespace::All,
            Whitespace::All => Whitespace::Exact,
        }
    }

    pub fn ignores(self) -> bool {
        self != Whitespace::Exact
    }

    fn key(self, line: &str) -> String {
        match self {
            Whitespace::Exact => line.to_string(),
            Whitespace::Trailing => line.trim_end().to_string(),
            Whitespace::All => line.split_whitespace().collect(),
        }
    }
}

fn lines(text: &str) -> Vec<&str> {
    let mut v: Vec<&str> = text.split('\n').collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

/// The side-by-side rows of `old` against `new`, matching lines the way `mode` says.
pub fn side_by_side(old: &str, new: &str, mode: Whitespace) -> Vec<DiffRow> {
    if !mode.ignores() {
        return git::side_by_side(old, new);
    }
    let keys = |text: &str| lines(text).into_iter().map(|l| mode.key(l)).collect::<Vec<_>>();
    let (a, b) = (keys(old), keys(new));
    let (a, b): (Vec<&str>, Vec<&str>) = (a.iter().map(String::as_str).collect(), b.iter().map(String::as_str).collect());
    let diff = similar::TextDiff::configure().algorithm(similar::Algorithm::Myers).timeout(std::time::Duration::from_millis(200)).diff_slices(&a, &b);
    git::rows_from_ops(diff.ops().iter().map(|op| op.as_tag_tuple()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_mode_is_the_plain_diff() {
        let (old, new) = ("a\nb \nc\n", "a\nb\nc\n");
        assert_eq!(side_by_side(old, new, Whitespace::Exact), git::side_by_side(old, new));
        assert!(side_by_side(old, new, Whitespace::Exact).iter().any(|r| matches!(r, DiffRow::Changed(..))));
    }

    #[test]
    fn trailing_whitespace_and_line_endings_do_not_count() {
        let rows = side_by_side("a\nb  \nc\r\n", "a\nb\nc\n", Whitespace::Trailing);
        assert!(rows.iter().all(|r| matches!(r, DiffRow::Same(..))), "{rows:?}");
        // Leading whitespace still does.
        let rows = side_by_side("a\nb\n", "a\n  b\n", Whitespace::Trailing);
        assert_eq!(rows, vec![DiffRow::Same(0, 0), DiffRow::Changed(Some(1), Some(1))]);
    }

    #[test]
    fn all_mode_ignores_indentation_and_inner_spacing_but_not_text() {
        let rows = side_by_side("fn  f() {\n    x\n}\n", "fn f() {\n\tx\n}\n", Whitespace::All);
        assert!(rows.iter().all(|r| matches!(r, DiffRow::Same(..))));
        let rows = side_by_side("a b\nc\n", "ab\nd\n", Whitespace::All);
        assert_eq!(rows, vec![DiffRow::Same(0, 0), DiffRow::Changed(Some(1), Some(1))]);
    }

    #[test]
    fn rows_keep_pointing_at_each_sides_own_lines() {
        let rows = side_by_side("x\n  y\nz\n", "w\nx\ny\nz\n", Whitespace::All);
        assert_eq!(rows, vec![DiffRow::Changed(None, Some(0)), DiffRow::Same(0, 1), DiffRow::Same(1, 2), DiffRow::Same(2, 3)]);
    }

    #[test]
    fn the_modes_cycle() {
        assert_eq!(Whitespace::Exact.next().next().next(), Whitespace::Exact);
        assert!(!Whitespace::Exact.ignores() && Whitespace::All.ignores());
    }
}
