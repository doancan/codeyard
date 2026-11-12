//! The refactoring preview in the workbench: when a review is shown, what Apply writes, and everything that must stop it
//! (stale files, stale editors, a failing step, a file outside the folder), on real folders and the real agent.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{Entity, TestAppContext, VisualTestContext};
use host::{Host, HostError};
use serde_json::{Value, json};

use super::apply::Fs;
use super::preview::RefactorPreview;
use crate::qa_integration_tests::wait_ticking;
use crate::qa_workbench_tests::{fresh, open, open_in, setup, setup_with_config};
use crate::replace_apply::Store;
use crate::workbench::Workbench;
use crate::workbench::refactor_glue::Done;

fn root(name: &str) -> PathBuf {
    crate::ui::canonical_for_test(fresh(name))
}

fn s(p: &Path) -> String {
    crate::ui::local_path(p)
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, text).unwrap();
    p
}

fn at(l: u32, c0: u32, c1: u32, new: &str) -> Value {
    json!({ "range": { "start": { "line": l, "character": c0 }, "end": { "line": l, "character": c1 } }, "newText": new })
}

fn edits(path: &Path, version: Value, list: Vec<Value>) -> Value {
    json!({ "textDocument": { "uri": format!("file://{}", s(path)), "version": version }, "edits": list })
}

/// Three files that use `x`: `a.rs` and `b.rs` with one call each and `c.rs` with two; the edit renames `x` to `k` in `a.rs`
/// and `b.rs` and (when `all`) in `c.rs`.
fn project(name: &str, all: bool) -> (PathBuf, Value) {
    let d = root(name);
    let (a, b, c) = (write(&d, "a.rs", "fn a() { x(); }\n"), write(&d, "b.rs", "fn b() { x(); }\n"), write(&d, "c.rs", "fn c() {\n    x();\n    x();\n}\n"));
    let mut list = vec![edits(&a, Value::Null, vec![at(0, 9, 10, "k")]), edits(&b, Value::Null, vec![at(0, 9, 10, "k")])];
    if all {
        list.push(edits(&c, Value::Null, vec![at(1, 4, 5, "k"), at(2, 4, 5, "k")]));
    }
    (d, json!({ "documentChanges": list }))
}

fn read(d: &Path, name: &str) -> String {
    std::fs::read_to_string(d.join(name)).unwrap()
}

/// What `done` was told, by name.
#[derive(Clone, Default)]
struct Told(Rc<RefCell<Vec<bool>>>);

impl Told {
    fn done(&self) -> Done {
        let log = self.0.clone();
        Box::new(move |_, applied, _| log.borrow_mut().push(applied))
    }
    fn all(&self) -> Vec<bool> {
        self.0.borrow().clone()
    }
}

fn propose(wb: &Entity<Workbench>, edit: &Value, told: &Told, cx: &mut VisualTestContext) {
    let done = told.done();
    wb.update(cx, |wb, cx| wb.review_workspace_edit_for_test("Rename to “k”", edit, Some(done), cx));
    cx.run_until_parked();
}

fn page(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<Entity<RefactorPreview>> {
    wb.read_with(cx, |wb, _| wb.refactor_page_for_test())
}

fn wait_page(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Entity<RefactorPreview> {
    wait_ticking(cx, "the refactor preview", |cx| page(wb, cx).is_some());
    page(wb, cx).unwrap()
}

fn press(v: &Entity<RefactorPreview>, cx: &mut VisualTestContext, keys: &[&str]) {
    for k in keys {
        v.update(cx, |v, cx| v.press_for_test(k, cx));
    }
    cx.run_until_parked();
}

fn phase(v: &Entity<RefactorPreview>, cx: &mut VisualTestContext) -> &'static str {
    v.read_with(cx, |v, _| v.phase_for_test())
}

fn wait_phase(v: &Entity<RefactorPreview>, want: &'static str, cx: &mut VisualTestContext) {
    wait_ticking(cx, want, |cx| phase(v, cx) == want);
}

fn editor_text(wb: &Entity<Workbench>, path: &Path, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&s(path), cx).map(|e| e.read(cx).buffer_text())).expect("the file is open")
}

fn dirty(wb: &Entity<Workbench>, path: &Path, cx: &mut VisualTestContext) -> bool {
    wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&s(path), cx).map(|e| e.read(cx).is_dirty())).expect("the file is open")
}

