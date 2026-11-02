//! Source Control with several repositories, against real repositories: discovery and selection, row actions on
//! duplicate relative paths, commit/sync/history on the selected repository, independent states, stale results after a
//! removal or replacement, per-repository refresh, keyboard and accessibility, and a large inventory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_integration_tests::{git_in, setup_with, wait_ticking};
use crate::repos::OpState;
use crate::scm::StashAction;
use crate::settings::Settings;
use crate::workbench::Workbench;

/// A folder holding `alpha` and `beta` (both with `same.txt`) and `alpha/libs/nested` (also with `same.txt`).
struct Fixture {
    w: PathBuf,
    alpha: PathBuf,
    beta: PathBuf,
    nested: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.w);
    }
}

fn s(p: &Path) -> String {
    crate::ui::local_path(p)
}

fn make_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git_in(dir, &["init", "-q"]);
    git_in(dir, &["config", "user.name", "QA"]);
    git_in(dir, &["config", "user.email", "qa@example.com"]);
    std::fs::write(dir.join("same.txt"), "one\n").unwrap();
    git_in(dir, &["add", "-A"]);
    git_in(dir, &["commit", "-q", "-m", &format!("init {}", crate::ui::file_name(&s(dir)))]);
}

fn fixture(name: &str) -> Fixture {
    let w = std::env::temp_dir().join(format!("codeyard-multi-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&w);
    std::fs::create_dir_all(&w).unwrap();
    let w = crate::ui::canonical_for_test(&w);
    let (alpha, beta) = (w.join("alpha"), w.join("beta"));
    make_repo(&alpha);
    make_repo(&beta);
    let nested = alpha.join("libs/nested");
    make_repo(&nested);
    // The nested repository is not part of alpha's changes.
    std::fs::write(alpha.join(".git/info/exclude"), "libs/\n").unwrap();
    Fixture { w, alpha, beta, nested }
}

fn open<'a>(cx: &'a mut TestAppContext, dir: &Path, repos: usize) -> (Entity<Workbench>, Entity<ScmPanel>, &'a mut VisualTestContext) {
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, hot_exit: false, ..Default::default() });
    wb.update_in(cx, |wb, window, cx| {
        wb.open_path(dir.to_str().unwrap(), window, cx);
        wb.run_command_for_test("View: Source Control", window, cx);
    });
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_for_states(cx, &scm, repos);
    (wb, scm, cx)
}

/// Wait until the panel lists `n` repositories, each with its state read, and one of them is selected.
fn wait_for_states(cx: &mut VisualTestContext, scm: &Entity<ScmPanel>, n: usize) {
    wait_ticking(cx, "repository inventory", |cx| {
        scm.read_with(cx, |s, _| s.repos.len() == n && s.repos.iter().all(|r| r.status.is_some()) && s.status.is_some())
    });
}

fn key(scm: &Entity<ScmPanel>, cx: &mut VisualTestContext, dir: &Path) -> RepoKey {
    let path = s(dir);
    scm.read_with(cx, |s, _| s.repos.iter().find(|r| r.id.worktree == path).map(|r| r.id.key)).unwrap_or_else(|| panic!("{path} not in the inventory"))
}

fn selected_root(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<String> {
    wb.read_with(cx, |wb, _| wb.git_status().map(|s| s.root.clone()))
}

fn staged_names(dir: &Path) -> String {
    git_in(dir, &["diff", "--cached", "--name-only"])
}

fn head(dir: &Path) -> String {
    git_in(dir, &["rev-parse", "HEAD"])
}

fn wait_idle(cx: &mut VisualTestContext, scm: &Entity<ScmPanel>, dir: &Path) {
    let root = s(dir);
    wait_ticking(cx, "operation finished", |cx| scm.read_with(cx, |s, _| !s.busy_in(&root)));
}

/// AC1: one entry per real repository, nested included, with its own branch; the panel switches to the aggregated
/// list, and a single repository keeps the plain panel.
#[gpui::test]
fn discovery_lists_every_repository_with_its_identity_and_branch(cx: &mut TestAppContext) {
    let f = fixture("inventory");
    git_in(&f.beta, &["checkout", "-q", "-b", "feature/beta"]);
    let (wb, scm, cx) = open(cx, &f.w, 3);
    let repos = scm.read_with(cx, |s, _| s.repos.clone());
    let paths: Vec<&str> = repos.iter().map(|r| r.id.worktree.as_str()).collect();
    assert_eq!(paths, vec![s(&f.alpha), s(&f.beta), s(&f.nested)]);
    for r in repos.iter() {
        assert_eq!(r.id.root, s(&f.w));
        assert_eq!(r.id.git_dir, format!("{}/.git", r.id.worktree));
        assert_eq!(r.id.host_gen, wb.read_with(cx, |wb, _| wb.repo_inventory().host_gen()));
    }
    let branches: Vec<Option<String>> = repos.iter().map(|r| r.status.as_ref().unwrap().branch.clone()).collect();
    assert_eq!(branches, vec![Some("main".into()), Some("feature/beta".into()), Some("main".into())]);
    // The workspace folder is no repository: the first repository found is the selected one.
    assert_eq!(selected_root(&wb, cx), Some(s(&f.alpha)));
    assert!(cx.debug_bounds("scm-repos").is_some(), "aggregated list shown");
    assert!(cx.debug_bounds("scm-repo-row-2").is_some());
    // Rediscovery keeps every identity.
    let keys: Vec<RepoKey> = repos.iter().map(|r| r.id.key).collect();
    wb.update(cx, |wb, cx| wb.discover_repos(cx));
    wait_ticking(cx, "rediscovery", |cx| wb.read_with(cx, |wb, _| !wb.repo_inventory().discovering));
    assert_eq!(scm.read_with(cx, |s, _| s.repos.iter().map(|r| r.id.key).collect::<Vec<_>>()), keys);
}

#[gpui::test]
fn a_single_repository_keeps_the_plain_panel(cx: &mut TestAppContext) {
    let f = fixture("single");
    let (wb, scm, cx) = open(cx, &f.beta, 1);
    assert!(!scm.read_with(cx, |s, _| s.multi()));
    assert!(cx.debug_bounds("scm-repos").is_none());
    assert_eq!(selected_root(&wb, cx), Some(s(&f.beta)));
    // Staging works exactly as before (the inventory's identity is checked, nothing else changes).
    std::fs::write(f.beta.join("same.txt"), "changed\n").unwrap();
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "change", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| !st.changes.is_empty())));
    scm.update(cx, |s, cx| s.stage(vec!["same.txt".into()], cx));
    wait_idle(cx, &scm, &f.beta);
    assert_eq!(staged_names(&f.beta).trim(), "same.txt");
    assert_eq!(scm.read_with(cx, |s, _| s.result_text()), None);
}

