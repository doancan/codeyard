//! Search Editor tabs in the workbench: retained queries, navigation, rerun, restart, replacement preview, apply, undo,
//! conflicts, stale hosts and folders, large lists, keyboard and accessibility; locally and over SSH.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{Entity, TestAppContext, VisualTestContext};
use host::{Host, HostError};

use crate::qa_workbench_tests::{fresh, open, setup, setup_persistent};
use crate::replace_apply::Store;
use crate::search_doc::{RunState, SearchSpec, Stale};
use crate::search_editor::{Review, SearchEditor, SearchEditorEvent};
use crate::workbench::Workbench;

fn wait(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    let start = std::time::Instant::now();
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(20 * crate::test_sshd::timeout_scale()), "timed out: {what}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
    p
}

fn root(name: &str) -> PathBuf {
    std::fs::canonicalize(fresh(name)).unwrap()
}

fn spec(pattern: &str, regex: bool, replacement: &str) -> SearchSpec {
    SearchSpec { pattern: pattern.into(), regex, case_sensitive: true, replacement: replacement.into(), context: 1, ..Default::default() }
}

fn open_editor(wb: &Entity<Workbench>, s: SearchSpec, cx: &mut VisualTestContext) -> Entity<SearchEditor> {
    let ed = wb.update_in(cx, |wb, window, cx| wb.open_search_editor_for_test(s, None, window, cx)).expect("a folder is open");
    wait(cx, "search editor run", |cx| ed.read_with(cx, |e, _| e.doc.state != RunState::Running && !e.is_running()));
    ed
}

fn emit(ed: &Entity<SearchEditor>, ev: SearchEditorEvent, cx: &mut VisualTestContext) {
    ed.update(cx, |_, cx| cx.emit(ev));
}

fn preview(ed: &Entity<SearchEditor>, cx: &mut VisualTestContext) {
    emit(ed, SearchEditorEvent::Preview, cx);
    wait(cx, "preview", |cx| ed.read_with(cx, |e, _| e.busy.is_none()));
}

fn apply(ed: &Entity<SearchEditor>, cx: &mut VisualTestContext) {
    emit(ed, SearchEditorEvent::Apply, cx);
    wait(cx, "apply", |cx| ed.read_with(cx, |e, _| e.busy.is_none()));
}

fn report(ed: &Entity<SearchEditor>, cx: &mut VisualTestContext) -> Vec<String> {
    ed.read_with(cx, |e, cx| e.status_for_test(cx).into_iter().chain(e.report_lines_for_test()).collect())
}

