//! The repositories of a workspace: the ones the open folders belong to and the ones nested inside them. Nested ones
//! are found by looking for `.git` entries, without running git: a search never follows a symlink, skips dependency and
//! build folders, and stops at fixed depth and size limits. Every git operation names its repository by an identity
//! (work tree and git directory) that is checked again right before the operation runs.

use host::Host;

/// Most repositories one workspace lists.
pub const MAX_REPOS: usize = 64;
/// Folder levels searched below a workspace folder.
const MAX_DEPTH: usize = 4;
/// Folders read per workspace folder.
const MAX_DIRS: usize = 4_000;
/// Never searched: dependencies, build output and caches hold many folders and no repositories of the user's.
const SKIP: &[&str] = &["node_modules", "target", ".venv", "venv", "__pycache__", ".gradle", ".cache", ".tox", "Pods", ".git"];

/// What a search found below one folder.
#[derive(Debug, Default, PartialEq)]
pub struct Nested {
    /// Work trees inside the folder (the folder itself excluded), in the order found (shallowest first).
    pub roots: Vec<String>,
    /// A limit was reached: some repositories may be missing.
    pub truncated: bool,
}

/// Repositories strictly inside `root`, breadth first. A folder with a `.git` entry (directory, or file for worktrees and
/// submodules) is a work tree; the search goes on below it, since repositories can nest.
pub fn nested(host: &dyn Host, root: &str, limit: usize) -> Nested {
    let mut found = Nested::default();
    let mut queue = std::collections::VecDeque::from([(root.trim_end_matches('/').to_string(), 0usize)]);
    let mut read = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        if read >= MAX_DIRS {
            found.truncated = true;
            break;
        }
        read += 1;
        let Ok(entries) = host.read_dir(&dir) else { continue };
        if depth > 0 && entries.iter().any(|e| e.name == ".git" && e.kind != proto::Kind::Symlink) {
            if found.roots.len() >= limit {
                found.truncated = true;
                break;
            }
            found.roots.push(dir.clone());
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        for e in entries {
            // Only real folders: a symlink could lead out of the workspace or into a cycle.
            if e.kind == proto::Kind::Dir && !SKIP.contains(&e.name.as_str()) {
                queue.push_back((format!("{dir}/{}", e.name), depth + 1));
            }
        }
    }
    found
}

/// A repository as an operation targets it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RepoId {
    /// The work tree (`git rev-parse --show-toplevel`).
    pub root: String,
    /// Its git directory (`--absolute-git-dir`): two work trees of one repository differ here.
    pub git_dir: String,
}

impl RepoId {
    pub fn of(status: &super::Status) -> Option<Self> {
        Some(Self { root: status.root.clone(), git_dir: status.git_dir.clone()? })
    }
}

/// Before a mutation: the folder is still that repository. A folder that was removed or replaced (another repository,
/// a plain folder) refuses the operation instead of running it somewhere else.
pub fn verify(host: &dyn Host, id: &RepoId) -> Result<(), String> {
    // The root is kept as the user opened it (symlinks unresolved, like `status_for`): an empty prefix says it is still the
    // top of a work tree, and the git directory says which repository that is.
    let out = super::git(host, &id.root, &["rev-parse", "--show-prefix", "--absolute-git-dir"])
        .map_err(|_| crate::i18n::tf("Repository is no longer available: {}", &[&id.root]))?;
    if !out.ok() {
        return Err(crate::i18n::tf("Repository is no longer available: {}", &[&id.root]));
    }
    let text = out.stdout_str();
    let mut lines = text.lines();
    let top = lines.next().is_some_and(|prefix| prefix.trim().is_empty());
    if top && lines.next().is_some_and(|dir| dir.trim().trim_end_matches('/') == id.git_dir.trim_end_matches('/')) {
        Ok(())
    } else {
        Err(crate::i18n::t("Repository changed while the operation was pending.").into())
    }
}

/// The repository of `path`: the deepest listed work tree that contains it.
pub fn containing<'a>(repos: impl IntoIterator<Item = &'a str>, path: &str) -> Option<&'a str> {
    repos.into_iter().filter(|root| path == *root || path.starts_with(&format!("{}/", root.trim_end_matches('/')))).max_by_key(|root| root.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_repositories_are_found_without_following_symlinks_or_dependencies() {
        if crate::test_git::git().arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("codeyard-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["app", "app/lib/inner", "node_modules/pkg", "deep/a/b/c/d/e"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
            std::fs::create_dir_all(dir.join(sub).join(".git")).unwrap();
        }
        // A worktree or submodule: `.git` is a file.
        std::fs::create_dir_all(dir.join("wt")).unwrap();
        std::fs::write(dir.join("wt/.git"), "gitdir: ../app/.git/worktrees/wt\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("app"), dir.join("link")).unwrap();
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        let found = nested(&host, &root, MAX_REPOS);
        let rel: Vec<String> = found.roots.iter().map(|r| r.strip_prefix(&format!("{root}/")).unwrap().to_string()).collect();
        assert_eq!(rel, vec!["app", "wt", "app/lib/inner"], "breadth first; no symlink, dependency folder or folder past the depth limit");
        assert!(!found.truncated);
        let limited = nested(&host, &root, 1);
        assert_eq!((limited.roots.len(), limited.truncated), (1, true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_deepest_repository_contains_a_path() {
        let repos = ["/w", "/w/lib", "/w/libx"];
        assert_eq!(containing(repos, "/w/lib/a.rs"), Some("/w/lib"));
        assert_eq!(containing(repos, "/w/libx/a.rs"), Some("/w/libx"));
        assert_eq!(containing(repos, "/w/src/a.rs"), Some("/w"));
        assert_eq!(containing(repos, "/other/a.rs"), None);
    }

    #[test]
    fn an_operation_is_refused_when_its_repository_was_replaced_or_removed() {
        let Some((dir, run)) = super::super::repo("repo-verify") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        let status = super::super::status_for(&host, &root, true).unwrap().unwrap();
        let id = RepoId::of(&status).unwrap();
        assert_eq!(verify(&host, &id), Ok(()));
        // Replaced by another repository at the same path.
        std::fs::remove_dir_all(dir.join(".git")).unwrap();
        run(&["init", "-q"]).unwrap();
        let other = RepoId { git_dir: format!("{}/elsewhere", id.git_dir), ..id.clone() };
        assert!(verify(&host, &other).unwrap_err().contains("changed while the operation was pending"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(verify(&host, &id).unwrap_err().contains("no longer available"));
    }
}