/// AC2: the same relative path in two repositories; every row action goes to the row's repository, rapidly and in
/// parallel, and opening a row opens that repository's file.
#[gpui::test]
fn row_actions_on_duplicate_paths_target_their_own_repository(cx: &mut TestAppContext) {
    let f = fixture("rows");
    std::fs::write(f.alpha.join("same.txt"), "one\nalpha\n").unwrap();
    std::fs::write(f.beta.join("same.txt"), "one\nbeta\nbeta\nbeta\n").unwrap();
    std::fs::write(f.beta.join("extra.txt"), "new\n").unwrap();
    let (wb, scm, cx) = open(cx, &f.w, 3);
    wb.update(cx, |wb, cx| wb.refresh_repos(cx));
    wait_ticking(cx, "changes in both", |cx| scm.read_with(cx, |s, _| s.repos.iter().take(2).all(|r| r.changes() > 0)));
    let (a, b) = (key(&scm, cx, &f.alpha), key(&scm, cx, &f.beta));
    // The aggregated list labels every change with its repository.
    let lines = scm.read_with(cx, |s, _| s.lines_for_test());
    let files: Vec<(usize, String)> = scm.read_with(cx, |s, _| {
        lines
            .iter()
            .filter_map(|l| match l {
                Line::File(i, _, ci) => Some((*i, s.repos[*i].status.as_ref().unwrap().changes[*ci].path.clone())),
                _ => None,
            })
            .collect()
    });
    assert!(files.contains(&(0, "same.txt".into())) && files.contains(&(1, "same.txt".into())), "{files:?}");
    // Stage in beta only (alpha is selected), then immediately in alpha: both run, each in its own repository.
    scm.update(cx, |s, cx| {
        s.stage_in(Some(b), vec!["same.txt".into()], cx);
        s.stage_in(Some(a), vec!["same.txt".into()], cx);
        // A second operation in a busy repository is not queued behind into another one.
        s.stage_in(Some(b), vec!["extra.txt".into()], cx);
    });
    wait_idle(cx, &scm, &f.alpha);
    wait_idle(cx, &scm, &f.beta);
    assert_eq!(staged_names(&f.beta).trim(), "same.txt");
    assert_eq!(staged_names(&f.alpha).trim(), "same.txt");
    assert_eq!(staged_names(&f.nested).trim(), "");
    // Unstage in beta: alpha keeps its index.
    scm.update(cx, |s, cx| s.unstage_in(Some(b), vec!["same.txt".into()], cx));
    wait_idle(cx, &scm, &f.beta);
    assert_eq!(staged_names(&f.beta).trim(), "");
    assert_eq!(staged_names(&f.alpha).trim(), "same.txt");
    // Discard in beta through the confirmation: the question names beta, the answer acts on beta only.
    wb.update(cx, |wb, cx| wb.refresh_repo(b, cx));
    wait_ticking(cx, "beta refreshed", |cx| scm.read_with(cx, |s, _| s.repos[1].status.as_ref().is_some_and(|st| st.staged().next().is_none())));
    let change = scm.read_with(cx, |s, _| s.repos[1].status.as_ref().unwrap().changes.iter().find(|c| c.path == "same.txt").cloned().unwrap());
    scm.update_in(cx, |s, window, cx| s.confirm_discard_in(Some(b), vec![change], window, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Discard");
    wait_idle(cx, &scm, &f.beta);
    assert_eq!(std::fs::read_to_string(f.beta.join("same.txt")).unwrap(), "one\n");
    assert_eq!(std::fs::read_to_string(f.alpha.join("same.txt")).unwrap(), "one\nalpha\n");
    // Opening a row: its repository becomes the selected one and the diff is that repository's file.
    std::fs::write(f.beta.join("same.txt"), "one\nbeta\nbeta\nbeta\n").unwrap();
    wb.update(cx, |wb, cx| wb.refresh_repo(b, cx));
    wait_ticking(cx, "beta changed again", |cx| scm.read_with(cx, |s, _| s.repos[1].changes() == 2));
    scm.update(cx, |_, cx| cx.emit(ScmEvent::OpenRepoDiff { repo: b, path: "same.txt".into(), staged: false }));
    wait_ticking(cx, "beta diff", |cx| wb.read_with(cx, |wb, cx| wb.diff_view().is_some_and(|d| d.read(cx).summary().0 == 3)));
    assert_eq!(selected_root(&wb, cx), Some(s(&f.beta)));
    // A row of a repository that is gone does nothing anywhere else.
    scm.update(cx, |s, cx| s.stage_in(Some(9_999), vec!["same.txt".into()], cx));
    assert_eq!(scm.read_with(cx, |s, _| s.result_text()).as_deref(), Some("Repository changed while the operation was pending."));
    cx.run_until_parked();
    assert_eq!(staged_names(&f.beta).trim(), "");
}

/// AC3: commit, sync and history use the selected repository only (with its own remote), and the confirmation names it.
#[gpui::test]
fn commit_sync_and_history_stay_on_the_selected_repository(cx: &mut TestAppContext) {
    let f = fixture("commit");
    let bare = f.w.join("beta-origin.git");
    git_in(&f.w, &["init", "-q", "--bare", bare.to_str().unwrap()]);
    git_in(&f.beta, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git_in(&f.beta, &["push", "-q", "-u", "origin", "main"]);
    std::fs::write(f.alpha.join("same.txt"), "alpha edit\n").unwrap();
    std::fs::write(f.beta.join("same.txt"), "beta edit\n").unwrap();
    let (wb, scm, cx) = open(cx, &f.w, 3);
    let b = key(&scm, cx, &f.beta);
    let alpha_head = head(&f.alpha);
    wb.update(cx, |wb, cx| {
        wb.select_repository(b, cx);
    });
    wait_ticking(cx, "beta selected", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.root == s_(&f.beta) && !st.changes.is_empty())));
    scm.update(cx, |s, cx| {
        s.stage(vec!["same.txt".into()], cx);
    });
    wait_idle(cx, &scm, &f.beta);
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "staged", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.staged().count() == 1)));
    scm.update(cx, |s, cx| {
        s.set_message("beta commit", cx);
        s.commit(cx);
    });
    wait_idle(cx, &scm, &f.beta);
    assert_eq!(git_in(&f.beta, &["log", "-1", "--format=%s"]).trim(), "beta commit");
    assert_eq!(head(&f.alpha), alpha_head, "alpha untouched");
    assert_eq!(git_in(&f.alpha, &["status", "--porcelain"]).trim(), "M same.txt", "alpha's change and index untouched");
    // History: the graph is beta's.
    wait_ticking(cx, "beta graph", |cx| scm.read_with(cx, |s, _| s.graph_subjects().first().map(String::as_str) == Some("beta commit")));
    assert!(!scm.read_with(cx, |s, _| s.graph_subjects()).iter().any(|c| c == "init alpha"));
    // Sync pushes beta to beta's remote, and the confirmation names beta.
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "ahead", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|s| s.ahead == 1)));
    cx.update(|_, cx| cx.global_mut::<Settings>().git_confirm_sync = true);
    scm.update(cx, |_, cx| cx.emit(ScmEvent::Sync));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "the sync asks first");
    cx.simulate_prompt_answer("Sync");
    wait_ticking(cx, "pushed", |cx| wb.read_with(cx, |wb, _| !wb.sync_running_for_test()));
    assert_eq!(git_in(&bare, &["log", "-1", "--format=%s", "main"]).trim(), "beta commit");
    assert_eq!(head(&f.alpha), alpha_head);
}

