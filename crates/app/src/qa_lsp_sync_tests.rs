//! What Codeyard sends in `textDocument/didChange` has to match the sync kind the server announced.

use gpui::{Entity, TestAppContext};
use serde_json::{Value, json};

use crate::editor::EditorView;
use crate::qa_integration_tests::{setup_with, wait_ticking};
use crate::qa_lsp_settings_tests::{dir, open_project};
use crate::settings::Settings;
use crate::test_lsp::FakeServer;
use crate::workbench::Workbench;

/// Opens `src/lib.rs` ("ğ😀 x\n") on a fake server announcing `capabilities`, inserts "ab" after the emoji and returns
/// the `didChange` params the server received.
fn did_change_after_edit(cx: &mut TestAppContext, name: &str, capabilities: Value) -> Option<Value> {
    let d = dir(&format!("sync-{name}"));
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "ğ😀 x\n").unwrap();
    let fake = FakeServer::new(&d.join("server"), &json!({ "capabilities": capabilities }))?;
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: true, lsp_servers: vec![("rust".into(), Some(fake.command().into()))], ..Settings::default() });
    open_project(&wb, cx, &root);
    wait_ticking(cx, "didOpen", |_| !fake.received("textDocument/didOpen").is_empty());
    let editor: Entity<EditorView> = wb.read_with(cx, |wb: &Workbench, _| wb.active_editor_entity()).unwrap();
    editor.update(cx, |e, cx| {
        e.insert_at_for_test(2, "ab", cx);
        e.lsp_context();
    });
    wait_ticking(cx, "didChange", |_| !fake.received("textDocument/didChange").is_empty());
    fake.received("textDocument/didChange").pop().map(|message| message["params"].clone())
}

/// A server that announced full sync (`textDocumentSync: 1`) reads only `contentChanges[0].text`: it must get the whole
/// document, not a range edit it cannot apply.
#[gpui::test]
fn a_full_sync_server_gets_the_whole_text_on_change(cx: &mut TestAppContext) {
    let Some(params) = did_change_after_edit(cx, "full", json!({ "textDocumentSync": 1 })) else { return };
    assert_eq!(params["contentChanges"], json!([{ "text": "ğ😀ab x\n" }]));
}

/// With incremental sync the edit goes as a range in UTF-16 units: the emoji is two units, so column 3 is after it.
#[gpui::test]
fn an_incremental_server_gets_a_utf16_range_on_change(cx: &mut TestAppContext) {
    let Some(params) = did_change_after_edit(cx, "incremental", json!({ "textDocumentSync": 2 })) else { return };
    let change = &params["contentChanges"][0];
    assert_eq!(change["text"], "ab");
    assert_eq!(change["range"]["start"], json!({ "line": 0, "character": 3 }));
}
