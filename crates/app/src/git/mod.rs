//! Git: the system `git` runs through the agent, with the same code path
//! locally and over SSH. Only commands and output parsing live here (independent of the UI): `status` is the
//! working tree and commits, `branch` branches/remotes/rebase/merge, `sync` pull/push/fetch/publish, `log` history and blame, `diff` the line
//! diff and block staging, `stash` the stash. The runner below is shared; everything else is re-exported.

use host::Host;

mod branch;
mod diff;
mod ignore;
mod log;
mod pull;
mod refs;
mod stash;
mod status;
mod sync;
mod tag;
mod worktree;

pub use branch::*;
pub use diff::*;
pub use ignore::*;
pub use log::*;
pub use pull::checkout_pull;
pub use refs::*;
pub use stash::*;
pub use status::*;
pub use sync::*;
pub use tag::*;
#[cfg(test)]
pub use worktree::parse_worktree_entries;
pub use worktree::{Worktree, worktree_entries};

fn git(host: &dyn Host, cwd: &str, args: &[&str]) -> Result<host::ExecOutput, String> {
    let out = host.exec("git", args, Some(cwd), None).map_err(|e| e.to_string())?;
    if out.code == Some(127) {
        return Err(crate::i18n::t("git is not installed on this host").into());
    }
    Ok(out)
}

pub(crate) fn git_ok(host: &dyn Host, cwd: &str, args: &[&str]) -> Result<String, String> {
    let out = git(host, cwd, args)?;
    if out.ok() {
        Ok(out.stdout_str())
    } else {
        let err = out.stderr_str();
        Err(if err.is_empty() { crate::i18n::tf("git {} failed", &[args.first().unwrap_or(&"")]) } else { err })
    }
}

/// Error message: stderr first (the real cause is there), otherwise stdout.
fn failure_text(out: &host::ExecOutput) -> String {
    let err = out.stderr_str();
    let text = if err.trim().is_empty() { out.stdout_str() } else { err };
    text.trim().to_string()
}

/// A fresh repository with one commit, and a runner for git commands in it; `None` without a git binary.
/// Shared by the tests of every submodule.
#[cfg(test)]
fn repo(name: &str) -> Option<(std::path::PathBuf, impl Fn(&[&str]) -> Option<String>)> {
    let dir = std::env::temp_dir().join(format!("codeyard-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.clone();
    let run = move |args: &[&str]| {
        crate::test_git::git().args(args).current_dir(&d).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    run(&["init", "-q"])?;
    std::fs::write(dir.join("f.txt"), "base\n").unwrap();
    run(&["add", "-A"])?;
    run(&["commit", "-q", "-m", "base"])?;
    Some((dir, run))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cherry-picking or reverting a *merge* commit needs `-m 1` (mainline = first parent); without it git refuses.
    #[test]
    fn cherry_pick_and_revert_of_a_merge_commit_use_the_first_parent() {
        use crate::graph::{CommitOp, run_op};
        let Some((dir, run)) = repo("mergepick") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        // main --- m (merge) ; side adds s.txt
        run(&["checkout", "-q", "-b", "side"]).unwrap();
        std::fs::write(dir.join("s.txt"), "s\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "s"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("m.txt"), "m\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "m"]).unwrap();
        run(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]).unwrap();
        let merge = run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
        // Revert of the merge removes s.txt (the side's changes), keeps m.txt.
        let msg = run_op(&host, &root, &merge, &CommitOp::Revert).expect("revert of a merge commit");
        assert!(msg.starts_with("Reverted"), "{msg}");
        assert!(!dir.join("s.txt").exists() && dir.join("m.txt").exists());
        // Cherry-pick of the merge onto a branch from the base brings in the side's changes.
        let base = run(&["rev-list", "--max-parents=0", "HEAD"]).unwrap().trim().to_string();
        run(&["checkout", "-q", "-b", "target", &base]).unwrap();
        let msg = run_op(&host, &root, &merge, &CommitOp::CherryPick).expect("cherry-pick of a merge commit");
        assert!(msg.starts_with("Cherry-picked"), "{msg}");
        assert!(dir.join("s.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
