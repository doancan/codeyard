//! What the Outline and the breadcrumbs make of a document's symbols: the rows under a filter and a sort order, the
//! chain of symbols around a line, the siblings of a symbol, and symbols from a language server. Pure data.

use super::{Kind, Symbol};
use crate::fuzzy::fold;

/// Most symbols kept from one document (a server cannot make the outline hold more) and the longest name kept.
pub const MAX_SYMBOLS: usize = 20_000;
const MAX_NAME: usize = 200;

/// The order of the Outline's rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    /// Document order, nested.
    #[default]
    Position,
    /// By name, flat.
    Name,
    /// Grouped by kind, then by name, flat.
    Kind,
}

impl Sort {
    pub fn next(self) -> Self {
        match self {
            Sort::Position => Sort::Name,
            Sort::Name => Sort::Kind,
            Sort::Kind => Sort::Position,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Sort::Position => "position",
            Sort::Name => "name",
            Sort::Kind => "kind",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "name" => Sort::Name,
            "kind" => Sort::Kind,
            _ => Sort::Position,
        }
    }
}

/// One row of the Outline: a symbol (by index), how deep it is drawn, and whether it is only shown as the surroundings of
/// a symbol that passed the filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub ix: usize,
    pub depth: usize,
    pub context: bool,
}

/// The symbol that encloses each symbol: the nearest earlier one that is less deep. One pass, linear in the symbols.
pub fn parents(symbols: &[Symbol]) -> Vec<Option<usize>> {
    let mut stack: Vec<usize> = Vec::new();
    symbols
        .iter()
        .enumerate()
        .map(|(ix, s)| {
            while stack.last().is_some_and(|&top| symbols[top].depth >= s.depth) {
                stack.pop();
            }
            let parent = stack.last().copied();
            stack.push(ix);
            parent
        })
        .collect()
}

/// The rows to draw for `filter` (words that must all be in the name, ignoring case and Turkish i/ı) in `sort` order.
/// In document order the filter keeps the enclosing symbols of a match, as context.
pub fn rows(symbols: &[Symbol], parents: &[Option<usize>], filter: &str, sort: Sort) -> Vec<Row> {
    let words: Vec<String> = filter.split_whitespace().map(|w| w.chars().map(fold).collect()).collect();
    let matches = |s: &Symbol| {
        let name: String = s.name.chars().map(fold).collect();
        words.iter().all(|w| name.contains(w.as_str()))
    };
    let hit: Vec<bool> = symbols.iter().map(matches).collect();
    match sort {
        Sort::Position => {
            let mut show = hit.clone();
            if !words.is_empty() {
                for ix in (0..symbols.len()).filter(|&i| hit[i]) {
                    let mut at = ix;
                    while let Some(parent) = parents[at] {
                        if show[parent] {
                            break;
                        }
                        show[parent] = true;
                        at = parent;
                    }
                }
            }
            (0..symbols.len()).filter(|&i| show[i]).map(|ix| Row { ix, depth: symbols[ix].depth, context: !hit[ix] }).collect()
        }
        Sort::Name | Sort::Kind => {
            let mut ixs: Vec<usize> = (0..symbols.len()).filter(|&i| hit[i]).collect();
            let name = |i: usize| -> String { symbols[i].name.chars().map(fold).collect() };
            if sort == Sort::Name {
                ixs.sort_by_cached_key(|&i| (name(i), symbols[i].line));
            } else {
                ixs.sort_by_cached_key(|&i| (symbols[i].kind, name(i), symbols[i].line));
            }
            ixs.into_iter().map(|ix| Row { ix, depth: 0, context: false }).collect()
        }
    }
}

/// The symbols around `line`, outermost first: the innermost symbol whose range holds the line and the ones that enclose
/// it. A line between two symbols of the same level (or after the last one's end) has none of them.
pub fn path_at(symbols: &[Symbol], line: usize) -> Vec<usize> {
    let mut stack: Vec<usize> = Vec::new();
    for (ix, s) in symbols.iter().enumerate() {
        if s.line > line {
            break;
        }
        while stack.last().is_some_and(|&top| symbols[top].depth >= s.depth || symbols[top].end_line < s.line) {
            stack.pop();
        }
        stack.push(ix);
    }
    while stack.last().is_some_and(|&top| symbols[top].end_line < line) {
        stack.pop();
    }
    stack
}

/// The symbols that share the parent of symbol `ix`, `ix` among them, in document order.
pub fn siblings(symbols: &[Symbol], ix: usize) -> Vec<usize> {
    if ix >= symbols.len() {
        return Vec::new();
    }
    let parents = parents(symbols);
    (0..symbols.len()).filter(|&j| parents[j] == parents[ix]).collect()
}

/// Gives every symbol the line its range ends on when the source knows only where it starts: the line before the next
/// symbol that is not deeper, or the last line.
pub fn fill_ends(symbols: &mut [Symbol], line_count: usize) {
    let last = line_count.saturating_sub(1);
    for ix in 0..symbols.len() {
        let depth = symbols[ix].depth;
        let end = symbols[ix + 1..].iter().find(|s| s.depth <= depth).map_or(last, |s| s.line.saturating_sub(1).max(symbols[ix].line));
        symbols[ix].end_line = end;
    }
}

