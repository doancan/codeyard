//! The refactoring preview with a language server that answers: a rename, a quick fix with a command and a server's own
//! `workspace/applyEdit`, through the real request path (what the server was asked, what it was answered, and when).

use std::path::{Path, PathBuf};

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::preview::RefactorPreview;
use crate::qa_integration_tests::{wait_secs, wait_ticking};
use crate::qa_lsp_settings_tests::{dir, open_project, settle, write_settings};
use crate::qa_workbench_tests::setup_with_config;
use crate::test_lsp::FakeServer;
use crate::workbench::Workbench;

fn s(p: &Path) -> String {
    crate::ui::local_path(p)
}

fn at(l: u32, c0: u32, c1: u32, new: &str) -> Value {
    json!({ "range": { "start": { "line": l, "character": c0 }, "end": { "line": l, "character": c1 } }, "newText": new })
}

fn edits(path: &Path, version: Value, list: Vec<Value>) -> Value {
    json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&s(path)), "version": version }, "edits": list })
}

/// A project: `src/lib.rs` (opened by `open_project`) and `src/other.rs`, closed.
fn project(d: &Path) -> (PathBuf, PathBuf) {
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::fs::write(root.join("src/other.rs"), "use crate::f;\nfn g() { f(); }\n").unwrap();
    (root.join("src/lib.rs"), root.join("src/other.rs"))
}

fn page(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<Entity<RefactorPreview>> {
    wb.read_with(cx, |wb, _| wb.refactor_page_for_test())
}

fn wait_for_open(fake: &FakeServer, cx: &mut VisualTestContext) {
    wait_ticking(cx, "the server to be told about the file", |_| !fake.received("textDocument/didOpen").is_empty());
}

fn press(v: &Entity<RefactorPreview>, cx: &mut VisualTestContext, keys: &[&str]) {
    for k in keys {
        v.update(cx, |v, cx| v.press_for_test(k, cx));
    }
    cx.run_until_parked();
}

fn rename_to(wb: &Entity<Workbench>, cx: &mut VisualTestContext, name: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Rename Symbol", window, cx));
    cx.run_until_parked();
    cx.simulate_input(name);
    cx.simulate_keystrokes("enter");
}

fn replies(fake: &FakeServer, id: u64) -> Vec<Value> {
    fake.replies().into_iter().filter(|r| r["id"] == id).map(|r| r["result"].clone()).collect()
}

/// A rename the server answers with edits in both files; `version` is what it says of the open `lib.rs`.
fn rename_scenario(lib: &Path, other: &Path, version: Value) -> Value {
    let edit = json!({ "documentChanges": [
        edits(lib, version, vec![at(0, 7, 8, "k")]),
        edits(other, Value::Null, vec![at(0, 11, 12, "k"), at(1, 9, 10, "k")]),
    ] });
    json!({ "capabilities": { "renameProvider": true }, "responses": { "textDocument/rename": { "result": edit } } })
}

#[gpui::test]
fn a_rename_the_server_answers_is_reviewed_checked_against_the_sent_version_and_applied(cx: &mut TestAppContext) {
    let d = dir("rp-rename");
    let (lib, other) = project(&d);
    // The editor sent version 1 with didOpen; the server's edit says so too.
    let Some(fake) = FakeServer::new(&d.join("server"), &rename_scenario(&lib, &other, json!(1))) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({}), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &d.join("project"));
    wait_for_open(&fake, cx);
    rename_to(&wb, cx, "k");
    wait_ticking(cx, "the rename request", |_| !fake.received("textDocument/rename").is_empty());
    assert_eq!(fake.received("textDocument/rename")[0]["params"]["newName"], "k");
    wait_ticking(cx, "the review", |cx| page(&wb, cx).is_some());
    let v = page(&wb, cx).unwrap();
    let rows = v.read_with(cx, |v, _| v.rows_for_test());
    assert!(rows.iter().any(|r| r == "[x] src/lib.rs") && rows.iter().any(|r| r == "[x] src/other.rs"), "{rows:?}");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "use crate::f;\nfn g() { f(); }\n", "nothing is written before Apply");
    press(&v, cx, &["apply"]);
    wait_ticking(cx, "done", |cx| v.read_with(cx, |v, _| v.phase_for_test()) == "done");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "use crate::k;\nfn g() { k(); }\n");
    let lib_text = wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&s(&lib), cx).map(|e| e.read(cx).buffer_text()));
    assert_eq!(lib_text.as_deref(), Some("pub fn k() {}\n"), "the open file changed in its editor");
}

