//! Branch details for the picker, and the less common ref operations: detached checkout, branch from a ref, remote
//! branches, upstream, and the remotes and worktrees that already exist (rename, URL, remove, prune).

use host::Host;

use super::{git, git_ok, plain_name, remotes};

/// A branch with what the picker shows next to its name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchInfo {
    /// `main`, or `origin/main` for a remote branch.
    pub name: String,
    pub remote: bool,
    pub current: bool,
    /// `origin/main`, when this local branch tracks one.
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// The upstream is configured but no longer exists on the remote.
    pub gone: bool,
    /// Committer time of the tip, seconds since the epoch.
    pub when: Option<i64>,
    pub subject: String,
}

/// Local and remote branches, most recently committed to first.
pub fn branch_infos(host: &dyn Host, root: &str) -> Result<Vec<BranchInfo>, String> {
    let format = "--format=%(refname)%09%(HEAD)%09%(upstream:short)%09%(upstream:track)%09%(committerdate:unix)%09%(subject)";
    let out = git_ok(host, root, &["for-each-ref", "--sort=-committerdate", format, "refs/heads", "refs/remotes"])?;
    Ok(parse_branch_infos(&out))
}

/// Same filtering as `parse_branches`: `HEAD` aliases and names that start with `-` are dropped.
pub fn parse_branch_infos(out: &str) -> Vec<BranchInfo> {
    let mut v = Vec::new();
    for line in out.lines() {
        // The subject is last and may hold tabs itself.
        let mut f = line.splitn(6, '\t');
        let (Some(refname), Some(head), Some(upstream), Some(track), Some(when), subject) = (f.next(), f.next(), f.next(), f.next(), f.next(), f.next()) else {
            continue;
        };
        let (name, remote) = if let Some(b) = refname.strip_prefix("refs/heads/") {
            (b, false)
        } else if let Some(b) = refname.strip_prefix("refs/remotes/").filter(|b| !b.ends_with("/HEAD")) {
            (b, true)
        } else {
            continue;
        };
        // On a remote branch only the part after the remote's name could pose as an option.
        let branch_part = if remote { name.split_once('/').map_or(name, |(_, b)| b) } else { name };
        if plain_name(name).is_err() || branch_part.starts_with('-') {
            continue;
        }
        let (ahead, behind, gone) = parse_track(track);
        v.push(BranchInfo {
            name: name.to_string(),
            remote,
            current: head == "*",
            upstream: Some(upstream).filter(|u| !u.is_empty()).map(str::to_string),
            ahead,
            behind,
            gone,
            when: when.parse().ok(),
            subject: subject.unwrap_or("").to_string(),
        });
    }
    v
}

/// `%(upstream:track)`: "", "[ahead 2]", "[behind 1]", "[ahead 2, behind 1]" or "[gone]" -> (ahead, behind, gone).
pub fn parse_track(track: &str) -> (usize, usize, bool) {
    let track = track.trim().trim_start_matches('[').trim_end_matches(']');
    if track == "gone" {
        return (0, 0, true);
    }
    let (mut ahead, mut behind) = (0, 0);
    for part in track.split(',') {
        let mut w = part.split_whitespace();
        match (w.next(), w.next().and_then(|n| n.parse().ok())) {
            (Some("ahead"), Some(n)) => ahead = n,
            (Some("behind"), Some(n)) => behind = n,
            _ => {}
        }
    }
    (ahead, behind, false)
}

/// The remote that tags and branches go to when none is chosen: `origin`, else the first one.
pub(crate) fn default_remote(host: &dyn Host, root: &str) -> Result<String, String> {
    let list = remotes(host, root)?;
    let remote = list
        .iter()
        .find(|(n, _)| n == "origin")
        .or(list.first())
        .map(|(n, _)| n.clone())
        .ok_or(crate::i18n::t("This repository has no remote. Add one with Git: Add Remote…"))?;
    plain_name(&remote)?;
    Ok(remote)
}

/// Check out a branch, tag or commit without moving a branch along (detached HEAD).
pub fn checkout_detached(host: &dyn Host, root: &str, rev: &str) -> Result<(), String> {
    plain_name(rev)?;
    git_ok(host, root, &["checkout", "-q", "--detach", rev]).map(|_| ())
}

/// New branch `name` that starts at `start` (a branch, tag or commit) and becomes the current one.
pub fn create_branch_from(host: &dyn Host, root: &str, name: &str, start: &str) -> Result<(), String> {
    plain_name(name)?;
    plain_name(start)?;
    git_ok(host, root, &["checkout", "-q", "-b", name, start]).map(|_| ())
}

