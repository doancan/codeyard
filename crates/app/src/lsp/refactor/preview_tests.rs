//! The review page on its own: its rows, the keyboard, what Apply asks for and what it says afterwards.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{Entity, TestAppContext};
use serde_json::{Value, json};

use super::*;
use crate::lsp::refactor::edit::parse;
use crate::lsp::refactor::env::HostEnv;
use crate::lsp::refactor::plan;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-refactor-preview-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    crate::ui::canonical_for_test(d)
}

fn s(p: &Path) -> String {
    crate::ui::local_path(p)
}

fn at(l: u32, c0: u32, c1: u32, new: &str) -> Value {
    json!({ "range": { "start": { "line": l, "character": c0 }, "end": { "line": l, "character": c1 } }, "newText": new })
}

fn text_edit(path: &Path, edits: Vec<Value>) -> Value {
    json!({ "textDocument": { "uri": format!("file://{}", s(path)), "version": null }, "edits": edits })
}

/// `a.rs` with three changes on two lines and `b.rs` with one, in a folder of its own.
fn project(name: &str) -> (PathBuf, Value) {
    let d = dir(name);
    std::fs::write(d.join("a.rs"), "let x = 1;\nlet y = x + x;\nlet z = 3;\nlet w = 4;\n").unwrap();
    std::fs::write(d.join("b.rs"), "x();\n").unwrap();
    let edit = json!({ "documentChanges": [
        text_edit(&d.join("a.rs"), vec![at(0, 4, 5, "k"), at(1, 8, 9, "k"), at(1, 12, 13, "k")]),
        text_edit(&d.join("b.rs"), vec![at(0, 0, 1, "k")]),
    ] });
    (d, edit)
}

fn plan_of(d: &Path, edit: &Value) -> Arc<RefactorPlan> {
    let env = HostEnv::new(Arc::new(host::local()), Default::default());
    Arc::new(plan::build(parse(edit), &env, &[s(d)], "local".into(), s(d), crate::encoding::Encoding::Utf8).unwrap())
}

fn view(cx: &mut TestAppContext, p: Arc<RefactorPlan>) -> (Entity<RefactorPreview>, Rc<RefCell<Vec<String>>>) {
    let view = cx.new(|cx| RefactorPreview::new("Rename to “k”".into(), p, cx));
    let events = Rc::new(RefCell::new(Vec::new()));
    let log = events.clone();
    cx.update(|cx| {
        cx.subscribe(&view, move |_, ev: &RefactorEvent, _| {
            log.borrow_mut().push(match ev {
                RefactorEvent::Apply => "apply".to_string(),
                RefactorEvent::Discard => "discard".into(),
                RefactorEvent::Undo => "undo".into(),
                RefactorEvent::Stop => "stop".into(),
                RefactorEvent::Open { path, line, column, len } => format!("open {} {line}:{column}:{len}", crate::ui::file_name(path)),
            })
        })
        .detach();
    });
    (view, events)
}

fn press(view: &Entity<RefactorPreview>, cx: &mut TestAppContext, keys: &[&str]) {
    for k in keys {
        view.update(cx, |v, cx| v.press_for_test(k, cx));
    }
}

fn rows(view: &Entity<RefactorPreview>, cx: &mut TestAppContext) -> Vec<String> {
    view.read_with(cx, |v, _| v.rows_for_test())
}

