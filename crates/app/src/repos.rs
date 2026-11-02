//! The repositories of a workspace: bounded discovery of the repositories in the workspace roots and nested inside them,
//! their identity (host generation, worktree, git directory and a fingerprint of that directory) and the inventory
//! Source Control shows. Every file and Git access goes through the host, so the same code runs locally and over SSH.
//!
//! Identity is what keeps a pending operation from landing somewhere else: an operation carries the `RepoId` it was
//! started for, the target is checked again on the host right before Git runs (`revalidate`), and a result is applied
//! only while the inventory still holds that exact identity (`Inventory::is_current`).

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use host::Host;

use crate::git::Status;

/// How deep below each workspace root nested repositories are looked for.
pub const MAX_DEPTH: usize = 4;
/// At most this many repositories in one inventory.
pub const MAX_REPOS: usize = 64;
/// At most this many directories are listed by one discovery (all roots together).
pub const MAX_DIRS: usize = 4000;

/// Folders that hold dependencies or build output: never searched (hidden folders, `.git` among them, are skipped too).
pub const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "build", "out", "bower_components", "Pods", "DerivedData", "__pycache__", "venv"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub depth: usize,
    pub repos: usize,
    pub dirs: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { depth: MAX_DEPTH, repos: MAX_REPOS, dirs: MAX_DIRS }
    }
}

/// A repository found on the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The workspace root it was found in (trust is decided per workspace root).
    pub root: String,
    pub worktree: String,
    pub git_dir: String,
    pub fingerprint: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Discovery {
    pub repos: Vec<Found>,
    /// A limit stopped the search: more repositories may exist.
    pub truncated: bool,
    /// Roots that could not be searched (host error): what is already known about them is kept.
    pub failed: Vec<(String, String)>,
    /// Directories listed (for the bounds tests).
    pub listed: usize,
}

/// Is this folder name skipped by discovery?
pub fn skipped(name: &str) -> bool {
    name.starts_with('.') || SKIP_DIRS.contains(&name)
}

struct Probe {
    worktree: String,
    git_dir: String,
    /// `dir` is the top of its worktree (not a folder inside another repository).
    top: bool,
}

fn trim(path: &str) -> &str {
    let t = path.trim_end_matches('/');
    if t.is_empty() { "/" } else { t }
}

/// Which repository `dir` belongs to. `Ok(None)`: none. `Err`: the host could not run Git at all.
fn probe(host: &dyn Host, dir: &str) -> Result<Option<Probe>, String> {
    let out = host.exec("git", &["rev-parse", "--show-toplevel", "--show-prefix", "--absolute-git-dir"], Some(dir), None).map_err(|e| e.to_string())?;
    if out.code == Some(127) {
        return Err(crate::i18n::t("git is not installed on this host").into());
    }
    if !out.ok() {
        return Ok(None);
    }
    let text = out.stdout_str();
    let mut lines = text.lines();
    let toplevel = lines.next().unwrap_or_default().trim().to_string();
    let prefix = lines.next().unwrap_or_default().trim().trim_end_matches('/').to_string();
    let Some(git_dir) = lines.next().map(|l| trim(l.trim()).to_string()).filter(|l| !l.is_empty()) else { return Ok(None) };
    if toplevel.is_empty() {
        return Ok(None);
    }
    // Like `git::status_for`: the worktree is derived from the opened path, since --show-toplevel resolves symlinks.
    let dir = trim(dir);
    let worktree = if prefix.is_empty() { dir.to_string() } else { dir.strip_suffix(&format!("/{prefix}")).map(str::to_string).unwrap_or(toplevel) };
    Ok(Some(Probe { worktree, git_dir, top: prefix.is_empty() }))
}

/// A value that changes when the git directory is created again (a repository deleted and initialized or cloned at the
/// same path) and never through normal use (commits, config changes, new hooks, `push -u`): the git directory's own
/// file identity (inode) and creation time. Where the filesystem reports neither, the creation-time stand-in is the
/// modification time of a file Git writes only when it makes the repository (`commondir` of a linked worktree,
/// `description`); without those it is 0, never a path that changes in use. `Err`: the host could not be asked (a
/// dropped connection), which must not pass for a new identity.
pub fn fingerprint(host: &dyn Host, git_dir: &str) -> Result<i64, String> {
    let stat = |path: &str| match host.stat(path) {
        Ok(m) => Ok(Some(m)),
        Err(host::HostError::Op(_)) => Ok(None),
        Err(e) => Err(e.to_string()),
    };
    if let Some(m) = stat(git_dir)?
        && (m.file_id != 0 || m.created_ms != 0)
    {
        return Ok(mix(m.file_id, m.created_ms));
    }
    for name in ["commondir", "description"] {
        if let Some(m) = stat(&crate::ui::join(git_dir, name))? {
            return Ok(m.mtime_ms);
        }
    }
    Ok(0)
}