fn s_(p: &Path) -> String {
    s(p)
}

/// AC3/AC5: a confirmation answered after the selection moved applies to nothing else.
#[gpui::test]
fn a_sync_confirmed_after_switching_repositories_is_not_redirected(cx: &mut TestAppContext) {
    let f = fixture("sync-switch");
    let bare = f.w.join("alpha-origin.git");
    git_in(&f.w, &["init", "-q", "--bare", bare.to_str().unwrap()]);
    git_in(&f.alpha, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git_in(&f.alpha, &["push", "-q", "-u", "origin", "main"]);
    git_in(&f.alpha, &["commit", "-q", "--allow-empty", "-m", "local"]);
    let (wb, scm, cx) = open(cx, &f.w, 3);
    wait_ticking(cx, "ahead", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|s| s.ahead == 1)));
    cx.update(|_, cx| cx.global_mut::<Settings>().git_confirm_sync = true);
    scm.update(cx, |_, cx| cx.emit(ScmEvent::Sync));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    let b = key(&scm, cx, &f.beta);
    wb.update(cx, |wb, cx| {
        wb.select_repository(b, cx);
    });
    cx.simulate_prompt_answer("Sync");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()).as_deref(), Some("Repository changed while the operation was pending."));
    assert_eq!(git_in(&bare, &["log", "-1", "--format=%s", "main"]).trim(), "init alpha", "nothing pushed");
}

/// AC4: merge conflict, rebase, detached HEAD and no upstream, each in its own repository, all shown correctly at once.
#[gpui::test]
fn repository_states_are_independent(cx: &mut TestAppContext) {
    let f = fixture("states");
    let conflict = |dir: &Path, cmd: &[&str]| {
        git_in(dir, &["checkout", "-q", "-b", "side"]);
        std::fs::write(dir.join("same.txt"), "side\n").unwrap();
        git_in(dir, &["commit", "-q", "-am", "side"]);
        git_in(dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("same.txt"), "main\n").unwrap();
        git_in(dir, &["commit", "-q", "-am", "main"]);
        let _ = crate::test_git::git().args(cmd).current_dir(dir).output();
    };
    conflict(&f.alpha, &["merge", "-q", "side"]);
    conflict(&f.beta, &["rebase", "-q", "side"]);
    let commit = head(&f.nested);
    git_in(&f.nested, &["checkout", "-q", "--detach", commit.trim()]);
    let (wb, scm, cx) = open(cx, &f.w, 3);
    wb.update(cx, |wb, cx| wb.refresh_repos(cx));
    wait_ticking(cx, "operation states", |cx| {
        scm.read_with(cx, |s, _| s.repos.iter().map(|r| r.op).collect::<Vec<_>>() == vec![OpState::Merging, OpState::Rebasing, OpState::Idle])
    });
    let repos = scm.read_with(cx, |s, _| s.repos.clone());
    let summaries: Vec<String> = repos.iter().map(summary).collect();
    assert!(summaries[0].starts_with("main · no upstream · Merge in progress · 1 conflict"), "{}", summaries[0]);
    assert!(summaries[1].contains("Rebase in progress") && summaries[1].contains("1 conflict"), "{}", summaries[1]);
    assert!(summaries[2].starts_with("detached HEAD") && !summaries[2].contains("conflicts"), "{}", summaries[2]);
    // The conflicted repositories show their files under Merge Changes; the detached one has none.
    let lines = scm.read_with(cx, |s, _| s.lines_for_test());
    assert!(lines.contains(&Line::Group(0, Group::Merge)) && lines.contains(&Line::Group(1, Group::Merge)));
    assert!(lines.contains(&Line::Note(2)));
    // Resolving alpha's conflict (staging it) changes alpha's state only.
    std::fs::write(f.alpha.join("same.txt"), "resolved\n").unwrap();
    let a = key(&scm, cx, &f.alpha);
    scm.update(cx, |s, cx| s.stage_in(Some(a), vec!["same.txt".into()], cx));
    wait_idle(cx, &scm, &f.alpha);
    wb.update(cx, |wb, cx| wb.refresh_repos(cx));
    wait_ticking(cx, "alpha resolved", |cx| scm.read_with(cx, |s, _| s.repos[0].status.as_ref().is_some_and(|st| st.unstaged().next().is_none())));
    let after = scm.read_with(cx, |s, _| s.repos.clone());
    assert_eq!(after[1].op, OpState::Rebasing);
    assert!(Arc::ptr_eq(after[1].status.as_ref().unwrap(), repos[1].status.as_ref().unwrap()), "beta's state was not touched");
}

