//! Network operations: pull, push, fetch, publish. Git never asks for a password here (the agent sets `GIT_TERMINAL_PROMPT=0`)
//! and gives up on a command after ten minutes, so a hung server ends in an error rather than a stuck button.

use host::Host;

use super::{failure_text, git, git_ok, has_unmerged, merge_in_progress, plain_name, rebase_in_progress, remotes};

/// One step of a sync. `Pull` follows `git.rebaseWhenSync`; `PullRebase` always rebases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncOp {
    Pull,
    PullRebase,
    /// Pull a branch of a chosen remote (`git pull <remote> <branch>`).
    PullFrom {
        remote: String,
        branch: String,
    },
    /// On a branch that is not published yet, pushing publishes it.
    Push,
    PushTo {
        remote: String,
    },
    Fetch,
    FetchAll,
    Publish,
}

/// The settings that change what the commands do (`git.rebaseWhenSync`, `git.pruneOnFetch`, `git.pullTags`, `git.fetchOnPull`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncOptions {
    pub rebase: bool,
    pub prune: bool,
    pub tags: bool,
    pub fetch_on_pull: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions { rebase: false, prune: true, tags: true, fetch_on_pull: false }
    }
}

/// What to run after a commit (`git.postCommitCommand`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostCommit {
    Push,
    Sync,
}

impl PostCommit {
    pub fn from_setting(value: &str) -> Option<PostCommit> {
        match value {
            "push" => Some(PostCommit::Push),
            "sync" => Some(PostCommit::Sync),
            _ => None,
        }
    }

    /// The steps for it. The status is stale right after a commit, so this does not look at ahead/behind: a sync
    /// pulls first (a no-op when nothing is new) and an unpublished branch is published.
    pub fn ops(self, has_upstream: bool) -> Vec<SyncOp> {
        match (self, has_upstream) {
            (PostCommit::Push, _) => vec![SyncOp::Push],
            (PostCommit::Sync, true) => vec![SyncOp::Pull, SyncOp::Push],
            (PostCommit::Sync, false) => vec![SyncOp::Publish],
        }
    }
}

/// How a step ended: `Conflicts` means a merge or a rebase is in progress and the steps after it should not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncStep {
    Done,
    Conflicts,
    /// The branch went to this remote.
    Published(String),
}

/// What the Sync button and the status bar arrows do: publish a branch without an upstream, pull and push when both
/// sides moved, otherwise whichever side did, and only fetch when nothing is pending.
pub fn sync_plan(has_upstream: bool, ahead: u32, behind: u32) -> Vec<SyncOp> {
    match (has_upstream, behind > 0, ahead > 0) {
        (false, ..) => vec![SyncOp::Publish],
        (true, true, true) => vec![SyncOp::Pull, SyncOp::Push],
        (true, true, false) => vec![SyncOp::Pull],
        (true, false, true) => vec![SyncOp::Push],
        (true, false, false) => vec![SyncOp::Fetch],
    }
}

/// "↓2 ↑1": commits to pull, then commits to push; a side that is zero is left out.
pub fn counts_label(ahead: u32, behind: u32) -> String {
    match (behind, ahead) {
        (0, 0) => String::new(),
        (b, 0) => format!("↓{b}"),
        (0, a) => format!("↑{a}"),
        (b, a) => format!("↓{b} ↑{a}"),
    }
}

/// Publish the current branch to `origin` (or else the first remote; an error if there is none) and set up tracking.
pub fn publish_branch(host: &dyn Host, root: &str) -> Result<String, String> {
    let remotes = remotes(host, root)?;
    let remote = remotes
        .iter()
        .find(|(n, _)| n == "origin")
        .or(remotes.first())
        .map(|(n, _)| n.clone())
        .ok_or(crate::i18n::t("This repository has no remote. Add one with Git: Add Remote…"))?;
    plain_name(&remote)?;
    git_ok(host, root, &["push", "-u", "--", &remote, "HEAD"])?;
    Ok(remote)
}

/// Fetch a pull request's head (`pull/N/head`) into `FETCH_HEAD`. The remote name comes from the repository's own config.
pub fn fetch_pull(host: &dyn Host, root: &str, remote: &str, number: u64) -> Result<(), String> {
    plain_name(remote)?;
    git_ok(host, root, &["fetch", "--quiet", "--", remote, &format!("pull/{number}/head")]).map(|_| ())
}

