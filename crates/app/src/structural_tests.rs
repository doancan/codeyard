//! Structural search and replace in the app: the matching engine's glue (previews, edits), the Search panel in structural
//! mode, and Replace through the workbench with open buffers, closed files, line endings, prompts and cancelling.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui::{Entity, TestAppContext, VisualTestContext};
use syntax::structural as st;

use crate::qa_workbench_tests::{fresh, open, setup};
use crate::search::SearchPanel;
use crate::structural as engine;
use crate::workbench::Workbench;

fn family(id: &str) -> &'static st::Family {
    st::family(id).unwrap()
}

fn pattern(id: &str, p: &str) -> st::Pattern {
    st::compile(family(id).language(), p).unwrap()
}

/// Real threads are involved (the agent, the reader): loop while waiting.
fn wait(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    let start = std::time::Instant::now();
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(10 * crate::test_sshd::timeout_scale()), "timed out: {what}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
    p
}

/// A path in the form the workbench keeps (`ui::local_path`: `/` on Windows too), so open buffers are found by it.
fn path_str(p: &Path) -> String {
    crate::ui::local_path(p)
}

/// Put the panel in structural mode with this pattern and wait until the search is over.
fn search(panel: &Entity<SearchPanel>, lang: &str, text: &str, cx: &mut VisualTestContext) {
    panel.update_in(cx, |s, window, cx| {
        s.open_structural(Some(family(lang)), None, false, window, cx);
        s.set_pattern_for_test(text, cx);
        s.rerun(cx);
    });
    wait(cx, "structural search", |cx| panel.read_with(cx, |s, _| s.summary_for_test().is_some() || s.structural_for_test().1.is_some()));
}

fn find_hits(panel: &Entity<SearchPanel>, cx: &mut VisualTestContext) -> Vec<(String, Vec<u32>)> {
    panel.read_with(cx, |s, _| s.result_summary())
}

// --- engine glue ---

#[test]
fn rows_carry_line_column_and_a_preview_without_indentation() {
    let text = "fn f() {\n    let ç = foo(1);\n\n    foo(2, foo(3));\n}\n";
    let rows = engine::line_matches(&pattern("Rust", "foo($$$A)"), family("Rust").language(), text).unwrap();
    let got: Vec<(u32, u32, &str)> = rows.iter().map(|r| (r.line, r.column, r.preview.as_str())).collect();
    assert_eq!(got, [(2, 12, "let ç = foo(1);"), (4, 4, "foo(2, foo(3));"), (4, 11, "foo(2, foo(3));")]);
    // The highlighted range is inside the preview, in bytes.
    let r = &rows[0];
    assert_eq!(&r.preview[r.ranges[0].0 as usize..r.ranges[0].1 as usize], "foo(1)");
}

#[test]
fn a_match_over_several_lines_is_listed_on_its_first_line() {
    let text = "fn f() {\n    if a {\n        b();\n    }\n}\n";
    let rows = engine::line_matches(&pattern("Rust", "if $C { $$$B }"), family("Rust").language(), text).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].line, rows[0].preview.as_str()), (2, "if a {"));
    let r = &rows[0].ranges[0];
    assert_eq!(&rows[0].preview[r.0 as usize..r.1 as usize], "if a {");
}

#[test]
fn editor_text_drops_the_bom_and_unifies_line_ends() {
    assert_eq!(engine::editor_text(b"\xEF\xBB\xBFa\r\nb\r\n").unwrap(), "a\nb\n");
    assert!(engine::editor_text(b"\xFF\xFE").is_none());
}

#[test]
fn replacements_leave_inner_matches_for_the_next_run_and_report_unparsable_results() {
    let lang = family("Rust").language();
    let src = "fn f() { id(id(1)); id(2); }";
    let p = pattern("Rust", "id($X)");
    let edits = engine::replacements(&p, lang, src, "wrap($X)");
    assert_eq!(engine::apply(src, &edits), "fn f() { wrap(id(1)); wrap(2); }");
    assert_eq!(engine::result_parses(&p, lang, src, "wrap($X)"), Some(true));
    assert_eq!(engine::result_parses(&p, lang, src, "wrap($X"), Some(false));
    assert_eq!(engine::result_parses(&pattern("Rust", "nothing($X)"), lang, src, "x"), None);
}

