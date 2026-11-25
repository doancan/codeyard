//! Automatic pull (`git.autoPull`): after a background fetch, a branch that is only behind its upstream is fast-forwarded.
//! Nothing here can lose work or make a merge: a branch with commits of its own, a tree with changes, a half-done merge,
//! rebase, cherry-pick or revert, a detached HEAD and a branch without an upstream are all left for the user.

use host::Host;

use super::{git, git_ok, has_unmerged, has_upstream, merge_in_progress, rebase_in_progress};

/// Why an automatic pull did nothing although the branch is behind (or may be).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// HEAD is not on a branch.
    DetachedHead,
    /// The branch has no upstream to pull from.
    NoUpstream,
    /// A merge, rebase, cherry-pick or revert is waiting for the user.
    Busy,
    /// The branch has commits the upstream does not: a pull would merge or rebase, which is the user's call.
    Diverged,
    /// Tracked files have changes: they are not touched behind the user's back.
    Dirty,
}

/// What an automatic pull did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoPull {
    /// The branch moved forward by this many commits.
    Pulled(u32),
    /// Nothing to bring in.
    UpToDate,
    Skipped(Skip),
}

/// Whether a cherry-pick or a revert is half-done (their state files are not covered by the merge and rebase checks).
fn sequencer_in_progress(host: &dyn Host, root: &str) -> bool {
    ["CHERRY_PICK_HEAD", "REVERT_HEAD"].iter().any(|r| git(host, root, &["rev-parse", "-q", "--verify", r]).is_ok_and(|o| o.ok()))
}

/// (commits only here, commits only upstream) of the checked-out branch.
fn ahead_behind(host: &dyn Host, root: &str) -> Result<(u32, u32), String> {
    let out = git_ok(host, root, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])?;
    let mut counts = out.split_whitespace().map(|n| n.parse::<u32>().map_err(|_| format!("unexpected rev-list output: {out}")));
    match (counts.next(), counts.next()) {
        (Some(ahead), Some(behind)) => Ok((ahead?, behind?)),
        _ => Err(format!("unexpected rev-list output: {out}")),
    }
}

