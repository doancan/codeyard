//! Picking a run with Shift-click: which commits it holds, and when it holds none.

use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::qa_integration_tests::{git_in, repo, wait_ticking};
use crate::qa_workbench_tests::{open, setup};
use crate::workbench::Workbench;

fn started<'a>(
    name: &str,
    subjects: &[&str],
    cx: &'a mut TestAppContext,
) -> (std::path::PathBuf, gpui::Entity<Workbench>, gpui::Entity<ScmPanel>, &'a mut VisualTestContext) {
    let dir = repo(name);
    for s in subjects {
        git_in(&dir, &["commit", "-q", "--allow-empty", "-m", s]);
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    let tip = git_in(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    wait_ticking(cx, "the graph", |cx| scm.read_with(cx, |s, _| s.commit_by_hash(&tip).is_some()));
    (dir, wb, scm, cx)
}

fn hash_of(dir: &std::path::Path, rev: &str) -> String {
    git_in(dir, &["rev-parse", rev]).trim().to_string()
}

fn pick(scm: &gpui::Entity<ScmPanel>, cx: &mut VisualTestContext, hash: &str, shift: bool) {
    let commit = scm.read_with(cx, |s, _| s.commit_by_hash(hash)).unwrap();
    scm.update(cx, |s, cx| s.pick_commit(commit, shift, cx));
}

#[gpui::test]
fn shift_click_picks_the_run_between_two_rows_oldest_first(cx: &mut TestAppContext) {
    let (dir, _wb, scm, cx) = started("range-run", &["one", "two", "three", "four"], cx);
    let (one, two, three, four) = (hash_of(&dir, "HEAD~3"), hash_of(&dir, "HEAD~2"), hash_of(&dir, "HEAD~1"), hash_of(&dir, "HEAD"));
    assert!(scm.read_with(cx, |s, _| s.range_run().is_empty()), "no range yet");
    pick(&scm, cx, &two, false);
    pick(&scm, cx, &four, true);
    assert_eq!(scm.read_with(cx, |s, _| s.range_run()), vec![two.clone(), three.clone(), four.clone()], "from the older row to the newer, either way round");
    pick(&scm, cx, &four, false);
    assert!(scm.read_with(cx, |s, _| s.range.is_none() && s.range_run().is_empty()), "a plain click is no range");
    pick(&scm, cx, &one, false);
    pick(&scm, cx, &three, true);
    pick(&scm, cx, &four, true);
    assert_eq!(scm.read_with(cx, |s, _| s.range_run()).first(), Some(&one), "Shift-click again keeps the start and moves the end");
    assert_eq!(scm.read_with(cx, |s, _| s.head_hash()), Some(four));
}

#[gpui::test]
fn rows_that_are_not_one_run_pick_nothing(cx: &mut TestAppContext) {
    let (dir, _wb, scm, cx) = started("range-merge", &["one"], cx);
    let base = hash_of(&dir, "HEAD");
    git_in(&dir, &["checkout", "-q", "-b", "side"]);
    git_in(&dir, &["commit", "-q", "--allow-empty", "-m", "side"]);
    let side = hash_of(&dir, "HEAD");
    git_in(&dir, &["checkout", "-q", "-"]);
    git_in(&dir, &["commit", "-q", "--allow-empty", "-m", "main"]);
    git_in(&dir, &["merge", "-q", "--no-ff", "-m", "merge", "side"]);
    let tip = hash_of(&dir, "HEAD");
    scm.update(cx, |s, cx| s.load_graph(cx));
    wait_ticking(cx, "the new graph", |cx| scm.read_with(cx, |s, _| s.commit_by_hash(&tip).is_some()));
    pick(&scm, cx, &base, false);
    pick(&scm, cx, &side, true);
    assert!(scm.read_with(cx, |s, _| s.range_run().is_empty()), "a commit of the side branch is not on the branch's own line");
}