pub fn has_upstream(host: &dyn Host, root: &str) -> bool {
    git(host, root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]).is_ok_and(|o| o.ok())
}

fn fetch(host: &dyn Host, root: &str, all: bool, prune: bool) -> Result<(), String> {
    let mut args = vec!["fetch"];
    if all {
        args.push("--all");
    }
    if prune {
        args.push("--prune");
    }
    git_ok(host, root, &args).map(|_| ())
}

/// `Ok(true)`: the pull stopped on conflicts (a merge or a rebase is in progress; the caller says which to continue).
/// `from`: a remote and a branch to pull instead of the upstream.
fn pull(host: &dyn Host, root: &str, rebase: bool, opts: SyncOptions, from: Option<(&str, &str)>) -> Result<bool, String> {
    if rebase_in_progress(host, root) {
        return Err(crate::i18n::t("A rebase is already in progress: use Git: Continue Rebase or Git: Abort Rebase").into());
    }
    if opts.fetch_on_pull {
        fetch(host, root, true, opts.prune)?;
    }
    let mut args = vec!["pull"];
    args.extend(if rebase { ["--rebase"].as_slice() } else { ["--no-rebase", "--no-edit"].as_slice() });
    if opts.tags {
        args.push("--tags");
    }
    if let Some((remote, branch)) = from {
        args.extend(["--", remote, branch]);
    }
    let out = git(host, root, &args)?;
    if out.ok() {
        return Ok(false);
    }
    if (merge_in_progress(host, root) || rebase_in_progress(host, root)) && has_unmerged(host, root) {
        return Ok(true);
    }
    Err(explain_pull_error(&failure_text(&out)))
}

/// A dirty tree is the common reason a pull is refused; git words it three ways, depending on merge or rebase.
fn explain_pull_error(err: &str) -> String {
    let e = err.to_lowercase();
    if e.contains("would be overwritten") || e.contains("unstaged changes") || e.contains("uncommitted changes") || e.contains("commit your changes or stash") {
        return crate::i18n::t("Commit or stash your changes first").into();
    }
    err.to_string()
}

fn push(host: &dyn Host, root: &str, remote: Option<&str>) -> Result<(), String> {
    match remote {
        Some(remote) => {
            // Tracking is only set when there is none, so pushing to a second remote does not move an existing one.
            let mut args = vec!["push"];
            if !has_upstream(host, root) {
                args.push("-u");
            }
            args.extend(["--", remote, "HEAD"]);
            git_ok(host, root, &args).map(|_| ())
        }
        None => match git_ok(host, root, &["push"]) {
            Ok(_) => Ok(()),
            // Not published yet: the same as "Publish Branch".
            Err(e) if e.contains("has no upstream branch") => publish_branch(host, root).map(|_| ()),
            Err(e) => Err(e),
        },
    }
}

/// Run one step.
pub fn sync(host: &dyn Host, root: &str, op: &SyncOp, opts: SyncOptions) -> Result<SyncStep, String> {
    let pulled = |conflicts: bool| if conflicts { SyncStep::Conflicts } else { SyncStep::Done };
    match op {
        SyncOp::Pull => pull(host, root, opts.rebase, opts, None).map(pulled),
        SyncOp::PullRebase => pull(host, root, true, opts, None).map(pulled),
        SyncOp::PullFrom { remote, branch } => {
            plain_name(remote)?;
            plain_name(branch)?;
            pull(host, root, opts.rebase, opts, Some((remote, branch))).map(pulled)
        }
        SyncOp::Push => push(host, root, None).map(|_| SyncStep::Done),
        SyncOp::PushTo { remote } => {
            plain_name(remote)?;
            push(host, root, Some(remote)).map(|_| SyncStep::Done)
        }
        SyncOp::Fetch => fetch(host, root, false, opts.prune).map(|_| SyncStep::Done),
        SyncOp::FetchAll => fetch(host, root, true, opts.prune).map(|_| SyncStep::Done),
        SyncOp::Publish => publish_branch(host, root).map(SyncStep::Published),
    }
}