/// Config folder with `settings.jsonc` holding `json`.
fn config(name: &str, json: Value) -> PathBuf {
    let d = root(name);
    std::fs::write(d.join("settings.jsonc"), json.to_string()).unwrap();
    d
}

// --- Reviewing ---

#[gpui::test]
fn a_rename_over_several_files_is_reviewed_and_nothing_is_written_until_apply(cx: &mut TestAppContext) {
    let (d, edit) = project("rp-review", true);
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    let told = Told::default();
    propose(&wb, &edit, &told, cx);
    let v = wait_page(&wb, cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("refactor", true)));
    assert_eq!(read(&d, "a.rs"), "fn a() { x(); }\n", "the review writes nothing");
    assert!(told.all().is_empty(), "the server is still waiting");
    let rows = v.read_with(cx, |v, _| v.rows_for_test());
    assert_eq!(rows.iter().filter(|r| r.starts_with("[x] ")).count(), 3, "{rows:?}");
    assert_eq!(v.read_with(cx, |v, _| v.status_for_test()), "Selected: 4 changes in 3 files.");
    // Take one change of c.rs out, then apply.
    let c_change = rows.iter().position(|r| r == "  [x] Line 3: x(); → k();" || r.starts_with("  [x] Line 3")).unwrap();
    press(&v, cx, &vec!["down"; c_change]);
    press(&v, cx, &["space"]);
    assert_eq!(v.read_with(cx, |v, _| v.status_for_test()), "Selected: 3 changes in 3 files.");
    press(&v, cx, &["apply"]);
    wait_phase(&v, "done", cx);
    assert_eq!(read(&d, "a.rs"), "fn a() { k(); }\n");
    assert_eq!(read(&d, "b.rs"), "fn b() { k(); }\n");
    assert_eq!(read(&d, "c.rs"), "fn c() {\n    k();\n    x();\n}\n", "the deselected change (line 3) stays");
    assert_eq!(v.read_with(cx, |v, _| v.report_for_test()), ["Changed 3 changes in 3 files"]);
    assert_eq!(told.all(), [true]);
    assert!(page(&wb, cx).is_none(), "the page is no longer waiting for an answer");
}

#[gpui::test]
fn undo_takes_back_every_file_written_and_only_those_nobody_changed(cx: &mut TestAppContext) {
    let (d, edit) = project("rp-undo", true);
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    propose(&wb, &edit, &Told::default(), cx);
    let v = wait_page(&wb, cx);
    press(&v, cx, &["apply"]);
    wait_phase(&v, "done", cx);
    // Somebody edits b.rs before the undo: it is reported and left as it is.
    std::fs::write(d.join("b.rs"), "fn b() { k(); } // mine\n").unwrap();
    v.update(cx, |v, cx| v.request_undo(cx));
    cx.run_until_parked();
    wait_ticking(cx, "undo", |cx| v.read_with(cx, |v, _| v.report_for_test().len() >= 2));
    assert_eq!(read(&d, "a.rs"), "fn a() { x(); }\n");
    assert_eq!(read(&d, "c.rs"), "fn c() {\n    x();\n    x();\n}\n");
    assert_eq!(read(&d, "b.rs"), "fn b() { k(); } // mine\n");
    let report = v.read_with(cx, |v, _| v.report_for_test());
    assert!(report.iter().any(|l| l.starts_with("Restored 2 files")), "{report:?}");
    assert!(report.iter().any(|l| l.contains("Undo conflict: b.rs")), "{report:?}");
    // Undo is offered once.
    v.update(cx, |v, cx| v.request_undo(cx));
    cx.run_until_parked();
    assert_eq!(read(&d, "a.rs"), "fn a() { x(); }\n");
}

#[gpui::test]
fn one_file_that_is_open_is_applied_at_once_as_one_undo_step(cx: &mut TestAppContext) {
    let d = root("rp-open-one");
    let a = write(&d, "a.rs", "fn a() { x(); x(); }\n");
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    open_in(&wb, &a, cx);
    let told = Told::default();
    let edit = json!({ "documentChanges": [edits(&a, Value::Null, vec![at(0, 9, 10, "k"), at(0, 14, 15, "k")])] });
    propose(&wb, &edit, &told, cx);
    assert!(page(&wb, cx).is_none(), "a rename inside one open file needs no extra click");
    assert_eq!(editor_text(&wb, &a, cx), "fn a() { k(); k(); }\n");
    assert!(dirty(&wb, &a, cx));
    assert_eq!(read(&d, "a.rs"), "fn a() { x(); x(); }\n", "open files change in their editor and are saved by the user");
    assert_eq!(told.all(), [true]);
    // One undo step in the editor takes both back.
    wb.update_in(cx, |wb, window, cx| {
        let e = wb.editor_for_path_for_test(&s(&a), cx).unwrap();
        e.update(cx, |e, cx| e.undo_edit(window, cx));
    });
    assert_eq!(editor_text(&wb, &a, cx), "fn a() { x(); x(); }\n");
}

