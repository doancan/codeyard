//! The Problems panel's rows under its filter: severity switches, text, active file, folded files and sort orders.

use super::*;
use crate::lsp::Diagnostic;
use std::collections::BTreeMap;

fn diag(severity: u8, line: u32, message: &str) -> Diagnostic {
    Diagnostic { start: (line, 2), end: (line, 3), severity, message: message.into(), source: Some("rustc".into()), ..Default::default() }
}

fn fixture() -> BTreeMap<String, Vec<Diagnostic>> {
    BTreeMap::from([
        ("/p/src/a.rs".to_string(), vec![diag(2, 1, "unused variable"), diag(1, 9, "borrow of moved value")]),
        ("/p/src/b.rs".to_string(), vec![diag(1, 3, "mismatched types"), diag(3, 0, "consider adding a type")]),
        ("/p/src/c.rs".to_string(), vec![diag(2, 5, "unused import")]),
    ])
}

fn texts(rows: &[ProblemRow]) -> Vec<String> {
    rows.iter()
        .map(|r| match r {
            ProblemRow::File { path, count, collapsed, .. } => format!("{}{} ({count})", if *collapsed { "+" } else { "" }, path.rsplit('/').next().unwrap()),
            ProblemRow::Problem { message, .. } => format!("  {message}"),
            ProblemRow::Related { text, .. } => format!("    {text}"),
        })
        .collect()
}

fn rows(filter: &ProblemFilter, active: Option<&str>) -> Vec<String> {
    texts(&problem_rows(&fixture(), Some("/p"), filter, active))
}

#[test]
fn without_a_filter_files_are_in_path_order_with_the_worst_problem_first() {
    assert_eq!(
        rows(&ProblemFilter::default(), None),
        [
            "a.rs (2)",
            "  borrow of moved value",
            "  unused variable",
            "b.rs (2)",
            "  mismatched types",
            "  consider adding a type",
            "c.rs (1)",
            "  unused import"
        ]
    );
}

#[test]
fn severity_switches_leave_out_files_with_nothing_to_show() {
    let only_errors = ProblemFilter { warnings: false, info: false, ..Default::default() };
    assert_eq!(rows(&only_errors, None), ["a.rs (1)", "  borrow of moved value", "b.rs (1)", "  mismatched types"]);
    let only_info = ProblemFilter { errors: false, warnings: false, ..Default::default() };
    assert_eq!(rows(&only_info, None), ["b.rs (1)", "  consider adding a type"]);
    let none = ProblemFilter { errors: false, warnings: false, info: false, ..Default::default() };
    assert!(rows(&none, None).is_empty());
}

#[test]
fn the_text_filter_matches_the_path_as_well_as_the_message() {
    let f = ProblemFilter { text: "unused".into(), ..Default::default() };
    assert_eq!(rows(&f, None), ["a.rs (1)", "  unused variable", "c.rs (1)", "  unused import"]);
    let by_file = ProblemFilter { text: "src/B.rs".into(), ..Default::default() };
    assert_eq!(rows(&by_file, None), ["b.rs (2)", "  mismatched types", "  consider adding a type"]);
}

#[test]
fn active_file_only_shows_that_file_and_nothing_when_no_file_is_open() {
    let f = ProblemFilter { active_file_only: true, ..Default::default() };
    assert_eq!(rows(&f, Some("/p/src/c.rs")), ["c.rs (1)", "  unused import"]);
    assert!(rows(&f, None).is_empty());
    assert!(rows(&f, Some("/p/src/none.rs")).is_empty());
}

#[test]
fn a_folded_file_is_only_its_own_row_and_keeps_its_count() {
    let mut f = ProblemFilter::default();
    f.collapsed.insert("/p/src/a.rs".into());
    assert_eq!(rows(&f, None)[..3], ["+a.rs (2)", "b.rs (2)", "  mismatched types"]);
}

#[test]
fn sorting_by_severity_puts_files_with_errors_first_and_by_line_orders_positions() {
    let by_severity = ProblemFilter { sort: Sort::Severity, ..Default::default() };
    let files: Vec<String> = rows(&by_severity, None).into_iter().filter(|r| !r.starts_with(' ')).collect();
    assert_eq!(files, ["a.rs (2)", "b.rs (2)", "c.rs (1)"], "a.rs and b.rs both have an error, c.rs only a warning");
    let mut all = fixture();
    all.insert("/p/src/0.rs".into(), vec![diag(3, 1, "only a note")]);
    let order: Vec<String> = texts(&problem_rows(&all, Some("/p"), &by_severity, None)).into_iter().filter(|r| !r.starts_with(' ')).collect();
    assert_eq!(order, ["a.rs (2)", "b.rs (2)", "c.rs (1)", "0.rs (1)"]);

    let by_line = ProblemFilter { sort: Sort::Line, ..Default::default() };
    assert_eq!(rows(&by_line, None)[..3], ["a.rs (2)", "  unused variable", "  borrow of moved value"]);
}

#[test]
fn rows_carry_the_columns_the_panel_shows() {
    let mut all = fixture();
    all.get_mut("/p/src/c.rs").unwrap()[0].code = Some("W1".into());
    let rows = problem_rows(&all, Some("/p"), &ProblemFilter::default(), None);
    let found = rows.iter().find_map(|r| match r {
        ProblemRow::Problem { path, message, source, code, text, .. } if path.ends_with("c.rs") => {
            Some((message.clone(), source.clone(), code.clone(), text.clone()))
        }
        _ => None,
    });
    assert_eq!(found, Some(("unused import".into(), "rustc".into(), "W1".into(), "W1: unused import".into())));
}

#[test]
fn totals_ignore_the_filter() {
    let counts = problem_totals(&fixture());
    assert_eq!((counts.errors, counts.warnings, counts.info, counts.total()), (2, 2, 1, 5));
}
