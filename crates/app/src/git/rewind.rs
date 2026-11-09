//! What a reset or an undo takes off the current branch, counted before it runs: how many commits leave it, how many of
//! those are already on its upstream (the next Sync brings them back) and how many exist on no other branch, tag or
//! remote (afterwards only the reflog finds them). The `HEAD` they were counted at pins the operation the user agrees to.

use host::Host;

use super::{git, git_ok};
use crate::i18n::t;

/// The commits a reset or an undo would take off the current branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Leaving {
    /// `HEAD` when they were counted.
    pub head: String,
    pub count: usize,
    /// The branch's upstream (`origin/main`), when it has one.
    pub upstream: Option<String>,
    /// How many are on the upstream.
    pub pushed: usize,
    /// How many are on no other local branch, remote-tracking branch or tag.
    pub only_here: usize,
}

/// The commits a reset to `target` takes off the branch: those `HEAD` has and `target` has not.
pub fn leaving(host: &dyn Host, root: &str, target: &str) -> Result<Leaving, String> {
    count(host, root, &["HEAD", &format!("^{target}")])
}

/// The commits *Undo Last Commit* takes off the branch: `HEAD` alone, or with a merge the commits it brought in.
pub fn undoing(host: &dyn Host, root: &str) -> Result<Leaving, String> {
    let has_parent = git(host, root, &["rev-parse", "--verify", "-q", "HEAD~1"]).is_ok_and(|o| o.ok());
    if has_parent { count(host, root, &["HEAD", "^HEAD~1"]) } else { count(host, root, &["HEAD"]) }
}

fn count(host: &dyn Host, root: &str, range: &[&str]) -> Result<Leaving, String> {
    let head = git_ok(host, root, &["rev-parse", "--verify", "-q", "HEAD"]).map_err(|_| t("There is no commit to undo").to_string())?.trim().to_string();
    let n = |extra: &[&str]| -> Result<usize, String> {
        let mut args = vec!["rev-list", "--count"];
        args.extend_from_slice(range);
        args.extend_from_slice(extra);
        git_ok(host, root, &args)?.trim().parse::<usize>().map_err(|e| e.to_string())
    };
    let count = n(&[])?;
    let upstream_ref = git(host, root, &["rev-parse", "--symbolic-full-name", "@{upstream}"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.stdout_str().trim().to_string())
        .filter(|r| !r.is_empty());
    let pushed = match &upstream_ref {
        Some(r) => count - n(&[&format!("^{r}")])?,
        None => 0,
    };
    let branch = git(host, root, &["symbolic-ref", "-q", "--short", "HEAD"]).ok().filter(|o| o.ok()).map(|o| o.stdout_str().trim().to_string());
    let exclude = branch.as_ref().map(|b| format!("--exclude={b}"));
    let mut elsewhere = vec!["--not", "--remotes", "--tags"];
    elsewhere.extend(exclude.as_deref());
    elsewhere.push("--branches");
    let only_here = n(&elsewhere)?;
    let upstream = upstream_ref.map(|r| r.strip_prefix("refs/remotes/").map(str::to_string).unwrap_or(r));
    Ok(Leaving { head, count, upstream, pushed, only_here })
}

/// `HEAD` is still `expected` (the commit the question was about), or nothing may run.
pub fn head_is(host: &dyn Host, root: &str, expected: &str) -> Result<(), String> {
    let now = git_ok(host, root, &["rev-parse", "--verify", "-q", "HEAD"]).unwrap_or_default();
    if now.trim() == expected { Ok(()) } else { Err(moved_message()) }
}

pub fn moved_message() -> String {
    t("The branch moved while the question was open. Nothing was changed.").into()
}

/// The short hash of `HEAD`, for "the branch was at …" (empty without a commit).
pub fn short_head(host: &dyn Host, root: &str) -> String {
    git_ok(host, root, &["rev-parse", "--short", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default()
}

#[cfg(test)]
#[path = "rewind_tests.rs"]
mod tests;