#[gpui::test]
fn the_setting_always_reviews_even_one_open_file_and_never_applies_at_once(cx: &mut TestAppContext) {
    for (mode, reviewed) in [("always", true), ("never", false), ("auto", false)] {
        let d = root(&format!("rp-mode-{mode}"));
        let a = write(&d, "a.rs", "fn a() { x(); }\n");
        let b = write(&d, "b.rs", "fn b() { x(); }\n");
        let cfg = config(&format!("rp-mode-cfg-{mode}"), json!({ "editor.refactorPreview": mode }));
        let (wb, cx) = setup_with_config(cx, &cfg);
        open(&wb, &d, cx);
        open_in(&wb, &a, cx);
        let one = json!({ "documentChanges": [edits(&a, Value::Null, vec![at(0, 9, 10, "k")])] });
        propose(&wb, &one, &Told::default(), cx);
        assert_eq!(page(&wb, cx).is_some(), reviewed, "{mode}: one open file");
        if reviewed {
            assert_eq!(editor_text(&wb, &a, cx), "fn a() { x(); }\n", "nothing is applied before the review ends");
        }
        // Two closed files: `auto` reviews them, `never` does not.
        let two = json!({ "documentChanges": [edits(&b, Value::Null, vec![at(0, 9, 10, "k")]), edits(&write(&d, "c.rs", "fn c() { x(); }\n"), Value::Null, vec![at(0, 9, 10, "k")])] });
        propose(&wb, &two, &Told::default(), cx);
        cx.run_until_parked();
        let two_reviewed = mode != "never";
        assert_eq!(
            page(&wb, cx).is_some_and(|v| v.read_with(cx, |v, _| v.rows_for_test().iter().any(|r| r.contains("b.rs")))),
            two_reviewed,
            "{mode}: two files"
        );
        if !two_reviewed {
            wait_ticking(cx, "written", |_| read(&d, "b.rs") == "fn b() { k(); }\n");
            assert_eq!(read(&d, "c.rs"), "fn c() { k(); }\n");
        }
    }
}

#[gpui::test]
fn file_operations_are_always_reviewed_whatever_the_setting_says(cx: &mut TestAppContext) {
    let d = root("rp-ops-never");
    let cfg = config("rp-ops-never-cfg", json!({ "editor.refactorPreview": "never" }));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open(&wb, &d, cx);
    let edit = json!({ "documentChanges": [{ "kind": "create", "uri": format!("file://{}", s(&d.join("neu.rs"))) }] });
    propose(&wb, &edit, &Told::default(), cx);
    let v = wait_page(&wb, cx);
    assert!(!d.join("neu.rs").exists());
    assert_eq!(v.read_with(cx, |v, _| v.rows_for_test()), ["[ ] Create neu.rs"]);
}

// --- Everything that must stop it ---

#[gpui::test]
fn a_file_changed_on_disk_after_the_preview_stops_everything_and_the_page_stays(cx: &mut TestAppContext) {
    let (d, edit) = project("rp-stale-disk", true);
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    let told = Told::default();
    propose(&wb, &edit, &told, cx);
    let v = wait_page(&wb, cx);
    std::fs::write(d.join("c.rs"), "fn c() { /* edited meanwhile */ }\n").unwrap();
    press(&v, cx, &["apply"]);
    wait_phase(&v, "review", cx);
    assert_eq!(read(&d, "a.rs"), "fn a() { x(); }\n", "a.rs was written first and was taken back");
    assert_eq!(read(&d, "b.rs"), "fn b() { x(); }\n");
    assert_eq!(read(&d, "c.rs"), "fn c() { /* edited meanwhile */ }\n");
    let report = v.read_with(cx, |v, _| v.report_for_test());
    assert!(report[0].contains("taken back"), "{report:?}");
    assert!(report.iter().any(|l| l == "File changed since preview: c.rs"), "{report:?}");
    assert!(told.all().is_empty(), "the edit is still undecided");
    // Closing the page turns the edit down.
    press(&v, cx, &["escape"]);
    assert_eq!(told.all(), [false]);
    assert!(page(&wb, cx).is_none());
}

