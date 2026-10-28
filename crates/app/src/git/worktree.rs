//! Worktree inventory. NUL-delimited porcelain keeps paths with quotes and line breaks intact.

use host::Host;

use super::git_ok;

/// A checkout and its branch, commit and availability as reported by Git.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub bare: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
}

/// Read all checkouts on the host that owns the repository; returns Git's error on failure.
pub fn worktree_entries(host: &dyn Host, root: &str) -> Result<Vec<Worktree>, String> {
    git_ok(host, root, &["worktree", "list", "--porcelain", "-z"]).map(|out| parse_worktree_entries(&out))
}

/// Parse `git worktree list --porcelain -z`, ignoring fields added by newer Git versions.
pub fn parse_worktree_entries(out: &str) -> Vec<Worktree> {
    let mut entries: Vec<Worktree> = Vec::new();
    for field in out.split('\0') {
        if let Some(path) = field.strip_prefix("worktree ") {
            entries.push(Worktree { path: path.into(), ..Default::default() });
        } else if let Some(entry) = entries.last_mut() {
            if let Some(branch) = field.strip_prefix("branch ") {
                entry.branch = Some(branch.strip_prefix("refs/heads/").unwrap_or(branch).into());
            } else if let Some(head) = field.strip_prefix("HEAD ") {
                entry.head = Some(head.into());
            } else if field == "bare" {
                entry.bare = true;
            } else if field == "locked" || field.starts_with("locked ") {
                entry.locked = Some(field.strip_prefix("locked ").unwrap_or_default().into());
            } else if field == "prunable" || field.starts_with("prunable ") {
                entry.prunable = Some(field.strip_prefix("prunable ").unwrap_or_default().into());
            }
        }
    }
    entries
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
