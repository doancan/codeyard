//! The Search view's scopes and history in a real workbench: real files, a real git repository, the real search agent.

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::search::scope::Scope;
use crate::workbench::Workbench;

const WAIT: u64 = 10;

fn files(search: &Entity<SearchPanel>, cx: &mut VisualTestContext) -> Vec<String> {
    search.read_with(cx, |s, _| s.result_summary().into_iter().map(|(p, _)| p).collect())
}

fn settle(search: &Entity<SearchPanel>, cx: &mut VisualTestContext) {
    crate::qa_integration_tests::wait_secs(cx, WAIT, "the search to finish", |cx| {
        search.read_with(cx, |s, _| !s.is_searching() && s.summary_for_test().is_some())
    });
}

/// Searches `text` in the Search view, waiting for the answer (the typing debounce is a timer the test does not wait for).
fn search_for(wb: &Entity<Workbench>, search: &Entity<SearchPanel>, text: &str, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.search_for(text, window, cx));
    rerun(search, cx);
}

fn rerun(search: &Entity<SearchPanel>, cx: &mut VisualTestContext) {
    search.update(cx, |s, cx| s.rerun(cx));
    settle(search, cx);
}

fn choose(wb: &Entity<Workbench>, search: &Entity<SearchPanel>, scope: Scope, cx: &mut VisualTestContext) {
    search.update(cx, |s, cx| s.set_scope(scope, cx));
    wb.update(cx, |wb, cx| wb.sync_search_scope(cx));
    rerun(search, cx);
}

#[gpui::test]
fn open_editors_limits_the_search_to_the_files_that_are_open(cx: &mut TestAppContext) {
    let dir = crate::qa_workbench_tests::project("scope-open");
    for f in ["a.txt", "b.txt", "src/c.txt"] {
        std::fs::write(dir.join(f), "needle\n").unwrap();
    }
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    search_for(&wb, &search, "needle", cx);
    assert_eq!(files(&search, cx), ["a.txt", "b.txt", "src/c.txt"]);

    // Nothing is open: nothing is searched, and the line says so.
    choose(&wb, &search, Scope::OpenEditors, cx);
    assert!(files(&search, cx).is_empty());
    assert_eq!(search.read_with(cx, |s, _| s.scope_note()).as_deref(), Some("No file is open in this folder."));

    // Opening files adds them without touching the panel: the files follow the editors.
    crate::qa_workbench_tests::open_in(&wb, &dir.join("b.txt"), cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("src/c.txt"), cx);
    wb.update(cx, |wb, cx| wb.sync_search_scope(cx));
    rerun(&search, cx);
    assert_eq!(files(&search, cx), ["b.txt", "src/c.txt"]);
    assert_eq!(search.read_with(cx, |s, _| s.scope_note()).as_deref(), Some("Only the 2 open files."));

    // Back to the whole workspace.
    choose(&wb, &search, Scope::Workspace, cx);
    assert_eq!(files(&search, cx).len(), 3);
    assert_eq!(search.read_with(cx, |s, _| s.scope_note()), None);
}

/// Names a glob would read as patterns, and a comma (which the include list cannot hold). All are legal on Windows.
#[gpui::test]
fn file_names_with_glob_characters_are_found_in_a_scope_and_their_siblings_are_not(cx: &mut TestAppContext) {
    let dir = crate::qa_workbench_tests::project("scope-names");
    let odd = ["pages/[id].txt", "a{1}.txt", "co,mma.txt", "çay şeker.txt"];
    let siblings = ["co-mma.txt", "a1.txt", "pages/i.txt"];
    std::fs::create_dir_all(dir.join("pages")).unwrap();
    for f in odd.iter().chain(siblings.iter()).chain(["plain.txt"].iter()) {
        std::fs::write(dir.join(f), "needle\n").unwrap();
    }
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    for f in odd {
        crate::qa_workbench_tests::open_in(&wb, &dir.join(f), cx);
    }
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    search_for(&wb, &search, "needle", cx);
    choose(&wb, &search, Scope::OpenEditors, cx);
    let mut expected: Vec<String> = odd.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(files(&search, cx), expected, "co-mma.txt matched the `?` for the comma, and is not listed; plain.txt is not open");
}