/// AC5: a repository removed, replaced, or deselected while its operation is pending: nothing reaches another
/// repository or the new one at the same path, and the panel says what happened.
#[gpui::test]
fn stale_operations_never_reach_another_or_a_replaced_repository(cx: &mut TestAppContext) {
    let f = fixture("stale");
    std::fs::write(f.alpha.join("same.txt"), "alpha\n").unwrap();
    std::fs::write(f.beta.join("same.txt"), "beta\n").unwrap();
    let (wb, scm, cx) = open(cx, &f.w, 3);
    wb.update(cx, |wb, cx| wb.refresh_repos(cx));
    wait_ticking(cx, "changes", |cx| scm.read_with(cx, |s, _| s.repos[1].changes() == 1));
    let b = key(&scm, cx, &f.beta);
    // Replace beta while staging in it is pending: the stage must not land in the new repository at the same path.
    scm.update(cx, |s, cx| s.stage_in(Some(b), vec!["same.txt".into()], cx));
    std::fs::remove_dir_all(&f.beta).unwrap();
    make_repo(&f.beta);
    std::fs::write(f.beta.join("same.txt"), "replacement\n").unwrap();
    wait_idle(cx, &scm, &f.beta);
    assert_eq!(staged_names(&f.beta).trim(), "", "nothing staged in the replacement");
    assert_eq!(staged_names(&f.alpha).trim(), "");
    let shown = scm.read_with(cx, |s, _| s.result_text()).unwrap_or_default();
    assert_eq!(shown, format!("Repository is no longer available: {}", s(&f.beta)));
    // The inventory is searched again: beta has a new identity, and the old key no longer resolves.
    wait_ticking(cx, "new identity", |cx| scm.read_with(cx, |s, _| s.repos.iter().any(|r| r.id.worktree == s_(&f.beta) && r.id.key != b)));
    scm.update(cx, |s, cx| s.stage_in(Some(b), vec!["same.txt".into()], cx));
    cx.run_until_parked();
    assert_eq!(staged_names(&f.beta).trim(), "");
    // Remove a repository while its operation has passed the check: the result is rejected, the panel is not changed
    // for another repository.
    wait_for_states(cx, &scm, 3);
    let b2 = key(&scm, cx, &f.beta);
    scm.update(cx, |s, cx| s.stage_in(Some(b2), vec!["same.txt".into()], cx));
    // The inventory drops beta before the result is applied (as a discovery that no longer finds it would).
    scm.update(cx, |s, cx| {
        let kept: Vec<Repo> = s.repos.iter().filter(|r| r.id.key != b2).cloned().collect();
        s.set_repos(Arc::new(kept), cx);
    });
    wait_idle(cx, &scm, &f.beta);
    assert_eq!(scm.read_with(cx, |s, _| s.result_text()).as_deref(), Some("Repository changed while the operation was pending."));
    assert!(scm.read_with(cx, |s, _| s.repo_result_for_test(b2).is_none()));
    // Deselected mid-commit: alpha's commit goes through, its post-commit push does not run against the new selection,
    // and the message box keeps the draft of the repository now selected.
    wait_for_states(cx, &scm, 3);
    let a = key(&scm, cx, &f.alpha);
    let b3 = key(&scm, cx, &f.beta);
    wb.update(cx, |wb, cx| {
        wb.select_repository(a, cx);
    });
    wait_ticking(cx, "alpha selected", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.root == s_(&f.alpha) && !st.changes.is_empty())));
    scm.update(cx, |s, cx| {
        s.set_message("alpha commit", cx);
        s.commit_and(git::PostCommit::Push, cx);
    });
    wb.update(cx, |wb, cx| {
        wb.select_repository(b3, cx);
    });
    scm.update(cx, |s, cx| s.set_message("beta draft", cx));
    wait_idle(cx, &scm, &f.alpha);
    assert_eq!(git_in(&f.alpha, &["log", "-1", "--format=%s"]).trim(), "alpha commit");
    assert_eq!(scm.read_with(cx, |s, cx| s.message_text(cx)), "beta draft");
    assert_eq!(scm.read_with(cx, |s, _| s.result_text()).as_deref(), Some("Repository changed while the operation was pending."));
    assert!(!wb.read_with(cx, |wb, _| wb.sync_running_for_test()), "no push started for beta");
    assert_eq!(scm.read_with(cx, |s, _| s.repo_result_for_test(a)).map(|r| r.starts_with("Committed")), Some(true));
    // Switching back shows alpha's own result and an empty draft (the committed one is gone).
    wb.update(cx, |wb, cx| {
        wb.select_repository(a, cx);
    });
    assert_eq!(scm.read_with(cx, |s, cx| s.message_text(cx)), "");
    assert!(scm.read_with(cx, |s, _| s.result_text()).is_some_and(|r| r.starts_with("Committed")));
    wb.update(cx, |wb, cx| {
        wb.select_repository(b3, cx);
    });
    assert_eq!(scm.read_with(cx, |s, cx| s.message_text(cx)), "beta draft");
}

/// AC5: the picked repository is deleted; Source Control does not quietly show the enclosing repository instead.
#[gpui::test]
fn a_deleted_picked_repository_is_let_go_of(cx: &mut TestAppContext) {
    let f = fixture("deleted");
    let (wb, scm, cx) = open(cx, &f.w, 3);
    let n = key(&scm, cx, &f.nested);
    wb.update(cx, |wb, cx| {
        wb.select_repository(n, cx);
    });
    wait_ticking(cx, "nested selected", |cx| selected_root(&wb, cx) == Some(s(&f.nested)));
    std::fs::remove_dir_all(f.nested.join(".git")).unwrap();
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "let go", |cx| scm.read_with(cx, |s, _| s.repos.len() == 2));
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()), Some(format!("Repository is no longer available: {}", s(&f.nested))));
    // Source Control says so, and goes back to the first repository of the inventory, not to the enclosing one by accident.
    wait_ticking(cx, "first repository", |cx| selected_root(&wb, cx) == Some(s(&f.alpha)));
    assert!(scm.read_with(cx, |s, _| s.repos.iter().all(|r| r.id.worktree != s_(&f.nested))));
}

/// The graph on screen is the one of the repository whose first commit is `want`, with nothing of the other one.
fn shows(s: &ScmPanel, want: &str, other: &str) -> bool {
    let subjects = s.graph_subjects();
    subjects.iter().any(|c| c == want) && !subjects.iter().any(|c| c == other)
}

fn stash_in(dir: &Path, text: &str, message: &str) {
    std::fs::write(dir.join("same.txt"), text).unwrap();
    git_in(dir, &["stash", "push", "-q", "-m", message]);
}

fn stash_messages(dir: &Path) -> Vec<String> {
    git_in(dir, &["stash", "list", "--format=%s"]).lines().map(str::to_string).collect()
}

fn select(wb: &Entity<Workbench>, cx: &mut VisualTestContext, key: RepoKey) {
    wb.update(cx, |wb, cx| {
        wb.select_repository(key, cx);
    });
}

/// AC3/AC5: switching while the history loads. The previous repository's commits and stashes leave the panel at the
/// switch (nothing of them can be acted on), a load that finishes late for it is thrown away, and the panel then shows
/// the selected repository's history and stashes only.
#[gpui::test]
fn a_switch_while_the_history_loads_never_shows_the_previous_history(cx: &mut TestAppContext) {
    let f = fixture("history-switch");
    stash_in(&f.alpha, "alpha stash\n", "alpha work");
    let (wb, scm, cx) = open(cx, &f.w, 3);
    wait_ticking(cx, "alpha history", |cx| scm.read_with(cx, |s, _| shows(s, "init alpha", "init beta") && s.stash_count() == 1));
    let (a, b) = (key(&scm, cx, &f.alpha), key(&scm, cx, &f.beta));
    assert_eq!(scm.read_with(cx, |s, _| s.history_target().map(|t| t.root)), Some(s(&f.alpha)));
    // Start a history load for alpha, then switch to beta before it finishes (no parking in between).
    scm.update(cx, |s, cx| s.load_graph(cx));
    assert!(scm.read_with(cx, |s, _| s.graph_loading));
    select(&wb, cx, b);
    scm.read_with(cx, |s, _| {
        assert!(s.graph.is_none() && s.stashes.is_empty() && s.history.is_none(), "alpha's history left at the switch");
    });
    wait_ticking(cx, "beta history", |cx| scm.read_with(cx, |s, _| shows(s, "init beta", "init alpha")));
    cx.run_until_parked();
    scm.read_with(cx, |s, _| {
        assert_eq!(s.stash_count(), 0, "alpha's stash is not listed for beta");
        assert_eq!(s.history_target().map(|t| t.root), Some(s_(&f.beta)));
    });
    // Back and forth quickly: whatever finishes last, only the selected repository's history stays.
    select(&wb, cx, a);
    select(&wb, cx, b);
    select(&wb, cx, a);
    wait_ticking(cx, "alpha again", |cx| scm.read_with(cx, |s, _| shows(s, "init alpha", "init beta") && s.stash_count() == 1));
    cx.run_until_parked();
    assert_eq!(scm.read_with(cx, |s, _| s.history_target().map(|t| t.root)), Some(s(&f.alpha)));
}

