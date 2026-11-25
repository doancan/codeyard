//! The Amend and Stash buttons under Commit: they act on the real repository, ask before rewriting a commit that is already
//! pushed, and stay out of the way when there is nothing to do.

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

use super::*;
use crate::git::{Change, Mark};
use crate::graph::RefLabel;
use crate::qa_integration_tests::{git_in, repo, repo_with_remote, setup_with, wait_ticking};
use crate::settings::Settings;
use crate::workbench::Workbench;

fn open<'a>(cx: &'a mut TestAppContext, dir: &std::path::Path) -> (Entity<Workbench>, Entity<ScmPanel>, &'a mut VisualTestContext) {
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, hot_exit: false, git_autofetch: false, ..Default::default() });
    wb.update_in(cx, |wb, window, cx| {
        wb.open_path(dir.to_str().unwrap(), window, cx);
        wb.run_command_for_test("View: Source Control", window, cx);
    });
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    cx.simulate_resize(gpui::size(px(1000.), px(900.)));
    wait_ticking(cx, "the graph", |cx| scm.read_with(cx, |s, _| s.graph.is_some() && s.status.is_some()));
    (wb, scm, cx)
}

fn click(cx: &mut VisualTestContext, selector: &'static str, modifiers: Modifiers) {
    let at = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is not drawn")).center();
    cx.simulate_click(at, modifiers);
}

fn stash_count(dir: &std::path::Path) -> usize {
    git_in(dir, &["stash", "list"]).lines().count()
}

fn commit_count(dir: &std::path::Path) -> usize {
    git_in(dir, &["rev-list", "--count", "HEAD"]).trim().parse().unwrap()
}

fn refresh(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
}

#[test]
fn a_commit_is_shared_when_a_remote_branch_or_the_upstream_has_it() {
    let status = |upstream: Option<&str>| Status { upstream: upstream.map(str::to_string), ..Default::default() };
    let commit = |outgoing: bool, refs: Vec<RefLabel>| GraphCommit { outgoing, refs, ..Default::default() };
    let remote = RefLabel { name: "origin/main".into(), kind: RefKind::Remote };
    assert!(is_shared(&status(Some("origin/main")), &commit(false, vec![])), "on the upstream: not counted as unpushed");
    assert!(!is_shared(&status(Some("origin/main")), &commit(true, vec![])), "ahead of the upstream: only here");
    assert!(is_shared(&status(None), &commit(true, vec![remote])), "a remote branch points at it");
    assert!(!is_shared(&status(None), &commit(false, vec![])), "no upstream, no remote branch: only here");
}

#[gpui::test]
fn stash_puts_tracked_changes_aside_and_shift_takes_new_files_too(cx: &mut TestAppContext) {
    let dir = repo("quick-stash");
    let (wb, scm, cx) = open(cx, &dir);
    // Nothing to stash: the button is there and does nothing.
    assert!(cx.debug_bounds("scm-quick-stash").is_some());
    click(cx, "scm-quick-stash", Modifiers::none());
    cx.run_until_parked();
    assert_eq!(stash_count(&dir), 0);
    // A change to a tracked file, and a new file beside it.
    std::fs::write(dir.join("README.md"), "changed\n").unwrap();
    std::fs::write(dir.join("new.txt"), "new\n").unwrap();
    refresh(&wb, cx);
    wait_ticking(cx, "both changes", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.changes.len() == 2)));
    click(cx, "scm-quick-stash", Modifiers::none());
    wait_ticking(cx, "stashed", |_| stash_count(&dir) == 1);
    assert_ne!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "changed\n", "the tracked change is parked");
    assert!(dir.join("new.txt").exists(), "a new file stays unless asked for");
    wait_ticking(cx, "the stash is listed", |cx| scm.read_with(cx, |s, _| s.stash_count() == 1));
    // With Shift the new file goes too.
    refresh(&wb, cx);
    wait_ticking(cx, "only the new file is left", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.changes.len() == 1)));
    std::fs::write(dir.join("README.md"), "again\n").unwrap();
    refresh(&wb, cx);
    wait_ticking(cx, "two changes again", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.changes.len() == 2)));
    click(cx, "scm-quick-stash", Modifiers::shift());
    wait_ticking(cx, "second stash", |_| stash_count(&dir) == 2);
    assert!(!dir.join("new.txt").exists(), "Shift takes the new file along");
}