/// `*` cannot be in a Windows file name.
#[cfg(unix)]
#[gpui::test]
fn a_file_name_with_a_star_is_found_in_a_scope(cx: &mut TestAppContext) {
    let dir = crate::qa_workbench_tests::project("scope-star");
    std::fs::write(dir.join("star*.txt"), "needle\n").unwrap();
    std::fs::write(dir.join("starry.txt"), "needle\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("star*.txt"), cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    search_for(&wb, &search, "needle", cx);
    choose(&wb, &search, Scope::OpenEditors, cx);
    assert_eq!(files(&search, cx), ["star*.txt"]);
}

/// A root-level file and files of the same name below it: the scope names only the one at the root, in the results and in
/// what Replace All reviews (a replacement must never reach `docs/README.md` or `vendor/x/README.md`).
#[gpui::test]
fn a_root_file_does_not_bring_in_same_named_files_deeper_down(cx: &mut TestAppContext) {
    let dir = crate::qa_integration_tests::repo("scope-deep");
    for f in ["docs", "vendor/x"] {
        std::fs::create_dir_all(dir.join(f)).unwrap();
    }
    for f in ["README.md", "docs/README.md", "vendor/x/README.md"] {
        std::fs::write(dir.join(f), "# proje\nneedle\n").unwrap();
    }
    // Committed, then only the root README changes: the same-named files below it are not changed files.
    crate::qa_integration_tests::git_in(&dir, &["add", "-A"]);
    crate::qa_integration_tests::git_in(&dir, &["commit", "-q", "-m", "readmes"]);
    std::fs::write(dir.join("README.md"), "# proje\nneedle changed\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("README.md"), cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    for scope in [Scope::OpenEditors, Scope::ChangedFiles] {
        if scope == Scope::ChangedFiles {
            crate::qa_integration_tests::wait_secs(cx, WAIT, "git status", |cx| {
                wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|s| s.changes.iter().any(|c| c.path == "README.md")))
            });
        }
        search_for(&wb, &search, "needle", cx);
        choose(&wb, &search, Scope::Workspace, cx);
        assert_eq!(files(&search, cx).len(), 3, "the whole workspace has three");
        choose(&wb, &search, scope, cx);
        assert_eq!(files(&search, cx), ["README.md"], "{scope:?}");
        let (summary, spec) = search.read_with(cx, |s, cx| (s.summary_for_test(), s.spec(cx)));
        assert_eq!(summary, Some((1, 1)), "the counts are the scope's, not the agent's");
        assert_eq!(spec.include, "/README.md", "Replace All and the Search Editor search the one file");
    }
    // Replace All opens the review: it lists the root README only.
    search.update(cx, |s, cx| {
        s.set_replacement("x", cx);
        s.request_replace_all(cx);
    });
    crate::qa_integration_tests::wait_secs(cx, WAIT, "the review", |cx| !wb.read_with(cx, |wb, cx| wb.search_editor_rows_for_test(cx)).is_empty());
    let rows = wb.read_with(cx, |wb, cx| wb.search_editor_rows_for_test(cx)).join("\n");
    assert!(rows.contains("README.md") && !rows.contains("docs/") && !rows.contains("vendor/"), "{rows}");
}

/// The typed include filter is not used while a scope limits the search, and the line says so.
#[gpui::test]
fn the_include_filter_is_not_used_under_a_scope_and_the_note_says_so(cx: &mut TestAppContext) {
    let dir = crate::qa_workbench_tests::project("scope-include");
    std::fs::write(dir.join("a.txt"), "needle\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("a.txt"), cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    search.update(cx, |s, cx| {
        s.show_globs = true;
        s.include.update(cx, |f, cx| f.set_text("*.rs", cx));
    });
    search_for(&wb, &search, "needle", cx);
    choose(&wb, &search, Scope::OpenEditors, cx);
    assert_eq!(files(&search, cx), ["a.txt"], "*.rs would have excluded it");
    assert!(search.read_with(cx, |s, _| s.scope_replaces_include()));
    choose(&wb, &search, Scope::Workspace, cx);
    assert!(!search.read_with(cx, |s, _| s.scope_replaces_include()));
}

#[gpui::test]
fn current_folder_searches_below_the_folder_of_the_front_file(cx: &mut TestAppContext) {
    let dir = crate::qa_workbench_tests::project("scope-folder");
    std::fs::create_dir_all(dir.join("src/deep")).unwrap();
    for f in ["top.txt", "src/mid.txt", "src/deep/low.txt"] {
        std::fs::write(dir.join(f), "needle\n").unwrap();
    }
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("src/mid.txt"), cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    search_for(&wb, &search, "needle", cx);
    choose(&wb, &search, Scope::CurrentFolder, cx);
    assert_eq!(files(&search, cx), ["src/deep/low.txt", "src/mid.txt"]);
    assert_eq!(search.read_with(cx, |s, _| s.scope_note()).as_deref(), Some("Only in the folder src."));
    // The front file moves to the root: the folder is the workspace.
    crate::qa_workbench_tests::open_in(&wb, &dir.join("top.txt"), cx);
    wb.update(cx, |wb, cx| wb.sync_search_scope(cx));
    rerun(&search, cx);
    assert_eq!(files(&search, cx).len(), 3);
    assert_eq!(search.read_with(cx, |s, _| s.scope_note()).as_deref(), Some("The current folder is the workspace."));
}

