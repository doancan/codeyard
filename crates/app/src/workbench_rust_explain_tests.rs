//! Explain error E0xxx: the server's link, `rustc --explain` without one, and the Problems row's button.

use accesskit::Role;
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::super::*;
use super::tests::{Fixture, fixture};
use crate::qa_integration_tests::wait_ticking;

const HREF: &str = "https://doc.rust-lang.org/error-index.html#E0308";

/// One error at the start of the file, as rust-analyzer 0.3.3113 published E0308 (with its link when `href`).
fn error_on_open(code: &str, href: Option<&str>) -> Value {
    let mut diagnostic = json!({
        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } },
        "severity": 1, "code": code, "source": "rustc", "message": "mismatched types\nexpected `i32`, found `&str`",
    });
    if let Some(href) = href {
        diagnostic["codeDescription"] = json!({ "href": href });
    }
    json!({ "method": "textDocument/publishDiagnostics", "params": { "uri": "$uri", "diagnostics": [diagnostic] } })
}

fn with_error<'a>(cx: &'a mut TestAppContext, name: &str, code: &str, href: Option<&str>) -> Option<Fixture<'a>> {
    let f = fixture(cx, name, json!({ "on_open": [error_on_open(code, href)] }), json!({}))?;
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the problem", |cx| wb.read_with(cx, |wb, _| !wb.problems().is_empty()));
    let ed = f.wb.read_with(f.cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(0, 1, cx));
    Some(f)
}

/// The server's link is opened in the browser, and nothing runs.
#[gpui::test]
fn an_error_with_a_link_opens_the_link(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = with_error(cx, "link", "E0308", Some(HREF)) else { return };
    f.run("Rust: Explain Error at Cursor");
    wait_ticking(f.cx, "the browser", |cx| cx.opened_url().as_deref() == Some(HREF));
}

/// Without a link `rustc --explain` runs and its text opens as a read-only page; the UI is not blocked meanwhile.
#[gpui::test]
fn an_error_without_a_link_is_explained_by_rustc(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = with_error(cx, "rustc", "E0382", None) else { return };
    f.run("Rust: Explain Error at Cursor");
    assert_eq!(crate::qa_integration_tests::notice(&f.wb.clone(), f.cx), "Looking up E0382…", "said at once, the lookup runs in the background");
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the page", |cx| wb.read_with(cx, |wb, _| matches!(wb.tabs.page, Some(Page::Markdown(_)))));
    let text = f.wb.read_with(f.cx, |wb, cx| wb.active_editor().map(|e| e.read(cx).text())).unwrap();
    assert!(text.starts_with("# Error code E0382\n\nA variable was used after its contents have been moved elsewhere."), "{text}");
    assert!(f.cx.opened_url().is_none());
}

/// A code that is not a rustc error (a lint) and a cursor off every problem both say so.
#[gpui::test]
fn lints_and_places_without_an_error_say_so(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = with_error(cx, "lint", "unused_variables", None) else { return };
    f.run("Rust: Explain Error at Cursor");
    f.wait_notice("No rustc error code at the cursor");
    let ed = f.wb.read_with(f.cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(2, 0, cx));
    f.run("Rust: Explain Error at Cursor");
    f.wait_notice("No rustc error code at the cursor");
}

/// The Problems row of an error code has an Explain button, named by the code, that does what the command does.
#[gpui::test]
fn the_problems_row_has_an_explain_button(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = with_error(cx, "row", "E0308", Some(HREF)) else { return };
    f.run("View: Problems");
    let wb = f.wb.clone();
    let mut found = None;
    wait_ticking(f.cx, "the button", |cx| {
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        found = snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some("Explain error E0308")).map(|(id, _)| (id, snap));
        found.is_some()
    });
    let (id, snap) = found.unwrap();
    f.wb.update_in(f.cx, |wb, window, cx| {
        let request = accesskit::ActionRequest { action: accesskit::Action::Click, target_tree: accesskit::TreeId::ROOT, target_node: id, data: None };
        wb.a11y_action_for_test(&request, &snap, window, cx);
    });
    wait_ticking(f.cx, "the browser", |cx| cx.opened_url().as_deref() == Some(HREF));
}
