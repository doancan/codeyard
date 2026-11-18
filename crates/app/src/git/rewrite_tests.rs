//! Squash and reorder in real repositories (#270): the result, and above all that nothing is left half done. Every failure test
//! compares the refs, the index bytes and the files of the checkout with what they were, and checks that the temporary worktree
//! is gone.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::*;

struct Fixture {
    dir: PathBuf,
    root: String,
    host: host::RemoteHost,
}

/// A repository with one commit ("base") in a fresh folder.
fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("codeyard-rewrite-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = Fixture { root: dir.to_string_lossy().into_owned(), dir, host: host::local() };
    f.git(&["init", "-q"]);
    std::fs::write(f.dir.join("f.txt"), "base\n").unwrap();
    f.git(&["add", "-A"]);
    f.git(&["commit", "-q", "-m", "base"]);
    f
}

impl Fixture {
    fn git(&self, args: &[&str]) -> String {
        let out = crate::test_git::git().args(args).current_dir(&self.dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A commit that writes `content` to `file`.
    fn commit(&self, file: &str, content: &str, message: &str) -> String {
        std::fs::write(self.dir.join(file), content).unwrap();
        self.git(&["add", file]);
        self.git(&["commit", "-q", "-m", message]);
        self.head()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    /// Subjects, newest first.
    fn subjects(&self) -> Vec<String> {
        self.git(&["log", "--format=%s"]).lines().map(str::to_string).collect()
    }

    fn tree(&self, rev: &str) -> String {
        self.git(&["rev-parse", &format!("{rev}^{{tree}}")]).trim().to_string()
    }

    fn branch(&self) -> String {
        self.git(&["symbolic-ref", "--short", "HEAD"]).trim().to_string()
    }

    fn rewrite(&self, tip: &str, what: &Rewrite) -> Result<String, String> {
        rewrite(&self.host, &self.root, tip, what)
    }

    /// Everything a failed rewrite must leave as it was: refs, the index's bytes, the files and the worktree list.
    fn snapshot(&self) -> (String, Vec<u8>, BTreeMap<String, Vec<u8>>, String) {
        let files = std::fs::read_dir(&self.dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name() != ".git" && e.path().is_file())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
            .collect();
        (self.git(&["for-each-ref"]), std::fs::read(self.dir.join(".git/index")).unwrap(), files, self.git(&["worktree", "list"]))
    }

    fn leftovers(&self) -> Vec<String> {
        std::fs::read_dir(self.dir.join(".git"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("codeyard-rewrite") || n == "worktrees")
            .filter(|n| n != "worktrees" || std::fs::read_dir(self.dir.join(".git/worktrees")).is_ok_and(|mut d| d.next().is_some()))
            .collect()
    }

    fn assert_untouched(&self, before: &(String, Vec<u8>, BTreeMap<String, Vec<u8>>, String), why: &str) {
        assert_eq!(&self.snapshot(), before, "{why}: refs, index, files and worktrees are as they were");
        assert!(self.leftovers().is_empty(), "{why}: no temporary worktree is left: {:?}", self.leftovers());
    }

    /// A bare remote with everything so far pushed.
    fn push_all(&self) {
        let bare = self.dir.with_extension("origin.git");
        let _ = std::fs::remove_dir_all(&bare);
        crate::test_git::git().args(["init", "-q", "--bare"]).arg(&bare).output().unwrap();
        self.git(&["remote", "add", "origin", &bare.to_string_lossy()]);
        let branch = self.branch();
        self.git(&["push", "-q", "-u", "origin", &branch]);
    }
}

fn squash(run: &[&str], message: &str) -> Rewrite {
    Rewrite::Squash { run: run.iter().map(|h| h.to_string()).collect(), message: message.to_string() }
}

#[test]
fn ac1_three_unpushed_commits_become_one_with_the_edited_message_and_the_same_tree() {
    let f = fixture("squash");
    let (a, b, c) = (f.commit("a.txt", "a", "wip a"), f.commit("b.txt", "b", "wip b"), f.commit("c.txt", "c", "wip c"));
    let tree = f.tree("HEAD");
    // Work in progress of the user: one staged and one unstaged change, which a rewrite must not touch.
    std::fs::write(f.dir.join("a.txt"), "a, edited and staged").unwrap();
    f.git(&["add", "a.txt"]);
    std::fs::write(f.dir.join("b.txt"), "b, edited").unwrap();
    let before = f.snapshot();
    let outcome = f.rewrite(&c, &squash(&[&a, &b, &c], "Add a, b and c\n\nIn one piece.")).unwrap();
    assert!(outcome.starts_with("Squashed 3 commits into "), "{outcome}");
    assert_eq!(f.subjects(), vec!["Add a, b and c", "base"], "one commit replaces the three");
    assert_eq!(f.git(&["log", "-1", "--format=%B"]).trim(), "Add a, b and c\n\nIn one piece.");
    assert_eq!(f.tree("HEAD"), tree, "the same tree");
    assert_eq!(f.git(&["log", "-1", "--format=%an"]).trim(), "Test", "the oldest commit's author");
    let after = f.snapshot();
    assert_eq!((&after.1, &after.2), (&before.1, &before.2), "the index and the files, staged and unstaged work included, are not touched");
    assert!(f.leftovers().is_empty());
    assert!(f.git(&["reflog", "-1"]).contains("codeyard: rewrite"), "the reflog names what moved the branch");
}

#[test]
fn a_run_below_the_tip_is_squashed_and_the_commits_above_it_are_replayed() {
    let f = fixture("squash-middle");
    let (a, b) = (f.commit("a.txt", "a", "a"), f.commit("b.txt", "b", "b"));
    f.commit("c.txt", "c", "c");
    let tip = f.commit("d.txt", "d", "d");
    let tree = f.tree("HEAD");
    f.rewrite(&tip, &squash(&[&a, &b], "ab")).unwrap();
    assert_eq!(f.subjects(), vec!["d", "c", "ab", "base"], "nothing above the run was dropped");
    assert_eq!(f.tree("HEAD"), tree, "the final tree equals the original tip's tree");
}

#[test]
fn ac2_a_pushed_commit_in_the_run_refuses_and_changes_nothing() {
    let f = fixture("pushed");
    let a = f.commit("a.txt", "a", "a");
    f.push_all();
    let b = f.commit("b.txt", "b", "b");
    let before = f.snapshot();
    let refused = f.rewrite(&b, &squash(&[&a, &b], "ab")).unwrap_err();
    assert_eq!(refused, "Only commits that are not pushed can be squashed");
    f.assert_untouched(&before, "a pushed commit");
    // With no upstream, a commit on any remote counts as pushed too.
    f.git(&["branch", "--unset-upstream"]);
    assert!(f.rewrite(&b, &squash(&[&a, &b], "ab")).is_err(), "still on origin/<branch>");
    f.assert_untouched(&before, "a commit on a remote without an upstream");
}

#[test]
fn ac3_two_independent_commits_swap_and_the_tip_tree_is_unchanged() {
    let f = fixture("swap");
    let (a, b) = (f.commit("a.txt", "a", "first"), f.commit("b.txt", "b", "second"));
    let tree = f.tree("HEAD");
    let author_dates = f.git(&["log", "--format=%an|%ad", "--date=raw"]);
    let outcome = f.rewrite(&b, &Rewrite::Move { hash: a.clone(), down: false }).unwrap();
    assert!(outcome.starts_with("Moved "), "{outcome}");
    assert_eq!(f.subjects(), vec!["first", "second", "base"], "moved up: it is the newer now");
    assert_eq!(f.tree("HEAD"), tree);
    let swapped = f.git(&["log", "--format=%an|%ad", "--date=raw"]);
    assert_eq!(
        swapped.lines().collect::<std::collections::BTreeSet<_>>(),
        author_dates.lines().collect::<std::collections::BTreeSet<_>>(),
        "authors and dates travel with their commits"
    );
    let tip = f.head();
    f.rewrite(&tip, &Rewrite::Move { hash: tip.clone(), down: true }).unwrap();
    assert_eq!(f.subjects(), vec!["second", "first", "base"], "and back down");
}

#[test]
fn ac4_a_conflicting_reorder_names_the_two_commits_and_changes_nothing() {
    let f = fixture("conflict");
    let a = f.commit("f.txt", "one\n", "one");
    let b = f.commit("f.txt", "one\ntwo\n", "two builds on one");
    std::fs::write(f.dir.join("untracked.txt"), "mine").unwrap();
    std::fs::write(f.dir.join("f.txt"), "one\ntwo\nedited\n").unwrap();
    let before = f.snapshot();
    let error = f.rewrite(&b, &Rewrite::Move { hash: a.clone(), down: false }).unwrap_err();
    assert_eq!(error, format!("Moving {} would conflict with {}: nothing was changed", &a[..7], &b[..7]), "names the two commits");
    f.assert_untouched(&before, "a conflicting reorder");
}

#[cfg(unix)]
fn hook(f: &Fixture, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = f.dir.join(".git/hooks").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn ac5_a_branch_that_moves_during_the_rewrite_installs_nothing() {
    let f = fixture("moved-during");
    let (a, b) = (f.commit("a.txt", "a", "a"), f.commit("b.txt", "b", "b"));
    // The first commit the rewrite makes advances the branch in the main checkout, as a commit from a terminal would.
    hook(
        &f,
        "pre-commit",
        &format!(
            "if [ ! -e '{m}/.git/moved' ]; then touch '{m}/.git/moved'; env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE -u GIT_COMMON_DIR -u GIT_PREFIX git -C '{m}' commit -q --allow-empty -m 'from a terminal'; fi",
            m = f.root
        ),
    );
    let error = f.rewrite(&b, &squash(&[&a, &b], "ab")).unwrap_err();
    assert_eq!(error, crate::git::moved_message());
    assert_eq!(f.subjects(), vec!["from a terminal", "b", "a", "base"], "the branch is where the other commit left it, with both originals");
    assert!(f.leftovers().is_empty());
}

#[cfg(unix)]
#[test]
fn a_refusing_hook_stops_the_rewrite_and_its_words_reach_the_user() {
    let f = fixture("hook");
    let (a, b) = (f.commit("a.txt", "a", "a"), f.commit("b.txt", "b", "b"));
    hook(&f, "commit-msg", "echo 'subject must start with TICKET-' >&2; exit 1");
    let before = f.snapshot();
    let error = f.rewrite(&b, &squash(&[&a, &b], "ab")).unwrap_err();
    assert!(error.contains("subject must start with TICKET-") || error.contains("could not be made"), "{error}");
    f.assert_untouched(&before, "a refusing hook");
}

#[cfg(unix)]
#[test]
fn a_signing_key_that_is_not_there_stops_the_rewrite_without_asking_anything() {
    let f = fixture("signing");
    let (a, b) = (f.commit("a.txt", "a", "a"), f.commit("b.txt", "b", "b"));
    f.git(&["config", "commit.gpgsign", "true"]);
    f.git(&["config", "gpg.program", "/usr/bin/false"]);
    let before = f.snapshot();
    assert!(f.rewrite(&b, &squash(&[&a, &b], "ab")).is_err());
    f.assert_untouched(&before, "unavailable signing");
}

#[test]
fn what_cannot_be_rewritten_is_refused_before_anything_is_written() {
    let f = fixture("refusals");
    let (a, b) = (f.commit("a.txt", "a", "a"), f.commit("b.txt", "b", "b"));
    let base = f.git(&["rev-list", "--max-parents=0", "HEAD"]).trim().to_string();
    let before = f.snapshot();
    let refused = |what: Rewrite, tip: &str| f.rewrite(tip, &what).unwrap_err();
    // A stale tip.
    assert_eq!(refused(squash(&[&a, &b], "x"), &a), crate::git::moved_message());
    // One commit is no run; the first commit of the repository has nothing to start from; a run that is not one.
    assert_eq!(refused(squash(&[&a], "x"), &b), "Choose at least two commits to squash.");
    assert_eq!(refused(squash(&[&base, &a], "x"), &b), "The first commit of the repository cannot be rewritten");
    assert_eq!(refused(squash(&[&b, &a], "x"), &b), "The chosen commits are not one run of this branch.", "the wrong way round");
    // Nothing above the tip and nothing below the first commit to swap with.
    assert!(refused(Rewrite::Move { hash: b.clone(), down: false }, &b).contains("no commit above"));
    assert_eq!(refused(Rewrite::Move { hash: base.clone(), down: true }, &b), "There is no commit below to swap with");
    // The limit.
    let limited = rewrite_limited(&f.host, &f.root, &b, &squash(&[&a, &b], "x"), 1).unwrap_err();
    assert_eq!(limited, "Rewriting more than 1 commits is refused");
    f.assert_untouched(&before, "a refusal");
    // A detached HEAD.
    f.git(&["checkout", "-q", "--detach"]);
    assert_eq!(refused(squash(&[&a, &b], "x"), &b), "Commits can only be rewritten on a branch, and HEAD is detached.");
}

#[test]
fn a_merge_in_the_way_is_refused_and_the_first_commit_cannot_be_squashed() {
    let f = fixture("merge");
    let base = f.head();
    let a = f.commit("a.txt", "a", "a");
    f.git(&["checkout", "-q", "-b", "side", &base]);
    f.commit("s.txt", "s", "side");
    f.git(&["checkout", "-q", "-"]);
    f.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    let merge = f.head();
    let b = f.commit("b.txt", "b", "b");
    let before = f.snapshot();
    // A merge above the run would have to be replayed: refused instead of dropped.
    let error = f.rewrite(&b, &squash(&[&a, &merge], "x")).unwrap_err();
    assert!(error.contains("merge commit") || error.contains("not one run"), "{error}");
    let error = f.rewrite(&b, &Rewrite::Move { hash: b.clone(), down: true }).unwrap_err();
    assert!(error.contains("merge commit"), "swapping with a merge: {error}");
    f.assert_untouched(&before, "a merge");
}

#[test]
fn a_branch_checked_out_in_another_worktree_is_refused() {
    let f = fixture("other-worktree");
    let (a, b) = (f.commit("a.txt", "a", "a"), f.commit("b.txt", "b", "b"));
    let branch = f.branch();
    let other = f.dir.with_extension("wt");
    let _ = std::fs::remove_dir_all(&other);
    // `--force`: the same branch in two places, which moving its ref would leave wrong in one of them.
    f.git(&["worktree", "add", "-q", "--force", &other.to_string_lossy(), &branch]);
    let before = f.snapshot();
    let error = f.rewrite(&b, &squash(&[&a, &b], "ab")).unwrap_err();
    assert!(error.contains("checked out in another worktree"), "{error}");
    assert_eq!(f.git(&["rev-parse", "HEAD"]).trim(), b);
    let _ = before;
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn the_default_message_joins_the_messages_oldest_first() {
    let f = fixture("message");
    let (a, b) = (f.commit("a.txt", "a", "first"), f.commit("b.txt", "b", "second\n\nwith a body"));
    assert_eq!(default_squash_message(&f.host, &f.root, &[a, b]).unwrap(), "first\n\nsecond\n\nwith a body");
}