#[gpui::test]
fn an_edit_for_another_version_of_an_open_file_is_refused_in_the_review(cx: &mut TestAppContext) {
    let d = dir("rp-rename-stale");
    let (lib, other) = project(&d);
    // The server computed it against version 7 of lib.rs; the editor sent version 1.
    let Some(fake) = FakeServer::new(&d.join("server"), &rename_scenario(&lib, &other, json!(7))) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({}), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &d.join("project"));
    wait_for_open(&fake, cx);
    rename_to(&wb, cx, "k");
    wait_ticking(cx, "the review", |cx| page(&wb, cx).is_some());
    let v = page(&wb, cx).unwrap();
    let rows = v.read_with(cx, |v, _| v.rows_for_test());
    assert_eq!(rows.iter().filter(|r| r.starts_with("[x] ")).count(), 1, "only other.rs can be chosen: {rows:?}");
    let lib_row = v.read_with(cx, |v, _| v.row_words_for_test(0));
    assert!(lib_row.contains("version 7") && lib_row.contains("version 1"), "{lib_row}");
    press(&v, cx, &["apply"]);
    wait_ticking(cx, "done", |cx| v.read_with(cx, |v, _| v.phase_for_test()) == "done");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "use crate::k;\nfn g() { k(); }\n");
    let lib_text = wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&s(&lib), cx).map(|e| e.read(cx).buffer_text()));
    assert_eq!(lib_text.as_deref(), Some("pub fn f() {}\n"), "the refused file is untouched");
}

#[gpui::test]
fn a_quick_fix_runs_its_command_only_after_its_edit_was_applied(cx: &mut TestAppContext) {
    for apply in [false, true] {
        let d = dir(&format!("rp-fix-{apply}"));
        let (lib, other) = project(&d);
        let action = json!({
            "title": "Rename everywhere", "kind": "quickfix",
            "edit": { "documentChanges": [
                edits(&lib, json!(1), vec![at(0, 7, 8, "k")]),
                edits(&other, Value::Null, vec![at(0, 11, 12, "k"), at(1, 9, 10, "k")]),
            ] },
            "command": { "title": "after", "command": "fake.afterFix", "arguments": [1, "two"] },
        });
        let scenario = json!({ "capabilities": { "codeActionProvider": true }, "responses": { "textDocument/codeAction": { "result": [action] } } });
        let Some(fake) = FakeServer::new(&d.join("server"), &scenario) else { return };
        let cfg = d.join("config");
        std::fs::create_dir_all(&cfg).unwrap();
        write_settings(&cfg, &fake, json!({}), json!({}));
        let (wb, cx) = setup_with_config(cx, &cfg);
        open_project(&wb, cx, &d.join("project"));
        wait_for_open(&fake, cx);
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Quick Fix…", window, cx));
        wait_ticking(cx, "the list of fixes", |_| !fake.received("textDocument/codeAction").is_empty());
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        wait_ticking(cx, "the review", |cx| page(&wb, cx).is_some());
        let v = page(&wb, cx).unwrap();
        assert!(fake.received("workspace/executeCommand").is_empty(), "the command waits for the edit");
        if apply {
            press(&v, cx, &["apply"]);
            wait_secs(cx, 10, "the command", |_| !fake.received("workspace/executeCommand").is_empty());
            let sent = fake.received("workspace/executeCommand");
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0]["params"], json!({ "command": "fake.afterFix", "arguments": [1, "two"] }));
            assert_eq!(std::fs::read_to_string(&other).unwrap(), "use crate::k;\nfn g() { k(); }\n");
        } else {
            press(&v, cx, &["escape"]);
            settle(cx);
            assert!(fake.received("workspace/executeCommand").is_empty(), "a refactoring that was turned down does not run its command");
            assert_eq!(std::fs::read_to_string(&other).unwrap(), "use crate::f;\nfn g() { f(); }\n");
        }
    }
}

