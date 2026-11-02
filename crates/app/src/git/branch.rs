//! Branches, checkout, worktrees, remotes, rebase and merge.

use host::Host;

use super::{failure_text, git, git_ok};

/// (local branches, remote branches "origin/x"); `HEAD` aliases excluded.
pub fn branches(host: &dyn Host, root: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let out = git_ok(host, root, &["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes"])?;
    Ok(parse_branches(&out))
}

/// A branch, tag or remote name that git would not take for an option. Git lets a repository hold a ref called
/// `--exec=cmd` (a hostile server can name its default branch so), and `git rebase --exec` runs it: names that start
/// with `-` never reach a command line.
pub(crate) fn plain_name(name: &str) -> Result<&str, String> {
    if name.is_empty() || name.starts_with('-') || name.contains(['\0', '\n', '\r']) {
        return Err(crate::i18n::tf("“{}” is not a usable branch or remote name", &[&name.escape_debug()]));
    }
    Ok(name)
}

pub fn parse_branches(out: &str) -> (Vec<String>, Vec<String>) {
    let (mut local, mut remote) = (Vec::new(), Vec::new());
    for line in out.lines() {
        if let Some(b) = line.strip_prefix("refs/heads/") {
            if plain_name(b).is_ok() {
                local.push(b.to_string());
            }
        } else if let Some(b) = line.strip_prefix("refs/remotes/").filter(|b| !b.ends_with("/HEAD")) {
            remote.push(b.to_string());
        }
    }
    (local, remote)
}

/// Switch branch. For a remote branch ("origin/x") a tracking local branch is created; `create`: new branch.
pub fn checkout(host: &dyn Host, root: &str, branch: &str, remote: bool, create: bool) -> Result<(), String> {
    plain_name(branch)?;
    let args: Vec<&str> = match (remote, create) {
        (_, true) => vec!["checkout", "-q", "-b", branch],
        (true, false) => vec!["checkout", "-q", "--track", branch],
        (false, false) => vec!["checkout", "-q", branch],
    };
    git_ok(host, root, &args).map(|_| ())
}

/// Rebase the current branch onto `onto` (non-interactive). `Ok(true)` on conflict: rebase in progress
/// (resolve the conflicts, then `rebase_continue` or `rebase_abort`).
pub fn rebase(host: &dyn Host, root: &str, onto: &str) -> Result<bool, String> {
    plain_name(onto)?;
    if rebase_in_progress(host, root) {
        return Err(crate::i18n::t("A rebase is already in progress: use Git: Continue Rebase or Git: Abort Rebase").into());
    }
    let out = git(host, root, &["-c", "core.editor=true", "rebase", onto])?;
    if out.ok() {
        return Ok(false);
    }
    // Conflict: a half-done rebase + an unresolved file (independent of the language of git's output).
    if rebase_in_progress(host, root) && has_unmerged(host, root) {
        return Ok(true);
    }
    Err(failure_text(&out))
}

pub fn rebase_continue(host: &dyn Host, root: &str) -> Result<bool, String> {
    let out = git(host, root, &["-c", "core.editor=true", "rebase", "--continue"])?;
    if out.ok() {
        return Ok(false);
    }
    if rebase_in_progress(host, root) && has_unmerged(host, root) {
        return Ok(true);
    }
    Err(failure_text(&out))
}

/// Whether there is an unresolved (conflicted) file.
pub fn has_unmerged(host: &dyn Host, root: &str) -> bool {
    git_ok(host, root, &["diff", "--name-only", "--diff-filter=U"]).is_ok_and(|o| !o.trim().is_empty())
}

pub fn rebase_abort(host: &dyn Host, root: &str) -> Result<(), String> {
    git_ok(host, root, &["rebase", "--abort"]).map(|_| ())
}

pub fn rebase_in_progress(host: &dyn Host, root: &str) -> bool {
    ["rebase-merge", "rebase-apply"].iter().any(|d| {
        git_ok(host, root, &["rev-parse", "--git-path", d])
            .ok()
            .map(|p| p.trim().to_string())
            .is_some_and(|p| host.stat(&if p.starts_with('/') { p } else { format!("{}/{p}", root.trim_end_matches('/')) }).is_ok())
    })
}

/// Worktree paths and branches for the command palette.
pub fn worktrees(host: &dyn Host, root: &str) -> Result<Vec<(String, Option<String>)>, String> {
    Ok(super::worktree_entries(host, root)?.into_iter().map(|entry| (entry.path, entry.branch)).collect())
}

