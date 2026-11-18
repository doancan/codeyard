//! Squashing and reordering the commits that are not pushed yet (#270). The new history is built off to the side, in a temporary
//! worktree that starts at the run's parent, and installed with one `update-ref` that names the old tip: the branch has either
//! the whole new history or stays as it was, and the checkout's index and files are never touched (the tree at the tip is the
//! same before and after, which is checked, so what is in the index and on disk stays right).
//!
//! What is refused, before anything is written: a detached `HEAD`, a commit that is on the upstream (or on any remote when there
//! is no upstream), a merge among the commits that would be rewritten, the repository's first commit, a branch that is checked
//! out in another worktree, and more than [`MAX_COMMITS`] commits. Commits above a squashed run are replayed on top of it, so
//! none is dropped; a reorder that cannot be replayed without a conflict names the two commits and changes nothing.

use host::Host;

use super::{git, git_ok, git_stdin};
use crate::i18n::{t, tf, tn};

/// Most commits one rewrite replays.
pub const MAX_COMMITS: usize = 1000;

/// What to do to the commits that are not pushed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rewrite {
    /// Make one commit of `run` (oldest first, an unbroken run on the branch) with `message`.
    Squash { run: Vec<String>, message: String },
    /// Swap a commit with its neighbour: `down` with its parent (older), otherwise with its child (newer).
    Move { hash: String, down: bool },
}

/// A commit's step on the way to the new history.
enum Step {
    /// Replay one commit as it was (its message, author and date).
    Pick(String),
    /// Replay several as one commit with this message, the author and date of the first.
    Squash(Vec<String>, String),
}

struct Plan {
    branch: String,
    old_tip: String,
    /// Where the new history starts: the parent of the lowest commit that changes.
    base: String,
    steps: Vec<Step>,
    /// The two commits a move swaps (the one asked for, its neighbour), named when the swap cannot be replayed.
    swapped: Option<(String, String)>,
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(7)]
}

/// The commits of `run` joined, oldest first, as the starting text of the squashed commit's message.
pub fn default_squash_message(host: &dyn Host, root: &str, run: &[String]) -> Result<String, String> {
    let mut messages = Vec::new();
    for hash in run {
        messages.push(git_ok(host, root, &["show", "-s", "--format=%B", hash])?.trim().to_string());
    }
    Ok(messages.join("\n\n"))
}

/// Rewrite the branch `HEAD` is on. `expected_tip` is the commit the caller saw as the tip: when the branch is anywhere else
/// nothing is changed. The text is what to tell the user.
pub fn rewrite(host: &dyn Host, root: &str, expected_tip: &str, what: &Rewrite) -> Result<String, String> {
    rewrite_limited(host, root, expected_tip, what, MAX_COMMITS)
}

