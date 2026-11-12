//! The Problems panel in a real workbench: the filter changes the rows and is kept per workspace, files fold, the
//! active-file switch follows the front editor, rows are copied, and the keyboard moves through the list.

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::lsp::Diagnostic;

fn diag(severity: u8, line: u32, message: &str) -> Diagnostic {
    Diagnostic {
        start: (line, 4),
        end: (line, 5),
        severity,
        message: message.into(),
        source: Some("rustc".into()),
        code: Some("E1".into()),
        ..Default::default()
    }
}

/// A workbench on a project with diagnostics for `src/main.rs` (an error and a warning) and `README.md` (a warning).
fn setup<'a>(cx: &'a mut TestAppContext, name: &str) -> (Entity<Workbench>, &'a mut VisualTestContext, String, String) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::qa_workbench_tests::project(name);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let main = ui::local_path(&dir.join("src/main.rs"));
    let readme = ui::local_path(&dir.join("README.md"));
    wb.update(cx, |wb, _| {
        wb.lsp.diagnostics.insert(main.clone(), vec![diag(1, 0, "cannot find value"), diag(2, 3, "unused variable")]);
        wb.lsp.diagnostics.insert(readme.clone(), vec![diag(2, 1, "unused heading")]);
    });
    (wb, cx, main, readme)
}

fn rows(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    wb.read_with(cx, |wb, cx| {
        wb.problem_rows_for_test(cx)
            .into_iter()
            .map(|r| match r {
                ProblemRow::File { path, count, collapsed, .. } => format!("{}{} {count}", if collapsed { "+" } else { "" }, ui::file_name(&path)),
                ProblemRow::Problem { message, .. } => format!("  {message}"),
                ProblemRow::Related { text, .. } => format!("    {text}"),
            })
            .collect()
    })
}

#[gpui::test]
fn switches_and_text_narrow_the_rows_and_come_back_with_the_workspace(cx: &mut TestAppContext) {
    let (wb, cx, _, _) = setup(cx, "problems-filter");
    assert_eq!(rows(&wb, cx), ["README.md 1", "  unused heading", "main.rs 2", "  cannot find value", "  unused variable"]);

    wb.update(cx, |wb, cx| wb.change_problems_filter(|f| f.warnings = false, cx));
    assert_eq!(rows(&wb, cx), ["main.rs 1", "  cannot find value"]);
    wb.update(cx, |wb, cx| wb.change_problems_filter(|f| f.text = "heading".into(), cx));
    assert!(rows(&wb, cx).is_empty(), "the README warning is hidden by the switch");
    wb.update(cx, |wb, cx| wb.change_problems_filter(|f| f.warnings = true, cx));
    assert_eq!(rows(&wb, cx), ["README.md 1", "  unused heading"]);

    // The filter is what a reopened workspace shows: drop the live copy and load the saved one.
    wb.update(cx, |wb, cx| {
        wb.chrome.problems.filter = ProblemFilter::default();
        wb.load_problems_filter(cx);
    });
    assert_eq!(rows(&wb, cx), ["README.md 1", "  unused heading"]);
    let text = wb.read_with(cx, |wb, cx| wb.chrome.problems.field.as_ref().map(|f| f.read(cx).text().to_string()));
    assert_eq!(text.as_deref(), Some("heading"), "the text box shows the saved text");
}

#[gpui::test]
fn typing_in_the_filter_box_filters(cx: &mut TestAppContext) {
    let (wb, cx, _, _) = setup(cx, "problems-typing");
    wb.update_in(cx, |wb, window, cx| {
        wb.show_problems(&ShowProblems, window, cx);
        let field = wb.chrome.problems.field.clone().expect("the filter box exists");
        field.update(cx, |f, cx| f.set_text("unused v", cx));
    });
    cx.run_until_parked();
    assert_eq!(rows(&wb, cx), ["main.rs 1", "  unused variable"]);
}