#[gpui::test]
fn rows_are_files_then_each_change_as_old_and_new_with_the_context_around(cx: &mut TestAppContext) {
    let (d, edit) = project("rows");
    let (v, _) = view(cx, plan_of(&d, &edit));
    let r = rows(&v, cx);
    assert_eq!(r[0], "[x] a.rs");
    assert_eq!(r[1], "  [x] Line 1, column 5: “x” → “k”", "{r:?}");
    assert_eq!(r[2], "  Line 1 after the change");
    // Line 2 has two changes: its context is shown once, after the second of them, and line 1 (a change) is not repeated.
    assert_eq!(r.iter().filter(|l| l.contains("Line 3: let z = 3;")).count(), 1, "{r:?}");
    let idx = |needle: &str| r.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle} in {r:?}"));
    assert!(idx("Line 3: let z = 3;") > idx("Line 2 after the change"), "the context comes after its change: {r:?}");
    assert!(r.iter().position(|l| l.contains("Line 3: let z")).unwrap() > r.iter().rposition(|l| l.starts_with("  [x] Line 2")).unwrap(), "{r:?}");
    assert!(r.contains(&"[x] b.rs".to_string()));
    // No context at all.
    v.update(cx, |v, cx| v.set_context_for_test(0, cx));
    assert!(!rows(&v, cx).iter().any(|l| l.contains("Line 3")));
    // More context reaches the next line too, never past the end of the file.
    v.update(cx, |v, cx| v.set_context_for_test(3, cx));
    let r = rows(&v, cx);
    assert!(r.iter().any(|l| l.contains("Line 4: let w = 4;")), "{r:?}");
    assert!(!r.iter().any(|l| l.starts_with("  Line 5")), "{r:?}");
}

#[gpui::test]
fn the_keyboard_selects_folds_and_opens(cx: &mut TestAppContext) {
    let (d, edit) = project("keys");
    let (v, events) = view(cx, plan_of(&d, &edit));
    assert_eq!(v.read_with(cx, |v, _| v.cursor_for_test()), Some(0));
    // Space on a file clears all of its changes; again, selects them all.
    press(&v, cx, &["space"]);
    assert_eq!(rows(&v, cx)[0], "[ ] a.rs");
    assert!(rows(&v, cx)[1].starts_with("  [ ] Line 1"));
    assert_eq!(v.read_with(cx, |v, _| v.status_for_test()), "Selected: 1 change in 1 file.");
    press(&v, cx, &["space"]);
    assert_eq!(rows(&v, cx)[0], "[x] a.rs");
    // Down to the first change, Space clears just it.
    press(&v, cx, &["down", "space"]);
    assert_eq!(rows(&v, cx)[0], "[-] a.rs");
    assert!(rows(&v, cx)[1].starts_with("  [ ] Line 1"));
    assert_eq!(v.read_with(cx, |v, _| v.status_for_test()), "Selected: 3 changes in 2 files.");
    // Enter on a change opens its file at the changed text (UTF-16 columns became characters).
    press(&v, cx, &["enter"]);
    assert_eq!(events.borrow().last().unwrap(), "open a.rs 1:4:1");
    // Left goes to the file, then folds it; Right unfolds.
    press(&v, cx, &["left"]);
    assert_eq!(v.read_with(cx, |v, _| v.cursor_for_test()), Some(0));
    let before = rows(&v, cx).len();
    press(&v, cx, &["left"]);
    assert!(rows(&v, cx).len() < before);
    assert_eq!(rows(&v, cx)[..2], ["[-] a.rs", "[x] b.rs"], "a.rs shows nothing below it: {:?}", rows(&v, cx));
    press(&v, cx, &["right"]);
    assert_eq!(rows(&v, cx).len(), before);
    // Up at the top stays at the top; Down past the end stays at the end.
    press(&v, cx, &["up", "up"]);
    assert_eq!(v.read_with(cx, |v, _| v.cursor_for_test()), Some(0));
    for _ in 0..40 {
        press(&v, cx, &["down"]);
    }
    assert_eq!(v.read_with(cx, |v, _| v.cursor_for_test()), Some(rows(&v, cx).len() - 1));
}

#[gpui::test]
fn apply_asks_once_and_discard_goes_back_one_step_at_a_time(cx: &mut TestAppContext) {
    let (d, edit) = project("apply");
    let (v, events) = view(cx, plan_of(&d, &edit));
    assert_eq!(v.read_with(cx, |v, _| v.apply_label_for_test()), "Apply Selected");
    press(&v, cx, &["apply"]);
    assert_eq!(*events.borrow(), ["apply"]);
    assert_eq!(v.read_with(cx, |v, _| v.phase_for_test()), "applying");
    // A second press while it runs does nothing; Escape asks to stop.
    press(&v, cx, &["apply", "escape"]);
    assert_eq!(*events.borrow(), ["apply", "stop"]);
    // It failed and was taken back: the page is for review again, with the reason, and can try again.
    v.update(cx, |v, cx| v.set_report(vec![("File changed since preview: a.rs".into(), true)], false, false, cx));
    assert_eq!(v.read_with(cx, |v, _| v.phase_for_test()), "review");
    assert_eq!(v.read_with(cx, |v, _| v.report_for_test()), ["File changed since preview: a.rs"]);
    assert!(rows(&v, cx).len() > 2, "the selection is kept");
    press(&v, cx, &["escape"]);
    assert_eq!(events.borrow().last().unwrap(), "discard");
}