#[gpui::test]
fn changed_files_are_the_ones_git_reports_in_a_real_repository(cx: &mut TestAppContext) {
    let dir = crate::qa_integration_tests::repo("scope-git");
    std::fs::write(dir.join("src/lib.rs"), "// satır 1\nneedle changed\n").unwrap();
    std::fs::write(dir.join("new.txt"), "needle untracked\n").unwrap();
    std::fs::write(dir.join("README.md"), "# proje\nneedle committed\n").unwrap();
    crate::qa_integration_tests::git_in(&dir, &["add", "README.md"]);
    crate::qa_integration_tests::git_in(&dir, &["commit", "-q", "-m", "readme"]);
    std::fs::write(dir.join("README.md"), "# proje\nneedle committed\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_integration_tests::wait_secs(cx, WAIT, "git status", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|s| s.changes.len() == 2)));
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    search_for(&wb, &search, "needle", cx);
    assert_eq!(files(&search, cx).len(), 3, "the committed README matches too");
    choose(&wb, &search, Scope::ChangedFiles, cx);
    assert_eq!(files(&search, cx), ["new.txt", "src/lib.rs"], "the committed file is not a change");
    assert_eq!(search.read_with(cx, |s, _| s.scope_note()).as_deref(), Some("Only the 2 changed files."));
}

#[gpui::test]
fn replace_all_and_the_search_editor_use_the_scope_and_refuse_an_empty_one(cx: &mut TestAppContext) {
    let dir = crate::qa_workbench_tests::project("scope-replace");
    for f in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(f), "needle\n").unwrap();
    }
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    search_for(&wb, &search, "needle", cx);
    choose(&wb, &search, Scope::OpenEditors, cx);
    // Nothing is open: Replace All must not review a replacement everywhere.
    search.update(cx, |s, cx| {
        s.set_replacement("x", cx);
        s.request_replace_all(cx);
        s.open_in_editor(cx);
    });
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.search_editor_count_for_test()), 0, "no Search Editor opened for an empty scope");

    crate::qa_workbench_tests::open_in(&wb, &dir.join("b.txt"), cx);
    wb.update(cx, |wb, cx| wb.sync_search_scope(cx));
    rerun(&search, cx);
    assert_eq!(files(&search, cx).len(), 1);
    let spec = search.read_with(cx, |s, cx| s.spec(cx));
    assert_eq!(spec.include, "/b.txt", "the review searches the same files");
    search.update(cx, |s, cx| s.request_replace_all(cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.search_editor_count_for_test()), 1, "the review opens for the scope's file");
}

#[gpui::test]
fn history_brings_back_the_options_filters_and_scope_of_each_search(cx: &mut TestAppContext) {
    let dir = crate::qa_workbench_tests::project("scope-history");
    std::fs::write(dir.join("a.txt"), "needle\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("a.txt"), cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    search_for(&wb, &search, "needle", cx);
    // A first search with options and filters, then a plain second one.
    search.update(cx, |s, cx| {
        s.set_regex(true, cx);
        s.case_sensitive = true;
        s.show_globs = true;
        s.include.update(cx, |f, cx| f.set_text("*.txt", cx));
        s.exclude.update(cx, |f, cx| f.set_text("src/**", cx));
        s.remember_for_test(cx);
        s.set_regex(false, cx);
        s.case_sensitive = false;
        s.include.update(cx, |f, cx| f.set_text("", cx));
        s.exclude.update(cx, |f, cx| f.set_text("", cx));
        s.query.update(cx, |q, cx| q.set_text("second", cx));
        s.remember_for_test(cx);
    });
    let entries = search.read_with(cx, |s, _| s.history_entries_for_test());
    assert_eq!((entries[0].include.as_str(), entries[0].exclude.as_str(), entries[0].regex, entries[0].case_sensitive), ("*.txt", "src/**", true, true));
    assert_eq!((entries[1].query.as_str(), entries[1].include.as_str(), entries[1].regex), ("second", "", false));

    // Up twice: the first search comes back with everything it was run with.
    search.update(cx, |s, cx| {
        s.step_history_for_test(true, cx);
        s.step_history_for_test(true, cx);
    });
    let (query, include, exclude, regex, case) = search
        .read_with(cx, |s, cx| (s.query.read(cx).text().to_string(), s.include_text(cx), s.exclude.read(cx).text().to_string(), s.regex, s.case_sensitive));
    assert_eq!((query.as_str(), include.as_str(), exclude.as_str(), regex, case), ("needle", "*.txt", "src/**", true, true));
}

#[gpui::test]
fn history_stays_bounded_with_options(cx: &mut TestAppContext) {
    let dir = crate::qa_workbench_tests::project("scope-history-max");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    search.update(cx, |s, cx| {
        for i in 0..80 {
            s.query.update(cx, |q, cx| q.set_text(&format!("q{}", i % 40), cx));
            s.regex = i % 2 == 0;
            s.remember_for_test(cx);
        }
    });
    let entries = search.read_with(cx, |s, _| s.history_entries_for_test());
    assert_eq!(entries.len(), crate::search::scope::HISTORY_MAX);
}