/// AC5: a stash button rendered for alpha and clicked after beta was selected acts on alpha's stash or not at all:
/// beta's stash at the same position is never touched, and a shifted position in alpha is refused.
#[gpui::test]
fn a_stash_drop_after_a_switch_never_touches_the_other_repository(cx: &mut TestAppContext) {
    let f = fixture("stash-switch");
    stash_in(&f.alpha, "alpha stash\n", "alpha work");
    stash_in(&f.beta, "beta stash\n", "beta work");
    let (wb, scm, cx) = open(cx, &f.w, 3);
    wait_ticking(cx, "alpha stashes", |cx| scm.read_with(cx, |s, _| s.stash_count() == 1 && s.history_target().is_some()));
    // What the Drop button of alpha's row holds (see `stash_section`): the history target and the entry.
    let (pinned, entry) = scm.read_with(cx, |s, _| (s.history_target(), s.stashes[0].clone()));
    assert_eq!(pinned.as_ref().map(|t| t.root.clone()), Some(s(&f.alpha)));
    let b = key(&scm, cx, &f.beta);
    select(&wb, cx, b);
    wait_ticking(cx, "beta stashes", |cx| scm.read_with(cx, |s, _| s.history_target().is_some_and(|t| t.root == s_(&f.beta))));
    // Another stash in alpha meanwhile: stash@{0} is no longer the entry that was shown.
    stash_in(&f.alpha, "alpha later\n", "alpha later");
    scm.update(cx, |s, cx| s.stash_action_in(pinned.clone(), entry.0.clone(), Some(entry.1.clone()), StashAction::Drop, cx));
    wait_idle(cx, &scm, &f.alpha);
    assert_eq!(stash_messages(&f.alpha).len(), 2, "a shifted entry is not dropped");
    assert_eq!(stash_messages(&f.beta).len(), 1, "beta untouched");
    assert_eq!(
        scm.read_with(cx, |s, _| s.repo_result_for_test(pinned.as_ref().unwrap().id.as_ref().unwrap().key)).as_deref(),
        Some("Repository changed while the operation was pending.")
    );
    // The entry still at its position: dropped in alpha, beta's stash at the same position stays.
    let (name, msg) = ("stash@{1}".to_string(), stash_messages(&f.alpha)[1].clone());
    scm.update(cx, |s, cx| s.stash_action_in(pinned.clone(), name, Some(msg), StashAction::Drop, cx));
    wait_idle(cx, &scm, &f.alpha);
    assert_eq!(stash_messages(&f.alpha).len(), 1);
    assert!(stash_messages(&f.alpha)[0].contains("alpha later"));
    assert_eq!(stash_messages(&f.beta).len(), 1, "beta untouched");
}

/// AC3/AC5: a hard reset asked for alpha and confirmed after beta was selected resets alpha (the repository the
/// question named), never beta.
#[gpui::test]
fn a_hard_reset_confirmed_after_the_selection_moved_stays_in_its_repository(cx: &mut TestAppContext) {
    let f = fixture("reset-switch");
    let first = head(&f.alpha).trim().to_string();
    git_in(&f.alpha, &["commit", "-q", "--allow-empty", "-m", "alpha second"]);
    git_in(&f.beta, &["commit", "-q", "--allow-empty", "-m", "beta second"]);
    let beta_head = head(&f.beta);
    let (wb, scm, cx) = open(cx, &f.w, 3);
    wait_ticking(cx, "alpha history", |cx| scm.read_with(cx, |s, _| s.graph_subjects().first().map(String::as_str) == Some("alpha second")));
    wb.update_in(cx, |wb, window, cx| {
        wb.scm_panel().update(cx, |s, cx| s.commit_op(first.clone(), crate::graph::CommitOp::Reset(crate::graph::ResetMode::Hard), window, cx))
    });
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    // Cancel resets nothing and leaves the keyboard where it was; the question is asked again.
    crate::question::cancel_keeps_focus(&wb, cx, "Cancel");
    assert_ne!(head(&f.alpha).trim(), first, "Cancel resets nothing");
    wb.update_in(cx, |wb, window, cx| {
        wb.scm_panel().update(cx, |s, cx| s.commit_op(first.clone(), crate::graph::CommitOp::Reset(crate::graph::ResetMode::Hard), window, cx))
    });
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    let b = key(&scm, cx, &f.beta);
    select(&wb, cx, b);
    cx.simulate_prompt_answer("Reset");
    wait_idle(cx, &scm, &f.alpha);
    assert_eq!(head(&f.alpha).trim(), first, "alpha was reset");
    assert_eq!(head(&f.beta), beta_head, "beta untouched");
}