/// One value from the file identity and the creation time (both feed it; neither alone has to be unique).
fn mix(file_id: u64, created_ms: i64) -> i64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (file_id, created_ms).hash(&mut h);
    // Never 0: 0 means "no fingerprint".
    (h.finish() | 1) as i64
}

/// Git processes run at once while checking candidates (each one is a process start, slow on some machines).
const PARALLEL_PROBES: usize = 8;

/// Check candidate folders with Git at the same time (one chunk); results keep the candidates' order.
fn probe_some(host: &dyn Host, dirs: &[String]) -> Vec<Result<Option<(Probe, i64)>, String>> {
    let check = |dir: &String| -> Result<Option<(Probe, i64)>, String> {
        match probe(host, dir)? {
            Some(p) if p.top => {
                let fp = fingerprint(host, &p.git_dir)?;
                Ok(Some((p, fp)))
            }
            // A broken `.git`: Git found the enclosing repository instead (or none). Not a repository of its own.
            _ => Ok(None),
        }
    };
    std::thread::scope(|scope| {
        let check = &check;
        let handles: Vec<_> = dirs.iter().map(|d| scope.spawn(move || check(d))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(crate::i18n::t("Repository discovery failed").to_string()))).collect()
    })
}

/// Find the repositories of the workspace: each root (which can sit inside a repository) and, breadth first, the
/// repositories nested below it, without following symbolic links and without entering skipped folders. Each real
/// repository is listed once (by its git directory), however many roots or links lead to it. Folders are listed one
/// by one (cheap); only folders holding a `.git` are checked with Git, a few at a time.
pub fn discover(host: &dyn Host, roots: &[String], limits: Limits, cancel: &AtomicBool) -> Discovery {
    let mut out = Discovery::default();
    let mut seen: HashSet<String> = HashSet::new();
    for root in roots {
        let root = trim(root);
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if out.repos.len() >= limits.repos {
            out.truncated = true;
            break;
        }
        let mut push = |out: &mut Discovery, p: Probe, fingerprint: i64| {
            if seen.insert(p.git_dir.clone()) {
                out.repos.push(Found { root: root.to_string(), worktree: p.worktree, git_dir: p.git_dir, fingerprint });
            }
        };
        let own = probe(host, root).and_then(|p| match p {
            Some(p) => fingerprint(host, &p.git_dir).map(|fp| Some((p, fp))),
            None => Ok(None),
        });
        match own {
            Ok(Some((p, fp))) => push(&mut out, p, fp),
            Ok(None) => {}
            Err(e) => {
                out.failed.push((root.to_string(), e));
                continue;
            }
        }
        // Candidates: folders holding `.git` as a folder (a repository) or a file (a linked worktree or a submodule);
        // a link named `.git` is not followed. A few more than the room left, since some may be broken.
        let room = limits.repos.saturating_sub(out.repos.len());
        let cap = room.saturating_mul(2).max(1);
        let mut candidates: Vec<String> = Vec::new();
        let mut more = false;
        let mut failed = None;
        let mut queue = VecDeque::from([(root.to_string(), 0usize)]);
        while let Some((dir, depth)) = queue.pop_front() {
            if cancel.load(Ordering::Relaxed) {
                return out;
            }
            if out.listed >= limits.dirs || candidates.len() >= cap {
                more = true;
                break;
            }
            out.listed += 1;
            let entries = match host.read_dir(&dir) {
                Ok(e) => e,
                // The root itself, or the connection: this root's result would be incomplete, so it counts as failed.
                Err(e) if dir == root || !matches!(e, host::HostError::Op(_)) => {
                    failed = Some(e.to_string());
                    break;
                }
                // A folder that cannot be read (permissions): skipped.
                Err(_) => continue,
            };
            if dir != root && entries.iter().any(|e| e.name == ".git" && matches!(e.kind, proto::Kind::Dir | proto::Kind::File)) {
                candidates.push(dir.clone());
            }
            if depth >= limits.depth {
                continue;
            }
            let mut dirs: Vec<&str> = entries.iter().filter(|e| e.kind == proto::Kind::Dir && !skipped(&e.name)).map(|e| e.name.as_str()).collect();
            dirs.sort_unstable();
            queue.extend(dirs.into_iter().map(|name| (crate::ui::join(&dir, name), depth + 1)));
        }
        // Checked a few at a time, and only until the inventory is full.
        for chunk in candidates.chunks(PARALLEL_PROBES) {
            if cancel.load(Ordering::Relaxed) {
                return out;
            }
            if failed.is_some() {
                break;
            }
            if out.repos.len() >= limits.repos {
                more = true;
                break;
            }
            for result in probe_some(host, chunk) {
                match result {
                    Ok(Some((p, fp))) if out.repos.len() < limits.repos => push(&mut out, p, fp),
                    Ok(Some(_)) => more = true,
                    Ok(None) => {}
                    Err(e) => {
                        failed.get_or_insert(e);
                    }
                }
            }
        }
        if let Some(e) = failed {
            out.failed.push((root.to_string(), e));
        } else if more {
            out.truncated = true;
        }
    }
    out
}