/// The server asks to apply an edit with a file operation: the answer comes when the user has chosen.
#[gpui::test]
fn a_server_edit_with_file_operations_is_reviewed_and_answered_when_chosen(cx: &mut TestAppContext) {
    for apply in [true, false] {
        let d = dir(&format!("rp-apply-edit-{apply}"));
        let (lib, other) = project(&d);
        let moved = d.join("project/src/moved.rs");
        let ask = json!({ "id": 41, "method": "workspace/applyEdit", "params": { "label": "Move", "edit": { "documentChanges": [
            { "kind": "rename", "oldUri": crate::lsp::path_to_uri(&s(&other)), "newUri": crate::lsp::path_to_uri(&s(&moved)) },
            edits(&lib, json!(1), vec![at(0, 0, 3, "pub")]),
        ] } } });
        let scenario = json!({ "on_open": [ask] });
        let Some(fake) = FakeServer::new(&d.join("server"), &scenario) else { return };
        let cfg = d.join("config");
        std::fs::create_dir_all(&cfg).unwrap();
        write_settings(&cfg, &fake, json!({}), json!({}));
        let (wb, cx) = setup_with_config(cx, &cfg);
        open_project(&wb, cx, &d.join("project"));
        wait_ticking(cx, "the review", |cx| page(&wb, cx).is_some());
        let v = page(&wb, cx).unwrap();
        settle(cx);
        assert!(replies(&fake, 41).is_empty(), "the server is not answered before the user chooses");
        assert!(other.exists() && !moved.exists());
        if apply {
            // The rename is chosen on purpose (the first row is the edit of lib.rs, then the rename).
            let rows = v.read_with(cx, |v, _| v.rows_for_test());
            let at = rows.iter().position(|r| r.contains("Rename")).unwrap();
            press(&v, cx, &vec!["down"; at]);
            press(&v, cx, &["space", "apply"]);
            wait_ticking(cx, "the answer", |_| !replies(&fake, 41).is_empty());
            assert_eq!(replies(&fake, 41), [json!({ "applied": true })]);
            assert!(moved.exists() && !other.exists());
        } else {
            press(&v, cx, &["escape"]);
            wait_ticking(cx, "the answer", |_| !replies(&fake, 41).is_empty());
            assert_eq!(replies(&fake, 41)[0]["applied"], false);
            assert!(replies(&fake, 41)[0]["failureReason"].is_string(), "a refusal says why");
            assert!(other.exists() && !moved.exists());
        }
    }
}

/// A text-only `workspace/applyEdit` goes through the same path: an edit in one open file is applied at once (and answered),
/// one in a file that is not open is reviewed first.
#[gpui::test]
fn a_text_only_server_edit_is_applied_at_once_in_an_open_file_and_reviewed_otherwise(cx: &mut TestAppContext) {
    let d = dir("rp-apply-edit-plain");
    let (lib, other) = project(&d);
    let in_open =
        json!({ "id": 42, "method": "workspace/applyEdit", "params": { "edit": { "documentChanges": [edits(&lib, json!(1), vec![at(0, 7, 8, "k")])] } } });
    let in_closed = json!({ "id": 43, "method": "workspace/applyEdit", "params": { "edit": { "changes": { crate::lsp::path_to_uri(&s(&other)): [at(0, 11, 12, "k")] } } } });
    let Some(fake) = FakeServer::new(&d.join("server"), &json!({ "on_open": [in_open, in_closed] })) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({}), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &d.join("project"));
    wait_ticking(cx, "the answer for the open file", |_| !replies(&fake, 42).is_empty());
    assert_eq!(replies(&fake, 42), [json!({ "applied": true })]);
    let lib_text = wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&s(&lib), cx).map(|e| e.read(cx).buffer_text()));
    assert_eq!(lib_text.as_deref(), Some("pub fn k() {}\n"));
    // The closed file is shown first; the server waits.
    wait_ticking(cx, "the review", |cx| page(&wb, cx).is_some());
    settle(cx);
    assert!(replies(&fake, 43).is_empty());
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "use crate::f;\nfn g() { f(); }\n");
    let v = page(&wb, cx).unwrap();
    press(&v, cx, &["apply"]);
    wait_ticking(cx, "the answer for the closed file", |_| !replies(&fake, 43).is_empty());
    assert_eq!(replies(&fake, 43), [json!({ "applied": true })]);
    assert!(std::fs::read_to_string(&other).unwrap().starts_with("use crate::k;"));
}