/// Fetch for `git.autofetch`: nothing to do (and no error) in a repository without a remote.
pub fn autofetch(host: &dyn Host, root: &str, opts: SyncOptions) -> Result<(), String> {
    if remotes(host, root)?.is_empty() {
        return Ok(());
    }
    fetch(host, root, false, opts.prune)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo;

    /// A repository with a bare `origin`, the branch pushed and tracking it, and a second clone for "someone else".
    struct Remote {
        dir: std::path::PathBuf,
        bare: std::path::PathBuf,
        other: std::path::PathBuf,
        run: Box<dyn Fn(&[&str]) -> Option<String>>,
    }

    impl Remote {
        fn new(name: &str) -> Option<Remote> {
            let (dir, _) = repo(name)?;
            let d = dir.clone();
            // Not `repo`'s runner: that one borrows `name`.
            let run = move |args: &[&str]| {
                crate::test_git::git()
                    .args(args)
                    .current_dir(&d)
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            };
            let bare = dir.with_extension("origin.git");
            let other = dir.with_extension("other");
            let _ = std::fs::remove_dir_all(&bare);
            let _ = std::fs::remove_dir_all(&other);
            crate::test_git::git().args(["init", "-q", "--bare"]).arg(&bare).output().ok()?;
            run(&["remote", "add", "origin", bare.to_str()?])?;
            run(&["push", "-q", "-u", "origin", "main"])?;
            Some(Remote { dir, bare, other, run: Box::new(run) })
        }

        fn root(&self) -> String {
            self.dir.to_string_lossy().into_owned()
        }

        /// Someone else pushes a commit that changes `file` to `text`.
        fn push_from_elsewhere(&self, file: &str, text: &str) {
            let git = |args: &[&str], dir: &std::path::Path| {
                let out = crate::test_git::git().args(args).current_dir(dir).output().unwrap();
                assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            };
            if !self.other.exists() {
                git(&["clone", "-q", self.bare.to_str().unwrap(), self.other.to_str().unwrap()], self.dir.parent().unwrap());
            }
            std::fs::write(self.other.join(file), text).unwrap();
            git(&["add", "-A"], &self.other);
            git(&["commit", "-q", "-m", "from elsewhere"], &self.other);
            git(&["push", "-q"], &self.other);
        }

        fn commit(&self, file: &str, text: &str) {
            std::fs::write(self.dir.join(file), text).unwrap();
            (self.run)(&["add", "-A"]).unwrap();
            (self.run)(&["commit", "-q", "-m", &format!("local {file}")]).unwrap();
        }
    }

    impl Drop for Remote {
        fn drop(&mut self) {
            for p in [&self.dir, &self.bare, &self.other] {
                let _ = std::fs::remove_dir_all(p);
            }
        }
    }

    #[test]
    fn the_sync_plan_follows_ahead_and_behind() {
        assert_eq!(sync_plan(true, 0, 0), [SyncOp::Fetch]);
        assert_eq!(sync_plan(true, 2, 0), [SyncOp::Push]);
        assert_eq!(sync_plan(true, 0, 3), [SyncOp::Pull]);
        assert_eq!(sync_plan(true, 1, 1), [SyncOp::Pull, SyncOp::Push]);
        assert_eq!(sync_plan(false, 0, 0), [SyncOp::Publish], "without an upstream, sync publishes");
        assert_eq!(sync_plan(false, 4, 0), [SyncOp::Publish]);
        assert_eq!(PostCommit::Sync.ops(true), [SyncOp::Pull, SyncOp::Push]);
        assert_eq!(PostCommit::Sync.ops(false), [SyncOp::Publish]);
        assert_eq!(PostCommit::Push.ops(false), [SyncOp::Push]);
        assert_eq!((PostCommit::from_setting("sync"), PostCommit::from_setting("none")), (Some(PostCommit::Sync), None));
    }

    #[test]
    fn counts_read_behind_first() {
        assert_eq!(counts_label(0, 0), "");
        assert_eq!(counts_label(2, 0), "↑2");
        assert_eq!(counts_label(0, 3), "↓3");
        assert_eq!(counts_label(2, 3), "↓3 ↑2");
    }

    #[test]
    fn publish_with_no_upstream_sets_tracking_and_sync_push_does_the_same() {
        let Some(r) = Remote::new("sync-publish") else { return };
        let host = host::local();
        let root = r.root();
        (r.run)(&["checkout", "-q", "-b", "feature"]).unwrap();
        assert!(!has_upstream(&host, &root));
        assert_eq!(sync(&host, &root, &SyncOp::Publish, SyncOptions::default()), Ok(SyncStep::Published("origin".into())));
        assert!(has_upstream(&host, &root));
        assert!((r.run)(&["--git-dir", r.bare.to_str().unwrap(), "rev-parse", "--verify", "-q", "feature"]).is_some(), "the branch is on the remote");
        // A plain push of an unpublished branch publishes it too.
        (r.run)(&["checkout", "-q", "-b", "second"]).unwrap();
        assert_eq!(sync(&host, &root, &SyncOp::Push, SyncOptions::default()), Ok(SyncStep::Done));
        assert!(has_upstream(&host, &root));
    }

    #[test]
    fn pull_merges_or_rebases_depending_on_the_option() {
        let Some(r) = Remote::new("sync-pull") else { return };
        let host = host::local();
        let root = r.root();
        r.commit("local.txt", "local\n");
        r.push_from_elsewhere("remote.txt", "remote\n");
        // Merge: a merge commit with two parents.
        assert_eq!(sync(&host, &root, &SyncOp::Pull, SyncOptions::default()), Ok(SyncStep::Done));
        assert!(r.dir.join("remote.txt").exists());
        assert_eq!((r.run)(&["rev-list", "--parents", "-1", "HEAD"]).unwrap().split_whitespace().count(), 3);
        // Rebase: history stays linear.
        r.commit("local2.txt", "local\n");
        r.push_from_elsewhere("remote2.txt", "remote\n");
        assert_eq!(sync(&host, &root, &SyncOp::PullRebase, SyncOptions::default()), Ok(SyncStep::Done));
        assert_eq!((r.run)(&["rev-list", "--parents", "-1", "HEAD"]).unwrap().split_whitespace().count(), 2, "rebased: one parent");
        // The option decides what plain Pull does.
        r.commit("local3.txt", "local\n");
        r.push_from_elsewhere("remote3.txt", "remote\n");
        assert_eq!(sync(&host, &root, &SyncOp::Pull, SyncOptions { rebase: true, ..Default::default() }), Ok(SyncStep::Done));
        assert_eq!((r.run)(&["rev-list", "--parents", "-1", "HEAD"]).unwrap().split_whitespace().count(), 2);
    }

    #[test]
    fn a_pull_that_conflicts_reports_it_for_merge_and_for_rebase() {
        let Some(r) = Remote::new("sync-conflict") else { return };
        let host = host::local();
        let root = r.root();
        r.commit("f.txt", "mine\n");
        r.push_from_elsewhere("f.txt", "theirs\n");
        assert_eq!(sync(&host, &root, &SyncOp::Pull, SyncOptions::default()), Ok(SyncStep::Conflicts));
        assert!(merge_in_progress(&host, &root));
        crate::git::merge_abort(&host, &root).unwrap();
        assert_eq!(sync(&host, &root, &SyncOp::PullRebase, SyncOptions::default()), Ok(SyncStep::Conflicts));
        assert!(rebase_in_progress(&host, &root));
        // While it is stopped, another pull does not start a second rebase.
        assert!(sync(&host, &root, &SyncOp::PullRebase, SyncOptions::default()).unwrap_err().contains("already in progress"));
        crate::git::rebase_abort(&host, &root).unwrap();
    }

    #[test]
    fn a_dirty_tree_that_blocks_the_pull_says_to_commit_or_stash() {
        let Some(r) = Remote::new("sync-dirty") else { return };
        let host = host::local();
        let root = r.root();
        r.push_from_elsewhere("f.txt", "theirs\n");
        std::fs::write(r.dir.join("f.txt"), "unsaved work\n").unwrap();
        for op in [SyncOp::Pull, SyncOp::PullRebase] {
            let err = sync(&host, &root, &op, SyncOptions::default()).unwrap_err();
            assert_eq!(err, "Commit or stash your changes first", "{op:?}");
        }
        assert_eq!(std::fs::read_to_string(r.dir.join("f.txt")).unwrap(), "unsaved work\n");
    }

    #[test]
    fn fetch_prunes_only_when_asked_and_pull_from_and_push_to_name_their_remote() {
        let Some(r) = Remote::new("sync-fetch") else { return };
        let host = host::local();
        let root = r.root();
        // A branch that exists on the remote, then is deleted there.
        (r.run)(&["push", "-q", "origin", "main:gone"]).unwrap();
        let listed = |r: &Remote| (r.run)(&["branch", "-r"]).unwrap().contains("origin/gone");
        assert!(listed(&r));
        crate::test_git::git().args(["--git-dir", r.bare.to_str().unwrap(), "branch", "-q", "-D", "gone"]).output().unwrap();
        assert_eq!(sync(&host, &root, &SyncOp::Fetch, SyncOptions { prune: false, ..Default::default() }), Ok(SyncStep::Done));
        assert!(listed(&r), "kept without pruning");
        assert_eq!(sync(&host, &root, &SyncOp::FetchAll, SyncOptions::default()), Ok(SyncStep::Done));
        assert!(!listed(&r), "pruned by default");
        // Pull from a named remote and branch, push to a named remote.
        r.push_from_elsewhere("remote.txt", "remote\n");
        let from = SyncOp::PullFrom { remote: "origin".into(), branch: "main".into() };
        assert_eq!(sync(&host, &root, &from, SyncOptions::default()), Ok(SyncStep::Done));
        assert!(r.dir.join("remote.txt").exists());
        r.commit("again.txt", "x\n");
        assert_eq!(sync(&host, &root, &SyncOp::PushTo { remote: "origin".into() }, SyncOptions::default()), Ok(SyncStep::Done));
        assert!((r.run)(&["rev-list", "--count", "origin/main..HEAD"]).unwrap().trim() == "0");
        // Names that look like options never get that far.
        assert!(sync(&host, &root, &SyncOp::PushTo { remote: "--mirror".into() }, SyncOptions::default()).is_err());
        assert!(sync(&host, &root, &SyncOp::PullFrom { remote: "origin".into(), branch: "--rebase".into() }, SyncOptions::default()).is_err());
    }

    #[test]
    fn autofetch_is_quiet_without_a_remote_and_fetches_with_one() {
        let Some((dir, _)) = repo("sync-autofetch-none") else { return };
        let host = host::local();
        assert_eq!(autofetch(&host, &dir.to_string_lossy(), SyncOptions::default()), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
        let Some(r) = Remote::new("sync-autofetch") else { return };
        r.push_from_elsewhere("remote.txt", "remote\n");
        assert_eq!(autofetch(&host, &r.root(), SyncOptions::default()), Ok(()));
        assert_eq!((r.run)(&["rev-list", "--count", "HEAD..origin/main"]).unwrap().trim(), "1", "fetched, not merged");
        assert!(!r.dir.join("remote.txt").exists());
    }

    /// A remote named like an option must not reach a command line: `git fetch --upload-pack=<cmd>` runs the command.
    #[cfg(unix)]
    #[test]
    fn a_remote_named_like_an_option_never_runs_as_one() {
        let Some(r) = Remote::new("sync-evil-remote") else { return };
        let host = host::local();
        let root = r.root();
        let marker = std::env::temp_dir().join(format!("codeyard-evil-remote-marker-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // Ref names cannot hold a space, so the shell gets `${IFS}`.
        let evil = format!("--upload-pack=touch${{IFS}}{}", marker.display());
        (r.run)(&["config", &format!("remote.{evil}.url"), "https://github.com/someone/repo.git"]).expect("git accepts the remote name");
        let listed = remotes(&host, &root).unwrap();
        assert!(listed.iter().all(|(n, _)| !n.starts_with('-')), "{listed:?}");
        assert!(listed.iter().any(|(n, _)| n == "origin"));
        assert!(fetch_pull(&host, &root, &evil, 1).is_err());
        assert!(sync(&host, &root, &SyncOp::PushTo { remote: evil.clone() }, SyncOptions::default()).is_err());
        assert!(!marker.exists(), "the command in the remote's name ran");
        // The control: the old command line, with the name passed on as it came, does run it (so the check above can fail).
        let _ = git(&host, &root, &["fetch", "--quiet", &evil, "pull/1/head"]);
        assert!(marker.exists(), "the control should have run the command");
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn a_pull_request_head_is_fetched_from_the_named_remote() {
        let Some(r) = Remote::new("sync-pr-fetch") else { return };
        let host = host::local();
        let root = r.root();
        // The server keeps a pull request's head under refs/pull/N/head.
        (r.run)(&["push", "-q", "origin", "HEAD:refs/pull/7/head"]).unwrap();
        assert_eq!(fetch_pull(&host, &root, "origin", 7), Ok(()));
        assert_eq!((r.run)(&["rev-parse", "FETCH_HEAD"]).unwrap().trim(), (r.run)(&["rev-parse", "HEAD"]).unwrap().trim());
        assert!(fetch_pull(&host, &root, "origin", 8).is_err(), "a missing pull request is an error");
    }
}