#[gpui::test]
fn nothing_selected_cannot_be_applied(cx: &mut TestAppContext) {
    let (d, edit) = project("nothing");
    let (v, events) = view(cx, plan_of(&d, &edit));
    // Both files off: 0 selected, Apply does nothing.
    press(&v, cx, &["space"]);
    let b = rows(&v, cx).iter().position(|l| l.contains("b.rs")).unwrap();
    for _ in 0..b {
        press(&v, cx, &["down"]);
    }
    press(&v, cx, &["space"]);
    assert_eq!(v.read_with(cx, |v, _| v.status_for_test()), "Nothing selected.");
    press(&v, cx, &["apply"]);
    assert!(events.borrow().is_empty());
    assert_eq!(v.read_with(cx, |v, _| v.phase_for_test()), "review");
}

#[gpui::test]
fn deleting_waits_for_a_second_go_and_names_what_goes(cx: &mut TestAppContext) {
    let d = dir("delete");
    std::fs::write(d.join("old.txt"), "x").unwrap();
    std::fs::create_dir(d.join("eski klasör")).unwrap();
    let edit = json!({ "documentChanges": [
        { "kind": "delete", "uri": format!("file://{}", s(&d.join("old.txt"))) },
        { "kind": "delete", "uri": format!("file://{}", s(&d.join("eski klasör"))), "options": { "recursive": true } },
        { "kind": "create", "uri": format!("file://{}", s(&d.join("yeni.txt"))) },
    ] });
    let (v, events) = view(cx, plan_of(&d, &edit));
    // File operations start unselected: nothing can be applied until one is chosen.
    assert_eq!(rows(&v, cx), ["[ ] Delete old.txt", "[ ] Delete eski klasör", "[ ] Create yeni.txt"]);
    assert_eq!(v.read_with(cx, |v, _| v.status_for_test()), "Nothing selected.");
    press(&v, cx, &["apply"]);
    assert!(events.borrow().is_empty());
    // Choose the create: it is written at once. Choose a delete as well: the first press only asks.
    press(&v, cx, &["down", "down", "space"]);
    assert_eq!(v.read_with(cx, |v, _| v.status_for_test()), "Selected: 1 file operation.");
    press(&v, cx, &["up", "up", "space", "down", "space"]);
    assert_eq!(v.read_with(cx, |v, _| v.status_for_test()), "Selected: 3 file operations.");
    press(&v, cx, &["apply"]);
    assert!(events.borrow().is_empty(), "nothing is deleted on the first press");
    assert_eq!(v.read_with(cx, |v, _| v.phase_for_test()), "confirm");
    // Escape is Back, not Discard.
    press(&v, cx, &["escape"]);
    assert_eq!(v.read_with(cx, |v, _| v.phase_for_test()), "review");
    assert!(events.borrow().is_empty());
    press(&v, cx, &["apply", "apply"]);
    assert_eq!(*events.borrow(), ["apply"]);
    // The question names both, and that it is for good.
    let view_text = v.read_with(cx, |v, _| v.deletes_for_test());
    assert_eq!(view_text, ["old.txt", "eski klasör"]);
}