/// "origin/feature/x" -> ("origin", "feature/x"), using the remotes that exist (their names can hold a slash too).
pub fn split_remote_branch<'a>(remotes: &[(String, String)], full: &'a str) -> Option<(&'a str, &'a str)> {
    remotes
        .iter()
        .filter_map(|(n, _)| full.strip_prefix(n.as_str()).and_then(|rest| rest.strip_prefix('/')).map(|b| (&full[..n.len()], b)))
        .max_by_key(|(r, _)| r.len())
        .filter(|(_, b)| !b.is_empty())
}

/// Delete `origin/x` on the server (`git push origin --delete x`); the local remote-tracking ref goes with it.
pub fn delete_remote_branch(host: &dyn Host, root: &str, full: &str) -> Result<(), String> {
    plain_name(full)?;
    let list = remotes(host, root)?;
    let (remote, branch) = split_remote_branch(&list, full).ok_or_else(|| crate::i18n::tf("“{}” is not a branch of a known remote", &[&full]))?;
    plain_name(branch)?;
    git_ok(host, root, &["push", remote, &format!(":refs/heads/{branch}")]).map(|_| ())
}

/// The current branch follows `upstream` (`origin/main`) from now on.
pub fn set_upstream(host: &dyn Host, root: &str, upstream: &str) -> Result<(), String> {
    plain_name(upstream)?;
    git_ok(host, root, &["branch", &format!("--set-upstream-to={upstream}")]).map(|_| ())
}

/// Rename a remote; git also moves its remote-tracking branches and the branches' upstream settings.
pub fn rename_remote(host: &dyn Host, root: &str, old: &str, new: &str) -> Result<(), String> {
    plain_name(old)?;
    plain_name(new)?;
    git_ok(host, root, &["remote", "rename", "--", old, new]).map(|_| ())
}

pub fn set_remote_url(host: &dyn Host, root: &str, name: &str, url: &str) -> Result<(), String> {
    plain_name(name)?;
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') {
        return Err(crate::i18n::tf("“{}” is not a usable remote URL", &[&url.escape_debug()]));
    }
    git_ok(host, root, &["remote", "set-url", "--", name, url]).map(|_| ())
}

/// Remove a linked worktree. Git refuses one with uncommitted changes unless `force`.
pub fn remove_worktree(host: &dyn Host, root: &str, path: &str, force: bool) -> Result<(), String> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", path]);
    git_ok(host, root, &args).map(|_| ())
}

