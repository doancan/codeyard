//! `git stash`.

use host::Host;

use super::git_ok;

/// `git stash push`; `untracked`: untracked files too.
pub fn stash(host: &dyn Host, root: &str, untracked: bool) -> Result<(), String> {
    let mut args = vec!["stash", "push", "-q"];
    if untracked {
        args.push("-u");
    }
    let before = git_ok(host, root, &["stash", "list", "--format=%H"]).unwrap_or_default();
    git_ok(host, root, &args)?;
    // With no changes git returns success but no stash is created.
    if git_ok(host, root, &["stash", "list", "--format=%H"]).unwrap_or_default() == before {
        return Err(crate::i18n::t("There are no local changes to stash").into());
    }
    Ok(())
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