#[test]
fn buffer_edits_count_characters_not_bytes() {
    let b = buffer::Buffer::from_str("fn f() { let ğ = 1; foo(ğ); }");
    let edits = engine::buffer_edits(&pattern("Rust", "foo($X)"), family("Rust").language(), &b, "bar($X)");
    assert_eq!(edits, [(20..26, "bar(ğ)".to_string())]);
}

#[test]
fn the_narrowing_search_asks_for_the_longest_token_in_files_of_the_language() {
    let p = pattern("Rust", "$R.unwrap_or($X.clone())");
    let q = engine::narrowing_query("/w", &p, family("Rust"), vec![], vec!["target".into()]);
    assert_eq!(
        (q.pattern.as_str(), q.regex, q.case_sensitive, q.include.as_slice(), q.exclude.as_slice()),
        ("unwrap_or", false, true, &["*.rs".to_string()][..], &["target".to_string()][..])
    );
    let ts = engine::narrowing_query("/w", &pattern("TypeScript", "f($A)"), family("TypeScript"), vec![], vec![]);
    assert_eq!(ts.include, ["*.ts", "*.mts", "*.cts", "*.tsx"]);
    let own = engine::narrowing_query("/w", &p, family("Rust"), vec!["src/**".into()], vec![]);
    assert_eq!(own.include, ["src/**"]);
}

#[gpui::test]
fn a_scan_that_is_cancelled_before_it_starts_reports_nothing(_cx: &mut TestAppContext) {
    let dir = fresh("structural-cancel-scan");
    write(&dir, "a.rs", b"fn f() { foo(1); }\n");
    let host: Arc<dyn host::Host> = Arc::new(host::local());
    let p = Arc::new(pattern("Rust", "foo($X)"));
    let q = engine::narrowing_query(&path_str(&dir), &p, family("Rust"), vec![], vec![]);
    let stream = host.search(q).unwrap();
    let job = engine::Job { family: family("Rust"), pattern: p, max_files: 100, open: Default::default() };
    let roots = vec![engine::Root { label: None, dir: path_str(&dir), stream }];
    let cancel = AtomicBool::new(true);
    let mut events = 0;
    engine::scan(host, job, roots, &cancel, |_| events += 1);
    assert_eq!(events, 0);
}

// --- the panel ---

