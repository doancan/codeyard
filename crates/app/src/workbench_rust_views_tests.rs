//! Docs, HIR, MIR and the syntax tree against the fake server (answers captured from rust-analyzer 0.3.3113).

use gpui::TestAppContext;
use serde_json::{Value, json};

use super::super::*;
use super::tests::{Fixture, fixture};
use crate::qa_integration_tests::wait_ticking;

fn real(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{}/testdata/rust-analyzer/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap()
}

/// The text of the page that opened (its Markdown source).
fn page_text(f: &mut Fixture) -> String {
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the page", |cx| wb.read_with(cx, |wb, _| matches!(wb.tabs.page, Some(Page::Markdown(_)))));
    f.wb.read_with(f.cx, |wb, cx| wb.active_editor().map(|e| e.read(cx).text())).unwrap()
}

/// HIR opens as a read-only page titled by the file, holding the server's text in a fenced block; the request carries
/// the file and the cursor.
#[gpui::test]
fn hir_opens_as_a_page_with_the_servers_text(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({ "responses": { "rust-analyzer/viewHir": { "result": real("view-hir.json") } } });
    let Some(mut f) = fixture(cx, "hir", scenario, json!({})) else { return };
    let ed = f.wb.read_with(f.cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(0, 8, cx));
    f.run("Rust: View HIR");
    let request = f.wait_received("rust-analyzer/viewHir");
    assert_eq!(request[0]["params"]["position"], json!({ "line": 0, "character": 8 }));
    assert_eq!(request[0]["params"]["textDocument"]["uri"], crate::lsp::path_to_uri(&f.file));
    assert_eq!(page_text(&mut f), "# HIR of lib.rs\n\n```text\nfn add(i32, i32) -> i32 {...}\n```\n");
}

#[gpui::test]
fn mir_opens_as_a_page_too(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({ "responses": { "rust-analyzer/viewMir": { "result": real("view-mir.json") } } });
    let Some(mut f) = fixture(cx, "mir", scenario, json!({})) else { return };
    f.run("Rust: View MIR");
    f.wait_received("rust-analyzer/viewMir");
    let text = page_text(&mut f);
    assert!(text.starts_with("# MIR of lib.rs\n\n```text\nfn add() {\n    let _0: i32;"), "{text}");
}

/// The syntax tree arrives as JSON and is read as the listing people know.
#[gpui::test]
fn the_syntax_tree_is_listed(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({ "responses": { "rust-analyzer/viewSyntaxTree": { "result": real("syntax-tree.json") } } });
    let Some(mut f) = fixture(cx, "tree", scenario, json!({})) else { return };
    f.run("Rust: View Syntax Tree");
    let request = f.wait_received("rust-analyzer/viewSyntaxTree");
    assert_eq!(request[0]["params"]["range"], Value::Null);
    let text = page_text(&mut f);
    assert!(text.starts_with("# Syntax tree of lib.rs\n\n```text\nSOURCE_FILE@0..116\n  FN@0..50\n"), "{text}");
}

/// An empty answer says there is nothing to show; a server without the request is told in words.
#[gpui::test]
fn an_empty_answer_and_a_missing_method_are_said_in_words(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({ "responses": { "rust-analyzer/viewHir": { "result": "" } } });
    let Some(mut f) = fixture(cx, "empty", scenario, json!({})) else { return };
    f.run("Rust: View HIR");
    f.wait_notice("Nothing to show for the item at the cursor");
    f.run("Rust: View MIR");
    f.wait_notice("The Rust language server does not support this command");
}

/// Documentation opens docs.rs in the browser.
#[gpui::test]
fn documentation_opens_the_web_page(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({ "responses": { "experimental/externalDocs": { "result": real("external-docs.json") } } });
    let Some(mut f) = fixture(cx, "docs", scenario, json!({})) else { return };
    f.run("Rust: Open Documentation");
    f.wait_received("experimental/externalDocs");
    wait_ticking(f.cx, "the browser", |cx| cx.opened_url().as_deref() == Some("https://doc.rust-lang.org/stable/std/macro.println.html"));
}

#[gpui::test]
fn documentation_that_does_not_exist_says_so(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({ "responses": { "experimental/externalDocs": { "result": null } } });
    let Some(mut f) = fixture(cx, "docs-none", scenario, json!({})) else { return };
    f.run("Rust: Open Documentation");
    f.wait_notice("No documentation found for the item at the cursor");
    assert!(f.cx.opened_url().is_none());
}

/// A link from the server that is not a web page or a local HTML file is never opened, whatever its scheme.
#[gpui::test]
fn documentation_links_of_other_kinds_are_never_opened(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({ "responses": { "experimental/externalDocs": { "result": { "web": "vscode://file/etc/passwd", "local": "file:///etc/passwd" } } } });
    let Some(mut f) = fixture(cx, "docs-bad", scenario, json!({})) else { return };
    f.run("Rust: Open Documentation");
    f.wait_notice("The language server sent a documentation link that is not a web page or a local HTML file, so it was not opened");
    assert!(f.cx.opened_url().is_none());
}

/// A local copy that exists opens in preference to the web page.
#[gpui::test]
fn a_local_html_copy_is_preferred_when_it_exists(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let page = crate::qa_lsp_settings_tests::dir("docs-local").join("fn.add.html");
    std::fs::write(&page, "<html></html>").unwrap();
    let local = crate::lsp::path_to_uri(&crate::ui::local_path(&page));
    let scenario = json!({ "responses": { "experimental/externalDocs": { "result": { "web": "https://docs.rs/demo", "local": local } } } });
    let Some(mut f) = fixture(cx, "docs-local-run", scenario, json!({})) else { return };
    f.run("Rust: Open Documentation");
    wait_ticking(f.cx, "the browser", |cx| cx.opened_url().as_deref() == Some(local.as_str()));
}
