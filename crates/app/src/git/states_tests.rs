//! Git in the middle of something, or in a state that is not "a branch with commits" (#308): what the graph's operations do then.
//! Each case is a real repository; what must not happen is a half-done operation of the user's being lost or hidden.

use crate::git::repo_for_test as repo;
use crate::graph::{self, CommitOp};

fn commit(dir: &std::path::Path, run: &impl Fn(&[&str]) -> Option<String>, file: &str, content: &str, message: &str) -> String {
    std::fs::write(dir.join(file), content).unwrap();
    run(&["add", file]).unwrap();
    run(&["commit", "-q", "-m", message]).unwrap();
    run(&["rev-parse", "HEAD"]).unwrap().trim().to_string()
}

/// A repository where a cherry-pick of `theirs` conflicts and is left waiting for the user.
fn mid_cherry_pick(name: &str) -> (std::path::PathBuf, impl Fn(&[&str]) -> Option<String>, String) {
    let (dir, run) = repo(name).expect("git is available");
    let base = run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
    run(&["checkout", "-q", "-b", "other", &base]).unwrap();
    let theirs = commit(&dir, &run, "f.txt", "theirs\n", "theirs");
    run(&["checkout", "-q", "-"]).unwrap();
    commit(&dir, &run, "f.txt", "ours\n", "ours");
    assert!(run(&["cherry-pick", &theirs]).is_none(), "the cherry-pick stops at a conflict");
    assert!(dir.join(".git/CHERRY_PICK_HEAD").exists());
    (dir, run, theirs)
}

#[test]
fn a_cherry_pick_from_the_graph_never_aborts_the_one_the_user_is_in_the_middle_of() {
    let (dir, run, _theirs) = mid_cherry_pick("states-cherry");
    let other = commit_elsewhere(&dir, &run);
    let host = host::local();
    let before = std::fs::read_to_string(dir.join("f.txt")).unwrap();
    let error = graph::run_op(&host, &dir.to_string_lossy(), &other, &CommitOp::CherryPick).unwrap_err();
    assert!(error.contains("Cherry-pick in progress") && error.contains("finish or abort"), "{error}");
    assert!(dir.join(".git/CHERRY_PICK_HEAD").exists(), "the user's cherry-pick is still waiting");
    assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), before, "and so are its conflict markers");
    assert!(run(&["status", "--porcelain"]).unwrap().contains("UU f.txt"));
}

/// A commit on a side branch that does not touch `f.txt`, left checked out nowhere.
fn commit_elsewhere(dir: &std::path::Path, run: &impl Fn(&[&str]) -> Option<String>) -> String {
    let head = run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
    run(&["branch", "-f", "scratch", &head]).unwrap();
    let tree = run(&["rev-parse", "HEAD^{tree}"]).unwrap().trim().to_string();
    let _ = dir;
    run(&["commit-tree", &tree, "-p", &head, "-m", "elsewhere"]).unwrap().trim().to_string()
}

#[test]
fn a_revert_is_refused_while_a_merge_waits_and_the_merge_is_left_alone() {
    let (dir, run) = repo("states-merge").expect("git is available");
    let base = run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
    run(&["checkout", "-q", "-b", "side", &base]).unwrap();
    commit(&dir, &run, "f.txt", "side\n", "side");
    run(&["checkout", "-q", "-"]).unwrap();
    let target = commit(&dir, &run, "f.txt", "main\n", "main");
    assert!(run(&["merge", "side"]).is_none(), "the merge stops at a conflict");
    assert!(dir.join(".git/MERGE_HEAD").exists());
    let host = host::local();
    let error = graph::run_op(&host, &dir.to_string_lossy(), &target, &CommitOp::Revert).unwrap_err();
    assert!(error.contains("Merge in progress"), "{error}");
    assert!(dir.join(".git/MERGE_HEAD").exists(), "the merge is still there");
}