/// What Git is in the middle of, read from the git directory (one listing, no Git process).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpState {
    #[default]
    Idle,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
    Bisecting,
}

impl OpState {
    pub fn read(host: &dyn Host, git_dir: &str) -> OpState {
        let Ok(entries) = host.read_dir(git_dir) else { return OpState::Idle };
        let has = |name: &str| entries.iter().any(|e| e.name == name);
        if has("rebase-merge") || has("rebase-apply") {
            OpState::Rebasing
        } else if has("MERGE_HEAD") {
            OpState::Merging
        } else if has("CHERRY_PICK_HEAD") {
            OpState::CherryPicking
        } else if has("REVERT_HEAD") {
            OpState::Reverting
        } else if has("BISECT_LOG") {
            OpState::Bisecting
        } else {
            OpState::Idle
        }
    }

    pub fn label(self) -> Option<&'static str> {
        match self {
            OpState::Idle => None,
            OpState::Merging => Some(crate::i18n::t("Merge in progress")),
            OpState::Rebasing => Some(crate::i18n::t("Rebase in progress")),
            OpState::CherryPicking => Some(crate::i18n::t("Cherry-pick in progress")),
            OpState::Reverting => Some(crate::i18n::t("Revert in progress")),
            OpState::Bisecting => Some(crate::i18n::t("Bisect in progress")),
        }
    }
}

pub type RepoKey = u64;

/// Who a repository is. A new key is given whenever any part changes, so an operation started for one identity can be
/// recognized as stale after a reconnect, a removal or a replacement at the same path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoId {
    pub key: RepoKey,
    pub host_gen: u64,
    pub root: String,
    pub worktree: String,
    pub git_dir: String,
    pub fingerprint: i64,
}

impl RepoId {
    pub fn name(&self) -> &str {
        crate::ui::file_name(&self.worktree)
    }
}

/// The repository is still there and is still the same one (not deleted, not created again at the same path).
pub fn revalidate(host: &dyn Host, id: &RepoId) -> Result<(), String> {
    match probe(host, &id.worktree) {
        Ok(Some(p)) if p.top && p.git_dir == id.git_dir && fingerprint(host, &p.git_dir) == Ok(id.fingerprint) => Ok(()),
        _ => Err(crate::i18n::tf("Repository is no longer available: {}", &[&id.worktree])),
    }
}

#[derive(Clone, Debug)]
pub struct Repo {
    pub id: Arc<RepoId>,
    /// Shown name: the folder name, or its path from the workspace root when two repositories share a name.
    pub label: String,
    pub status: Option<Arc<Status>>,
    pub error: Option<String>,
    pub op: OpState,
}

impl Repo {
    pub fn changes(&self) -> usize {
        self.status.as_ref().map_or(0, |s| s.changes.len())
    }
}

#[derive(Default)]
pub struct Inventory {
    host_gen: u64,
    next_key: RepoKey,
    pub repos: Vec<Repo>,
    pub selected: Option<RepoKey>,
    /// Latest discovery started; an older one's result is dropped.
    pub generation: u64,
    pub discovering: bool,
    pub truncated: bool,
}

impl Inventory {
    /// The host changed (connect, reconnect, another folder): every identity ends here.
    pub fn reset(&mut self) {
        self.host_gen += 1;
        self.generation += 1;
        self.repos.clear();
        self.selected = None;
        self.discovering = false;
        self.truncated = false;
    }

    pub fn host_gen(&self) -> u64 {
        self.host_gen
    }

