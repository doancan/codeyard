//! Related locations and the full compiler message, end to end: what a language server attaches to a diagnostic shows in
//! the hover and in Problems, opens on a click, and *Problems: Copy Full Diagnostic* puts it on the clipboard. On the
//! scriptable fake server, and against a real rust-analyzer when one is installed.

use gpui::{ClipboardItem, Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::editor::{EditorView, HoverRow};
use crate::lsp::path_to_uri;
use crate::qa_integration_tests::{notice, setup_with, wait_ticking};
use crate::qa_lsp_settings_tests::{cargo_project, dir, open_project, rust_analyzer_installed, wait_checked};
use crate::settings::Settings;
use crate::test_lsp::FakeServer;
use crate::workbench::{ProblemRow, Workbench};

const A: &str = "pub fn f() {\n    let t = s;\n    println!(\"{s}\");\n    let n = 1;\n    let m = 2;\n}\n";
const RENDERED: &str =
    "\u{1b}[1m\u{1b}[91merror[E0382]\u{1b}[0m: borrow of moved value: `s`\n --> src/a.rs:3:16\n  |\n2 |     let t = s;\n  |             - value moved here\n";

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({ "start": { "line": line, "character": start }, "end": { "line": line, "character": end } })
}

fn related(uri: &str, line: u32, col: u32, message: &str) -> Value {
    json!({ "location": { "uri": uri, "range": range(line, col, col + 1) }, "message": message })
}

/// A project with `src/a.rs` (the file with problems) and `src/b.rs` (where one related location points), served by the
/// fake: on open it publishes, for `src/a.rs`, an E0382 error with related locations in both files and the compiler's
/// colourful output, and a warning with 22 related locations; after a change it publishes an empty list.
fn project<'a>(cx: &'a mut TestAppContext, name: &str) -> Option<(Entity<Workbench>, &'a mut VisualTestContext, String, String, FakeServer)> {
    let d = dir(name);
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), A).unwrap();
    std::fs::write(root.join("src/b.rs"), format!("{}ğ😀 s\n", "// x\n".repeat(11))).unwrap();
    let (a, b) = (root.join("src/a.rs").to_string_lossy().into_owned(), root.join("src/b.rs").to_string_lossy().into_owned());
    let (ua, ub) = (path_to_uri(&a), path_to_uri(&b));
    let many: Vec<Value> = (0..22).map(|_| related(&ua, 3, 8, "also here")).collect();
    let publish = json!({ "method": "textDocument/publishDiagnostics", "params": { "uri": ua, "diagnostics": [
        { "range": range(2, 15, 16), "severity": 1, "code": "E0382", "source": "rustc", "message": "borrow of moved value: `s`",
          "relatedInformation": [related(&ua, 1, 12, "value moved here"), related(&ub, 11, 4, "borrow later used here")],
          "data": { "rendered": RENDERED } },
        { "range": range(4, 8, 9), "severity": 2, "code": "unused_variables", "message": "unused variable: `m`", "relatedInformation": many },
    ] } });
    let clear = json!({ "method": "textDocument/publishDiagnostics", "params": { "uri": ua, "diagnostics": [] } });
    let fake = FakeServer::new(&d.join("server"), &json!({ "on_open": [publish], "after": { "textDocument/didChange": [clear] } }))?;
    let (wb, cx) = setup_with(cx, Settings { lsp_servers: vec![("rust".into(), Some(fake.command().into()))], ..Settings::default() });
    open_project(&wb, cx, &root);
    let file = a.clone();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(file, window, cx));
    wait_ticking(cx, "the diagnostics of a.rs", |cx| wb.read_with(cx, |wb, _| wb.problems().iter().any(|(p, n)| p == &a && *n == 2)));
    Some((wb, cx, a, b, fake))
}

fn active(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Entity<EditorView> {
    wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap()
}

/// Hover at a (0-based line, character) of the active editor, through the keyboard command.
fn hover(wb: &Entity<Workbench>, cx: &mut VisualTestContext, line: usize, col: usize) -> Vec<HoverRow> {
    let ed = active(wb, cx);
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(line, col, cx));
    cx.simulate_keystrokes("cmd-k cmd-i");
    wait_ticking(cx, "the hover", |cx| ed.read_with(cx, |e, _| !e.hover_rows_for_test().is_empty()));
    ed.read_with(cx, |e, _| e.hover_rows_for_test())
}

fn copy_at(wb: &Entity<Workbench>, cx: &mut VisualTestContext, line: usize, col: usize) -> String {
    active(wb, cx).update(cx, |e, cx| e.set_cursor_line_col_for_test(line, col, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Problems: Copy Full Diagnostic", window, cx));
    notice(wb, cx)
}

fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|c| c.text())
}