#[test]
fn operations_run_as_before_when_nothing_is_in_progress() {
    let (dir, run) = repo("states-idle").expect("git is available");
    let host = host::local();
    let a = commit(&dir, &run, "a.txt", "a", "a");
    commit(&dir, &run, "b.txt", "b", "b");
    run(&["checkout", "-q", "--detach", "HEAD~1"]).unwrap();
    let message = graph::run_op(&host, &dir.to_string_lossy(), &a, &CommitOp::Tag("kept".into())).unwrap();
    assert!(message.contains("kept"), "{message}");
}

fn empty_repo(name: &str) -> std::path::PathBuf {
    crate::test_git::hermetic();
    let dir = std::env::temp_dir().join(format!("codeyard-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(crate::test_git::git().args(["init", "-q"]).current_dir(&dir).status().unwrap().success());
    dir
}

#[test]
fn a_repository_without_commits_has_an_empty_graph_and_no_head() {
    let dir = empty_repo("states-unborn");
    let host = host::local();
    let root = dir.to_string_lossy().into_owned();
    assert_eq!(graph::log(&host, &root, 50, graph::Scope::All), Ok(Vec::new()), "no commits is an empty history, not an error");
    assert_eq!(graph::log(&host, &root, 50, graph::Scope::Current), Ok(Vec::new()));
    let status = crate::git::status_for(&host, &root, true).expect("the status of a repository without commits reads").expect("it is a repository");
    assert!(status.changes.is_empty() && status.head.is_none_or(|h| h.is_empty()), "an empty repository is clean and has no head commit");
    assert_eq!(crate::git::short_head(&host, &root), "", "and there is no head to name");
}

/// What the app shows for the failure of `git <args>` run in `dir`: git's own text through `explain`.
fn shown(dir: &std::path::Path, args: &[&str]) -> String {
    let host = host::local();
    let err = crate::git::git_ok(&host, &dir.to_string_lossy(), args).expect_err("the command fails");
    crate::git::explain(&err).unwrap_or_else(|| panic!("git's message for {args:?} is not recognised: {err}"))
}

#[test]
fn what_this_git_really_says_is_what_explain_recognises() {
    let (dir, run) = repo("states-real").expect("git is available");
    let bare = dir.with_extension("origin.git");
    let _ = std::fs::remove_dir_all(&bare);
    crate::test_git::git().args(["init", "-q", "--bare"]).arg(&bare).output().unwrap();
    let branch = run(&["symbolic-ref", "--short", "HEAD"]).unwrap().trim().to_string();
    // No upstream: pushing and pulling name Publish Branch.
    run(&["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();
    assert!(shown(&dir, &["push"]).contains("Publish Branch"));
    assert!(shown(&dir, &["pull", "--ff-only"]).contains("Publish Branch"), "pull without an upstream");
    run(&["push", "-q", "-u", "origin", &branch]).unwrap();
    // Divergent branches: both sides moved, and git is not told how to combine them.
    let other = dir.with_extension("other");
    let _ = std::fs::remove_dir_all(&other);
    crate::test_git::git().args(["clone", "-q"]).arg(&bare).arg(&other).output().unwrap();
    let in_other = |args: &[&str]| crate::test_git::git().args(args).current_dir(&other).output().unwrap();
    std::fs::write(other.join("theirs.txt"), "t").unwrap();
    in_other(&["add", "-A"]);
    in_other(&["commit", "-q", "-m", "theirs"]);
    in_other(&["push", "-q"]);
    commit(&dir, &run, "ours.txt", "o", "ours");
    run(&["fetch", "-q"]).unwrap();
    assert!(shown(&dir, &["pull"]).contains("both have new commits"), "a plain pull on diverged branches");
    assert!(shown(&dir, &["pull", "--ff-only"]).contains("both have new commits"), "pull --ff-only on diverged branches");
    // Local changes in the way of a switch: the files are named.
    run(&["checkout", "-q", "-b", "other"]).unwrap();
    commit(&dir, &run, "f.txt", "other\n", "other changes f");
    run(&["checkout", "-q", &branch]).unwrap();
    std::fs::write(dir.join("f.txt"), "uncommitted\n").unwrap();
    let blocked = shown(&dir, &["checkout", "other"]);
    assert!(blocked.contains("f.txt") && blocked.contains("commit or stash"), "{blocked}");
    run(&["checkout", "-q", "--", "f.txt"]).unwrap();
    // Unmerged files: a commit while a merge waits.
    run(&["checkout", "-q", "-b", "left"]).unwrap();
    commit(&dir, &run, "g.txt", "left\n", "left");
    run(&["checkout", "-q", &branch]).unwrap();
    commit(&dir, &run, "g.txt", "right\n", "right");
    assert!(run(&["merge", "left"]).is_none());
    assert!(shown(&dir, &["commit", "-m", "x"]).contains("conflicts"), "commit during a merge");
    assert!(shown(&dir, &["pull", "--ff-only"]).contains("conflicts"), "pull during a merge");
    run(&["merge", "--abort"]).unwrap();
    // A detached HEAD: pull has no branch to update.
    run(&["checkout", "-q", "--detach"]).unwrap();
    assert!(shown(&dir, &["pull", "--ff-only"]).contains("detached"));
}

#[test]
fn a_cherry_pick_that_waits_can_be_continued_once_resolved_or_aborted() {
    let (dir, run, _) = mid_cherry_pick("states-seq-continue");
    let host = host::local();
    let root = dir.to_string_lossy().into_owned();
    // Still conflicted: continuing says so, and the cherry-pick keeps waiting.
    let error = crate::git::sequencer(&host, &root, crate::git::Sequencer::CherryPick, crate::git::Step::Continue).unwrap_err();
    assert!(crate::git::explain(&error).is_some_and(|t| t.contains("conflicts")), "{error}");
    assert!(dir.join(".git/CHERRY_PICK_HEAD").exists());
    // Resolved and staged: it finishes with a commit.
    std::fs::write(dir.join("f.txt"), "resolved\n").unwrap();
    run(&["add", "f.txt"]).unwrap();
    let before = run(&["rev-list", "--count", "HEAD"]).unwrap();
    crate::git::sequencer(&host, &root, crate::git::Sequencer::CherryPick, crate::git::Step::Continue).unwrap();
    assert!(!dir.join(".git/CHERRY_PICK_HEAD").exists());
    assert_eq!(run(&["rev-list", "--count", "HEAD"]).unwrap().trim().parse::<u32>().unwrap(), before.trim().parse::<u32>().unwrap() + 1, "one commit was made");
    assert_eq!(run(&["log", "-1", "--format=%s"]).unwrap().trim(), "theirs");
}

#[test]
fn aborting_a_cherry_pick_or_a_revert_puts_the_repository_back() {
    let (dir, run, _) = mid_cherry_pick("states-seq-abort");
    let host = host::local();
    let root = dir.to_string_lossy().into_owned();
    crate::git::sequencer(&host, &root, crate::git::Sequencer::CherryPick, crate::git::Step::Abort).unwrap();
    assert!(!dir.join(".git/CHERRY_PICK_HEAD").exists());
    assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "ours\n", "the file is as it was before the cherry-pick");
    // A revert that conflicts.
    let first = run(&["rev-list", "--max-parents=0", "HEAD"]).unwrap().trim().to_string();
    commit(&dir, &run, "f.txt", "later\n", "later change");
    assert!(run(&["revert", "--no-edit", "HEAD~1"]).is_none(), "reverting an earlier change that later work built on conflicts");
    assert!(dir.join(".git/REVERT_HEAD").exists(), "{first}");
    crate::git::sequencer(&host, &root, crate::git::Sequencer::Revert, crate::git::Step::Abort).unwrap();
    assert!(!dir.join(".git/REVERT_HEAD").exists());
    assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "later\n");
}