#[gpui::test]
fn an_editor_typed_into_after_the_preview_stops_everything(cx: &mut TestAppContext) {
    let (d, edit) = project("rp-stale-buffer", false);
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    open_in(&wb, &d.join("a.rs"), cx);
    propose(&wb, &edit, &Told::default(), cx);
    let v = wait_page(&wb, cx);
    wb.update(cx, |wb, cx| {
        let e = wb.editor_for_path_for_test(&s(&d.join("a.rs")), cx).unwrap();
        e.update(cx, |e, cx| e.replace_all_text_for_test("fn a() { x(); } // typed\n", cx));
    });
    press(&v, cx, &["apply"]);
    wait_phase(&v, "review", cx);
    assert_eq!(read(&d, "b.rs"), "fn b() { x(); }\n", "b.rs is not written when a.rs cannot be");
    assert_eq!(editor_text(&wb, &d.join("a.rs"), cx), "fn a() { x(); } // typed\n");
    let report = v.read_with(cx, |v, _| v.report_for_test());
    assert_eq!(report, ["File changed since preview: a.rs"]);
}

/// A host whose writes can be made to fail: the nth write fails.
struct Faulty {
    inner: Arc<dyn Host>,
    fail_write: usize,
    writes: std::sync::atomic::AtomicUsize,
}

impl Store for Faulty {
    fn read(&self, path: &str) -> Result<(Vec<u8>, u64), HostError> {
        self.inner.read(path)
    }
    fn write(&self, path: &str, data: Vec<u8>, expect: Option<u64>) -> Result<u64, HostError> {
        if self.writes.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == self.fail_write {
            return Err(HostError::Op(proto::Error::PermissionDenied(path.into())));
        }
        self.inner.write(path, data, expect)
    }
}

impl Fs for Faulty {
    fn create(&self, path: &str) -> Result<(), HostError> {
        self.inner.create_file(path)
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), HostError> {
        self.inner.rename(from, to)
    }
    fn delete(&self, path: &str) -> Result<(), HostError> {
        self.inner.delete(path)
    }
    fn is_dir(&self, path: &str) -> bool {
        self.inner.is_dir(path)
    }
    fn dir_is_empty(&self, path: &str) -> bool {
        self.inner.dir_is_empty(path)
    }
}

#[gpui::test]
fn a_failing_write_takes_back_the_open_editor_and_the_files_already_written(cx: &mut TestAppContext) {
    let (d, edit) = project("rp-rollback", true);
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    open_in(&wb, &d.join("a.rs"), cx);
    propose(&wb, &edit, &Told::default(), cx);
    let v = wait_page(&wb, cx);
    // a.rs is an editor; b.rs is written, then writing c.rs is refused.
    let fs = Faulty { inner: Arc::new(host::local()), fail_write: 1, writes: Default::default() };
    wb.update(cx, |wb, _| wb.set_refactor_fs_for_test(Arc::new(fs)));
    press(&v, cx, &["apply"]);
    wait_phase(&v, "review", cx);
    assert_eq!(editor_text(&wb, &d.join("a.rs"), cx), "fn a() { x(); }\n", "the editor's change was taken back");
    assert!(!dirty(&wb, &d.join("a.rs"), cx));
    assert_eq!(read(&d, "b.rs"), "fn b() { x(); }\n");
    assert_eq!(read(&d, "c.rs"), "fn c() {\n    x();\n    x();\n}\n");
    let report = v.read_with(cx, |v, _| v.report_for_test());
    assert!(report[0].contains("Nothing was changed"), "{report:?}");
    assert!(report.iter().any(|l| l.starts_with("Not changed: c.rs")), "{report:?}");
}