/// Fifty closed files with `always`: reviewed, and answered `applied: false` when discarded.
#[gpui::test]
fn fifty_files_from_the_server_are_reviewed_and_a_discard_is_answered(cx: &mut TestAppContext) {
    let d = dir("rp-apply-edit-many");
    let (_lib, _other) = project(&d);
    let mut list = Vec::new();
    for i in 0..50 {
        let p = d.join(format!("project/src/m{i:02}.rs"));
        std::fs::write(&p, "fn m() { f(); }\n").unwrap();
        list.push(edits(&p, Value::Null, vec![at(0, 9, 10, "k")]));
    }
    let ask = json!({ "id": 44, "method": "workspace/applyEdit", "params": { "edit": { "documentChanges": list } } });
    let Some(fake) = FakeServer::new(&d.join("server"), &json!({ "on_open": [ask] })) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({}), json!({ "editor.refactorPreview": "always" }));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &d.join("project"));
    wait_ticking(cx, "the review", |cx| page(&wb, cx).is_some());
    let v = page(&wb, cx).unwrap();
    assert_eq!(v.read_with(cx, |v, _| v.rows_for_test().iter().filter(|r| r.starts_with("[x] ")).count()), 50);
    settle(cx);
    assert!(replies(&fake, 44).is_empty());
    press(&v, cx, &["escape"]);
    wait_ticking(cx, "the answer", |_| !replies(&fake, 44).is_empty());
    assert_eq!(replies(&fake, 44)[0]["applied"], false);
    assert!(replies(&fake, 44)[0]["failureReason"].is_string());
    assert_eq!(std::fs::read_to_string(d.join("project/src/m00.rs")).unwrap(), "fn m() { f(); }\n");
}

/// The trust gate applies to the server's own edits too: a folder that is not (or no longer) trusted is refused and the
/// server is told why.
#[gpui::test]
fn a_server_edit_in_an_untrusted_folder_is_refused_with_a_reason(cx: &mut TestAppContext) {
    let d = dir("rp-apply-edit-untrusted");
    let (lib, other) = project(&d);
    let ask =
        json!({ "id": 45, "method": "workspace/applyEdit", "params": { "edit": { "documentChanges": [edits(&lib, json!(1), vec![at(0, 7, 8, "k")])] } } });
    // The server asks after it has been asked to rename; the gate is turned on in between (the folder was trusted when it
    // started and is not any more).
    let scenario = json!({ "capabilities": { "renameProvider": true }, "responses": { "textDocument/rename": { "result": null } }, "after": { "textDocument/rename": [ask] } });
    let Some(fake) = FakeServer::new(&d.join("server"), &scenario) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({}), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &d.join("project"));
    wait_for_open(&fake, cx);
    cx.update(|_, cx| {
        let mut settings = cx.global::<crate::settings::Settings>().clone();
        settings.workspace_trust = true;
        cx.set_global(settings);
    });
    assert!(wb.read_with(cx, |wb, cx| wb.restricted_for_test(cx)), "the folder is untrusted now");
    rename_to(&wb, cx, "k");
    wait_ticking(cx, "the answer", |_| !replies(&fake, 45).is_empty());
    assert_eq!(replies(&fake, 45)[0]["applied"], false);
    assert!(replies(&fake, 45)[0]["failureReason"].is_string());
    assert!(page(&wb, cx).is_none());
    let lib_text = wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&s(&lib), cx).map(|e| e.read(cx).buffer_text()));
    assert_eq!(lib_text.as_deref(), Some("pub fn f() {}\n"));
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "use crate::f;\nfn g() { f(); }\n");
}