/// AC3/AC5: a linked worktree shares every commit with its main checkout, so a commit hash proves nothing about the
/// repository. A history action from the main checkout's menu, picked after the linked one was selected, still runs
/// in the main checkout.
#[gpui::test]
fn history_actions_of_linked_worktrees_stay_in_their_checkout(cx: &mut TestAppContext) {
    let w = std::env::temp_dir().join(format!("codeyard-multi-linked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&w);
    std::fs::create_dir_all(&w).unwrap();
    let w = crate::ui::canonical_for_test(&w);
    // In the form the workbench opens folders (`C:/…/main` on Windows, not `\\?\C:\…`, which `git worktree add` rejects).
    let main = PathBuf::from(format!("{}/main", crate::ui::local_path(&w)));
    make_repo(&main);
    git_in(&main, &["commit", "-q", "--allow-empty", "-m", "second"]);
    let side = PathBuf::from(format!("{}/side", s(&main)));
    git_in(&main, &["worktree", "add", "-q", "-b", "side", side.to_str().unwrap()]);
    std::fs::write(main.join(".git/info/exclude"), "side/\n").unwrap();
    let (wb, scm, cx) = open(cx, &main, 2);
    wait_ticking(cx, "main history", |cx| scm.read_with(cx, |s, _| s.graph_subjects().first().map(String::as_str) == Some("second")));
    let commit = scm.read_with(cx, |s, _| s.graph_commit(1).unwrap());
    // The commit menu opens on the main checkout's history.
    scm.update(cx, |_, cx| cx.emit(ScmEvent::CommitMenu(commit, gpui::point(gpui::px(10.), gpui::px(10.)))));
    cx.run_until_parked();
    let linked = key(&scm, cx, &side);
    select(&wb, cx, linked);
    wb.update(cx, |wb, cx| wb.select_menu_item_for_test("Checkout (Detached)", cx));
    wait_idle(cx, &scm, &main);
    cx.run_until_parked();
    assert_eq!(git_in(&main, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "HEAD", "the main checkout is detached");
    assert_eq!(git_in(&side, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "side", "the linked checkout is untouched");
    let _ = std::fs::remove_dir_all(w);
}

/// AC3/AC5: the branch picker, the stash picker and the named stash input, each opened on alpha and confirmed after
/// beta was selected: nothing runs in beta (the branch checkout and the named stash are refused; the picked stash is
/// alpha's and is applied to alpha).
#[gpui::test]
fn pickers_confirmed_after_a_switch_do_not_act_on_the_new_selection(cx: &mut TestAppContext) {
    let f = fixture("pickers");
    git_in(&f.alpha, &["branch", "alpha-only"]);
    git_in(&f.beta, &["branch", "alpha-only"]);
    stash_in(&f.alpha, "alpha stash\n", "alpha work");
    stash_in(&f.beta, "beta stash\n", "beta work");
    let (wb, scm, cx) = open(cx, &f.w, 3);
    let b = key(&scm, cx, &f.beta);
    let beta_status = || git_in(&f.beta, &["status", "--porcelain"]);
    let before = beta_status();
    // Branch picker on alpha, confirmed on beta.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Checkout to…", window, cx));
    wait_ticking(cx, "branch list", |cx| wb.read_with(cx, |wb, cx| wb.overlay_query(cx).is_some()));
    select(&wb, cx, b);
    cx.simulate_input("alpha-only");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()).as_deref(), Some("Repository changed while the operation was pending."));
    assert_eq!(git_in(&f.beta, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "main", "beta stays on its branch");
    assert_eq!(git_in(&f.alpha, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "main");
    // Stash picker (pop) on alpha, confirmed on beta: alpha's stash is popped in alpha, beta's stash stays.
    let a = key(&scm, cx, &f.alpha);
    select(&wb, cx, a);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Pop Stash…", window, cx));
    wait_ticking(cx, "stash list", |cx| wb.read_with(cx, |wb, cx| wb.overlay_query(cx).is_some()));
    select(&wb, cx, b);
    cx.simulate_keystrokes("enter");
    wait_idle(cx, &scm, &f.alpha);
    cx.run_until_parked();
    assert!(stash_messages(&f.alpha).is_empty(), "alpha's stash popped in alpha");
    assert_eq!(std::fs::read_to_string(f.alpha.join("same.txt")).unwrap(), "alpha stash\n");
    assert_eq!(stash_messages(&f.beta).len(), 1, "beta's stash untouched");
    assert_eq!(beta_status(), before);
    // Named stash input on beta... opened on alpha, confirmed on beta: refused.
    select(&wb, cx, a);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Stash with Name…", window, cx));
    cx.run_until_parked();
    select(&wb, cx, b);
    std::fs::write(f.beta.join("same.txt"), "beta change\n").unwrap();
    cx.simulate_input("named");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()).as_deref(), Some("Repository changed while the operation was pending."));
    assert_eq!(stash_messages(&f.beta).len(), 1, "no stash made in beta");
    assert!(stash_messages(&f.alpha).is_empty(), "no stash made in alpha either");
}

/// Polling with many repositories: at most `MAX_REFRESHES` refreshes run at once, the selected repository is left
/// to the main refresh, and a repository still waiting from the last poll is not queued twice.
#[gpui::test]
fn polling_many_repositories_is_bounded(cx: &mut TestAppContext) {
    let w = std::env::temp_dir().join(format!("codeyard-multi-poll-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&w);
    std::fs::create_dir_all(&w).unwrap();
    let w = crate::ui::canonical_for_test(&w);
    for i in 0..24 {
        let d = w.join(format!("r{i:02}"));
        std::fs::create_dir_all(&d).unwrap();
        git_in(&d, &["init", "-q"]);
    }
    let (wb, scm, cx) = open(cx, &w, 24);
    let (_, started, _) = wb.read_with(cx, |wb, _| wb.repo_counters_for_test());
    // Two polls in a row, before anything finishes: 23 refreshes at most (selected excluded), 8 at a time.
    wb.update(cx, |wb, cx| {
        wb.refresh_repos(cx);
        wb.refresh_repos(cx);
    });
    let max = crate::workbench::repos_glue::MAX_REFRESHES;
    let (_, now, peak) = wb.read_with(cx, |wb, _| wb.repo_counters_for_test());
    assert_eq!(now - started, max, "only a full set of slots starts at once");
    let (running, waiting) = wb.read_with(cx, |wb, _| wb.repo_queue_for_test());
    let selected = key(&scm, cx, &w.join("r00"));
    assert_eq!(running.len(), max);
    assert_eq!(running.len() + waiting.len(), 23, "every repository but the selected one, each queued once over two polls");
    assert!(!running.contains(&selected) && !waiting.contains(&selected), "the selected repository is left to the main refresh");
    let unique: std::collections::HashSet<_> = waiting.iter().collect();
    assert_eq!(unique.len(), waiting.len(), "no repository waits twice");
    cx.run_until_parked();
    let (_, _, peak_after) = wb.read_with(cx, |wb, _| wb.repo_counters_for_test());
    assert!(peak <= max && peak_after <= max, "never more than {max} at once ({peak_after})");
    assert!(wb.read_with(cx, |wb, _| wb.repo_queue_for_test()).1.is_empty(), "the queue drains");
    assert_eq!(scm.read_with(cx, |s, _| s.repos.len()), 24);
    let _ = std::fs::remove_dir_all(w);
}

/// Discovery runs when the roots change and when an entry is gone, not on every refresh: repeated Refresh clicks list
/// the workspace again only once the last search is old.
#[gpui::test]
fn repeated_refreshes_do_not_search_the_workspace_again(cx: &mut TestAppContext) {
    let f = fixture("rediscover");
    let (wb, scm, cx) = open(cx, &f.w, 3);
    let (searches, ..) = wb.read_with(cx, |wb, _| wb.repo_counters_for_test());
    let refresh = cx.debug_bounds("scm-refresh").unwrap().center();
    wb.update(cx, |wb, _| wb.age_discovery_for_test(false));
    for _ in 0..5 {
        cx.simulate_click(refresh, Modifiers::none());
        cx.run_until_parked();
    }
    assert_eq!(wb.read_with(cx, |wb, _| wb.repo_counters_for_test().0), searches, "no search for a plain refresh");
    // After a while a refresh does search (a repository created in a terminal shows up).
    make_repo(&f.w.join("gamma"));
    wb.update(cx, |wb, _| wb.age_discovery_for_test(true));
    cx.simulate_click(refresh, Modifiers::none());
    wait_for_states(cx, &scm, 4);
    assert_eq!(wb.read_with(cx, |wb, _| wb.repo_counters_for_test().0), searches + 1);
    // A repository that is gone triggers one search (not one per refresh of it).
    std::fs::remove_dir_all(f.beta.join(".git")).unwrap();
    let b = key(&scm, cx, &f.beta);
    wb.update(cx, |wb, cx| {
        wb.refresh_repo(b, cx);
        wb.refresh_inventory_for_test(cx);
    });
    wait_ticking(cx, "beta gone", |cx| scm.read_with(cx, |s, _| s.repos.len() == 3));
    assert_eq!(wb.read_with(cx, |wb, _| wb.repo_counters_for_test().0), searches + 2);
}

/// The status bar's branch follows the selected repository, also through fast selection changes (each repository's
/// branch comes from its own status, never from a late read for an earlier one).
#[gpui::test]
fn the_status_bar_branch_follows_fast_selection(cx: &mut TestAppContext) {
    let f = fixture("branch-bar");
    git_in(&f.beta, &["checkout", "-q", "-b", "beta-branch"]);
    git_in(&f.nested, &["checkout", "-q", "-b", "nested-branch"]);
    let (wb, scm, cx) = open(cx, &f.w, 3);
    let row = cx.debug_bounds("scm-repo-row-0").unwrap();
    cx.simulate_click(row.center(), Modifiers::none());
    cx.run_until_parked();
    cx.simulate_keystrokes("down down");
    wait_ticking(cx, "nested selected", |cx| selected_root(&wb, cx) == Some(s(&f.nested)));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.status_bar_branch_for_test()).as_deref(), Some("nested-branch"));
    cx.simulate_keystrokes("up");
    wait_ticking(cx, "beta selected", |cx| selected_root(&wb, cx) == Some(s(&f.beta)));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.status_bar_branch_for_test()).as_deref(), Some("beta-branch"));
    assert!(scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.branch.as_deref() == Some("beta-branch"))));
}