#[gpui::test]
fn a_file_outside_the_folder_is_shown_refused_and_never_written(cx: &mut TestAppContext) {
    let (d, edit) = project("rp-outside", false);
    let outside = root("rp-outside-other");
    let secret = write(&outside, "secret.txt", "abc\n");
    let mut list = edit["documentChanges"].as_array().unwrap().clone();
    list.push(edits(&secret, Value::Null, vec![at(0, 0, 1, "X")]));
    // The same file through `..`.
    list.push(edits(&d.join("..").join(outside.file_name().unwrap()).join("secret.txt"), Value::Null, vec![at(0, 1, 2, "Y")]));
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    propose(&wb, &json!({ "documentChanges": list }), &Told::default(), cx);
    let v = wait_page(&wb, cx);
    let rows = v.read_with(cx, |v, _| v.rows_for_test());
    assert_eq!(rows.iter().filter(|r| r.starts_with("[x] ")).count(), 2, "only a.rs and b.rs can be selected: {rows:?}");
    assert!(rows.iter().filter(|r| r.contains("secret.txt")).count() >= 1);
    press(&v, cx, &["apply"]);
    wait_phase(&v, "done", cx);
    assert_eq!(std::fs::read_to_string(outside.join("secret.txt")).unwrap(), "abc\n");
    assert_eq!(read(&d, "a.rs"), "fn a() { k(); }\n");
    std::fs::remove_dir_all(&outside).ok();
}

#[cfg(unix)]
#[gpui::test]
fn a_link_inside_the_folder_that_points_out_is_refused(cx: &mut TestAppContext) {
    let d = root("rp-link");
    let outside = root("rp-link-outside");
    write(&outside, "keys.txt", "abc\n");
    std::os::unix::fs::symlink(&outside, d.join("linked")).unwrap();
    let a = write(&d, "a.rs", "fn a() { x(); }\n");
    let b = write(&d, "b.rs", "fn b() { x(); }\n");
    let edit = json!({ "documentChanges": [
        edits(&a, Value::Null, vec![at(0, 9, 10, "k")]),
        edits(&b, Value::Null, vec![at(0, 9, 10, "k")]),
        edits(&d.join("linked/keys.txt"), Value::Null, vec![at(0, 0, 1, "X")]),
    ] });
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    propose(&wb, &edit, &Told::default(), cx);
    let v = wait_page(&wb, cx);
    press(&v, cx, &["apply"]);
    wait_phase(&v, "done", cx);
    assert_eq!(std::fs::read_to_string(outside.join("keys.txt")).unwrap(), "abc\n");
    std::fs::remove_dir_all(&outside).ok();
}

// --- File operations ---

#[gpui::test]
fn create_rename_and_delete_are_chosen_confirmed_and_followed_by_the_tabs(cx: &mut TestAppContext) {
    let d = root("rp-ops");
    let a = write(&d, "a.rs", "mod util;\n");
    let util = write(&d, "util.rs", "pub fn x() {}\n");
    let gone = write(&d, "gone.rs", "// bye\n");
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    open_in(&wb, &a, cx);
    open_in(&wb, &util, cx);
    open_in(&wb, &gone, cx);
    let edit = json!({ "documentChanges": [
        edits(&a, Value::Null, vec![at(0, 4, 8, "tools")]),
        { "kind": "rename", "oldUri": format!("file://{}", s(&util)), "newUri": format!("file://{}", s(&d.join("tools.rs"))) },
        { "kind": "create", "uri": format!("file://{}", s(&d.join("sub/neu.rs"))) },
        edits(&d.join("sub/neu.rs"), Value::Null, vec![at(0, 0, 0, "// neu\n")]),
        { "kind": "delete", "uri": format!("file://{}", s(&gone)) },
    ] });
    let told = Told::default();
    propose(&wb, &edit, &told, cx);
    let v = wait_page(&wb, cx);
    let rows = v.read_with(cx, |v, _| v.rows_for_test());
    assert_eq!(rows.iter().filter(|r| r.starts_with("[ ] ")).count(), 4, "the operations and the edit that needs one start unselected: {rows:?}");
    // Choose the rename, the create (which brings its edit) and the delete.
    for name in ["Rename util.rs", "Create sub/neu.rs", "Delete gone.rs"] {
        let at = v.read_with(cx, |v, _| v.rows_for_test()).iter().position(|r| r.contains(name)).unwrap();
        press(&v, cx, &["up"; 30]);
        press(&v, cx, &vec!["down"; at]);
        press(&v, cx, &["space"]);
    }
    assert_eq!(v.read_with(cx, |v, _| v.status_for_test()), "Selected: 2 changes in 2 files, 3 file operations.");
    // The delete asks first; nothing happened yet.
    press(&v, cx, &["apply"]);
    assert_eq!(phase(&v, cx), "confirm");
    assert!(gone.exists() && util.exists() && !d.join("sub").exists());
    press(&v, cx, &["apply"]);
    wait_phase(&v, "done", cx);
    assert!(!gone.exists() && !util.exists());
    assert_eq!(read(&d, "tools.rs"), "pub fn x() {}\n");
    assert_eq!(read(&d, "sub/neu.rs"), "// neu\n");
    assert_eq!(editor_text(&wb, &a, cx), "mod tools;\n", "the open file a.rs took its change in the editor");
    // The renamed file's tab follows it; the deleted file's tab is closed.
    assert!(wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&s(&d.join("tools.rs")), cx)).is_some());
    assert!(wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&s(&gone), cx)).is_none());
    assert_eq!(told.all(), [true]);
    // File operations cannot be undone, so there is no Undo.
    let report = v.read_with(cx, |v, _| v.report_for_test());
    assert!(report.iter().any(|l| l == "Renamed util.rs to tools.rs") && report.iter().any(|l| l == "Deleted gone.rs"), "{report:?}");
    let access = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("refactor", true)));
    assert!(access.refactor.as_ref().is_some_and(|r| !r.view.can_undo), "{:?}", access.refactor.as_ref().map(|r| r.view.can_undo));
}