#[gpui::test]
fn structural_search_reads_unsaved_buffers_and_skips_what_it_cannot_parse(cx: &mut TestAppContext) {
    let dir = fresh("structural-search");
    write(&dir, "closed.rs", b"fn a(v: Option<String>, w: String) -> String { v.unwrap_or(w.clone()) }\n");
    write(&dir, "saved.rs", b"fn b() {}\n"); // no match on disk, but the open buffer has one
    write(&dir, "broken.rs", b"fn c( { x.unwrap_or(y.clone()) \n");
    write(&dir, "notes.py", b"x.unwrap_or(y.clone())\n"); // another language: never looked at
    write(&dir, "blob.rs", &[b"fn d() { x.unwrap_or(y.clone()); }\n".as_slice(), &vec![b' '; engine::MAX_FILE_BYTES]].concat());
    write(&dir, "bin.rs", b"\0\0 unwrap_or\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(path_str(&dir.join("saved.rs")), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.replace_all_text_for_test("fn b(o: Option<i32>, p: i32) -> i32 { o.unwrap_or(p.clone()) }\n", cx));
    let panel = wb.read_with(cx, |wb, _| wb.search_panel());

    search(&panel, "Rust", "$R.unwrap_or($X.clone())", cx);
    assert_eq!(find_hits(&panel, cx), [("closed.rs".to_string(), vec![1]), ("saved.rs".to_string(), vec![1])]);
    let (on, error, totals) = panel.read_with(cx, |s, _| s.structural_for_test());
    assert!(on && error.is_none());
    assert_eq!((totals.files, totals.matches, totals.skipped_parse), (2, 2, 1));
    // The 2 MB file is counted apart from the parse errors; the NUL file never gets that far (it is not a candidate).
    assert_eq!(totals.skipped_other, 1, "{totals:?}");
    let note = panel.read_with(cx, |s, cx| s.status_line_for_test(cx)).unwrap();
    assert!(note.contains("1 file skipped, parse errors"), "{note}");
}

#[gpui::test]
fn a_pattern_that_does_not_parse_shows_why_and_does_not_run(cx: &mut TestAppContext) {
    let dir = fresh("structural-bad-pattern");
    write(&dir, "a.rs", b"fn f() { foo(1); }\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let panel = wb.read_with(cx, |wb, _| wb.search_panel());
    search(&panel, "Rust", "foo(", cx);
    let (_, error, totals) = panel.read_with(cx, |s, _| s.structural_for_test());
    let error = error.expect("an error under the field");
    assert!(error.starts_with("The pattern does not parse: ") && error.contains("line 1, column"), "{error}");
    assert_eq!(totals, engine::Totals::default());
    assert!(find_hits(&panel, cx).is_empty());
    // The same words in Turkish.
    let _g = crate::i18n::LangGuard::set("tr");
    let e = st::compile(family("Rust").language(), "a(); b();").unwrap_err();
    assert_eq!(crate::search::describe_pattern_error("a(); b();", &e), "Kalıp tek bir ifade ya da deyimdir, birkaç tane olamaz.");
}

#[gpui::test]
fn the_file_limit_setting_cuts_the_candidates_and_says_so(cx: &mut TestAppContext) {
    let dir = fresh("structural-max-files");
    for i in 0..4 {
        write(&dir, &format!("f{i}.rs"), b"fn f() { foo(1); }\n");
    }
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().search_structural_max_files = 2);
    open(&wb, &dir, cx);
    let panel = wb.read_with(cx, |wb, _| wb.search_panel());
    search(&panel, "Rust", "foo($X)", cx);
    let totals = panel.read_with(cx, |s, _| s.structural_for_test().2);
    assert!(totals.candidates_cut);
    assert_eq!(find_hits(&panel, cx).len(), 2);
    let note = panel.read_with(cx, |s, cx| s.status_line_for_test(cx)).unwrap();
    assert!(note.contains("only the first 2 files were looked at (search.structural.maxFiles)"), "{note}");
}

#[gpui::test]
fn changing_the_pattern_cancels_the_run_before_it(cx: &mut TestAppContext) {
    let dir = fresh("structural-cancel");
    for i in 0..30 {
        write(&dir, &format!("f{i}.rs"), format!("fn f{i}() {{ foo({i}); bar({i}); }}\n").as_bytes());
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let panel = wb.read_with(cx, |wb, _| wb.search_panel());
    panel.update_in(cx, |s, window, cx| {
        s.open_structural(Some(family("Rust")), None, false, window, cx);
        s.set_pattern_for_test("foo($X)", cx);
        s.rerun(cx);
        // Straight away another pattern: the first run must never add its rows.
        s.set_pattern_for_test("bar($X)", cx);
        s.rerun(cx);
    });
    wait(cx, "second run", |cx| panel.read_with(cx, |s, _| s.summary_for_test().is_some()));
    cx.run_until_parked();
    let hits = find_hits(&panel, cx);
    assert_eq!(hits.len(), 30);
    panel.update(cx, |s, cx| s.set_pattern_for_test("", cx));
    wait(cx, "cleared", |cx| panel.read_with(cx, |s, _| !s.is_searching()));
}

// --- replace ---

fn open_dirty(wb: &Entity<Workbench>, path: &Path, text: &str, cx: &mut VisualTestContext) -> Entity<crate::editor::EditorView> {
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(path_str(path), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.replace_all_text_for_test(text, cx));
    ed
}

fn disk(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap()
}

#[gpui::test]
fn replace_edits_open_buffers_and_closed_files_and_keeps_line_endings(cx: &mut TestAppContext) {
    let dir = fresh("structural-replace");
    let crlf = write(&dir, "crlf.rs", b"fn a() {\r\n    x.unwrap_or(1.clone());\r\n}\r\n");
    let mixed = write(&dir, "mixed.rs", b"fn b() {\n    y.unwrap_or(2.clone());\r\n    z();\r\n}\n");
    let bom = write(&dir, "bom.rs", b"\xEF\xBB\xBFfn c() { w.unwrap_or(\xC3\xA7a\xC4\x9F.clone()); }\n");
    let open_file = write(&dir, "open.rs", b"fn d() {}\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let ed = open_dirty(&wb, &open_file, "fn d() {\n    q.unwrap_or(3.clone());\n    q.unwrap_or(4);\n}\n", cx);
    let files = vec![path_str(&crlf), path_str(&mixed), path_str(&bom), path_str(&open_file)];
    let p = Arc::new(pattern("Rust", "$R.unwrap_or($X.clone())"));
    wb.update(cx, |wb, cx| wb.replace_structural_now(files, family("Rust"), p, "$R.unwrap_or_else(|| $X.clone())".into(), cx));
    wait(cx, "files written", |_| disk(&bom).windows(14).any(|w| w == b"unwrap_or_else"));
    assert_eq!(disk(&crlf), b"fn a() {\r\n    x.unwrap_or_else(|| 1.clone());\r\n}\r\n");
    assert_eq!(disk(&mixed), b"fn b() {\n    y.unwrap_or_else(|| 2.clone());\r\n    z();\r\n}\n", "untouched lines keep their own endings");
    assert_eq!(disk(&bom), "\u{feff}fn c() { w.unwrap_or_else(|| \u{e7}a\u{11f}.clone()); }\n".as_bytes());
    // The open buffer changed, was not saved, and one undo restores it.
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "fn d() {\n    q.unwrap_or_else(|| 3.clone());\n    q.unwrap_or(4);\n}\n");
    assert!(ed.read_with(cx, |e, _| e.is_dirty()));
    assert_eq!(disk(&open_file), b"fn d() {}\n");
    ed.update_in(cx, |e, window, cx| {
        window.focus(&gpui::Focusable::focus_handle(e, cx));
    });
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "fn d() {\n    q.unwrap_or(3.clone());\n    q.unwrap_or(4);\n}\n");
}

#[gpui::test]
fn a_file_changed_since_it_was_read_is_not_overwritten(cx: &mut TestAppContext) {
    // The write carries the hash that was read; a conflicting disk copy is reported, not clobbered. The agent answers
    // `Conflict`; here the file is replaced by something that no longer matches, which the edit simply does not touch.
    let dir = fresh("structural-stale");
    let f = write(&dir, "a.rs", b"fn a() { foo(1); }\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    std::fs::write(&f, b"fn a() { bar(1); }\n").unwrap();
    let p = Arc::new(pattern("Rust", "foo($X)"));
    wb.update(cx, |wb, cx| wb.replace_structural_now(vec![path_str(&f)], family("Rust"), p, "baz($X)".into(), cx));
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(disk(&f), b"fn a() { bar(1); }\n");
}

#[gpui::test]
fn files_outside_the_opened_folders_are_never_written(cx: &mut TestAppContext) {
    let dir = fresh("structural-roots");
    let outside = fresh("structural-roots-outside");
    let target = write(&outside, "x.rs", b"fn a() { foo(1); }\n");
    write(&dir, "a.rs", b"fn a() { foo(1); }\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.confirm_structural_replace(vec![path_str(&target)], "Rust", "foo($X)".into(), "baz($X)".into(), 1, window, cx));
    cx.run_until_parked();
    assert!(cx.pending_prompt().is_none(), "nothing to ask about: the only file is outside the workspace");
    assert_eq!(disk(&target), b"fn a() { foo(1); }\n");
}

#[gpui::test]
fn replace_all_asks_first_and_then_writes_through_the_prompt(cx: &mut TestAppContext) {
    let dir = fresh("structural-prompt");
    let a = write(&dir, "a.rs", b"fn a() { foo(1); foo(2); }\n");
    let b = write(&dir, "b.rs", b"fn b() { foo(3); }\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let panel = wb.read_with(cx, |wb, _| wb.search_panel());
    search(&panel, "Rust", "foo($X)", cx);
    panel.update(cx, |s, cx| {
        s.set_replacement("baz($X, 0)", cx);
        s.request_replace_all(cx);
    });
    wait(cx, "confirmation", |cx| cx.pending_prompt().is_some());
    let (title, detail) = cx.pending_prompt().unwrap();
    assert_eq!(title, "Replace 3 matches in 2 files with “baz($X, 0)”?");
    assert!(detail.contains("closed files are written to disk"), "{detail}");
    assert_eq!(disk(&a), b"fn a() { foo(1); foo(2); }\n", "read-only until the answer");
    // Cancel writes nothing and leaves the keyboard where it was; asking again offers the same.
    crate::question::cancel_keeps_focus(&wb, cx, "Cancel");
    assert_eq!(disk(&a), b"fn a() { foo(1); foo(2); }\n", "Cancel writes nothing");
    panel.update(cx, |s, cx| s.request_replace_all(cx));
    wait(cx, "confirmation again", |cx| cx.pending_prompt().is_some());
    cx.simulate_prompt_answer("Replace");
    wait(cx, "written", |_| disk(&b) == b"fn b() { baz(3, 0); }\n");
    assert_eq!(disk(&a), b"fn a() { baz(1, 0); baz(2, 0); }\n");
}

#[gpui::test]
fn a_replacement_that_leaves_a_file_unparsable_is_offered_for_skipping(cx: &mut TestAppContext) {
    let dir = fresh("structural-broken");
    let fine = write(&dir, "fine.rs", b"fn a() { foo(1); }\n");
    let broken = write(&dir, "broken.rs", b"fn b() { foo(1, 2); }\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let panel = wb.read_with(cx, |wb, _| wb.search_panel());
    search(&panel, "Rust", "foo($$$A)", cx);
    panel.update(cx, |s, cx| {
        s.set_replacement("$$$A", cx); // `1;` is fine, `1, 2;` is not
        s.request_replace_all(cx);
    });
    wait(cx, "warning", |cx| cx.pending_prompt().is_some());
    let (_, detail) = cx.pending_prompt().unwrap();
    assert_eq!(detail, "The result would not parse in 1 file: broken.rs");
    cx.simulate_prompt_answer("Skip Those Files");
    wait(cx, "fine file written", |_| disk(&fine) == b"fn a() { 1; }\n");
    assert_eq!(disk(&broken), b"fn b() { foo(1, 2); }\n");
}

#[gpui::test]
fn per_file_replace_and_dismiss_work_like_in_text_search(cx: &mut TestAppContext) {
    let dir = fresh("structural-per-file");
    let a = write(&dir, "a.rs", b"fn a() { foo(1); }\n");
    let b = write(&dir, "b.rs", b"fn b() { foo(2); }\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let panel = wb.read_with(cx, |wb, _| wb.search_panel());
    search(&panel, "Rust", "foo($X)", cx);
    assert_eq!(find_hits(&panel, cx).len(), 2);
    panel.update(cx, |s, cx| {
        s.set_replacement("bar($X)", cx);
        s.dismiss_for_test(1, cx); // b.rs
        s.request_replace_all(cx);
    });
    wait(cx, "confirmation", |cx| cx.pending_prompt().is_some());
    assert_eq!(cx.pending_prompt().unwrap().0, "Replace 1 match in 1 file with “bar($X)”?");
    cx.simulate_prompt_answer("Replace");
    wait(cx, "written", |_| disk(&a) == b"fn a() { bar(1); }\n");
    assert_eq!(disk(&b), b"fn b() { foo(2); }\n");
}

#[gpui::test]
fn the_palette_commands_open_structural_mode_in_the_language_of_the_active_file(cx: &mut TestAppContext) {
    let dir = fresh("structural-palette");
    let py = write(&dir, "tool.py", b"print(1)\n");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(path_str(&py), window, cx));
    cx.run_until_parked();
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Search: Structural Search…", window, cx));
    cx.run_until_parked();
    let panel = wb.read_with(cx, |wb, _| wb.search_panel());
    assert!(panel.read_with(cx, |s, _| s.structural_for_test().0));
    search(&panel, "Python", "print($X)", cx);
    assert_eq!(find_hits(&panel, cx), [("tool.py".to_string(), vec![1])]);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Search: Structural Replace…", window, cx));
    cx.run_until_parked();
    assert!(crate::commands::action_by_id("search.structuralSearch").is_some() && crate::commands::action_by_id("search.structuralReplace").is_some());
}

/// The same flow over ssh: the agent searches and serves the files, the matching and the edits happen here.
#[gpui::test]
fn structural_search_and_replace_work_on_a_remote_folder(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("structural-remote") else { return };
    let work = fresh("structural-remote-root");
    let a = write(&work, "src/a.rs", b"fn a() {\r\n    foo(1);\r\n}\r\n");
    let (wb, cx) = crate::qa_integration_tests::ssh_workbench(cx, &work);
    let panel = wb.read_with(cx, |wb, _| wb.search_panel());
    search(&panel, "Rust", "foo($X)", cx);
    assert_eq!(find_hits(&panel, cx), [("src/a.rs".to_string(), vec![2])]);
    panel.update(cx, |s, cx| {
        s.set_replacement("bar($X)", cx);
        s.request_replace_all(cx);
    });
    wait(cx, "confirmation", |cx| cx.pending_prompt().is_some());
    cx.simulate_prompt_answer("Replace");
    wait(cx, "written", |_| disk(&a) == b"fn a() {\r\n    bar(1);\r\n}\r\n");
}