fn rewrite_limited(host: &dyn Host, root: &str, expected_tip: &str, what: &Rewrite, limit: usize) -> Result<String, String> {
    let plan = plan(host, root, expected_tip, what, limit)?;
    let dir = format!("{}/codeyard-rewrite-{}", git_ok(host, root, &["rev-parse", "--absolute-git-dir"])?.trim(), now_ms());
    let built = git_ok(host, root, &["worktree", "add", "--detach", "--quiet", &dir, &plan.base]).and_then(|_| build(host, &dir, &plan));
    // Gone in every outcome: a failure, a refusing hook and a conflict leave nothing behind.
    let _ = git(host, root, &["worktree", "remove", "--force", &dir]);
    let _ = git(host, root, &["worktree", "prune"]);
    let new_tip = built?;
    // The files at the tip must be what they were: then the index and the working tree, which belong to the old tip, are right
    // for the new one too.
    let (old_tree, new_tree) = (tree_of(host, root, &plan.old_tip)?, tree_of(host, root, &new_tip)?);
    if old_tree != new_tree {
        return Err(t("The rewritten commits would end with different files. Nothing was changed.").into());
    }
    install(host, root, &plan, &new_tip)?;
    Ok(match what {
        Rewrite::Squash { run, .. } => tn("Squashed {0} commit into {1}", "Squashed {0} commits into {1}", run.len(), &[&short(&new_tip)]),
        Rewrite::Move { hash, .. } => tf("Moved {}", &[&short(hash)]),
    })
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

fn tree_of(host: &dyn Host, root: &str, commit: &str) -> Result<String, String> {
    Ok(git_ok(host, root, &["rev-parse", &format!("{commit}^{{tree}}")])?.trim().to_string())
}

/// Check everything that can be checked without writing, and decide the steps.
fn plan(host: &dyn Host, root: &str, expected_tip: &str, what: &Rewrite, limit: usize) -> Result<Plan, String> {
    let branch = git(host, root, &["symbolic-ref", "-q", "--short", "HEAD"])?;
    let branch = branch.ok().then(|| branch.stdout_str().trim().to_string()).filter(|b| !b.is_empty());
    let Some(branch) = branch else { return Err(t("Commits can only be rewritten on a branch, and HEAD is detached.").into()) };
    let old_tip = git_ok(host, root, &["rev-parse", "--verify", "-q", "HEAD"]).map_err(|_| super::moved_message())?.trim().to_string();
    if old_tip != expected_tip {
        return Err(super::moved_message());
    }
    // A merge, cherry-pick or revert waiting for the user: moving the branch under it would leave it pointing at the wrong place.
    if let Some(label) = super::blocking_operation(host, root) {
        return Err(super::finish_first(label));
    }
    if checked_out_elsewhere(host, root, &branch)? {
        return Err(tf("The branch {} is checked out in another worktree, so its commits cannot be rewritten here.", &[&branch]));
    }
    // The commits that are not on the upstream (or on any remote when there is none), newest first along the branch.
    let upstream = git(host, root, &["rev-parse", "--symbolic-full-name", "@{upstream}"])?;
    let upstream = upstream.ok().then(|| upstream.stdout_str().trim().to_string()).filter(|r| !r.is_empty());
    let exclude: Vec<String> = match &upstream {
        Some(r) => vec![format!("^{r}")],
        None => vec!["--not".into(), "--remotes".into()],
    };
    let range = |extra: &[&str]| -> Vec<String> {
        let mut args: Vec<String> = vec!["rev-list".into()];
        args.extend(extra.iter().map(|s| s.to_string()));
        args.push("HEAD".into());
        args.extend(exclude.clone());
        args
    };
    let run_git = |args: Vec<String>| git_ok(host, root, &args.iter().map(String::as_str).collect::<Vec<_>>());
    let unpushed: std::collections::HashSet<String> = run_git(range(&[]))?.lines().map(str::to_string).collect();
    // `hash parent...` per commit of the branch's own line, newest first.
    let chain: Vec<(String, Vec<String>)> = run_git(range(&["--first-parent", "--parents"]))?
        .lines()
        .map(|line| {
            let mut ids = line.split_whitespace().map(str::to_string);
            (ids.next().unwrap_or_default(), ids.collect())
        })
        .collect();
    let at = |hash: &str| chain.iter().position(|(h, _)| h == hash);
    let mut swapped = None;
    // `lowest` and `highest` are positions in `chain`: the oldest commit that changes has the biggest one.
    let (lowest, highest, steps_for): (usize, usize, Box<dyn Fn(&[String]) -> Vec<Step>>) = match what {
        Rewrite::Squash { run, message } => {
            if run.len() < 2 {
                return Err(t("Choose at least two commits to squash.").into());
            }
            if run.iter().any(|h| !unpushed.contains(h)) {
                return Err(t("Only commits that are not pushed can be squashed").into());
            }
            let (Some(oldest), Some(newest)) = (at(&run[0]), at(&run[run.len() - 1])) else {
                return Err(t("The chosen commits are not one run of this branch.").into());
            };
            // `chain` is newest first: the run is the slice from its newest to its oldest, which must be exactly `run`.
            let in_order = oldest >= newest && oldest - newest + 1 == run.len() && chain[newest..=oldest].iter().rev().zip(run).all(|((h, _), r)| h == r);
            if !in_order {
                return Err(t("The chosen commits are not one run of this branch.").into());
            }
            let (run, message) = (run.clone(), message.clone());
            (oldest, newest, Box::new(move |_| vec![Step::Squash(run.clone(), message.clone())]))
        }
        Rewrite::Move { hash, down } => {
            let Some(here) = at(hash) else { return Err(t("Only commits that are not pushed can be moved").into()) };
            // `chain[0]` is the tip: the neighbour above is the next newer one, below the next older one.
            let other = if *down { here + 1 } else { here.wrapping_sub(1) };
            let Some((neighbour, _)) = chain.get(other) else {
                return Err(if *down { t("There is no commit below to swap with") } else { t("There is no commit above to swap with") }.into());
            };
            if !unpushed.contains(hash) || !unpushed.contains(neighbour) {
                return Err(t("Only commits that are not pushed can be moved").into());
            }
            let (low, high) = (here.max(other), here.min(other));
            swapped = Some((hash.clone(), neighbour.clone()));
            (low, high, Box::new(move |c: &[String]| vec![Step::Pick(c[0].clone()), Step::Pick(c[1].clone())]))
        }
    };
    // Every commit from the run's oldest to the tip is replayed, so each must be an ordinary single-parent commit; the
    // oldest needs a parent to start from.
    if chain[highest..=lowest].iter().any(|(_, parents)| parents.len() > 1) {
        return Err(t("A merge commit cannot be part of the run, or above it").into());
    }
    let Some(base) = chain[lowest].1.first().cloned() else { return Err(t("The first commit of the repository cannot be rewritten").into()) };
    if lowest + 1 > limit {
        return Err(tf("Rewriting more than {} commits is refused", &[&limit]));
    }
    // The new order of the part that changes (oldest first), then every commit above it as it was.
    let changed: Vec<String> = match what {
        Rewrite::Squash { run, .. } => run.clone(),
        Rewrite::Move { .. } => vec![chain[highest].0.clone(), chain[lowest].0.clone()],
    };
    let mut steps = steps_for(&changed);
    steps.extend(chain[..highest].iter().rev().map(|(h, _)| Step::Pick(h.clone())));
    Ok(Plan { branch, old_tip, base, steps, swapped })
}

fn checked_out_elsewhere(host: &dyn Host, root: &str, branch: &str) -> Result<bool, String> {
    let listing = git_ok(host, root, &["worktree", "list", "--porcelain"])?;
    let wanted = format!("branch refs/heads/{branch}");
    Ok(listing.lines().filter(|l| l.trim_end() == wanted).count() > 1)
}

/// Replay the steps in the temporary worktree `dir`; the new tip.
fn build(host: &dyn Host, dir: &str, plan: &Plan) -> Result<String, String> {
    let mut before = plan.base.clone();
    for step in &plan.steps {
        let hashes: Vec<&str> = match step {
            Step::Pick(h) => vec![h.as_str()],
            Step::Squash(run, _) => run.iter().map(String::as_str).collect(),
        };
        let mut args = vec!["cherry-pick", "--no-commit"];
        args.extend(&hashes);
        let applied = git(host, dir, &args)?;
        if !applied.ok() {
            return Err(match (step, &plan.swapped) {
                (Step::Pick(_), Some((moved, other))) => tf("Moving {0} would conflict with {1}: nothing was changed", &[&short(moved), &short(other)]),
                (Step::Pick(h), None) => tf("{} would conflict while the commits are rewritten: nothing was changed", &[&short(h)]),
                (Step::Squash(..), _) => tf("{} would conflict while the commits are rewritten: nothing was changed", &[&short(hashes[0])]),
            });
        }
        let committed = match step {
            // `-C`: the message, author and date of the commit being replayed.
            Step::Pick(h) => git(host, dir, &["commit", "--allow-empty", "--no-edit", "-C", h]),
            Step::Squash(_, message) => {
                let ident = git_ok(host, dir, &["show", "-s", "--format=%an <%ae>%x1f%aI", hashes[0]])?;
                let (author, date) = ident.trim().split_once('\x1f').unwrap_or((ident.trim(), ""));
                let mut args = vec!["commit", "--allow-empty", "-F", "-", "--author", author];
                if !date.is_empty() {
                    args.extend(["--date", date]);
                }
                git_stdin(host, dir, &args, Some(message.clone().into_bytes()))
            }
        }?;
        if !committed.ok() {
            let why = super::failure_text(&committed);
            return Err(tf("A commit could not be made while rewriting ({}). Nothing was changed.", &[&why.lines().next().unwrap_or("git commit failed")]));
        }
        before = git_ok(host, dir, &["rev-parse", "HEAD"])?.trim().to_string();
    }
    Ok(before)
}

/// Move the branch to `new_tip` if it is still at the old tip, and `HEAD` is still on it.
fn install(host: &dyn Host, root: &str, plan: &Plan, new_tip: &str) -> Result<(), String> {
    let head = git(host, root, &["symbolic-ref", "-q", "--short", "HEAD"])?;
    if !head.ok() || head.stdout_str().trim() != plan.branch {
        return Err(super::moved_message());
    }
    let reference = format!("refs/heads/{}", plan.branch);
    let moved = git(host, root, &["update-ref", "-m", "codeyard: rewrite unpushed commits", &reference, new_tip, &plan.old_tip])?;
    if moved.ok() { Ok(()) } else { Err(super::moved_message()) }
}

#[cfg(test)]
#[path = "rewrite_tests.rs"]
mod tests;
