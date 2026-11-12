//! Reading a `WorkspaceEdit`: both shapes, the file operations, and what a server can get wrong.

use super::*;
use serde_json::json;

/// Captured from rust-analyzer (see crates/app/testdata/README.md): renaming `hesapla` in `util.rs` of the demo project.
const RENAME_FN: &str = include_str!("../../../testdata/refactor/rename_fn.json");
/// Captured from rust-analyzer: renaming the module `util` to `araçlar` (a rename of `util.rs` comes with the text edits).
const RENAME_MOD: &str = include_str!("../../../testdata/refactor/rename_mod.json");

fn text(op: &Op) -> &TextOp {
    match op {
        Op::Text(t) => t,
        other => panic!("not a text edit: {other:?}"),
    }
}

#[test]
fn a_real_rename_answer_gives_one_entry_per_file_with_the_versions_the_server_knew() {
    let parsed = parse(&serde_json::from_str(RENAME_FN).unwrap());
    assert!(parsed.skipped.is_empty());
    let files: Vec<_> = parsed.ops.iter().map(|op| (text(op).path.as_str(), text(op).version, text(op).edits.len())).collect();
    assert_eq!(
        files,
        [("/ws/demo/src/util.rs", Some(1), 2), ("/ws/demo/src/ayar.rs", None, 3), ("/ws/demo/src/main.rs", None, 2), ("/ws/demo/src/rapor.rs", None, 2),]
    );
    assert_eq!(text(&parsed.ops[0]).edits[0], TextEdit { start: (0, 7), end: (0, 14), new_text: "topla".into() });
    assert!(!parsed.has_resource_ops());
    assert_eq!(parsed.edit_count(), 9);
}

#[test]
fn a_real_module_rename_has_a_file_rename_with_the_turkish_name_decoded() {
    let parsed = parse(&serde_json::from_str(RENAME_MOD).unwrap());
    assert!(parsed.has_resource_ops());
    let rename = parsed.ops.iter().find(|op| op.is_resource()).unwrap();
    assert_eq!(*rename, Op::Rename { from: "/ws/demo/src/util.rs".into(), to: "/ws/demo/src/araçlar.rs".into(), overwrite: false, ignore_if_exists: false });
}

#[test]
fn the_old_changes_map_is_read_without_versions() {
    let edit = json!({ "changes": { "file:///w/a.rs": [{ "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 5 } }, "newText": "x" }] } });
    let parsed = parse(&edit);
    assert_eq!(parsed.ops.len(), 1);
    assert_eq!(text(&parsed.ops[0]).path, "/w/a.rs");
    assert_eq!(text(&parsed.ops[0]).version, None);
}

#[test]
fn document_changes_win_when_both_are_present() {
    let edit = json!({
        "changes": { "file:///w/old.rs": [] },
        "documentChanges": [{ "textDocument": { "uri": "file:///w/new.rs", "version": 3 }, "edits": [] }],
    });
    let parsed = parse(&edit);
    assert_eq!(parsed.ops.len(), 1);
    assert_eq!(text(&parsed.ops[0]).path, "/w/new.rs");
    assert_eq!(text(&parsed.ops[0]).version, Some(3));
}

#[test]
fn create_rename_and_delete_keep_their_options() {
    let edit = json!({ "documentChanges": [
        { "kind": "create", "uri": "file:///w/n.rs", "options": { "ignoreIfExists": true } },
        { "kind": "rename", "oldUri": "file:///w/a.rs", "newUri": "file:///w/b.rs", "options": { "overwrite": true } },
        { "kind": "delete", "uri": "file:///w/d", "options": { "recursive": true, "ignoreIfNotExists": true } },
    ] });
    assert_eq!(
        parse(&edit).ops,
        [
            Op::Create { path: "/w/n.rs".into(), overwrite: false, ignore_if_exists: true },
            Op::Rename { from: "/w/a.rs".into(), to: "/w/b.rs".into(), overwrite: true, ignore_if_exists: false },
            Op::Delete { path: "/w/d".into(), recursive: true, ignore_if_not_exists: true },
        ]
    );
}

#[test]
fn windows_file_uris_become_drive_paths() {
    let edit = json!({ "documentChanges": [
        { "kind": "create", "uri": "file:///C:/Users/ada/%C5%9Fablon/a.rs" },
        { "kind": "rename", "oldUri": "file:///C%3A/w/a.rs", "newUri": "file://server/share/b.rs" },
    ] });
    let ops = parse(&edit).ops;
    assert_eq!(ops[0], Op::Create { path: "C:/Users/ada/şablon/a.rs".into(), overwrite: false, ignore_if_exists: false });
    assert!(matches!(&ops[1], Op::Rename { from, to, .. } if from == "C:/w/a.rs" && to == "//server/share/b.rs"));
}

#[test]
fn entries_that_are_not_files_or_not_known_are_skipped_with_a_reason() {
    let edit = json!({ "documentChanges": [
        { "textDocument": { "uri": "https://example.com/a.rs", "version": null }, "edits": [] },
        { "kind": "teleport", "uri": "file:///w/a.rs" },
        { "kind": "create", "uri": "untitled:Untitled-1" },
    ] });
    let parsed = parse(&edit);
    assert!(parsed.ops.is_empty());
    assert_eq!(parsed.skipped.len(), 3, "{:?}", parsed.skipped);
    assert!(parsed.skipped[1].contains("teleport"));
}

#[test]
fn a_malformed_edit_marks_the_whole_document_instead_of_being_guessed_at() {
    let good = json!({ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }, "newText": "a" });
    for bad in [
        json!({ "newText": "a" }),
        json!({ "range": { "start": { "line": 0 }, "end": { "line": 0, "character": 1 } }, "newText": "a" }),
        json!({ "range": { "start": { "line": -1, "character": 0 }, "end": { "line": 0, "character": 1 } }, "newText": "a" }),
        json!({ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } } }),
    ] {
        let edit = json!({ "documentChanges": [{ "textDocument": { "uri": "file:///w/a.rs", "version": null }, "edits": [good.clone(), bad] }] });
        let op = parse(&edit).ops.remove(0);
        let t = text(&op);
        assert!(t.malformed.is_some());
        assert!(t.edits.is_empty(), "no part of a malformed list is kept");
    }
    let no_list = json!({ "documentChanges": [{ "textDocument": { "uri": "file:///w/a.rs", "version": null } }] });
    assert!(text(&parse(&no_list).ops[0]).malformed.is_some());
}

#[test]
fn an_end_of_line_column_of_any_size_is_kept() {
    let edit = json!({ "changes": { "file:///w/a.rs": [
        { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 9223372036854775807u64 } }, "newText": "" },
    ] } });
    assert_eq!(text(&parse(&edit).ops[0]).edits[0].end, (0, u32::MAX));
}

#[test]
fn anything_else_gives_an_empty_edit() {
    for v in [json!(null), json!([]), json!("x"), json!({}), json!({ "changes": 5, "documentChanges": null })] {
        assert_eq!(parse(&v), Parsed::default());
    }
}