pub fn kind_from_lsp(kind: u8) -> Kind {
    match kind {
        1..=4 => Kind::Module,
        5 => Kind::Class,
        6 | 9 => Kind::Method,
        7 => Kind::Property,
        8 => Kind::Field,
        10 => Kind::Enum,
        11 => Kind::Interface,
        12 => Kind::Function,
        13 | 15..=21 => Kind::Variable,
        14 | 22 => Kind::Constant,
        23 | 26 => Kind::Type,
        _ => Kind::Other,
    }
}

/// A server's document symbols as outline symbols. A flat answer (SymbolInformation) has no depth: it is taken from which
/// symbol's range holds which.
pub fn from_lsp(symbols: &[crate::lsp::Symbol]) -> Vec<Symbol> {
    let mut out: Vec<Symbol> = symbols
        .iter()
        .take(MAX_SYMBOLS)
        .map(|s| Symbol {
            name: s.name.chars().take(MAX_NAME).collect(),
            kind: kind_from_lsp(s.kind),
            line: s.line as usize,
            col: s.col as usize,
            end_line: (s.end_line as usize).max(s.line as usize),
            depth: s.depth,
        })
        .collect();
    if out.len() > 1 && out.iter().all(|s| s.depth == 0) {
        let mut stack: Vec<usize> = Vec::new();
        for ix in 0..out.len() {
            while stack.last().is_some_and(|&top| out[top].end_line < out[ix].line || (out[top].end_line == out[ix].line && out[top].line == out[ix].line)) {
                stack.pop();
            }
            out[ix].depth = stack.len();
            stack.push(ix);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str, kind: Kind, line: usize, end_line: usize, depth: usize) -> Symbol {
        Symbol { name: name.into(), kind, line, col: 0, end_line, depth }
    }

    /// struct Shape (0-9) { new (1-2), area (4-8) { inner (5-6) } }, then fn helper (11-13), then Const (15-15).
    fn doc() -> Vec<Symbol> {
        vec![
            sym("Shape", Kind::Class, 0, 9, 0),
            sym("new", Kind::Method, 1, 2, 1),
            sym("area", Kind::Method, 4, 8, 1),
            sym("inner", Kind::Function, 5, 6, 2),
            sym("helper", Kind::Function, 11, 13, 0),
            sym("LIMIT", Kind::Constant, 15, 15, 0),
        ]
    }

    fn names(symbols: &[Symbol], ixs: &[usize]) -> Vec<String> {
        ixs.iter().map(|&i| symbols[i].name.clone()).collect()
    }

    #[test]
    fn the_path_is_the_chain_of_enclosing_symbols_and_empty_between_symbols() {
        let d = doc();
        assert_eq!(names(&d, &path_at(&d, 5)), ["Shape", "area", "inner"]);
        assert_eq!(names(&d, &path_at(&d, 7)), ["Shape", "area"]);
        assert_eq!(names(&d, &path_at(&d, 0)), ["Shape"]);
        assert_eq!(names(&d, &path_at(&d, 3)), ["Shape"], "a blank line inside the class is in the class");
        assert!(path_at(&d, 10).is_empty(), "between two top-level symbols");
        assert_eq!(names(&d, &path_at(&d, 12)), ["helper"]);
        assert!(path_at(&d, 20).is_empty(), "after the last symbol's end");
        assert!(path_at(&[], 3).is_empty());
    }

    #[test]
    fn siblings_share_a_parent() {
        let d = doc();
        assert_eq!(names(&d, &siblings(&d, 1)), ["new", "area"]);
        assert_eq!(names(&d, &siblings(&d, 3)), ["inner"]);
        assert_eq!(names(&d, &siblings(&d, 4)), ["Shape", "helper", "LIMIT"]);
        assert!(siblings(&d, 99).is_empty());
    }

    #[test]
    fn the_filter_keeps_the_enclosing_symbols_as_context_in_document_order() {
        let d = doc();
        let found = rows(&d, &parents(&d), "INN", Sort::Position);
        assert_eq!(
            found.iter().map(|r| (d[r.ix].name.as_str(), r.depth, r.context)).collect::<Vec<_>>(),
            [("Shape", 0, true), ("area", 1, true), ("inner", 2, false)]
        );
        assert_eq!(rows(&d, &parents(&d), "", Sort::Position).len(), 6);
        assert!(rows(&d, &parents(&d), "zzz", Sort::Position).is_empty());
        assert_eq!(rows(&d, &parents(&d), "are  sha", Sort::Position).len(), 0, "every word must match one name");
        assert_eq!(
            rows(&d, &parents(&d), "ar", Sort::Position).iter().map(|r| d[r.ix].name.as_str()).collect::<Vec<_>>(),
            ["Shape", "area"],
            "an ancestor is shown once"
        );
    }

    #[test]
    fn name_and_kind_orders_are_flat() {
        let d = doc();
        let by = |sort| rows(&d, &parents(&d), "", sort).into_iter().map(|r| (d[r.ix].name.clone(), r.depth)).collect::<Vec<_>>();
        assert_eq!(by(Sort::Name).into_iter().map(|(n, _)| n).collect::<Vec<_>>(), ["area", "helper", "inner", "LIMIT", "new", "Shape"]);
        let kinds = by(Sort::Kind);
        assert_eq!(kinds.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["Shape", "helper", "inner", "area", "new", "LIMIT"]);
        assert!(kinds.iter().all(|(_, depth)| *depth == 0));
    }

    #[test]
    fn turkish_letters_fold_in_the_filter() {
        let d = vec![sym("İşlem", Kind::Function, 0, 1, 0), sym("ılık", Kind::Function, 2, 3, 0)];
        assert_eq!(rows(&d, &parents(&d), "islem", Sort::Name).len(), 1);
        assert_eq!(rows(&d, &parents(&d), "ILIK", Sort::Name).len(), 1);
    }

    #[test]
    fn ends_come_from_the_next_symbol_that_is_not_deeper() {
        let mut d = vec![sym("A", Kind::Class, 0, 0, 0), sym("m", Kind::Method, 2, 0, 1), sym("n", Kind::Method, 5, 0, 1), sym("B", Kind::Class, 9, 0, 0)];
        fill_ends(&mut d, 20);
        assert_eq!(d.iter().map(|s| s.end_line).collect::<Vec<_>>(), [8, 4, 8, 19]);
        let mut same = vec![sym("a", Kind::Function, 3, 0, 0), sym("b", Kind::Function, 3, 0, 0)];
        fill_ends(&mut same, 4);
        assert_eq!(same[0].end_line, 3, "never before its own line");
    }

    #[test]
    fn server_symbols_become_outline_symbols_with_kinds_depth_and_bounds() {
        let lsp = |name: &str, kind: u8, line: u32, end: u32, depth: usize| crate::lsp::Symbol {
            name: name.into(),
            detail: String::new(),
            kind,
            line,
            col: 4,
            end_line: end,
            depth,
        };
        let nested = from_lsp(&[lsp("Shape", 5, 0, 9, 0), lsp("new", 6, 1, 2, 1), lsp("area", 12, 4, 8, 1)]);
        assert_eq!(nested.iter().map(|s| (s.kind, s.depth, s.col)).collect::<Vec<_>>(), [(Kind::Class, 0, 4), (Kind::Method, 1, 4), (Kind::Function, 1, 4)]);
        // A flat answer (all depth 0): depth from containment.
        let flat = from_lsp(&[lsp("Shape", 23, 0, 9, 0), lsp("new", 6, 1, 2, 0), lsp("area", 6, 4, 8, 0), lsp("helper", 12, 11, 13, 0)]);
        assert_eq!(flat.iter().map(|s| s.depth).collect::<Vec<_>>(), [0, 1, 1, 0]);
        // Bounded.
        let many: Vec<crate::lsp::Symbol> = (0..MAX_SYMBOLS as u32 + 50).map(|i| lsp(&"x".repeat(500), 12, i, i, 0)).collect();
        let kept = from_lsp(&many);
        assert_eq!((kept.len(), kept[0].name.chars().count()), (MAX_SYMBOLS, 200));
        assert_eq!([kind_from_lsp(2), kind_from_lsp(22), kind_from_lsp(77), kind_from_lsp(15)], [Kind::Module, Kind::Constant, Kind::Other, Kind::Variable]);
    }

    #[test]
    fn parents_are_found_in_one_pass_and_match_the_nesting() {
        let d = doc();
        assert_eq!(parents(&d), [None, Some(0), Some(0), Some(2), None, None]);
        assert!(parents(&[]).is_empty());
    }

    /// 20,000 top-level symbols with a filter used to cost a backward scan per match.
    #[test]
    fn a_filter_over_twenty_thousand_symbols_is_fast() {
        let many: Vec<Symbol> = (0..20_000).map(|i| sym(&format!("item_{i}"), Kind::Function, i, i, 0)).collect();
        let started = std::time::Instant::now();
        let p = parents(&many);
        let found = rows(&many, &p, "item_1", Sort::Position);
        let by_name = rows(&many, &p, "item_1", Sort::Name);
        assert_eq!((found.len(), by_name.len()), (11_111, 11_111));
        assert_eq!(siblings(&many, 5).len(), 20_000);
        assert!(started.elapsed().as_millis() < 1500 * u128::from(crate::test_sshd::timeout_scale()), "{:?}", started.elapsed());
    }

    /// With every symbol at depth 0 the old ancestor walk was quadratic; here a deep chain with a filter on the last one.
    #[test]
    fn a_deep_chain_with_a_filter_on_the_leaf_is_fast() {
        let chain: Vec<Symbol> = (0..2_000).map(|i| sym(&format!("n{i}"), Kind::Class, i, 5_000, i)).collect();
        let started = std::time::Instant::now();
        let found = rows(&chain, &parents(&chain), "n1999", Sort::Position);
        assert_eq!(found.len(), 2_000);
        assert!(started.elapsed().as_millis() < 1500 * u128::from(crate::test_sshd::timeout_scale()));
    }
}