#[gpui::test]
fn a_rename_onto_a_taken_name_is_refused_in_the_review_and_nothing_moves(cx: &mut TestAppContext) {
    let d = root("rp-taken");
    write(&d, "a.rs", "1\n");
    write(&d, "b.rs", "2\n");
    let edit = json!({ "documentChanges": [{ "kind": "rename", "oldUri": format!("file://{}", s(&d.join("a.rs"))), "newUri": format!("file://{}", s(&d.join("b.rs"))), "options": { "overwrite": true } }] });
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    propose(&wb, &edit, &Told::default(), cx);
    let v = wait_page(&wb, cx);
    assert_eq!(v.read_with(cx, |v, _| v.rows_for_test()), ["Rename a.rs to b.rs"], "no check box: it cannot be chosen");
    press(&v, cx, &["space", "apply"]);
    assert_eq!(phase(&v, cx), "review");
    assert_eq!((read(&d, "a.rs"), read(&d, "b.rs")), ("1\n".into(), "2\n".into()));
}

// --- Keys and the tree ---

#[gpui::test]
fn the_page_takes_keystrokes_through_the_real_bindings(cx: &mut TestAppContext) {
    let (d, edit) = project("rp-keys", true);
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    let told = Told::default();
    propose(&wb, &edit, &told, cx);
    let v = wait_page(&wb, cx);
    // The page has the focus: Down, Space and the modifier with Enter are bound.
    cx.simulate_keystrokes("down down space");
    assert!(v.read_with(cx, |v, _| v.rows_for_test()[1].starts_with("  [ ] ")), "{:?}", v.read_with(cx, |v, _| v.rows_for_test()));
    cx.simulate_keystrokes("cmd-enter");
    wait_phase(&v, "done", cx);
    assert_eq!(read(&d, "a.rs"), "fn a() { x(); }\n", "the file whose change was cleared stays");
    assert_eq!(read(&d, "b.rs"), "fn b() { k(); }\n");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.page_for_test()).is_none());
    assert_eq!(told.all(), [true], "closing a finished page answers nobody");
}

#[gpui::test]
fn the_page_is_drawn_with_rows_for_a_long_refactoring_and_a_narrow_window(cx: &mut TestAppContext) {
    let d = root("rp-draw");
    let mut list = Vec::new();
    for i in 0..400 {
        let p = write(&d, &format!("f{i:03}.rs"), "fn f() { x(); }\n");
        list.push(edits(&p, Value::Null, vec![at(0, 9, 10, "k")]));
    }
    let (wb, cx) = setup(cx);
    open(&wb, &d, cx);
    propose(&wb, &json!({ "documentChanges": list }), &Told::default(), cx);
    let v = wait_page(&wb, cx);
    cx.simulate_resize(gpui::size(gpui::px(420.), gpui::px(600.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("rp-row-0").is_some(), "the first row is drawn");
    assert!(cx.debug_bounds("rp-row-900").is_none(), "rows off screen are not built");
    assert_eq!(v.read_with(cx, |v, _| v.rows_for_test().len()), 400 * 3);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
}