#[gpui::test]
fn a_clicked_file_row_folds_and_opens_its_problems(cx: &mut TestAppContext) {
    let (wb, cx, main, _) = setup(cx, "problems-fold");
    wb.update_in(cx, |wb, window, cx| wb.click_problem_row_for_test(2, window, cx));
    assert_eq!(rows(&wb, cx), ["README.md 1", "  unused heading", "+main.rs 2"]);
    assert!(wb.read_with(cx, |wb, _| wb.problem_filter().collapsed.contains(&main)));
    wb.update_in(cx, |wb, window, cx| wb.click_problem_row_for_test(2, window, cx));
    assert_eq!(rows(&wb, cx).len(), 5);
    // "Collapse All" folds every file, and a second press opens them all.
    wb.update(cx, |wb, cx| wb.toggle_all_problem_files(cx));
    assert_eq!(rows(&wb, cx), ["+README.md 1", "+main.rs 2"]);
    wb.update(cx, |wb, cx| wb.toggle_all_problem_files(cx));
    assert_eq!(rows(&wb, cx).len(), 5);
}

#[gpui::test]
fn active_file_only_follows_the_front_editor(cx: &mut TestAppContext) {
    let (wb, cx, main, readme) = setup(cx, "problems-active");
    wb.update_in(cx, |wb, window, cx| wb.toggle_problems_active_file(&ToggleProblemsActiveFile, window, cx));
    assert!(rows(&wb, cx).is_empty(), "no file is open");
    crate::qa_workbench_tests::open_in(&wb, std::path::Path::new(&readme), cx);
    assert_eq!(rows(&wb, cx), ["README.md 1", "  unused heading"]);
    crate::qa_workbench_tests::open_in(&wb, std::path::Path::new(&main), cx);
    assert_eq!(rows(&wb, cx).len(), 3);
}

#[gpui::test]
fn copying_all_or_one_row_puts_lines_with_place_and_origin_on_the_clipboard(cx: &mut TestAppContext) {
    let (wb, cx, _, _) = setup(cx, "problems-copy");
    wb.update(cx, |wb, cx| wb.change_problems_filter(|f| f.warnings = false, cx));
    wb.update_in(cx, |wb, window, cx| wb.copy_all_problems(&CopyAllProblems, window, cx));
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("src/main.rs:1:5: error [rustc E1] cannot find value"),
        "only what the filter shows"
    );
    wb.update(cx, |wb, cx| wb.change_problems_filter(|f| f.warnings = true, cx));
    // Row 3 is the error of main.rs, row 2 its file row (every problem of the file).
    wb.update(cx, |wb, cx| wb.copy_problem_row(3, cx));
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("src/main.rs:1:5: error [rustc E1] cannot find value"));
    wb.update(cx, |wb, cx| wb.copy_problem_row(2, cx));
    let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap();
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(wb.read_with(cx, |wb, _| matches!(wb.notice(), Some(Notice::Info(_)))));
}

#[gpui::test]
fn the_list_takes_the_keyboard_and_enter_folds_a_file_or_opens_a_problem(cx: &mut TestAppContext) {
    let (wb, cx, main, _) = setup(cx, "problems-keys");
    wb.update_in(cx, |wb, window, cx| {
        wb.show_problems(&ShowProblems, window, cx);
        wb.focus_row_list_for_test(a11y::RowFrame::Problem, 2, window, cx);
    });
    cx.simulate_keystrokes("enter");
    assert_eq!(rows(&wb, cx), ["README.md 1", "  unused heading", "+main.rs 2"], "Enter on a file row folds it");
    cx.simulate_keystrokes("enter");
    assert_eq!(rows(&wb, cx).len(), 5, "and again opens it");
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("enter");
    crate::qa_integration_tests::wait_secs(cx, 5, "the problem's file to open", |cx| wb.read_with(cx, |wb, _| wb.active_editor().is_some()));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_editor().map(|e| e.read(cx).path.clone())), Some(main));
}