/// Forget worktrees whose folder is gone; returns how many records were dropped.
pub fn prune_worktrees(host: &dyn Host, root: &str) -> Result<usize, String> {
    let out = git(host, root, &["worktree", "prune", "-v"])?;
    if !out.ok() {
        return Err(super::failure_text(&out));
    }
    // `-v` reports each dropped record on stderr.
    Ok(out.stderr_str().lines().chain(out.stdout_str().lines()).filter(|l| l.starts_with("Removing ")).count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{add_worktree, branches, repo, worktrees};

    fn bare(name: &str) -> std::path::PathBuf {
        let bare = std::env::temp_dir().join(format!("codeyard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        crate::test_git::git().args(["init", "-q", "--bare"]).arg(&bare).output().unwrap();
        bare
    }

    #[test]
    fn track_text_variants() {
        assert_eq!(parse_track(""), (0, 0, false));
        assert_eq!(parse_track("[ahead 2]"), (2, 0, false));
        assert_eq!(parse_track("[behind 13]"), (0, 13, false));
        assert_eq!(parse_track("[ahead 2, behind 1]"), (2, 1, false));
        assert_eq!(parse_track("[gone]"), (0, 0, true));
    }

    #[test]
    fn picker_rows_carry_tracking_and_last_commit() {
        let line = |r: &str, head: &str, up: &str, track: &str, when: &str, subject: &str| format!("{r}\t{head}\t{up}\t{track}\t{when}\t{subject}\n");
        let out = [
            line("refs/heads/main", "*", "origin/main", "[ahead 1, behind 2]", "1700000100", "fix: a\ttab in the subject"),
            line("refs/heads/topic", " ", "", "", "1700000000", "wip"),
            line("refs/heads/old", " ", "origin/old", "[gone]", "1600000000", "old work"),
            line("refs/remotes/origin/main", " ", "", "", "1700000100", "fix"),
            line("refs/remotes/origin/HEAD", " ", "", "", "1700000100", "fix"),
        ]
        .concat();
        let infos = parse_branch_infos(&out);
        assert_eq!(infos.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["main", "topic", "old", "origin/main"]);
        let main = &infos[0];
        assert!(main.current && !main.remote);
        assert_eq!((main.ahead, main.behind, main.gone), (1, 2, false));
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!(main.subject, "fix: a\ttab in the subject");
        assert_eq!(main.when, Some(1_700_000_100));
        assert!(infos[2].gone && infos[2].ahead == 0);
        assert!(infos[3].remote && infos[3].upstream.is_none());
    }

    #[test]
    fn picker_rows_drop_names_that_look_like_options() {
        let out =
            "refs/heads/--exec=x\t \t\t\t1\ts\nrefs/heads/-D\t \t\t\t1\ts\nrefs/remotes/origin/--upload-pack=x\t \t\t\t1\ts\nrefs/heads/fine\t \t\t\t1\ts\n";
        let infos = parse_branch_infos(out);
        assert_eq!(infos.len(), 1, "{infos:?}");
        assert_eq!(infos[0].name, "fine");
        // The plain-name list keeps working next to it.
        assert_eq!(crate::git::parse_branches("refs/heads/--exec=x\nrefs/heads/fine\n").0, ["fine"]);
    }

    #[test]
    fn branch_infos_from_a_real_repository() {
        let Some((dir, run)) = repo("infos") else { return };
        let bare = bare("infos-bare");
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        run(&["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();
        run(&["push", "-q", "-u", "origin", "main"]).unwrap();
        // One commit ahead locally; a second branch with a newer, different commit.
        std::fs::write(dir.join("f.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "second"]).unwrap();
        run(&["checkout", "-q", "-b", "newer"]).unwrap();
        std::fs::write(dir.join("g.txt"), "g\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "tip of newer"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        let infos = branch_infos(&host, &root).unwrap();
        let main = infos.iter().find(|b| b.name == "main").unwrap();
        assert!(main.current && main.upstream.as_deref() == Some("origin/main"));
        assert_eq!((main.ahead, main.behind), (1, 0));
        assert_eq!(main.subject, "second");
        assert!(main.when.is_some_and(|t| t > 1_600_000_000));
        let newer = infos.iter().find(|b| b.name == "newer").unwrap();
        assert!(!newer.current && newer.upstream.is_none());
        assert_eq!(newer.subject, "tip of newer");
        assert!(infos.iter().any(|b| b.remote && b.name == "origin/main"));
        assert!(infos.iter().all(|b| b.name != "origin/HEAD"));
        // Same names as the plain listing.
        let (local, remote) = branches(&host, &root).unwrap();
        let mut got: Vec<_> = infos.iter().map(|b| b.name.clone()).collect();
        let mut want: Vec<_> = local.into_iter().chain(remote).collect();
        got.sort();
        want.sort();
        assert_eq!(got, want);
        // The upstream goes away on the server: it shows as gone after a pruning fetch.
        run(&["push", "-q", "origin", "newer"]).unwrap();
        run(&["branch", "-q", "--set-upstream-to=origin/newer", "newer"]).unwrap();
        run(&["push", "-q", "origin", "--delete", "newer"]).unwrap();
        run(&["fetch", "-q", "--prune"]).unwrap();
        let infos = branch_infos(&host, &root).unwrap();
        assert!(infos.iter().find(|b| b.name == "newer").unwrap().gone);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn detached_checkout_and_branch_from_a_ref() {
        let Some((dir, run)) = repo("fromref") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        run(&["tag", "v1"]).unwrap();
        std::fs::write(dir.join("f.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "second"]).unwrap();
        assert_eq!(checkout_detached(&host, &root, "v1"), Ok(()));
        assert_eq!(run(&["branch", "--show-current"]).unwrap().trim(), "");
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "base\n");
        assert_eq!(create_branch_from(&host, &root, "hotfix", "v1"), Ok(()));
        assert_eq!(run(&["branch", "--show-current"]).unwrap().trim(), "hotfix");
        assert_eq!(create_branch_from(&host, &root, "side", "main"), Ok(()));
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "two\n");
        // Neither end can pose as an option, and an existing name is not reused.
        assert!(create_branch_from(&host, &root, "--orphan", "main").is_err());
        assert!(create_branch_from(&host, &root, "x", "--detach").is_err());
        assert!(checkout_detached(&host, &root, "--force").is_err());
        assert!(create_branch_from(&host, &root, "hotfix", "main").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_branch_delete_and_upstream() {
        let Some((dir, run)) = repo("remotebranch") else { return };
        let bare = bare("remotebranch-bare");
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        run(&["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();
        run(&["push", "-q", "origin", "main", "main:feature/x", "main:other"]).unwrap();
        run(&["fetch", "-q"]).unwrap();
        // No upstream yet: set it, then read it back.
        assert_eq!(set_upstream(&host, &root, "origin/other"), Ok(()));
        assert_eq!(run(&["rev-parse", "--abbrev-ref", "main@{upstream}"]).unwrap().trim(), "origin/other");
        assert!(set_upstream(&host, &root, "origin/nope").is_err());
        assert!(set_upstream(&host, &root, "--unset").is_err());
        // A slash in the branch name stays with the branch.
        let list = vec![("origin".to_string(), String::new())];
        assert_eq!(split_remote_branch(&list, "origin/feature/x"), Some(("origin", "feature/x")));
        assert_eq!(split_remote_branch(&list, "upstream/x"), None);
        assert_eq!(split_remote_branch(&list, "origin/"), None);
        assert_eq!(delete_remote_branch(&host, &root, "origin/feature/x"), Ok(()));
        assert!(run(&["ls-remote", "--heads", "origin", "feature/x"]).unwrap().trim().is_empty());
        assert!(!branches(&host, &root).unwrap().1.contains(&"origin/feature/x".to_string()), "the tracking ref went with it");
        assert!(delete_remote_branch(&host, &root, "nowhere/x").is_err());
        assert!(delete_remote_branch(&host, &root, "--delete").is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn rename_and_retarget_a_remote() {
        let Some((dir, run)) = repo("remotename") else { return };
        let bare = bare("remotename-bare");
        let other = bare.with_extension("moved");
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        run(&["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();
        run(&["push", "-q", "-u", "origin", "main"]).unwrap();
        assert_eq!(rename_remote(&host, &root, "origin", "upstream"), Ok(()));
        let list = remotes(&host, &root).unwrap();
        assert_eq!(list.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["upstream"]);
        // Branches that tracked it follow the new name.
        assert_eq!(run(&["rev-parse", "--abbrev-ref", "main@{upstream}"]).unwrap().trim(), "upstream/main");
        assert!(rename_remote(&host, &root, "ghost", "x").is_err());
        assert!(rename_remote(&host, &root, "upstream", "--x").is_err());
        assert_eq!(set_remote_url(&host, &root, "upstream", " https://example.com/a/b.git \n"), Ok(()));
        assert_eq!(remotes(&host, &root).unwrap()[0].1, "https://example.com/a/b.git");
        assert!(set_remote_url(&host, &root, "upstream", "").is_err());
        assert!(set_remote_url(&host, &root, "upstream", "--upload-pack=x").is_err());
        assert!(set_remote_url(&host, &root, "ghost", "https://x/y").is_err());
        assert_eq!(remotes(&host, &root).unwrap()[0].1, "https://example.com/a/b.git");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn remove_and_prune_worktrees() {
        let Some((dir, _run)) = repo("wtremove") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        let wt = std::env::temp_dir().join(format!("codeyard-wtremove-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wt);
        let path = wt.to_string_lossy().into_owned();
        assert_eq!(add_worktree(&host, &root, &path, "linked"), Ok(()));
        // A dirty worktree is kept unless forced.
        std::fs::write(wt.join("dirty.txt"), "x\n").unwrap();
        assert!(remove_worktree(&host, &root, &path, false).is_err());
        assert!(wt.exists());
        assert_eq!(remove_worktree(&host, &root, &path, true), Ok(()));
        assert!(!wt.exists());
        assert_eq!(worktrees(&host, &root).unwrap().len(), 1);
        // Pruning: delete a worktree's folder behind git's back, then forget it.
        assert_eq!(add_worktree(&host, &root, &path, "gone"), Ok(()));
        std::fs::remove_dir_all(&wt).unwrap();
        assert_eq!(worktrees(&host, &root).unwrap().len(), 2, "git still lists it");
        assert_eq!(prune_worktrees(&host, &root), Ok(1));
        assert_eq!(worktrees(&host, &root).unwrap().len(), 1);
        assert_eq!(prune_worktrees(&host, &root), Ok(0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