#[gpui::test]
fn a_selected_edit_without_the_operation_it_needs_cannot_be_applied(cx: &mut TestAppContext) {
    let d = dir("needs");
    let new = d.join("n.rs");
    let edit = json!({ "documentChanges": [
        { "kind": "create", "uri": format!("file://{}", s(&new)) },
        text_edit(&new, vec![at(0, 0, 0, "fn f() {}\n")]),
    ] });
    let (v, events) = view(cx, plan_of(&d, &edit));
    // Choosing the create chooses the edit that needs it; clearing only the create (with Space on it) clears both.
    press(&v, cx, &["space"]);
    assert_eq!(rows(&v, cx)[..2], ["[x] Create n.rs", "[x] n.rs"]);
    press(&v, cx, &["space"]);
    assert_eq!(rows(&v, cx)[..2], ["[ ] Create n.rs", "[ ] n.rs"]);
    // The edit alone, without its create: the page says what is missing and does not apply.
    press(&v, cx, &["down", "down", "space"]);
    assert_eq!(v.read_with(cx, |v, _| v.status_for_test()), "n.rs needs a file operation that is not selected.");
    press(&v, cx, &["apply"]);
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn refused_files_show_why_and_cannot_be_selected(cx: &mut TestAppContext) {
    let d = dir("refused");
    let outside = dir("refused-outside");
    std::fs::write(outside.join("keys.txt"), "k\n").unwrap();
    std::fs::write(d.join("a.rs"), "abc\n").unwrap();
    let edit = json!({ "documentChanges": [
        text_edit(&outside.join("keys.txt"), vec![at(0, 0, 1, "X")]),
        text_edit(&d.join("a.rs"), vec![at(0, 0, 1, "X")]),
    ] });
    let (v, events) = view(cx, plan_of(&d, &edit));
    let r = rows(&v, cx);
    assert_eq!(r[0], s(&outside.join("keys.txt")), "a path outside the folder is shown whole: {r:?}");
    press(&v, cx, &["space"]);
    assert_eq!(rows(&v, cx)[0], r[0], "a refused file has no check box and cannot be selected");
    // A refused file has no change rows; the other file is still applied on its own.
    assert_eq!(r.iter().filter(|l| l.starts_with("  ")).count(), 2);
    press(&v, cx, &["apply"]);
    assert_eq!(*events.borrow(), ["apply"]);
    std::fs::remove_dir_all(&outside).ok();
}

#[gpui::test]
fn after_applying_only_the_report_is_left_and_stale_pages_say_so(cx: &mut TestAppContext) {
    let (d, edit) = project("report");
    let (v, events) = view(cx, plan_of(&d, &edit));
    press(&v, cx, &["apply"]);
    v.update(cx, |v, cx| v.set_report(vec![("Changed 4 changes in 2 files".into(), false)], true, true, cx));
    assert_eq!(v.read_with(cx, |v, _| v.phase_for_test()), "done");
    assert!(rows(&v, cx).is_empty());
    assert_eq!(v.read_with(cx, |v, _| v.report_for_test()), ["Changed 4 changes in 2 files"]);
    v.update(cx, |v, cx| v.request_undo(cx));
    assert_eq!(events.borrow().last().unwrap(), "undo");
    v.update(cx, |v, cx| v.set_undone(vec![("Restored 2 files: a.rs, b.rs".into(), false)], cx));
    v.update(cx, |v, cx| v.request_undo(cx));
    assert_eq!(events.borrow().iter().filter(|e| *e == "undo").count(), 1, "Undo is offered once");
    press(&v, cx, &["escape"]);
    assert_eq!(events.borrow().last().unwrap(), "discard", "Escape closes the finished page");
    // A page whose plan went stale before Apply: nothing to apply any more.
    let (v, events) = view(cx, plan_of(&d, &edit));
    v.update(cx, |v, cx| v.set_stale("The connection or folder changed since the preview. Nothing was changed.".into(), cx));
    press(&v, cx, &["apply"]);
    assert!(events.borrow().is_empty());
    assert_eq!(v.read_with(cx, |v, _| v.phase_for_test()), "done");
}

#[test]
fn line_breaks_inside_a_shown_line_become_a_symbol_and_the_range_follows() {
    // Multi-byte letters before and inside the range; "\r\n" is one break.
    let (text, range) = visible_breaks("şa\r\nbç\nd", 3..8);
    assert_eq!(&*text, "şa↵bç↵d");
    // The range covered "\r\nbç"; the break is 3 bytes as "↵" where it was 2.
    assert_eq!(&text[range.clone()], "↵bç", "{range:?}");
    let (text, range) = visible_breaks("plain", 1..3);
    assert_eq!((&*text, range), ("plain", 1..3));
    let (text, range) = visible_breaks("a\rb", 0..3);
    assert_eq!((&*text, range), ("a↵b", 0..5));
}