/// New worktree: checks out `branch` if given; otherwise creates it from the current branch with `-b`.
pub fn add_worktree(host: &dyn Host, root: &str, path: &str, branch: &str) -> Result<(), String> {
    let exists = branches(host, root)?.0.iter().any(|b| b == branch);
    if exists {
        git_ok(host, root, &["worktree", "add", "--", path, branch]).map(|_| ())
    } else {
        git_ok(host, root, &["worktree", "add", "-b", branch, "--", path]).map(|_| ())
    }
}

/// Merge the branch into the current branch (`--no-edit`). On conflict `Ok(true)`: merge in progress,
/// conflicts are resolved in Source Control. Any other failure is `Err`.
pub fn merge(host: &dyn Host, root: &str, branch: &str) -> Result<bool, String> {
    plain_name(branch)?;
    let out = git(host, root, &["merge", "--no-edit", branch])?;
    if out.ok() {
        return Ok(false);
    }
    if merge_in_progress(host, root) && has_unmerged(host, root) {
        return Ok(true);
    }
    Err(failure_text(&out))
}

pub fn merge_abort(host: &dyn Host, root: &str) -> Result<(), String> {
    git_ok(host, root, &["merge", "--abort"]).map(|_| ())
}

/// Whether `.git/MERGE_HEAD` exists (merge half-done).
pub fn merge_in_progress(host: &dyn Host, root: &str) -> bool {
    git(host, root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok_and(|o| o.ok())
}

/// Delete a local branch. Without `force` an unmerged branch is not deleted (git `-d`).
pub fn delete_branch(host: &dyn Host, root: &str, branch: &str, force: bool) -> Result<(), String> {
    git_ok(host, root, &["branch", if force { "-D" } else { "-d" }, "--", branch]).map(|_| ())
}

/// Number of commits on `branch` that are not on the current branch (for the delete confirmation).
pub fn unmerged_commits(host: &dyn Host, root: &str, branch: &str) -> Result<usize, String> {
    let range = format!("HEAD..{branch}");
    let out = git_ok(host, root, &["rev-list", "--count", &range])?;
    out.trim().parse().map_err(|_| crate::i18n::tf("unexpected output: {}", &[&out.trim()]))
}

pub fn rename_branch(host: &dyn Host, root: &str, old: &str, new: &str) -> Result<(), String> {
    git_ok(host, root, &["branch", "-m", "--", old, new]).map(|_| ())
}

/// Remotes: (name, URL).
pub fn remotes(host: &dyn Host, root: &str) -> Result<Vec<(String, String)>, String> {
    let out = git(host, root, &["config", "--null", "--get-regexp", r"^remote\..*\."])?;
    // No matching configuration is a normal empty inventory (exit 1).
    if out.ok() || out.code == Some(1) && out.stdout.is_empty() && out.stderr.is_empty() {
        Ok(parse_remotes(&out.stdout_str()))
    } else {
        Err(failure_text(&out))
    }
}

/// Effective fetch URL after Git applies `url.*.insteadOf` rules. Keep the stored
/// value from `remotes` for editing; use this one when identifying the hosting service.
pub fn resolved_remote_url(host: &dyn Host, root: &str, name: &str) -> Result<String, String> {
    plain_name(name)?;
    let url = git_ok(host, root, &["remote", "get-url", "--", name])?;
    Ok(url.strip_suffix('\n').unwrap_or(&url).to_string())
}

/// Parse NUL-delimited `git config` records (`key\nvalue\0`). The value may contain
/// whitespace, including newlines. Keep the first configured fetch URL verbatim.
pub fn parse_remotes(out: &str) -> Vec<(String, String)> {
    let mut entries: Vec<(String, Option<String>)> = Vec::new();
    for record in out.split('\0') {
        let Some((key, value)) = record.split_once('\n') else { continue };
        let Some((name, field)) = key.strip_prefix("remote.").and_then(|key| key.rsplit_once('.')) else { continue };
        // Never offer option-shaped names to callers that will pass them to Git.
        if plain_name(name).is_err() {
            continue;
        }
        let index = entries.iter().position(|(entry, _)| entry == name).unwrap_or_else(|| {
            entries.push((name.to_string(), None));
            entries.len() - 1
        });
        if field == "url" && entries[index].1.is_none() {
            entries[index].1 = Some(value.to_string());
        }
    }
    let mut entries: Vec<_> = entries.into_iter().map(|(name, url)| (name, url.unwrap_or_default())).collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

pub fn add_remote(host: &dyn Host, root: &str, name: &str, url: &str) -> Result<(), String> {
    git_ok(host, root, &["remote", "add", "--", name, url]).map(|_| ())
}

pub fn remove_remote(host: &dyn Host, root: &str, name: &str) -> Result<(), String> {
    git_ok(host, root, &["remote", "remove", "--", name]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{publish_branch, repo};

    /// A hostile server can name a branch `--exec=<cmd>`; `git rebase <name>` would run it. Such names are neither offered nor passed on.
    #[cfg(unix)]
    #[test]
    fn a_branch_named_like_an_option_never_reaches_git_as_one() {
        let Some((dir, run)) = repo("dashbranch") else { return };
        let marker = std::env::temp_dir().join(format!("codeyard-dashbranch-marker-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // Ref names cannot hold a space, so the shell gets `${IFS}`.
        let evil = format!("--exec=touch${{IFS}}{}", marker.display());
        run(&["update-ref", &format!("refs/heads/{evil}"), "HEAD"]).expect("git accepts the ref name");
        run(&["checkout", "-q", "-b", "mine"]).unwrap();
        let host = host::local();
        let root = dir.to_str().unwrap();
        let (local, _) = branches(&host, root).unwrap();
        assert!(local.iter().any(|b| b == "mine"), "{local:?}");
        assert!(!local.iter().any(|b| b.starts_with('-')), "{local:?}");
        assert!(rebase(&host, root, &evil).is_err());
        assert!(merge(&host, root, &evil).is_err());
        assert!(checkout(&host, root, &evil, false, false).is_err());
        assert!(!marker.exists(), "the command in the branch name ran");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_delete_rename_and_abort() {
        let Some((dir, run)) = repo("branchops") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        // feature: changes f.txt; main: a separate file (merges cleanly).
        run(&["checkout", "-q", "-b", "feature"]).unwrap();
        std::fs::write(dir.join("g.txt"), "g\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "g"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        assert_eq!(merge(&host, &root, "feature"), Ok(false));
        assert!(dir.join("g.txt").exists());
        // A merged branch is deleted with -d; the current branch cannot be deleted.
        assert!(delete_branch(&host, &root, "main", false).is_err());
        assert_eq!(delete_branch(&host, &root, "feature", false), Ok(()));
        // Unmerged branch: -d refuses, -D deletes.
        run(&["checkout", "-q", "-b", "wip"]).unwrap();
        std::fs::write(dir.join("w.txt"), "w\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "w"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        assert!(delete_branch(&host, &root, "wip", false).unwrap_err().contains("not fully merged"));
        assert_eq!(delete_branch(&host, &root, "wip", true), Ok(()));
        // Rename.
        run(&["branch", "old"]).unwrap();
        assert_eq!(rename_branch(&host, &root, "old", "new"), Ok(()));
        assert!(branches(&host, &root).unwrap().0.contains(&"new".to_string()));
        // Conflict: both change f.txt differently → Ok(true), abort cleans up.
        run(&["checkout", "-q", "-b", "c1"]).unwrap();
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        run(&["commit", "-qam", "one"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "two"]).unwrap();
        assert_eq!(merge(&host, &root, "c1"), Ok(true));
        assert!(merge_in_progress(&host, &root));
        assert_eq!(merge_abort(&host, &root), Ok(()));
        assert!(!merge_in_progress(&host, &root));
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "two\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_config_parsing_keeps_fetch_urls_and_safe_names_in_sorted_order() {
        let config = "remote.z.fetch\n+refs/heads/*:refs/remotes/z/*\0remote.origin.pushurl\n/push only\0remote.origin.url\n/fetch with spaces\0remote.origin.url\n/second fetch\0remote.team.name.url\n/repo (fetch)\0remote.--upload-pack=command.url\nignored\0remote.empty.url\n\0remote.empty.url\nnot the first URL\0remote.pushdefault\norigin\0malformed\0";
        assert_eq!(
            parse_remotes(config),
            vec![
                ("empty".into(), "".into()),
                ("origin".into(), "/fetch with spaces".into()),
                ("team.name".into(), "/repo (fetch)".into()),
                ("z".into(), "".into()),
            ]
        );
    }

    #[test]
    fn remotes_preserve_complete_configured_urls() {
        let Some((dir, _)) = repo("remote-url-values") else { return };
        let host = host::local();
        let root = crate::ui::local_path(&dir);
        for url in ["C:/team repos/çalışma.git", "/srv/repo (fetch) (push)", " /srv/repo\twith tab.git ", "/srv/first\nsecond.git", ""] {
            git_ok(&host, &root, &["config", "remote.origin.url", url]).unwrap();
            assert_eq!(
                remotes(&host, &root).unwrap(),
                vec![("origin".into(), url.into())],
                "URL contents must not be split or interpreted as Git's display suffix"
            );
        }
        git_ok(&host, &root, &["config", "url.https://example.invalid/.insteadOf", "shortcut:"]).unwrap();
        git_ok(&host, &root, &["config", "remote.origin.url", "shortcut:project with spaces"]).unwrap();
        assert_eq!(resolved_remote_url(&host, &root, "origin").unwrap(), "https://example.invalid/project with spaces");
        assert_eq!(
            remotes(&host, &root).unwrap(),
            vec![("origin".into(), "shortcut:project with spaces".into())],
            "editing must show the stored URL rather than replacing its insteadOf shorthand"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remotes_add_remove_publish() {
        let Some((dir, run)) = repo("remotes") else { return };
        let bare = std::env::temp_dir().join(format!("codeyard-bare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        crate::test_git::git().args(["init", "-q", "--bare"]).arg(&bare).output().unwrap();
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        assert!(publish_branch(&host, &root).unwrap_err().contains("no remote"));
        assert_eq!(add_remote(&host, &root, "origin", &bare.to_string_lossy()), Ok(()));
        assert_eq!(remotes(&host, &root).unwrap().len(), 1);
        assert_eq!(publish_branch(&host, &root), Ok("origin".to_string()));
        assert!(run(&["rev-parse", "--abbrev-ref", "main@{upstream}"]).is_some(), "tracking was set up");
        assert_eq!(remove_remote(&host, &root, "origin"), Ok(()));
        assert!(remotes(&host, &root).unwrap().is_empty());
        assert_eq!(parse_remotes("remote.o.url\ngit@x:a.git\0remote.o.pushurl\ngit@x:push.git\0remote.u.url\nhttps://h/u\0").len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn rebase_and_worktrees() {
        let Some((dir, run)) = repo("rebasewt") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        // Clean rebase: feature lands on top of main's advanced state.
        run(&["checkout", "-q", "-b", "feat"]).unwrap();
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "a"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "b"]).unwrap();
        run(&["checkout", "-q", "feat"]).unwrap();
        assert_eq!(rebase(&host, &root, "main"), Ok(false));
        assert!(dir.join("b.txt").exists() && dir.join("a.txt").exists());
        assert!(!rebase_in_progress(&host, &root));
        // Conflicting rebase: Ok(true) → abort puts everything back.
        run(&["checkout", "-q", "-b", "c1", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        run(&["commit", "-qam", "one"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "two"]).unwrap();
        run(&["checkout", "-q", "c1"]).unwrap();
        assert_eq!(rebase(&host, &root, "main"), Ok(true));
        assert!(rebase_in_progress(&host, &root));
        assert_eq!(rebase_abort(&host, &root), Ok(()));
        assert!(!rebase_in_progress(&host, &root));
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "one\n");
        // Worktree: a new branch and an existing branch.
        let wt = std::env::temp_dir().join(format!("codeyard-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wt);
        assert_eq!(add_worktree(&host, &root, &wt.to_string_lossy(), "wt-new"), Ok(()));
        let list = worktrees(&host, &root).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|(_, b)| b.as_deref() == Some("wt-new")));
        assert!(wt.join("f.txt").exists() || wt.exists());
        let _ = crate::test_git::git().args(["worktree", "remove", "--force"]).arg(&wt).current_dir(&dir).output();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn error_texts_and_unmerged_count_report_the_real_cause() {
        let Some((dir, run)) = repo("errtext") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        // Dirty tree + merge: the error line must be the real cause, not "Updating …".
        run(&["checkout", "-q", "-b", "other"]).unwrap();
        std::fs::write(dir.join("f.txt"), "other\n").unwrap();
        run(&["commit", "-qam", "other"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "dirty\n").unwrap();
        let err = merge(&host, &root, "other").unwrap_err();
        assert!(!err.starts_with("Updating"), "{err}");
        assert!(!merge_in_progress(&host, &root));
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        // Count: a number on a normal branch, an error on an unborn HEAD.
        assert_eq!(unmerged_commits(&host, &root, "other"), Ok(1));
        run(&["checkout", "-q", "--orphan", "orph"]).unwrap();
        assert!(unmerged_commits(&host, &root, "other").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebase_refuses_when_one_is_running_and_reports_non_conflict_failures() {
        let Some((dir, run)) = repo("rebaseerr") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        run(&["checkout", "-q", "-b", "c1"]).unwrap();
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        run(&["commit", "-qam", "one"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "two"]).unwrap();
        run(&["checkout", "-q", "c1"]).unwrap();
        assert_eq!(rebase(&host, &root, "main"), Ok(true));
        assert!(rebase(&host, &root, "main").unwrap_err().contains("already in progress"));
        // Without resolving, continue: the conflict persists (Ok(true)); resolving and adding finishes it.
        assert_eq!(rebase_continue(&host, &root), Ok(true));
        std::fs::write(dir.join("f.txt"), "resolved\n").unwrap();
        run(&["add", "f.txt"]).unwrap();
        assert_eq!(rebase_continue(&host, &root), Ok(false));
        assert!(!rebase_in_progress(&host, &root));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