/// AC1, AC2: the hover lists both related locations (same file by line, the other by its path in the folder) and a click
/// opens the other file at the related line and UTF-16 column (after `ğ😀 ` the column is 4 in UTF-16, 3 in characters).
#[gpui::test]
fn qa_diag_hover_shows_related_locations_and_a_click_opens_them(cx: &mut TestAppContext) {
    let Some((wb, cx, _, b, _fake)) = project(cx, "hover") else { return };
    let rows = hover(&wb, cx, 2, 15);
    assert_eq!(
        rows,
        vec![
            HoverRow::Diagnostic { severity: 1, text: "E0382: borrow of moved value: `s`".into() },
            HoverRow::Related { text: "line 2: value moved here".into(), path: rows_path(&rows, 1), line: 1, col: 12 },
            HoverRow::Related { text: "src/b.rs:12: borrow later used here".into(), path: b.clone(), line: 11, col: 4 },
        ]
    );
    active(&wb, cx).update(cx, |e, cx| e.click_hover_row_for_test(2, cx));
    wait_ticking(cx, "b.rs at the related position", |cx| {
        let ed = active(&wb, cx);
        ed.read_with(cx, |e, _| e.path == b && e.cursor_position() == (12, 4))
    });
}

fn rows_path(rows: &[HoverRow], i: usize) -> String {
    match &rows[i] {
        HoverRow::Related { path, .. } => path.clone(),
        other => panic!("not a related row: {other:?}"),
    }
}

/// AC3 and "a new list replaces the old": related locations are child rows of their problem in Problems, a click on one
/// opens it, the 20-location limit holds, and when the server publishes again the old rows go with their diagnostic.
#[gpui::test]
fn qa_diag_problems_show_related_rows_that_open(cx: &mut TestAppContext) {
    let Some((wb, cx, a, b, _fake)) = project(cx, "problems") else { return };
    let rows = wb.read_with(cx, |wb, cx| wb.problem_rows_for_test(cx));
    assert!(matches!(&rows[0], ProblemRow::File { path, count: 2, .. } if path == &a), "{:?}", rows[0]);
    assert!(matches!(&rows[1], ProblemRow::Problem { text, line: 2, .. } if text == "E0382: borrow of moved value: `s`"), "{:?}", rows[1]);
    assert_eq!(rows[2], ProblemRow::Related { text: "line 2: value moved here".into(), path: a.clone(), line: 1, col: 12 });
    assert_eq!(rows[3], ProblemRow::Related { text: "src/b.rs:12: borrow later used here".into(), path: b.clone(), line: 11, col: 4 });
    assert!(matches!(&rows[4], ProblemRow::Problem { text, .. } if text == "unused_variables: unused variable: `m`"));
    assert_eq!(rows.len(), 5 + 20, "at most 20 related rows per problem");

    wb.update_in(cx, |wb, window, cx| wb.click_problem_row_for_test(3, window, cx));
    wait_ticking(cx, "b.rs at the related position", |cx| {
        let ed = active(&wb, cx);
        ed.read_with(cx, |e, _| e.path == b && e.cursor_position() == (12, 4))
    });

    // A change makes the fake publish an empty list for a.rs: its problem and related rows go.
    let file = a.clone();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(file, window, cx));
    active(&wb, cx).update(cx, |e, cx| e.insert_at_for_test(0, "// edit\n", cx));
    wait_ticking(cx, "the old rows to go", |cx| {
        wb.read_with(cx, |wb, cx| !wb.problem_rows_for_test(cx).iter().any(|r| matches!(r, ProblemRow::File { path, .. } if path == &a)))
    });
}

/// AC4, AC5: the copy is the compiler's output without colour codes, or the parts when there is none; with nothing under
/// the cursor the clipboard is left alone.
#[gpui::test]
fn qa_diag_copy_full_diagnostic(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some((wb, cx, _, _, _fake)) = project(cx, "copy") else { return };
    assert_eq!(copy_at(&wb, cx, 2, 15), "Diagnostic copied");
    assert_eq!(
        clipboard(cx).as_deref(),
        Some("error[E0382]: borrow of moved value: `s`\n --> src/a.rs:3:16\n  |\n2 |     let t = s;\n  |             - value moved here\n")
    );

    assert_eq!(copy_at(&wb, cx, 4, 8), "Diagnostic copied");
    let fallback = clipboard(cx).unwrap();
    let lines: Vec<&str> = fallback.lines().collect();
    assert_eq!(lines[0], "warning[unused_variables]: unused variable: `m`");
    assert_eq!(lines[1], "  --> src/a.rs:4:9: also here");
    assert_eq!(lines.len(), 21, "the message and the 20 related locations kept");

    cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
    assert_eq!(copy_at(&wb, cx, 0, 0), "No problem at the cursor");
    assert_eq!(clipboard(cx).as_deref(), Some("before"), "unchanged");
}

