//! `git stash`.

use host::Host;

use super::git_ok;

/// What goes into a new stash, and under which name.
#[derive(Clone, Debug, Default)]
pub struct StashOptions {
    /// Untracked files too (`-u`).
    pub untracked: bool,
    /// Only what is staged, leaving the rest of the working tree as it is (`--staged`, git 2.35 or newer).
    pub staged: bool,
    /// Replaces git's "WIP on branch: …" description.
    pub message: Option<String>,
}

/// `git stash push`; `untracked`: untracked files too.
pub fn stash(host: &dyn Host, root: &str, untracked: bool) -> Result<(), String> {
    stash_push(host, root, &StashOptions { untracked, ..Default::default() })
}

pub fn stash_push(host: &dyn Host, root: &str, opts: &StashOptions) -> Result<(), String> {
    let mut args = vec!["stash", "push", "-q"];
    if opts.untracked {
        args.push("-u");
    }
    if opts.staged {
        args.push("--staged");
    }
    // `-m` takes the next argument as its value, so a message that starts with `-` stays a message.
    if let Some(m) = opts.message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        args.extend(["-m", m]);
    }
    // With `--staged` and nothing staged git fails without a message, so ask first.
    if opts.staged && super::git(host, root, &["diff", "--cached", "--quiet"]).is_ok_and(|o| o.ok()) {
        return Err(crate::i18n::t("There are no staged changes to stash").into());
    }
    let before = git_ok(host, root, &["stash", "list", "--format=%H"]).unwrap_or_default();
    git_ok(host, root, &args)?;
    // With no changes git returns success but no stash is created.
    if git_ok(host, root, &["stash", "list", "--format=%H"]).unwrap_or_default() == before {
        return Err(if opts.staged {
            crate::i18n::t("There are no staged changes to stash").into()
        } else {
            crate::i18n::t("There are no local changes to stash").into()
        });
    }
    Ok(())
}

/// The commit behind `stash@{n}` and the files it changed (tracked files only: a stash keeps untracked ones apart).
/// That commit and its first parent are what a diff of the stash compares, like any other commit.
pub fn stash_files(host: &dyn Host, root: &str, stash: &str) -> Result<(String, Vec<(char, String, Option<String>)>), String> {
    let hash = git_ok(host, root, &["rev-parse", "--verify", "-q", &format!("{stash}^{{commit}}")])?.trim().to_string();
    let out = git_ok(host, root, &["diff", "--name-status", "-M", &format!("{hash}^"), &hash])?;
    Ok((hash, crate::graph::parse_name_status(&out)))
}

/// (ref "stash@{0}", description), newest first.
pub fn stash_list(host: &dyn Host, root: &str) -> Result<Vec<(String, String)>, String> {
    let out = git_ok(host, root, &["stash", "list", "--format=%gd%x09%s"])?;
    Ok(out.lines().filter_map(|l| l.split_once('\t')).map(|(r, m)| (r.to_string(), m.to_string())).collect())
}

/// `pop`: apply and drop from the list; otherwise just apply.
pub fn stash_apply(host: &dyn Host, root: &str, stash: &str, pop: bool) -> Result<(), String> {
    git_ok(host, root, &["stash", if pop { "pop" } else { "apply" }, "-q", stash]).map(|_| ())
}

pub fn stash_drop(host: &dyn Host, root: &str, stash: &str) -> Result<(), String> {
    git_ok(host, root, &["stash", "drop", "-q", stash]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo;

    /// `--staged` arrived in git 2.35; `stash push -h` lists it when it is there.
    fn git_has_staged_stash() -> bool {
        let out = crate::test_git::git().args(["stash", "push", "-h"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).contains("--staged") || String::from_utf8_lossy(&out.stderr).contains("--staged")
    }

    #[test]
    fn staged_only_stash_leaves_the_rest_in_place() {
        if !git_has_staged_stash() {
            return;
        }
        let Some((dir, run)) = repo("stashstaged") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        std::fs::write(dir.join("g.txt"), "g\n").unwrap();
        run(&["add", "g.txt"]).unwrap();
        run(&["commit", "-q", "-m", "g"]).unwrap();
        std::fs::write(dir.join("f.txt"), "staged edit\n").unwrap();
        run(&["add", "f.txt"]).unwrap();
        std::fs::write(dir.join("g.txt"), "unstaged edit\n").unwrap();
        // f.txt is the only staged file: it goes into the stash, the unstaged g.txt stays where it is.
        let opts = StashOptions { staged: true, ..Default::default() };
        assert_eq!(stash_push(&host, &root, &opts), Ok(()));
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "base\n");
        assert_eq!(std::fs::read_to_string(dir.join("g.txt")).unwrap(), "unstaged edit\n");
        assert_eq!(stash_list(&host, &root).unwrap().len(), 1);
        // Nothing staged any more: no stash is created and the message says why.
        let err = stash_push(&host, &root, &opts).unwrap_err();
        assert!(err.contains("no staged changes"), "{err}");
        assert_eq!(stash_list(&host, &root).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn named_stash_keeps_the_given_description() {
        let Some((dir, _run)) = repo("stashnamed") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        std::fs::write(dir.join("f.txt"), "edit\n").unwrap();
        let named = StashOptions { message: Some("  half-done parser  ".into()), ..Default::default() };
        assert_eq!(stash_push(&host, &root, &named), Ok(()));
        std::fs::write(dir.join("f.txt"), "edit 2\n").unwrap();
        // A blank name falls back to git's own description; a name starting with `-` is only text.
        assert_eq!(stash_push(&host, &root, &StashOptions { message: Some(" ".into()), ..Default::default() }), Ok(()));
        std::fs::write(dir.join("f.txt"), "edit 3\n").unwrap();
        assert_eq!(stash_push(&host, &root, &StashOptions { message: Some("--keep-index".into()), ..Default::default() }), Ok(()));
        let list = stash_list(&host, &root).unwrap();
        assert_eq!(list.len(), 3);
        assert!(list[0].1.ends_with("--keep-index"), "{list:?}");
        assert!(list[1].1.contains("WIP on"), "{list:?}");
        assert!(list[2].1.ends_with("half-done parser"), "{list:?}");
        // The plain entry point still behaves as before.
        std::fs::write(dir.join("new.txt"), "n\n").unwrap();
        assert!(stash(&host, &root, false).is_err(), "an untracked file alone is not stashed without -u");
        assert_eq!(stash(&host, &root, true), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stash_files_list_what_the_stash_changed() {
        let Some((dir, run)) = repo("stashfiles") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        std::fs::write(dir.join("f.txt"), "edited\n").unwrap();
        std::fs::write(dir.join("added.txt"), "a\n").unwrap();
        run(&["add", "added.txt"]).unwrap();
        assert_eq!(stash(&host, &root, false), Ok(()));
        let (hash, files) = stash_files(&host, &root, "stash@{0}").unwrap();
        assert_eq!(hash, run(&["rev-parse", "stash@{0}"]).unwrap().trim());
        let mut files: Vec<_> = files.into_iter().map(|(s, p, _)| (s, p)).collect();
        files.sort();
        assert_eq!(files, [('A', "added.txt".to_string()), ('M', "f.txt".to_string())]);
        // The commit's version of the file is the stashed text, its parent's the committed one.
        assert_eq!(crate::git::show(&host, &root, &hash, "f.txt").as_deref(), Some("edited\n"));
        assert_eq!(crate::git::show(&host, &root, &format!("{hash}^"), "f.txt").as_deref(), Some("base\n"));
        assert!(stash_files(&host, &root, "stash@{4}").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