/// The draft typed for a repository that disappears goes with it: the repository shown instead starts with its own
/// draft, and the text comes back when the repository does.
#[gpui::test]
fn a_draft_stays_with_a_repository_that_disappears(cx: &mut TestAppContext) {
    let f = fixture("draft-gone");
    let (wb, scm, cx) = open(cx, &f.w, 3);
    let n = key(&scm, cx, &f.nested);
    select(&wb, cx, n);
    wait_ticking(cx, "nested selected", |cx| scm.read_with(cx, |s, _| s.status.as_ref().is_some_and(|st| st.root == s_(&f.nested))));
    scm.update(cx, |s, cx| s.set_message("nested draft", cx));
    let saved = f.w.join("nested-git");
    std::fs::rename(f.nested.join(".git"), &saved).unwrap();
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "fallback", |cx| selected_root(&wb, cx) == Some(s(&f.alpha)) && scm.read_with(cx, |s, _| s.repos.len() == 2));
    cx.run_until_parked();
    assert_eq!(scm.read_with(cx, |s, cx| s.message_text(cx)), "", "alpha does not get nested's draft");
    // The repository comes back (same git directory, same identity): its draft returns with it.
    std::fs::rename(&saved, f.nested.join(".git")).unwrap();
    wb.update(cx, |wb, cx| wb.discover_repos(cx));
    wait_for_states(cx, &scm, 3);
    let n = key(&scm, cx, &f.nested);
    select(&wb, cx, n);
    assert_eq!(scm.read_with(cx, |s, cx| s.message_text(cx)), "nested draft");
}

/// AC6: an external change in one repository refreshes that repository only; the others keep their state (same Arc),
/// the selection and the selected repository's history stay.
#[gpui::test]
fn an_external_change_refreshes_only_its_repository(cx: &mut TestAppContext) {
    let f = fixture("invalidate");
    let (wb, scm, cx) = open(cx, &f.w, 3);
    wait_ticking(cx, "graph", |cx| scm.read_with(cx, |s, _| !s.graph_subjects().is_empty()));
    let before = scm.read_with(cx, |s, _| s.repos.clone());
    let graph_before = scm.read_with(cx, |s, _| s.graph.clone().unwrap());
    let b = key(&scm, cx, &f.beta);
    // A branch and a commit made outside the editor.
    git_in(&f.beta, &["checkout", "-q", "-b", "outside"]);
    git_in(&f.beta, &["commit", "-q", "--allow-empty", "-m", "outside"]);
    wb.update(cx, |wb, cx| wb.refresh_repos_for_test(&[format!("{}/.git/HEAD", s(&f.beta))], cx));
    wait_ticking(cx, "beta branch", |cx| scm.read_with(cx, |s, _| s.repos[1].status.as_ref().unwrap().branch.as_deref() == Some("outside")));
    let after = scm.read_with(cx, |s, _| s.repos.clone());
    assert!(Arc::ptr_eq(after[0].status.as_ref().unwrap(), before[0].status.as_ref().unwrap()), "alpha untouched");
    assert!(Arc::ptr_eq(after[2].status.as_ref().unwrap(), before[2].status.as_ref().unwrap()), "nested untouched");
    assert_eq!(after[1].id.key, b, "identity survives a refresh");
    assert_eq!(selected_root(&wb, cx), Some(s(&f.alpha)), "selection kept");
    assert!(Arc::ptr_eq(&scm.read_with(cx, |s, _| s.graph.clone().unwrap()), &graph_before), "alpha's history not reloaded");
}