/// AC6: in Turkish, the related rows, "+N more", the command and its notices.
#[gpui::test]
fn qa_diag_related_rows_and_notices_in_turkish(cx: &mut TestAppContext) {
    let _tr = crate::i18n::LangGuard::set("tr");
    let Some((wb, cx, _, _, _fake)) = project(cx, "turkish") else { return };
    let rows = hover(&wb, cx, 4, 8);
    assert!(matches!(&rows[1], HoverRow::Related { text, .. } if text == "satır 4: also here"), "{:?}", rows[1]);
    assert_eq!(rows.last(), Some(&HoverRow::More("(+2 daha)".into())));
    assert_eq!(rows.len(), 1 + 20 + 1);
    assert_eq!(crate::i18n::t("Problems: Copy Full Diagnostic"), "Sorunlar: Tanının Tamamını Kopyala");
    assert_eq!(copy_at(&wb, cx, 4, 8), "Tanı kopyalandı");
    assert_eq!(copy_at(&wb, cx, 0, 0), "İmleçte sorun yok");
}

/// AC7: rust-analyzer's use-after-move error carries the line where the value moved and the compiler's own output.
#[gpui::test]
fn qa_diag_real_rust_analyzer_explains_a_use_after_move(cx: &mut TestAppContext) {
    if !rust_analyzer_installed() {
        return;
    }
    let lib = "pub fn f() {\n    let s = String::new();\n    let t = s;\n    println!(\"{s}\");\n    drop(t);\n}\n";
    let (project, cfg) = cargo_project("cy_moved", lib, "");
    std::fs::write(cfg.join("settings.jsonc"), "{}").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    let file = open_project(&wb, cx, &project);
    let e0382 = |d: &crate::lsp::Diagnostic| d.code.as_deref() == Some("E0382") && d.severity == 1;
    wait_checked(&wb, cx, "E0382", |cx| wb.read_with(cx, |wb, _| wb.lsp_diagnostics_for_test(&file).iter().any(e0382)));
    let diags = wb.read_with(cx, |wb, _| wb.lsp_diagnostics_for_test(&file));
    let d = diags.iter().find(|d| e0382(d)).unwrap();
    assert!(d.related.iter().any(|r| r.path == file && r.start.0 == 2 && r.message.contains("moved")), "{d:#?}");
    assert!(d.rendered.as_deref().is_some_and(|r| r.contains("-->") && !r.contains('\u{1b}')), "{d:#?}");
    // rust-analyzer also publishes the related locations as hints; Problems shows the error once, with its related rows.
    assert!(diags.iter().filter(|d| d.severity == 4).count() >= 2, "the hint copies this guards against: {diags:#?}");
    let rows = wb.read_with(cx, |wb, cx| wb.problem_rows_for_test(cx));
    let problems: Vec<&ProblemRow> = rows.iter().filter(|r| matches!(r, ProblemRow::Problem { path, .. } if path == &file)).collect();
    assert_eq!(problems.len(), 1, "the error alone, not again for each related location or the macro call: {rows:#?}");
    assert!(rows.iter().any(|r| matches!(r, ProblemRow::Related { text, .. } if text == "line 3: value moved here")), "{rows:#?}");
}

/// Over SSH the rows are the same and a click opens the related file on the host.
#[gpui::test]
fn qa_diag_related_locations_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("diag-related") else { return };
    let d = dir("ssh");
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), A).unwrap();
    std::fs::write(root.join("src/b.rs"), format!("{}ğ😀 s\n", "// x\n".repeat(11))).unwrap();
    let (a, b) = (root.join("src/a.rs").to_string_lossy().into_owned(), root.join("src/b.rs").to_string_lossy().into_owned());
    let publish = json!({ "method": "textDocument/publishDiagnostics", "params": { "uri": path_to_uri(&a), "diagnostics": [
        { "range": range(2, 15, 16), "severity": 1, "code": "E0382", "message": "borrow of moved value: `s`",
          "relatedInformation": [related(&path_to_uri(&b), 11, 4, "borrow later used here")] },
    ] } });
    let Some(fake) = FakeServer::new(&d.join("server"), &json!({ "on_open": [publish] })) else { return };
    let (wb, cx) = setup_with(cx, Settings { lsp_servers: vec![("rust".into(), Some(fake.command().into()))], ..Settings::default() });
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 30, "connected", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    let r = root.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&r, window, cx));
    cx.run_until_parked();
    crate::qa_integration_tests::ssh_open_file(&wb, cx, &root.join("src/a.rs"));
    crate::qa_integration_tests::wait_secs(cx, 30, "the diagnostic over ssh", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        wb.read_with(cx, |wb, _| wb.problems().iter().any(|(p, _)| p == &a))
    });
    let rows = wb.read_with(cx, |wb, cx| wb.problem_rows_for_test(cx));
    assert_eq!(rows[2], ProblemRow::Related { text: "src/b.rs:12: borrow later used here".into(), path: b.clone(), line: 11, col: 4 });
    wb.update_in(cx, |wb, window, cx| wb.click_problem_row_for_test(2, window, cx));
    crate::qa_integration_tests::wait_secs(cx, 30, "b.rs opened on the host", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        wb.read_with(cx, |wb, _| wb.active_editor_entity()).is_some_and(|ed| ed.read_with(cx, |e, _| e.path == b && e.cursor_position() == (12, 4)))
    });
    assert!(active(&wb, cx).read_with(cx, |e, _| e.text().contains("ğ😀 s")));
}
