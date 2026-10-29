//! Several repositories in one workspace, with real git: the list, picking one, operations that stay in the picked
//! repository even with identical file names, and refusing an operation on a repository that went away.

use std::path::Path;

use super::*;
use crate::qa_integration_tests::{git_in, repo, wait_ticking};

/// A workspace folder that is a repository, with another repository nested in `lib/` (both have `same.txt`).
fn workspace(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let outer = repo(name);
    let inner = outer.join("lib");
    std::fs::create_dir_all(&inner).unwrap();
    for dir in [&outer, &inner] {
        std::fs::write(dir.join("same.txt"), "one\n").unwrap();
    }
    git_in(&inner, &["init", "-q"]);
    git_in(&inner, &["config", "user.name", "QA"]);
    git_in(&inner, &["config", "user.email", "qa@example.com"]);
    git_in(&inner, &["add", "-A"]);
    git_in(&inner, &["commit", "-q", "-m", "inner"]);
    std::fs::write(outer.join(".gitignore"), "lib/\n").unwrap();
    git_in(&outer, &["add", "-A"]);
    git_in(&outer, &["commit", "-q", "-m", "outer"]);
    for dir in [&outer, &inner] {
        std::fs::write(dir.join("same.txt"), "two\n").unwrap();
    }
    (outer, inner)
}

fn staged(dir: &Path) -> String {
    git_in(dir, &["diff", "--cached", "--name-only"])
}

fn shown_root(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> Option<String> {
    wb.read_with(cx, |wb, _| wb.vcs.status.as_ref().map(|s| s.root.clone()))
}

fn rows(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> Vec<crate::scm::RepoRow> {
    wb.read_with(cx, |wb, cx| wb.scm.read(cx).repos().to_vec())
}

fn open_scm(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext, dir: &Path) {
    crate::qa_workbench_tests::open(wb, dir, cx);
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(Panel::Scm);
        wb.refresh_git(0, cx);
    });
    wait_ticking(cx, "both repositories listed with their counts", |cx| {
        let r = rows(wb, cx);
        r.len() == 2 && r.iter().all(|r| !r.loading && r.changes == 1)
    });
}

#[gpui::test]
fn staging_in_the_picked_repository_leaves_the_other_one_alone(cx: &mut gpui::TestAppContext) {
    let (outer, inner) = workspace("repos-pick");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    open_scm(&wb, cx, &outer);
    let r = rows(&wb, cx);
    let inner_root = r.iter().find(|r| r.location.as_deref() == Some("lib")).expect("nested repository row").root.clone();
    assert!(r.iter().any(|r| r.selected && r.location.is_none()), "the folder's own repository is shown first: {r:?}");
    // Assistive technology gets the same list, with the selection and each row's counts.
    let access = cx.update(|window, cx| wb.read(cx).scm.read(cx).a11y_view(window, cx));
    assert_eq!(access.repos.len(), 2);
    assert!(access.repos.iter().any(|r| r.selected && r.description().contains("changes: 1")), "{:?}", access.repos);
    wb.update(cx, |wb, cx| wb.select_repo(inner_root.clone(), cx));
    wait_ticking(cx, "the nested repository in Source Control", |cx| shown_root(&wb, cx).as_deref() == Some(inner_root.as_str()));
    wb.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.stage(vec!["same.txt".into()], cx)));
    wait_ticking(cx, "the file staged", |_| staged(&inner).contains("same.txt"));
    assert_eq!(staged(&outer), "", "the same relative path in the other repository is untouched");
    // The pick holds when a file of the other repository becomes active.
    crate::qa_workbench_tests::open_in(&wb, &outer.join("same.txt"), cx);
    wb.update(cx, |wb, cx| wb.sync_scm_root(cx));
    cx.run_until_parked();
    assert_eq!(shown_root(&wb, cx).as_deref(), Some(inner_root.as_str()));
}

#[gpui::test]
fn without_a_pick_source_control_follows_the_active_file_into_a_nested_repository(cx: &mut gpui::TestAppContext) {
    let (outer, inner) = workspace("repos-follow");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    open_scm(&wb, cx, &outer);
    crate::qa_workbench_tests::open_in(&wb, &inner.join("same.txt"), cx);
    wb.update(cx, |wb, cx| wb.sync_scm_root(cx));
    wait_ticking(cx, "the nested repository shown", |cx| shown_root(&wb, cx).is_some_and(|r| r.ends_with("/lib")));
}

#[gpui::test]
fn an_operation_on_a_repository_that_went_away_does_nothing(cx: &mut gpui::TestAppContext) {
    let (outer, inner) = workspace("repos-gone");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    open_scm(&wb, cx, &outer);
    let inner_root = rows(&wb, cx).into_iter().find(|r| r.location.as_deref() == Some("lib")).unwrap().root;
    wb.update(cx, |wb, cx| wb.select_repo(inner_root.clone(), cx));
    wait_ticking(cx, "the nested repository in Source Control", |cx| shown_root(&wb, cx).as_deref() == Some(inner_root.as_str()));
    std::fs::remove_dir_all(inner.join(".git")).unwrap();
    // Whichever comes first, the operation (it checks the repository before it runs) or a refresh (it reports the
    // repository gone), nothing may happen in the enclosing repository.
    wb.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.stage(vec!["same.txt".into()], cx)));
    wait_ticking(cx, "the refusal", |cx| {
        wb.read_with(cx, |wb, cx| {
            wb.scm.read(cx).result_text().is_some_and(|t| t.contains("no longer available"))
                || wb.vcs.error.as_ref().is_some_and(|e| e.contains("no longer available"))
        })
    });
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "the repository reported gone", |cx| {
        wb.read_with(cx, |wb, _| wb.vcs.status.is_none() && wb.vcs.error.as_ref().is_some_and(|e| e.contains("no longer available")))
    });
    wb.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.stage(vec!["same.txt".into()], cx)));
    cx.run_until_parked();
    assert_eq!(staged(&outer), "", "nothing ran in the enclosing repository instead");
}