/// AC8: the repository list by keyboard (arrows move the selection, focus stays on the list) and by AccessKit
/// (options with their state; a click selects; a stale option does nothing).
#[gpui::test]
fn repositories_are_selectable_by_keyboard_and_accessibility(cx: &mut TestAppContext) {
    use accesskit::{Action, ActionRequest, Role, TreeId};
    let f = fixture("a11y");
    let (wb, scm, cx) = open(cx, &f.w, 3);
    let row = cx.debug_bounds("scm-repo-row-0").unwrap();
    cx.simulate_click(row.center(), Modifiers::none());
    cx.run_until_parked();
    cx.simulate_keystrokes("down");
    wait_ticking(cx, "beta by keyboard", |cx| selected_root(&wb, cx) == Some(s(&f.beta)));
    cx.simulate_keystrokes("down");
    wait_ticking(cx, "nested by keyboard", |cx| selected_root(&wb, cx) == Some(s(&f.nested)));
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(selected_root(&wb, cx), Some(s(&f.nested)), "stops at the end");
    cx.simulate_keystrokes("home");
    wait_ticking(cx, "home", |cx| selected_root(&wb, cx) == Some(s(&f.alpha)));
    let focused = cx.update(|window, cx| scm.read(cx).repo_focus.is_focused(window));
    assert!(focused, "focus stays on the repository list");
    // Collapse alpha's rows with the keyboard.
    cx.simulate_keystrokes("left");
    let a = key(&scm, cx, &f.alpha);
    assert!(scm.read_with(cx, |s, _| s.collapsed_repos.contains(&a)));
    cx.simulate_keystrokes("right");
    assert!(!scm.read_with(cx, |s, _| s.collapsed_repos.contains(&a)));
    // AccessKit: a list of options, the selected one focused, each described with its state and path.
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert_eq!(snap.focus, crate::workbench::a11y::FocusSnap::ScmRepos);
    let tree = snap.to_tree();
    let options: Vec<_> = tree.nodes.iter().filter(|(_, n)| n.role() == Role::ListBoxOption && n.description().is_some_and(|d| d.contains(&s(&f.w)))).collect();
    assert_eq!(options.len(), 3);
    assert_eq!(options[0].1.label(), Some("alpha"));
    assert_eq!(options[0].1.is_selected(), Some(true));
    assert!(options[1].1.description().unwrap().contains("main · no upstream"));
    assert_eq!(tree.focus, options[0].0);
    assert!(tree.nodes.iter().any(|(_, n)| n.role() == Role::Label && n.value().is_some_and(|v| v.starts_with("alpha · main"))), "branch names the repository");
    let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: options[1].0, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
    wait_ticking(cx, "beta by AccessKit", |cx| selected_root(&wb, cx) == Some(s(&f.beta)));
    // The same (old) snapshot after the inventory changed: an option that now means another repository does nothing.
    let nested_option = options[2].0;
    scm.update(cx, |s, cx| {
        let kept: Vec<Repo> = s.repos.iter().filter(|r| r.id.worktree != s_(&f.nested)).cloned().collect();
        s.set_repos(Arc::new(kept), cx);
    });
    let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: nested_option, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
    cx.run_until_parked();
    assert_eq!(selected_root(&wb, cx), Some(s(&f.beta)));
}

/// AC7: hundreds of repositories with changes: discovery is bounded, runs off the UI thread, and the list renders only
/// the rows on screen.
#[gpui::test]
fn a_large_inventory_is_bounded_and_virtualized(cx: &mut TestAppContext) {
    let w = std::env::temp_dir().join(format!("codeyard-multi-large-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&w);
    std::fs::create_dir_all(&w).unwrap();
    let w = crate::ui::canonical_for_test(&w);
    for i in 0..120 {
        let d = w.join(format!("r{i:03}"));
        std::fs::create_dir_all(&d).unwrap();
        git_in(&d, &["init", "-q"]);
        for j in 0..5 {
            std::fs::write(d.join(format!("f{j}.txt")), "x\n").unwrap();
        }
    }
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, hot_exit: false, ..Default::default() });
    wb.update(cx, |wb, _| wb.set_repo_limits_for_test(crate::repos::Limits { repos: 40, ..Default::default() }));
    wb.update_in(cx, |wb, window, cx| {
        wb.open_path(w.to_str().unwrap(), window, cx);
        wb.run_command_for_test("View: Source Control", window, cx);
    });
    // The search is a task on the background executor: the UI update that starts it returns before anything is listed
    // (in the app that executor's threads do the listing; the test executor runs it later, on its own schedule).
    let listed_inside = wb.update(cx, |wb, cx| {
        wb.discover_repos(cx);
        (wb.repo_inventory().discovering, wb.repo_inventory().repos.len())
    });
    assert_eq!(listed_inside, (true, 0), "the update returned with the search still pending and nothing listed");
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    crate::qa_integration_tests::wait_secs(cx, 120, "large inventory", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        scm.read_with(cx, |s, _| s.repos.len() == 40 && s.repos.iter().all(|r| r.status.is_some()))
    });
    assert!(wb.read_with(cx, |wb, _| wb.repo_inventory().truncated));
    let lines = scm.read_with(cx, |s, _| s.lines_for_test().len());
    assert!(lines >= 40 * 7, "{lines} rows");
    cx.run_until_parked();
    let started = std::time::Instant::now();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(cx.debug_bounds("scm-repo-row-0").is_some());
    assert!(cx.debug_bounds("scm-repo-row-39").is_none(), "rows off screen are not built");
    let _ = std::fs::remove_dir_all(w);
}

/// SSH: the inventory and a row's action run on the remote host, in the row's repository.
#[gpui::test]
fn repositories_are_discovered_and_targeted_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("multi-repo-ssh") else { return };
    let f = fixture("ssh");
    std::fs::write(f.beta.join("same.txt"), "remote beta\n").unwrap();
    std::fs::write(f.alpha.join("same.txt"), "remote alpha\n").unwrap();
    let (wb, cx) = crate::qa_integration_tests::ssh_workbench(cx, &f.w);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Source Control", window, cx));
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    crate::qa_integration_tests::wait_secs(cx, 60, "remote inventory", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        scm.read_with(cx, |s, _| s.repos.len() == 3 && s.repos.iter().all(|r| r.status.is_some()))
    });
    assert!(scm.read_with(cx, |s, _| s.host.is_remote()));
    let b = scm.read_with(cx, |s, _| s.repos.iter().find(|r| r.id.worktree.ends_with("/beta")).map(|r| r.id.key)).unwrap();
    scm.update(cx, |s, cx| s.stage_in(Some(b), vec!["same.txt".into()], cx));
    crate::qa_integration_tests::wait_secs(cx, 60, "remote stage", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        scm.read_with(cx, |s, _| s.busy.is_empty())
    });
    assert_eq!(staged_names(&f.beta).trim(), "same.txt");
    assert_eq!(staged_names(&f.alpha).trim(), "");
}

#[test]
fn the_summary_reads_like_the_row() {
    use crate::repos::{Repo, RepoId};
    let st = git::Status { root: "/w/a".into(), branch: Some("main".into()), upstream: Some("origin/main".into()), ahead: 2, behind: 1, ..Default::default() };
    let repo = Repo {
        id: Arc::new(RepoId { key: 1, host_gen: 0, root: "/w".into(), worktree: "/w/a".into(), git_dir: "/w/a/.git".into(), fingerprint: 1 }),
        label: "a".into(),
        status: Some(Arc::new(st)),
        error: None,
        op: OpState::Idle,
    };
    assert_eq!(summary(&repo), format!("main · {} · 0 changes", git::counts_label(2, 1)));
    let failing = Repo { status: None, error: Some("Git is paused".into()), ..repo };
    assert_eq!(summary(&failing), "Git is paused");
}