#[gpui::test]
fn stash_with_only_new_files_takes_them(cx: &mut TestAppContext) {
    let dir = repo("quick-stash-new");
    let (wb, scm, cx) = open(cx, &dir);
    std::fs::write(dir.join("only-new.txt"), "x\n").unwrap();
    refresh(&wb, cx);
    wait_ticking(cx, "the new file", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.changes.len() == 1)));
    click(cx, "scm-quick-stash", Modifiers::none());
    wait_ticking(cx, "stashed", |_| stash_count(&dir) == 1);
    assert!(!dir.join("only-new.txt").exists(), "nothing else to stash: the new file is what is stashed");
}

#[gpui::test]
fn amend_adds_the_staged_changes_to_a_commit_that_is_only_here_without_asking(cx: &mut TestAppContext) {
    let dir = repo("quick-amend");
    let (wb, scm, cx) = open(cx, &dir);
    let before = commit_count(&dir);
    std::fs::write(dir.join("extra.txt"), "extra\n").unwrap();
    git_in(&dir, &["add", "extra.txt"]);
    refresh(&wb, cx);
    wait_ticking(cx, "staged", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.staged().count() == 1)));
    click(cx, "scm-quick-amend", Modifiers::none());
    wait_ticking(cx, "amended", |_| git_in(&dir, &["show", "--name-only", "--format=", "HEAD"]).contains("extra.txt"));
    assert_eq!(commit_count(&dir), before, "the commit was replaced, not added to");
    assert!(!cx.has_pending_prompt(), "nobody else has this commit: no question");
}

#[gpui::test]
fn amend_asks_before_it_rewrites_a_commit_that_is_already_pushed(cx: &mut TestAppContext) {
    let (dir, bare, other) = repo_with_remote("quick-amend-pushed");
    let (wb, scm, cx) = open(cx, &dir);
    let head = git_in(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("extra.txt"), "extra\n").unwrap();
    git_in(&dir, &["add", "extra.txt"]);
    refresh(&wb, cx);
    wait_ticking(cx, "staged", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.staged().count() == 1)));
    click(cx, "scm-quick-amend", Modifiers::none());
    wait_ticking(cx, "the question", |cx| cx.has_pending_prompt());
    assert_eq!(git_in(&dir, &["rev-parse", "HEAD"]), head, "nothing happens before the answer");
    // No: it stays as it was.
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(git_in(&dir, &["rev-parse", "HEAD"]), head);
    // Yes: it is amended.
    click(cx, "scm-quick-amend", Modifiers::none());
    wait_ticking(cx, "the question again", |cx| cx.has_pending_prompt());
    cx.simulate_prompt_answer("Amend");
    wait_ticking(cx, "amended", |_| git_in(&dir, &["rev-parse", "HEAD"]) != head);
    for p in [&dir, &bare, &other] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[gpui::test]
fn the_buttons_do_nothing_while_conflicts_wait(cx: &mut TestAppContext) {
    let dir = repo("quick-conflict");
    let (wb, scm, cx) = open(cx, &dir);
    let st = scm.read_with(cx, |s, _| s.status.clone().unwrap());
    let conflicted = Status { changes: vec![Change { path: "f.txt".into(), orig: None, staged: None, unstaged: Some(Mark::Conflict) }], ..(*st).clone() };
    scm.update(cx, |s, cx| {
        s.status = Some(std::sync::Arc::new(conflicted));
        cx.notify();
    });
    cx.run_until_parked();
    click(cx, "scm-quick-stash", Modifiers::none());
    click(cx, "scm-quick-amend", Modifiers::none());
    cx.run_until_parked();
    assert_eq!(stash_count(&dir), 0);
    assert!(!cx.has_pending_prompt());
    let _ = wb;
}