    pub fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.discovering = true;
        self.generation
    }

    /// Apply a discovery. Unchanged repositories keep their key and cached state; a repository whose git directory was
    /// created again gets a new key (the selection follows its path). Repositories of roots that could not be searched
    /// are kept as they were. `false`: the discovery is stale (a newer one was started) and nothing changed.
    pub fn apply(&mut self, generation: u64, found: Discovery) -> bool {
        if generation != self.generation {
            return false;
        }
        self.discovering = false;
        self.truncated = found.truncated;
        let failed: HashSet<&str> = found.failed.iter().map(|(r, _)| r.as_str()).collect();
        let selected_path = self.selected.and_then(|k| self.get(k)).map(|r| r.id.worktree.clone());
        let mut old = std::mem::take(&mut self.repos);
        let mut repos = Vec::with_capacity(found.repos.len());
        for f in found.repos {
            let reuse = old.iter().position(|r| r.id.worktree == f.worktree && r.id.git_dir == f.git_dir && r.id.fingerprint == f.fingerprint);
            match reuse {
                Some(i) => repos.push(old.swap_remove(i)),
                None => {
                    self.next_key += 1;
                    let id = RepoId {
                        key: self.next_key,
                        host_gen: self.host_gen,
                        root: f.root,
                        worktree: f.worktree,
                        git_dir: f.git_dir,
                        fingerprint: f.fingerprint,
                    };
                    repos.push(Repo { id: Arc::new(id), label: String::new(), status: None, error: None, op: OpState::Idle });
                }
            }
        }
        // A root that failed to list keeps what was known about it (one failing host or folder does not wipe the others).
        let known: HashSet<String> = repos.iter().map(|r| r.id.worktree.clone()).collect();
        repos.extend(old.into_iter().filter(|r| failed.contains(r.id.root.as_str()) && !known.contains(&r.id.worktree)));
        self.repos = repos;
        self.label();
        self.selected = match self.selected {
            Some(k) if self.get(k).is_some() => Some(k),
            _ => selected_path.and_then(|p| self.by_worktree(&p)).map(|r| r.id.key),
        };
        true
    }

    fn label(&mut self) {
        let names: Vec<String> = self.repos.iter().map(|r| r.id.name().to_string()).collect();
        for (i, r) in self.repos.iter_mut().enumerate() {
            let name = &names[i];
            r.label = if names.iter().filter(|n| *n == name).count() > 1 { display_path(&r.id.root, &r.id.worktree) } else { name.clone() };
        }
    }

    pub fn get(&self, key: RepoKey) -> Option<&Repo> {
        self.repos.iter().find(|r| r.id.key == key)
    }

    pub fn by_worktree(&self, path: &str) -> Option<&Repo> {
        let path = trim(path);
        self.repos.iter().find(|r| r.id.worktree == path)
    }

    /// The innermost repository holding `path` (a file, a folder or a git directory path).
    pub fn containing(&self, path: &str) -> Option<&Repo> {
        let inside = |base: &str| path == base || path.starts_with(&format!("{}/", base.trim_end_matches('/')));
        self.repos.iter().filter(|r| inside(&r.id.worktree) || inside(&r.id.git_dir)).max_by_key(|r| r.id.worktree.len())
    }

    /// The identity is still the one in the inventory: a result for it may be applied.
    pub fn is_current(&self, id: &RepoId) -> bool {
        id.host_gen == self.host_gen && self.get(id.key).is_some_and(|r| *r.id == *id)
    }

    /// Store a refreshed state. Only this repository's entry changes, and only when something differs (an unchanged
    /// status keeps its `Arc`, so nothing that depends on it reloads). `false`: stale or unchanged.
    pub fn set_state(&mut self, id: &RepoId, status: Result<Option<Status>, String>, op: OpState) -> bool {
        if !self.is_current(id) {
            return false;
        }
        let Some(repo) = self.repos.iter_mut().find(|r| r.id.key == id.key) else { return false };
        let (status, error) = match status {
            Ok(Some(st)) if trim(&st.root) == id.worktree => (Some(st), None),
            Ok(_) => (None, Some(crate::i18n::tf("Repository is no longer available: {}", &[&id.worktree]))),
            Err(e) => (None, Some(e)),
        };
        let mut changed = false;
        if repo.status.as_deref() != status.as_ref() {
            repo.status = status.map(Arc::new);
            changed = true;
        }
        if repo.error != error {
            repo.error = error;
            changed = true;
        }
        if repo.op != op {
            repo.op = op;
            changed = true;
        }
        changed
    }

    /// The selected repository's status arrived through the main refresh: share the same `Arc`.
    pub fn set_selected_status(&mut self, status: &Arc<Status>) -> bool {
        let Some(repo) = self.repos.iter_mut().find(|r| r.id.worktree == trim(&status.root)) else { return false };
        let key = repo.id.key;
        let changed = repo.status.as_ref().is_none_or(|s| !Arc::ptr_eq(s, status)) || repo.error.is_some();
        repo.status = Some(status.clone());
        repo.error = None;
        self.selected = Some(key);
        changed
    }

    pub fn select(&mut self, key: RepoKey) -> bool {
        if self.get(key).is_none() {
            return false;
        }
        self.selected = Some(key);
        true
    }
}

/// `root-name/inner/path` for a repository under a workspace root, the full path otherwise.
pub fn display_path(root: &str, worktree: &str) -> String {
    let root = trim(root);
    match worktree.strip_prefix(&format!("{}/", root.trim_end_matches('/'))) {
        Some(rel) => format!("{}/{rel}", crate::ui::file_name(root)),
        None if worktree == root => crate::ui::file_name(root).to_string(),
        None => worktree.to_string(),
    }
}

#[cfg(test)]
#[path = "repos_tests.rs"]
mod tests;