/// AC1: the tab keeps the query, options and context; navigation opens the right match; rerun refreshes the same model.
#[gpui::test]
fn search_editor_retains_query_navigates_and_reruns(cx: &mut TestAppContext) {
    let dir = root("se-retain");
    write(&dir, "src/a.rs", b"fn a() {}\nlet needle = 1;\nfn b() {}\n");
    write(&dir, "src/b.rs", b"// needle here\n");
    write(&dir, "skip/c.txt", b"needle\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let s = SearchSpec { include: "src/**".into(), whole_word: true, ..spec("needle", false, "") };
    let ed = open_editor(&wb, s.clone(), cx);
    let (doc_spec, prov, files, state) = ed.read_with(cx, |e, _| (e.doc.spec.clone(), e.doc.provenance.clone(), e.doc.files.len(), e.doc.state.clone()));
    assert_eq!(doc_spec, s);
    assert_eq!(prov.root, dir.to_string_lossy());
    assert_eq!(prov.host, "Local");
    assert_eq!((files, state), (2, RunState::Complete), "the include glob was kept");
    // Context lines arrive after the run.
    wait(cx, "context", |cx| ed.read_with(cx, |e, _| e.rows_for_test().iter().any(|r| r == "Line 1: fn a() {}")));
    // F4 opens the first match at its line and column.
    ed.update(cx, |e, cx| e.step_match(1, cx));
    wait(cx, "opened", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some_and(|p| p.ends_with("src/a.rs")));
    let at = wb.read_with(cx, |wb, cx| wb.active_editor_entity().unwrap().read(cx).cursor_position());
    assert_eq!(at.0, 2, "line 2");
    // Rerun after the files change: the same tab, a new generation, fresh results.
    let generation = ed.read_with(cx, |e, _| e.doc.generation);
    write(&dir, "src/d.rs", b"needle needle\n");
    emit(&ed, SearchEditorEvent::Rerun, cx);
    wait(cx, "rerun", |cx| ed.read_with(cx, |e, _| e.doc.state == RunState::Complete && e.doc.generation > generation));
    assert_eq!(ed.read_with(cx, |e, _| e.doc.files.len()), 3);
    assert_eq!(wb.read_with(cx, |wb, _| wb.search_editors_for_test().len()), 1);
}

/// Two retained searches are separate tabs, each with its own query and results.
#[gpui::test]
fn two_search_editors_are_retained_side_by_side(cx: &mut TestAppContext) {
    let dir = root("se-two");
    write(&dir, "a.txt", b"alpha\nbeta\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let a = open_editor(&wb, spec("alpha", false, ""), cx);
    let b = open_editor(&wb, spec("beta", false, ""), cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.search_editors_for_test().len()), 2);
    assert_eq!(wb.read_with(cx, |wb, _| wb.active_search_editor_for_test()), Some(b.clone()));
    wb.update_in(cx, |wb, window, cx| wb.show_search_editor_for_test(a.clone(), window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.active_search_editor_for_test()), Some(a.clone()));
    assert_eq!(a.read_with(cx, |e, _| e.doc.spec.pattern.clone()), "alpha");
    assert_eq!(b.read_with(cx, |e, _| e.doc.files[0].matches[0].line), 2);
    // Both tabs are drawn in the tab bar.
    cx.run_until_parked();
    assert!(cx.debug_bounds("se-row-0").is_some());
    // ⌘W closes the one in front only.
    cx.simulate_keystrokes("cmd-w");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.search_editors_for_test()), vec![b]);
}

/// AC2: restart restores the query and bounded results, marked stale; nothing runs or is replaced until asked.
#[gpui::test]
fn restart_restores_stale_search_editors_and_never_replaces(cx: &mut TestAppContext) {
    let home = root("se-restart-home");
    let dir = root("se-restart");
    let file = write(&dir, "a.txt", b"old old\n");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    let ed = open_editor(&wb, spec("old", false, "new"), cx);
    assert_eq!(ed.read_with(cx, |e, _| e.doc.match_count()), 2);
    let wb = crate::qa_workbench_tests::restart_with(wb, cx, &home, crate::settings::Startup::Continue);
    wait(cx, "restored", |cx| wb.read_with(cx, |wb, _| !wb.search_editors_for_test().is_empty()));
    let ed = wb.read_with(cx, |wb, _| wb.search_editors_for_test()[0].clone());
    let (pattern, replacement, stale, state, running) =
        ed.read_with(cx, |e, _| (e.doc.spec.pattern.clone(), e.doc.spec.replacement.clone(), e.doc.stale, e.doc.state.clone(), e.is_running()));
    assert_eq!((pattern.as_str(), replacement.as_str()), ("old", "new"));
    assert_eq!(stale, Some(Stale::Restored));
    assert_eq!(state, RunState::Complete, "the saved run state is kept, the stale mark is separate");
    assert!(!running, "nothing runs on restore");
    assert!(report(&ed, cx).iter().any(|l| l.contains("Restored from the last session")));
    // A preview of restored results can be looked at, but Apply is refused.
    preview(&ed, cx);
    assert!(ed.read_with(cx, |e, _| e.has_plan()));
    assert_eq!(ed.read_with(cx, |e, _| e.apply_blocker()), Some("Search results are incomplete; rerun before replacing.".into()));
    apply(&ed, cx);
    assert_eq!(std::fs::read(&file).unwrap(), b"old old\n", "never replaced from stale results");
    // Rerun makes them current.
    emit(&ed, SearchEditorEvent::Rerun, cx);
    wait(cx, "rerun", |cx| ed.read_with(cx, |e, _| e.doc.state == RunState::Complete && e.doc.stale.is_none()));
    // A file that vanished is simply not found again.
    std::fs::remove_file(&file).unwrap();
    emit(&ed, SearchEditorEvent::Rerun, cx);
    wait(cx, "rerun 2", |cx| ed.read_with(cx, |e, _| e.doc.state == RunState::Complete && e.doc.files.is_empty()));
}

/// AC3/AC4: the preview shows exact diffs and counts and changes nothing; Apply writes only the selected changes, with
/// captures, CRLF and a byte-order mark preserved.
#[gpui::test]
fn preview_then_apply_selected_changes_exactly(cx: &mut TestAppContext) {
    let dir = root("se-apply");
    let a = write(&dir, "a.txt", b"x=1\r\ny=2\r\n");
    let b = write(&dir, "b.txt", b"\xEF\xBB\xBFz=3\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let ed = open_editor(&wb, spec(r"(\w)=(\d)", true, "$2:$1"), cx);
    preview(&ed, cx);
    assert_eq!(ed.read_with(cx, |e, _| e.apply_label_for_test()), "Apply 3 changes in 2 files");
    let rows = ed.read_with(cx, |e, _| e.rows_for_test());
    assert!(rows.contains(&"Line 1: x=1 → 1:x".to_string()), "{rows:?}");
    assert_eq!(std::fs::read(&a).unwrap(), b"x=1\r\ny=2\r\n", "nothing changes before Apply");
    assert_eq!(std::fs::read(&b).unwrap(), b"\xEF\xBB\xBFz=3\n");
    // Deselect the second change of a.txt with the keyboard: rows are file, old, new, old, new…
    ed.update_in(cx, |e, window, cx| e.focus_list(window, cx));
    cx.simulate_keystrokes("down down down space");
    assert_eq!(ed.read_with(cx, |e, _| e.apply_label_for_test()), "Apply 2 changes in 2 files");
    apply(&ed, cx);
    assert_eq!(std::fs::read(&a).unwrap(), b"1:x\r\ny=2\r\n");
    assert_eq!(std::fs::read(&b).unwrap(), b"\xEF\xBB\xBF3:z\n");
    let lines = report(&ed, cx);
    assert!(lines.iter().any(|l| l.starts_with("Changed 2 changes in")), "{lines:?}");
    assert_eq!(ed.read_with(cx, |e, _| e.doc.stale), Some(Stale::Replaced), "results no longer describe the files");
}

/// AC5: an open dirty buffer is the source of truth, and edits made after the preview are conflicts, never overwritten.
#[gpui::test]
fn dirty_buffers_win_and_later_edits_are_conflicts(cx: &mut TestAppContext) {
    let dir = root("se-dirty");
    let open_file = write(&dir, "open.txt", b"keep\n");
    let closed = write(&dir, "closed.txt", b"token\n");
    let edited_later = write(&dir, "later.txt", b"token\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    // The disk of open.txt has no match yet; its unsaved text does.
    let path = open_file.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(path, window, cx));
    cx.run_until_parked();
    let buf = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    std::fs::write(&open_file, b"token on disk\n").unwrap();
    buf.update(cx, |e, cx| {
        e.apply_buffer_edits(|_| vec![(0..4, "token unsaved".to_string())], cx);
    });
    let ed = open_editor(&wb, spec("token", false, "T"), cx);
    preview(&ed, cx);
    let rows = ed.read_with(cx, |e, _| e.rows_for_test());
    assert!(rows.iter().any(|r| r.contains("token unsaved")), "the buffer's text, not the disk's: {rows:?}");
    // After the preview: someone edits a closed file.
    std::fs::write(&edited_later, b"token changed elsewhere\n").unwrap();
    apply(&ed, cx);
    assert_eq!(std::fs::read(&closed).unwrap(), b"T\n");
    assert_eq!(std::fs::read(&edited_later).unwrap(), b"token changed elsewhere\n", "never overwritten");
    assert_eq!(std::fs::read(&open_file).unwrap(), b"token on disk\n", "open files are edited in the buffer, not saved");
    assert_eq!(buf.read_with(cx, |e, _| e.buffer_text()), "T unsaved\n");
    let lines = report(&ed, cx);
    assert!(lines.iter().any(|l| l == "File changed since preview: later.txt"), "{lines:?}");

    // A buffer typed in after its preview is a conflict too, and keeps what was typed.
    let second = write(&dir, "second.txt", b"zz\n");
    let path = second.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(path, window, cx));
    cx.run_until_parked();
    let buf2 = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let ed2 = open_editor(&wb, spec("zz", false, "Z"), cx);
    preview(&ed2, cx);
    buf2.update(cx, |e, cx| {
        e.apply_buffer_edits(|_| vec![(0..0, "typed ".to_string())], cx);
    });
    apply(&ed2, cx);
    assert_eq!(buf2.read_with(cx, |e, _| e.buffer_text()), "typed zz\n");
    assert_eq!(std::fs::read(&second).unwrap(), b"zz\n");
    assert!(report(&ed2, cx).iter().any(|l| l == "File changed since preview: second.txt"));
}

/// AC6: one Undo restores the batch; a file edited after the replacement is an undo conflict and keeps the edit.
#[gpui::test]
fn batch_undo_restores_and_reports_undo_conflicts(cx: &mut TestAppContext) {
    let dir = root("se-undo");
    let a = write(&dir, "a.txt", b"v1 a\r\n");
    let b = write(&dir, "b.txt", b"v1 b\n");
    let o = write(&dir, "o.txt", b"v1 o\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let path = o.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(path, window, cx));
    cx.run_until_parked();
    let buf = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let ed = open_editor(&wb, spec("v1", false, "v2"), cx);
    preview(&ed, cx);
    apply(&ed, cx);
    assert_eq!(std::fs::read(&a).unwrap(), b"v2 a\r\n");
    assert_eq!(buf.read_with(cx, |e, _| e.buffer_text()), "v2 o\n");
    assert!(wb.read_with(cx, |wb, _| wb.has_replace_undo_for_test()));
    std::fs::write(&b, b"edited after\n").unwrap();
    emit(&ed, SearchEditorEvent::Undo, cx);
    wait(cx, "undo", |cx| ed.read_with(cx, |e, _| e.busy.is_none() && e.report_lines_for_test().iter().any(|l| l.contains("Restored"))));
    assert_eq!(std::fs::read(&a).unwrap(), b"v1 a\r\n");
    assert_eq!(std::fs::read(&b).unwrap(), b"edited after\n", "the later edit is not lost");
    assert_eq!(buf.read_with(cx, |e, _| e.buffer_text()), "v1 o\n");
    let lines = report(&ed, cx);
    assert!(lines.iter().any(|l| l == "Undo conflict: b.txt changed after the replacement and was left as it is."), "{lines:?}");
    assert!(!wb.read_with(cx, |wb, _| wb.has_replace_undo_for_test()), "undo is one-shot");
    // Undo from the palette with nothing to undo says so.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Search: Undo Last Replace", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()), Some("There is no replacement to undo.".into()));
}

/// AC7: truncated or cancelled results block Apply, and a search from another folder cannot be opened, previewed or rerun.
#[gpui::test]
fn incomplete_and_foreign_results_stop_unsafe_work(cx: &mut TestAppContext) {
    let dir = root("se-incomplete");
    for i in 0..5 {
        write(&dir, &format!("f{i}.txt"), b"hit hit\n");
    }
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().search_max_results = 3);
    open(&wb, &dir, cx);
    let ed = open_editor(&wb, spec("hit", false, "x"), cx);
    assert_eq!(ed.read_with(cx, |e, _| e.doc.state.clone()), RunState::Truncated);
    assert!(report(&ed, cx).iter().any(|l| l.contains("stopped at the result limit")));
    preview(&ed, cx);
    apply(&ed, cx);
    for i in 0..5 {
        assert_eq!(std::fs::read(dir.join(format!("f{i}.txt"))).unwrap(), b"hit hit\n", "blocked: nothing written");
    }
    assert_eq!(ed.read_with(cx, |e, _| e.apply_blocker()), Some("Search results are incomplete; rerun before replacing.".into()));

    // Cancelled.
    ed.update(cx, |e, cx| {
        e.doc.cancel();
        e.doc.state = RunState::Cancelled;
        cx.notify();
    });
    assert!(report(&ed, cx).iter().any(|l| l.contains("cancelled")));

    // Another folder: the search is stale, opening and rerunning are refused.
    ed.update(cx, |e, _| e.doc.provenance.root = "/elsewhere".into());
    wb.update_in(cx, |wb, window, cx| wb.show_search_editor_for_test(ed.clone(), window, cx));
    assert_eq!(ed.read_with(cx, |e, _| e.doc.stale), Some(Stale::Workspace));
    emit(&ed, SearchEditorEvent::Rerun, cx);
    cx.run_until_parked();
    assert!(report(&ed, cx).iter().any(|l| l.contains("Open this search's folder")));
    let before = wb.read_with(cx, |wb, cx| wb.active_path(cx));
    emit(&ed, SearchEditorEvent::Open { path: dir.join("f0.txt").to_string_lossy().into_owned(), line: 1, column: 0, len: 3 }, cx);
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), before, "a match from another folder is not opened here");
}

/// AC7: a preview reply for an older run is dropped (rerun while the preview is being made).
#[gpui::test]
fn a_delayed_preview_for_an_older_run_is_dropped(cx: &mut TestAppContext) {
    let dir = root("se-delayed");
    write(&dir, "a.txt", b"q\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let ed = open_editor(&wb, spec("q", false, "Q"), cx);
    emit(&ed, SearchEditorEvent::Preview, cx);
    // Before the preview task finishes, the search runs again.
    let host = wb.read_with(cx, |wb, _| wb.host_for_test());
    ed.update(cx, |e, cx| e.run(host, cx));
    wait(cx, "settled", |cx| ed.read_with(cx, |e, _| e.busy.is_none() && !e.is_running()));
    assert!(!ed.read_with(cx, |e, _| e.has_plan()), "the plan of the older run was not shown");
    assert_eq!(std::fs::read(dir.join("a.txt")).unwrap(), b"q\n");
}

/// A store that fails the second write: the report says exactly what changed and what did not.
struct FailSecond {
    inner: Arc<dyn Host>,
    n: AtomicUsize,
}

impl Store for FailSecond {
    fn read(&self, path: &str) -> Result<(Vec<u8>, u64), HostError> {
        self.inner.read(path)
    }
    fn write(&self, path: &str, data: Vec<u8>, expect: Option<u64>) -> Result<u64, HostError> {
        if self.n.fetch_add(1, Ordering::SeqCst) == 1 {
            return Err(HostError::Op(proto::Error::PermissionDenied(path.into())));
        }
        self.inner.write(path, data, expect)
    }
    fn exists(&self, path: &str) -> Result<bool, HostError> {
        Store::exists(&self.inner, path)
    }
}

/// AC7: a failure part way reports per-file outcomes; the files that failed keep their bytes and undo covers the rest.
#[gpui::test]
fn a_failure_mid_batch_reports_each_file(cx: &mut TestAppContext) {
    let dir = root("se-partial");
    let files: Vec<PathBuf> = (0..3).map(|i| write(&dir, &format!("f{i}.txt"), b"w\n")).collect();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let host = wb.read_with(cx, |wb, _| wb.host_for_test());
    wb.update(cx, |wb, _| wb.set_replace_store_for_test(Arc::new(FailSecond { inner: host, n: AtomicUsize::new(0) })));
    let ed = open_editor(&wb, spec("w", false, "W"), cx);
    preview(&ed, cx);
    apply(&ed, cx);
    let bytes: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();
    assert_eq!(bytes, vec![b"W\n".to_vec(), b"w\n".to_vec(), b"W\n".to_vec()]);
    let lines = report(&ed, cx);
    assert!(lines.iter().any(|l| l.starts_with("Not changed: f1.txt")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("Changed 2 changes in") && l.contains("f0.txt") && l.contains("f2.txt")), "{lines:?}");
    emit(&ed, SearchEditorEvent::Undo, cx);
    wait(cx, "undo", |cx| ed.read_with(cx, |e, _| e.busy.is_none() && e.report_lines_for_test().iter().any(|l| l.contains("Restored"))));
    assert!(files.iter().all(|f| std::fs::read(f).unwrap() == b"w\n"));
}

/// AC8: thousands of results stay a virtualized list: few rows are drawn, the keyboard moves through them and Enter opens.
#[gpui::test]
fn large_results_are_virtualized_and_keyboard_navigable(cx: &mut TestAppContext) {
    let dir = root("se-large");
    let body: String = (0..200).map(|i| format!("hit {i}\n")).collect();
    for i in 0..20 {
        write(&dir, &format!("f{i:02}.txt"), body.as_bytes());
    }
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().search_max_results = 10_000);
    open(&wb, &dir, cx);
    let ed = open_editor(&wb, SearchSpec { context: 0, ..spec("hit", false, "") }, cx);
    assert_eq!(ed.read_with(cx, |e, _| e.doc.match_count()), 4000);
    let total = ed.read_with(cx, |e, _| e.rows_for_test().len());
    assert_eq!(total, 4020);
    cx.run_until_parked();
    assert!(cx.debug_bounds("se-row-0").is_some());
    assert!(cx.debug_bounds("se-row-3000").is_none(), "rows off screen are not built");
    ed.update_in(cx, |e, window, cx| e.focus_list(window, cx));
    cx.simulate_keystrokes("down down");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_for_test()), Some(2));
    cx.simulate_keystrokes("enter");
    wait(cx, "opened", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some_and(|p| p.ends_with("f00.txt")));
    // The accessibility tree exposes a bounded window of rows with the full count.
    wb.update_in(cx, |wb, window, cx| {
        let ed = wb.search_editors_for_test()[0].clone();
        wb.show_search_editor_for_test(ed, window, cx);
    });
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let se = snap.search_editor.as_ref().expect("the Search Editor is in the tree");
    assert_eq!(se.view.rows_total, 4020);
    assert!(se.view.rows.len() <= 200);
    let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
    assert!(tree.state().focus().is_some());
}

/// The sidebar's Replace All opens a review instead of writing: nothing changes until Apply in the Search Editor.
#[gpui::test]
fn sidebar_replace_all_opens_a_review(cx: &mut TestAppContext) {
    let dir = root("se-sidebar");
    let a = write(&dir, "a.txt", b"foo\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    cx.simulate_keystrokes("cmd-shift-f");
    cx.simulate_input("foo");
    search.update(cx, |s, cx| s.set_replacement("bar", cx));
    wait(cx, "sidebar search", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        search.read_with(cx, |s, _| !s.is_searching() && s.summary_for_test().is_some())
    });
    search.update(cx, |s, cx| s.request_replace_all(cx));
    wait(cx, "review", |cx| wb.read_with(cx, |wb, cx| wb.active_search_editor_for_test().is_some_and(|e| e.read(cx).has_plan())));
    assert!(cx.pending_prompt().is_none(), "no blind confirmation");
    assert_eq!(std::fs::read(&a).unwrap(), b"foo\n");
    let ed = wb.read_with(cx, |wb, _| wb.active_search_editor_for_test()).unwrap();
    assert_eq!(ed.read_with(cx, |e, _| e.apply_label_for_test()), "Apply 1 changes in 1 files");
    apply(&ed, cx);
    assert_eq!(std::fs::read(&a).unwrap(), b"bar\n");
}

/// Replace in one file from the sidebar: the review selects only that file's changes.
#[gpui::test]
fn per_file_review_selects_only_that_file(cx: &mut TestAppContext) {
    let dir = root("se-only");
    let a = write(&dir, "a.txt", b"foo\n");
    let b = write(&dir, "b.txt", b"foo foo\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let only = Some(Review::Only(b.to_string_lossy().into_owned()));
    let ed = wb.update_in(cx, |wb, window, cx| wb.open_search_editor_for_test(spec("foo", false, "bar"), only, window, cx)).unwrap();
    wait(cx, "review", |cx| ed.read_with(cx, |e, _| e.has_plan()));
    assert_eq!(ed.read_with(cx, |e, _| e.apply_label_for_test()), "Apply 2 changes in 1 files");
    apply(&ed, cx);
    assert_eq!(std::fs::read(&a).unwrap(), b"foo\n");
    assert_eq!(std::fs::read(&b).unwrap(), b"bar bar\n");
}

/// The same flow over ssh: the agent searches, reads and writes; preview, apply and undo keep exact bytes.
#[gpui::test]
fn search_editor_replaces_and_undoes_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("search-editor-remote") else { return };
    let dir = root("se-remote");
    let a = write(&dir, "a.txt", b"one\r\ntwo one\r\n");
    let (wb, cx) = crate::qa_integration_tests::ssh_workbench(cx, &dir);
    let ed = open_editor(&wb, spec("one", false, "1"), cx);
    assert_eq!(ed.read_with(cx, |e, _| e.doc.match_count()), 2);
    assert_ne!(ed.read_with(cx, |e, _| e.doc.provenance.host.clone()), "Local");
    preview(&ed, cx);
    assert_eq!(std::fs::read(&a).unwrap(), b"one\r\ntwo one\r\n");
    apply(&ed, cx);
    assert_eq!(std::fs::read(&a).unwrap(), b"1\r\ntwo 1\r\n");
    std::fs::write(dir.join("b.txt"), b"later\n").unwrap();
    emit(&ed, SearchEditorEvent::Undo, cx);
    wait(cx, "undo", |cx| ed.read_with(cx, |e, _| e.busy.is_none() && e.report_lines_for_test().iter().any(|l| l.contains("Restored"))));
    assert_eq!(std::fs::read(&a).unwrap(), b"one\r\ntwo one\r\n");
    // An external edit between preview and apply is a conflict over ssh too.
    emit(&ed, SearchEditorEvent::Rerun, cx);
    wait(cx, "rerun", |cx| ed.read_with(cx, |e, _| e.doc.state == RunState::Complete && e.doc.stale.is_none()));
    preview(&ed, cx);
    std::fs::write(&a, b"one changed\n").unwrap();
    apply(&ed, cx);
    assert_eq!(std::fs::read(&a).unwrap(), b"one changed\n");
    assert!(report(&ed, cx).iter().any(|l| l == "File changed since preview: a.txt"));
}

/// AC6: a replaced file that was opened and edited (unsaved) before Undo is an undo conflict; the disk keeps the
/// replacement and the buffer keeps the unsaved edit.
#[gpui::test]
fn undo_skips_a_file_reopened_with_unsaved_edits(cx: &mut TestAppContext) {
    let dir = root("se-undo-dirty");
    let a = write(&dir, "a.txt", b"v1\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let ed = open_editor(&wb, spec("v1", false, "v2"), cx);
    preview(&ed, cx);
    apply(&ed, cx);
    assert_eq!(std::fs::read(&a).unwrap(), b"v2\n");
    let path = a.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(path, window, cx));
    cx.run_until_parked();
    let buf = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    buf.update(cx, |e, cx| {
        e.apply_buffer_edits(|_| vec![(0..0, "mine ".to_string())], cx);
    });
    emit(&ed, SearchEditorEvent::Undo, cx);
    wait(cx, "undo", |cx| ed.read_with(cx, |e, _| e.busy.is_none() && e.report_lines_for_test().iter().any(|l| l.starts_with("Undo conflict"))));
    assert_eq!(std::fs::read(&a).unwrap(), b"v2\n");
    assert_eq!(buf.read_with(cx, |e, _| e.buffer_text()), "mine v2\n");
}

/// Non-UTF-8 files keep their encoding; text the encoding cannot hold is refused for that file; mixed line endings and
/// a missing final newline survive untouched.
#[gpui::test]
fn encodings_and_line_endings_survive_apply(cx: &mut TestAppContext) {
    let dir = root("se-encodings");
    // "Işık mark\r\n" in Windows-1254 (ş=0xFE, ı=0xFD).
    let turkish = write(&dir, "tr.txt", b"I\xfe\xfdk mark\r\n");
    let mixed = write(&dir, "mixed.txt", b"mark\nx\r\nmark\ry mark");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let ed = open_editor(&wb, spec("mark", false, "ğ"), cx);
    assert_eq!(ed.read_with(cx, |e, _| e.doc.files.len()), 2);
    preview(&ed, cx);
    apply(&ed, cx);
    assert_eq!(std::fs::read(&turkish).unwrap(), b"I\xfe\xfdk \xf0\r\n", "written back in Windows-1254");
    assert_eq!(std::fs::read(&mixed).unwrap(), "ğ\nx\r\nğ\ry ğ".as_bytes(), "every ending as it was");
    // An emoji cannot be stored in Windows-1254: that file is refused with a reason, the other is changed.
    std::fs::write(&turkish, b"I\xfe\xfdk mark\r\n").unwrap();
    std::fs::write(&mixed, b"mark\n").unwrap();
    ed.update(cx, |e, cx| e.set_fields_for_test("mark", "😀", false, cx));
    emit(&ed, SearchEditorEvent::Rerun, cx);
    wait(cx, "rerun", |cx| ed.read_with(cx, |e, _| e.doc.state == RunState::Complete && e.doc.stale.is_none()));
    preview(&ed, cx);
    assert_eq!(ed.read_with(cx, |e, _| e.apply_label_for_test()), "Apply 1 changes in 1 files", "the refused file is not selectable");
    apply(&ed, cx);
    assert_eq!(std::fs::read(&turkish).unwrap(), b"I\xfe\xfdk mark\r\n");
    assert_eq!(std::fs::read(&mixed).unwrap(), "😀\n".as_bytes());
}