/// Fast-forwards the checked-out branch to its upstream when that is all a pull would do, with the fetch already done.
/// `Err`: git refused the fast-forward, with its own words.
pub fn auto_pull(host: &dyn Host, root: &str) -> Result<AutoPull, String> {
    if !git(host, root, &["symbolic-ref", "-q", "HEAD"])?.ok() {
        return Ok(AutoPull::Skipped(Skip::DetachedHead));
    }
    if !has_upstream(host, root) {
        return Ok(AutoPull::Skipped(Skip::NoUpstream));
    }
    let (ahead, behind) = ahead_behind(host, root)?;
    if behind == 0 {
        return Ok(AutoPull::UpToDate);
    }
    if merge_in_progress(host, root) || rebase_in_progress(host, root) || sequencer_in_progress(host, root) || has_unmerged(host, root) {
        return Ok(AutoPull::Skipped(Skip::Busy));
    }
    if ahead > 0 {
        return Ok(AutoPull::Skipped(Skip::Diverged));
    }
    // Untracked files stay out of it: git itself refuses a fast-forward that would overwrite one.
    if !git_ok(host, root, &["status", "--porcelain", "--untracked-files=no"])?.trim().is_empty() {
        return Ok(AutoPull::Skipped(Skip::Dirty));
    }
    match git_ok(host, root, &["merge", "--ff-only", "--quiet", "@{upstream}"]) {
        Ok(_) => Ok(AutoPull::Pulled(behind)),
        // The user (or another window) is running a git command of their own: this one waits for the next fetch.
        Err(e) if e.contains(".lock") => Ok(AutoPull::Skipped(Skip::Busy)),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo_for_test;

    /// A repository with a bare `origin` it tracks, and a second clone for "someone else".
    struct Fixture {
        dir: std::path::PathBuf,
        other: std::path::PathBuf,
        bare: std::path::PathBuf,
    }

    fn run_in(dir: &std::path::Path, args: &[&str]) -> String {
        let out = crate::test_git::git().args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    impl Fixture {
        fn new(name: &str) -> Option<Fixture> {
            let (dir, _) = repo_for_test(name)?;
            let (bare, other) = (dir.with_extension("origin.git"), dir.with_extension("other"));
            let _ = std::fs::remove_dir_all(&bare);
            let _ = std::fs::remove_dir_all(&other);
            crate::test_git::git().args(["init", "-q", "--bare"]).arg(&bare).output().ok()?;
            run_in(&dir, &["remote", "add", "origin", bare.to_str()?]);
            run_in(&dir, &["push", "-q", "-u", "origin", "main"]);
            run_in(dir.parent()?, &["clone", "-q", bare.to_str()?, other.to_str()?]);
            Some(Fixture { dir, other, bare })
        }

        fn root(&self) -> String {
            self.dir.to_string_lossy().into_owned()
        }

        /// Someone else pushes `n` commits, and this clone fetches them.
        fn upstream_moves(&self, n: u32) {
            for i in 0..n {
                std::fs::write(self.other.join(format!("up{i}.txt")), "up\n").unwrap();
                run_in(&self.other, &["add", "-A"]);
                run_in(&self.other, &["commit", "-q", "-m", &format!("up {i}")]);
            }
            run_in(&self.other, &["push", "-q"]);
            run_in(&self.dir, &["fetch", "-q"]);
        }

        fn head(&self) -> String {
            run_in(&self.dir, &["rev-parse", "HEAD"]).trim().to_string()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for p in [&self.dir, &self.other, &self.bare] {
                let _ = std::fs::remove_dir_all(p);
            }
        }
    }

    #[test]
    fn a_branch_that_is_only_behind_moves_forward_and_nothing_else_changes() {
        let Some(f) = Fixture::new("autopull-ff") else { return };
        let host = host::local();
        assert_eq!(auto_pull(&host, &f.root()), Ok(AutoPull::UpToDate));
        f.upstream_moves(2);
        assert_eq!(auto_pull(&host, &f.root()), Ok(AutoPull::Pulled(2)));
        assert!(f.dir.join("up1.txt").exists());
        assert_eq!(run_in(&f.dir, &["rev-list", "--count", "HEAD..origin/main"]).trim(), "0");
        assert_eq!(auto_pull(&host, &f.root()), Ok(AutoPull::UpToDate), "a second run finds nothing");
        assert_eq!(run_in(&f.dir, &["log", "--format=%s", "-3"]).lines().count(), 3, "history is linear: no merge commit was made");
    }

    #[test]
    fn a_branch_with_commits_of_its_own_is_left_alone() {
        let Some(f) = Fixture::new("autopull-diverged") else { return };
        std::fs::write(f.dir.join("mine.txt"), "mine\n").unwrap();
        run_in(&f.dir, &["add", "-A"]);
        run_in(&f.dir, &["commit", "-q", "-m", "mine"]);
        f.upstream_moves(1);
        let before = f.head();
        assert_eq!(auto_pull(&host::local(), &f.root()), Ok(AutoPull::Skipped(Skip::Diverged)));
        assert_eq!(f.head(), before, "no merge, no rebase");
        // Only ahead: nothing to bring in.
        let Some(g) = Fixture::new("autopull-ahead") else { return };
        std::fs::write(g.dir.join("mine.txt"), "mine\n").unwrap();
        run_in(&g.dir, &["add", "-A"]);
        run_in(&g.dir, &["commit", "-q", "-m", "mine"]);
        assert_eq!(auto_pull(&host::local(), &g.root()), Ok(AutoPull::UpToDate));
    }

    #[test]
    fn changes_in_tracked_files_are_never_touched() {
        let Some(f) = Fixture::new("autopull-dirty") else { return };
        f.upstream_moves(1);
        std::fs::write(f.dir.join("f.txt"), "unsaved work\n").unwrap();
        let before = f.head();
        assert_eq!(auto_pull(&host::local(), &f.root()), Ok(AutoPull::Skipped(Skip::Dirty)));
        assert_eq!(f.head(), before);
        assert_eq!(std::fs::read_to_string(f.dir.join("f.txt")).unwrap(), "unsaved work\n");
        // An untracked file in the way of an incoming one: git refuses, and says so instead of overwriting.
        run_in(&f.dir, &["checkout", "--", "f.txt"]);
        std::fs::write(f.dir.join("up0.txt"), "mine, untracked\n").unwrap();
        assert!(auto_pull(&host::local(), &f.root()).is_err());
        assert_eq!(std::fs::read_to_string(f.dir.join("up0.txt")).unwrap(), "mine, untracked\n");
    }

    #[test]
    fn a_half_done_operation_a_detached_head_and_a_missing_upstream_are_skipped() {
        let host = host::local();
        let Some(f) = Fixture::new("autopull-busy") else { return };
        f.upstream_moves(1);
        // A cherry-pick that stopped on a conflict.
        run_in(&f.dir, &["checkout", "-q", "-b", "side"]);
        std::fs::write(f.dir.join("f.txt"), "side\n").unwrap();
        run_in(&f.dir, &["commit", "-q", "-am", "side"]);
        run_in(&f.dir, &["checkout", "-q", "main"]);
        std::fs::write(f.dir.join("f.txt"), "main\n").unwrap();
        run_in(&f.dir, &["commit", "-q", "-am", "main"]);
        f.upstream_moves(0);
        let out = crate::test_git::git().args(["cherry-pick", "side"]).current_dir(&f.dir).output().unwrap();
        assert!(!out.status.success());
        // Diverged and busy: busy is reported first, because it is what the user has to deal with.
        assert_eq!(auto_pull(&host, &f.root()), Ok(AutoPull::Skipped(Skip::Busy)));
        run_in(&f.dir, &["cherry-pick", "--abort"]);
        run_in(&f.dir, &["checkout", "-q", "--detach"]);
        assert_eq!(auto_pull(&host, &f.root()), Ok(AutoPull::Skipped(Skip::DetachedHead)));
        run_in(&f.dir, &["checkout", "-q", "-b", "unpublished"]);
        assert_eq!(auto_pull(&host, &f.root()), Ok(AutoPull::Skipped(Skip::NoUpstream)));
    }
}
