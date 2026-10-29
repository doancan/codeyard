//! Discovery bounds, identity and the inventory's invalidation rules, against real repositories on disk (and a host
//! that drops its connection part-way).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use super::*;

fn base(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("codeyard-repos-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Paths as git prints them (macOS: /private/var/... for /var/...), so worktrees and git dirs compare as strings.
    std::fs::canonicalize(&dir).unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git().args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repository with one commit holding `same.txt` (every fixture repository has a file at that same relative path).
fn repo_at(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    std::fs::write(dir.join("same.txt"), format!("{}\n", dir.display())).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn run(roots: &[String], limits: Limits) -> Discovery {
    discover(&host::local(), roots, limits, &AtomicBool::new(false))
}

fn worktrees(d: &Discovery) -> Vec<String> {
    d.repos.iter().map(|f| f.worktree.clone()).collect()
}

#[test]
fn heavy_and_hidden_folders_are_skipped() {
    for name in ["node_modules", "target", ".git", ".venv", ".cache", "dist", "build", "__pycache__"] {
        assert!(skipped(name), "{name}");
    }
    for name in ["src", "crates", "libs", "vendor", "packages"] {
        assert!(!skipped(name), "{name}");
    }
}

/// AC1: each root, its nested repositories, and one entry per real repository however many roots lead to it.
#[test]
fn discovery_lists_roots_and_nested_repositories_once() {
    let w = base("discover");
    let alpha = w.join("alpha");
    repo_at(&alpha);
    repo_at(&alpha.join("libs/inner"));
    // Dependencies, build output and hidden folders are not searched.
    repo_at(&alpha.join("node_modules/dep"));
    repo_at(&alpha.join("target/out"));
    repo_at(&alpha.join(".cache/clone"));
    let group = w.join("group");
    std::fs::create_dir_all(&group).unwrap();
    repo_at(&group.join("one"));
    repo_at(&group.join("two"));
    // A root that is a folder inside a repository, and (unix) a link to a repository: the same repositories again.
    let mut roots = vec![s(&alpha), s(&group), s(&alpha.join("libs"))];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&alpha, w.join("alpha-link")).unwrap();
        roots.push(s(&w.join("alpha-link")));
    }
    let d = run(&roots, Limits::default());
    assert!(d.failed.is_empty(), "{:?}", d.failed);
    assert!(!d.truncated);
    assert_eq!(worktrees(&d), vec![s(&alpha), s(&alpha.join("libs/inner")), s(&group.join("one")), s(&group.join("two"))]);
    let roots_of: Vec<&str> = d.repos.iter().map(|f| f.root.as_str()).collect();
    assert_eq!(roots_of, vec![s(&alpha), s(&alpha), s(&group), s(&group)]);
    for f in &d.repos {
        assert_eq!(f.git_dir, format!("{}/.git", f.worktree));
        assert_ne!(f.fingerprint, 0, "{}", f.worktree);
    }
    let _ = std::fs::remove_dir_all(w);
}

#[test]
fn discovery_stops_at_the_depth_count_and_listing_limits() {
    let w = base("limits");
    repo_at(&w.join("a/b/c/deep"));
    for i in 0..5 {
        repo_at(&w.join(format!("flat{i}")));
    }
    let root = vec![s(&w)];
    // Depth: `a/b/c/deep` sits four levels down.
    let shallow = run(&root, Limits { depth: 3, ..Limits::default() });
    assert!(!worktrees(&shallow).iter().any(|p| p.ends_with("deep")), "{:?}", worktrees(&shallow));
    let deep = run(&root, Limits { depth: 4, ..Limits::default() });
    assert!(worktrees(&deep).iter().any(|p| p.ends_with("deep")));
    // Count.
    let few = run(&root, Limits { repos: 2, ..Limits::default() });
    assert_eq!(few.repos.len(), 2);
    assert!(few.truncated);
    // Listing budget.
    let listed = run(&root, Limits { dirs: 3, ..Limits::default() });
    assert!(listed.truncated);
    assert_eq!(listed.listed, 3);
    let _ = std::fs::remove_dir_all(w);
}

/// Links are never followed: a loop back to the root (or to a parent) ends nothing and repeats nothing.
#[cfg(unix)]
#[test]
fn symlink_loops_do_not_extend_discovery() {
    let w = base("loop");
    repo_at(&w.join("real"));
    std::os::unix::fs::symlink(&w, w.join("real/up")).unwrap();
    std::os::unix::fs::symlink(&w, w.join("self")).unwrap();
    std::fs::create_dir_all(w.join("a")).unwrap();
    std::os::unix::fs::symlink("..", w.join("a/back")).unwrap();
    let started = std::time::Instant::now();
    let d = run(&[s(&w)], Limits::default());
    assert_eq!(worktrees(&d), vec![s(&w.join("real"))]);
    assert!(d.listed <= 4, "listed {} folders", d.listed);
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(w);
}

/// A `.git` that is not a usable repository (an empty folder, a file pointing nowhere, a link) is not listed, and the
/// enclosing repository is not listed again in its place.
#[test]
fn a_broken_git_directory_is_not_a_repository() {
    let w = base("corrupt");
    repo_at(&w.join("good"));
    std::fs::create_dir_all(w.join("good/empty/.git")).unwrap();
    std::fs::create_dir_all(w.join("good/pointer")).unwrap();
    std::fs::write(w.join("good/pointer/.git"), "gitdir: /nonexistent/codeyard/.git\n").unwrap();
    std::fs::create_dir_all(w.join("plain/garbage/.git")).unwrap();
    std::fs::write(w.join("plain/garbage/.git/HEAD"), "not a ref\n").unwrap();
    #[cfg(unix)]
    {
        std::fs::create_dir_all(w.join("linked")).unwrap();
        std::os::unix::fs::symlink(w.join("good/.git"), w.join("linked/.git")).unwrap();
    }
    let d = run(&[s(&w)], Limits::default());
    assert_eq!(worktrees(&d), vec![s(&w.join("good"))]);
    assert!(d.failed.is_empty());
    let _ = std::fs::remove_dir_all(w);
}

/// A repository nested in a folder its parent ignores is still a repository of the workspace (Git itself shows nothing
/// of it in the parent); only the fixed heavy folders are skipped.
#[test]
fn a_nested_repository_in_an_ignored_folder_is_listed() {
    let w = base("ignored");
    repo_at(&w);
    std::fs::write(w.join(".gitignore"), "ignored/\n").unwrap();
    repo_at(&w.join("ignored/inner"));
    let d = run(&[s(&w)], Limits::default());
    assert_eq!(worktrees(&d), vec![s(&w), s(&w.join("ignored/inner"))]);
    let _ = std::fs::remove_dir_all(w);
}

/// Linked worktrees have their own git directory: each is its own entry, with `.git` as a file.
#[test]
fn linked_worktrees_are_separate_entries() {
    let w = base("worktrees");
    let main = w.join("main");
    repo_at(&main);
    git(&main, &["worktree", "add", "-q", "-b", "side", &s(&main.join("side"))]);
    let d = run(&[s(&main)], Limits::default());
    assert_eq!(worktrees(&d), vec![s(&main), s(&main.join("side"))]);
    assert_ne!(d.repos[0].git_dir, d.repos[1].git_dir);
    let _ = std::fs::remove_dir_all(w);
}

fn inventory(roots: &[String]) -> Inventory {
    let mut inv = Inventory::default();
    let g = inv.begin();
    assert!(inv.apply(g, run(roots, Limits::default())));
    inv
}

/// Identity: keys survive a rediscovery, a repository created again at the same path is a new identity (the selection
/// follows the path), and a removed one is no longer current.
#[test]
fn identity_survives_refresh_and_changes_on_replacement_or_removal() {
    let w = base("identity");
    let (a, b) = (w.join("a"), w.join("b"));
    repo_at(&a);
    repo_at(&b);
    let roots = vec![s(&w)];
    let mut inv = inventory(&roots);
    let key_a = inv.by_worktree(&s(&a)).unwrap().id.key;
    let old_b = inv.by_worktree(&s(&b)).unwrap().id.clone();
    assert!(inv.select(old_b.key));
    let g = inv.begin();
    inv.apply(g, run(&roots, Limits::default()));
    assert_eq!(inv.by_worktree(&s(&a)).unwrap().id.key, key_a, "unchanged repository keeps its key");
    assert!(inv.is_current(&old_b));
    // Replace b: same path, same git directory path, a new repository.
    std::fs::remove_dir_all(&b).unwrap();
    repo_at(&b);
    assert!(revalidate(&host::local(), &old_b).is_err(), "a replaced repository fails revalidation");
    let g = inv.begin();
    inv.apply(g, run(&roots, Limits::default()));
    let new_b = inv.by_worktree(&s(&b)).unwrap().id.clone();
    assert_ne!(new_b.key, old_b.key);
    assert!(!inv.is_current(&old_b));
    assert_eq!(inv.selected, Some(new_b.key), "the selection follows the path");
    assert!(revalidate(&host::local(), &new_b).is_ok());
    // Remove b.
    std::fs::remove_dir_all(&b).unwrap();
    let err = revalidate(&host::local(), &new_b).unwrap_err();
    assert_eq!(err, format!("Repository is no longer available: {}", s(&b)));
    let g = inv.begin();
    inv.apply(g, run(&roots, Limits::default()));
    assert!(inv.by_worktree(&s(&b)).is_none());
    assert_eq!(inv.selected, None);
    assert!(inv.get(key_a).is_some());
    let _ = std::fs::remove_dir_all(w);
}

/// The fingerprint is the git directory's own identity (inode and creation time), so everyday use never changes it:
/// config edits, new hooks, `git push -u` (which writes the config), commits, fetches. Only a new git directory does,
/// immediately (no waiting for a clock tick, so it holds on filesystems with one-second times too).
#[test]
fn the_fingerprint_survives_normal_use_and_changes_on_recreation() {
    let w = base("fingerprint");
    let (repo, bare) = (w.join("repo"), w.join("origin.git"));
    repo_at(&repo);
    git(&w, &["init", "-q", "--bare", &s(&bare)]);
    let host = host::local();
    let git_dir = s(&repo.join(".git"));
    let first = fingerprint(&host, &git_dir).unwrap();
    assert_ne!(first, 0, "the local host reports an identity for the git directory");
    git(&repo, &["config", "core.editor", "true"]);
    git(&repo, &["config", "codeyard.test", "1"]);
    std::fs::create_dir_all(repo.join(".git/hooks")).unwrap();
    std::fs::write(repo.join(".git/hooks/pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::write(repo.join(".git/info/exclude"), "*.tmp\n").unwrap();
    git(&repo, &["remote", "add", "origin", &s(&bare)]);
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "more"]);
    git(&repo, &["fetch", "-q", "origin"]);
    assert_eq!(fingerprint(&host, &git_dir).unwrap(), first, "normal use keeps the identity");
    // The identity of a discovered entry, checked again on the host, still passes.
    let found = run(&[s(&repo)], Limits::default());
    let id = RepoId { key: 1, host_gen: 0, root: s(&repo), worktree: s(&repo), git_dir: git_dir.clone(), fingerprint: found.repos[0].fingerprint };
    assert_eq!(id.fingerprint, first);
    assert!(revalidate(&host, &id).is_ok());
    // Deleted and created again right away: another git directory, another identity.
    std::fs::remove_dir_all(&repo).unwrap();
    repo_at(&repo);
    assert_ne!(fingerprint(&host, &git_dir).unwrap(), first);
    assert!(revalidate(&host, &id).is_err());
    let _ = std::fs::remove_dir_all(w);
}

#[test]
fn an_older_discovery_never_overwrites_a_newer_one() {
    let w = base("stale-discovery");
    repo_at(&w.join("a"));
    let mut inv = Inventory::default();
    let first = inv.begin();
    let second = inv.begin();
    assert!(!inv.apply(first, run(&[s(&w)], Limits::default())));
    assert!(inv.repos.is_empty() && inv.discovering);
    assert!(inv.apply(second, Discovery::default()));
    assert!(!inv.discovering);
    // A host change ends every identity.
    let g = inv.begin();
    inv.apply(g, run(&[s(&w)], Limits::default()));
    let id = inv.repos[0].id.clone();
    inv.reset();
    let g = inv.begin();
    inv.apply(g, run(&[s(&w)], Limits::default()));
    assert!(!inv.is_current(&id), "same path after a host change is a new identity");
    assert_ne!(inv.repos[0].id.host_gen, id.host_gen);
    let _ = std::fs::remove_dir_all(w);
}

/// AC6 (unit): a refresh changes only its own entry, keeps the `Arc` when nothing changed, and a stale result is dropped.
#[test]
fn state_updates_touch_only_their_repository() {
    let w = base("state");
    let (a, b) = (w.join("a"), w.join("b"));
    repo_at(&a);
    repo_at(&b);
    let mut inv = inventory(&[s(&w)]);
    let host = host::local();
    let ids: Vec<Arc<RepoId>> = inv.repos.iter().map(|r| r.id.clone()).collect();
    for id in &ids {
        assert!(inv.set_state(id, crate::git::status_for(&host, &id.worktree, true), OpState::Idle));
    }
    let before_a = inv.repos[0].status.clone().unwrap();
    // Unchanged: no change reported and the same Arc.
    assert!(!inv.set_state(&ids[0], crate::git::status_for(&host, &s(&a), true), OpState::Idle));
    assert!(Arc::ptr_eq(&before_a, inv.repos[0].status.as_ref().unwrap()));
    // b changes on disk: only b's entry changes.
    std::fs::write(b.join("same.txt"), "edited\n").unwrap();
    assert!(inv.set_state(&ids[1], crate::git::status_for(&host, &s(&b), true), OpState::Idle));
    assert!(Arc::ptr_eq(&before_a, inv.repos[0].status.as_ref().unwrap()));
    assert_eq!(inv.repos[1].changes(), 1);
    // A result for an identity that is gone is dropped.
    let stale = RepoId { key: 999, ..(*ids[1]).clone() };
    assert!(!inv.set_state(&stale, Ok(None), OpState::Merging));
    assert_eq!(inv.repos[1].op, OpState::Idle);
    // A status that resolved to another repository (the nested one was deleted) is an error, not the parent's status.
    let parent = crate::git::status_for(&host, &s(&a), true);
    assert!(inv.set_state(&ids[1], parent, OpState::Idle));
    assert!(inv.repos[1].status.is_none());
    assert_eq!(inv.repos[1].error.as_deref(), Some(format!("Repository is no longer available: {}", s(&b)).as_str()));
    let _ = std::fs::remove_dir_all(w);
}

#[test]
fn a_root_that_fails_keeps_what_was_known_about_it() {
    let mut inv = Inventory::default();
    let found = |root: &str, path: &str| Found { root: root.into(), worktree: path.into(), git_dir: format!("{path}/.git"), fingerprint: 1 };
    let g = inv.begin();
    inv.apply(g, Discovery { repos: vec![found("/w1", "/w1/a"), found("/w2", "/w2/b")], ..Default::default() });
    let keys: Vec<RepoKey> = inv.repos.iter().map(|r| r.id.key).collect();
    let g = inv.begin();
    inv.apply(g, Discovery { repos: vec![found("/w1", "/w1/a")], failed: vec![("/w2".into(), "not connected".into())], ..Default::default() });
    assert_eq!(inv.repos.iter().map(|r| r.id.key).collect::<Vec<_>>(), keys);
    // Searched again successfully without it: gone.
    let g = inv.begin();
    inv.apply(g, Discovery { repos: vec![found("/w1", "/w1/a")], ..Default::default() });
    assert_eq!(inv.repos.len(), 1);
}

#[test]
fn same_names_are_told_apart_by_their_path() {
    let mut inv = Inventory::default();
    let found = |root: &str, path: &str| Found { root: root.into(), worktree: path.into(), git_dir: format!("{path}/.git"), fingerprint: 1 };
    let g = inv.begin();
    inv.apply(g, Discovery { repos: vec![found("/w/one", "/w/one/app"), found("/w/two", "/w/two/app"), found("/w/two", "/w/two/lib")], ..Default::default() });
    let labels: Vec<&str> = inv.repos.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, vec!["one/app", "two/app", "lib"]);
    assert_eq!(inv.containing("/w/two/app/src/main.rs").map(|r| r.label.as_str()), Some("two/app"));
    assert_eq!(inv.containing("/w/two/app/.git/index").map(|r| r.label.as_str()), Some("two/app"));
    assert!(inv.containing("/w/two/application").is_none());
}

/// AC4 (unit): merge and rebase states are read from the git directory, independently per repository.
#[test]
fn operation_state_is_read_per_repository() {
    let w = base("opstate");
    let (merging, rebasing, clean) = (w.join("merging"), w.join("rebasing"), w.join("clean"));
    for dir in [&merging, &rebasing] {
        repo_at(dir);
        git(dir, &["checkout", "-q", "-b", "side"]);
        std::fs::write(dir.join("same.txt"), "side\n").unwrap();
        git(dir, &["commit", "-q", "-am", "side"]);
        git(dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("same.txt"), "main\n").unwrap();
        git(dir, &["commit", "-q", "-am", "main"]);
    }
    repo_at(&clean);
    let _ = crate::test_git::git().args(["merge", "-q", "side"]).current_dir(&merging).output();
    let _ = crate::test_git::git().args(["rebase", "-q", "side"]).current_dir(&rebasing).output();
    let host = host::local();
    let dir = |p: &Path| s(&p.join(".git"));
    assert_eq!(OpState::read(&host, &dir(&merging)), OpState::Merging);
    assert_eq!(OpState::read(&host, &dir(&rebasing)), OpState::Rebasing);
    assert_eq!(OpState::read(&host, &dir(&clean)), OpState::Idle);
    assert_eq!(OpState::Merging.label(), Some("Merge in progress"));
    assert_eq!(OpState::Idle.label(), None);
    let _ = std::fs::remove_dir_all(w);
}

/// AC7: hundreds of repositories; discovery stays within its bounds and returns quickly.
#[test]
fn hundreds_of_repositories_stay_bounded() {
    let w = base("many");
    // Real git directories are slow to create by the hundred; `git init` without a commit is enough for discovery.
    for i in 0..300 {
        let d = w.join(format!("group{}/repo{i:03}", i % 10));
        std::fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q"]);
    }
    let started = std::time::Instant::now();
    let d = run(&[s(&w)], Limits::default());
    let took = started.elapsed();
    assert_eq!(d.repos.len(), MAX_REPOS);
    assert!(d.truncated);
    assert!(d.listed <= MAX_DIRS);
    assert!(took < std::time::Duration::from_secs(30), "discovery took {took:?}");
    // A cancelled discovery stops right away.
    let cancelled = discover(&host::local(), &[s(&w)], Limits::default(), &AtomicBool::new(true));
    assert!(cancelled.repos.is_empty() && cancelled.listed == 0);
    let _ = std::fs::remove_dir_all(w);
}

/// A host that loses its connection after a number of requests (an SSH drop in the middle of a discovery).
struct Dropping {
    inner: host::RemoteHost,
    left: AtomicUsize,
}

impl host::Host for Dropping {
    fn info(&self) -> &proto::HostInfo {
        self.inner.info()
    }
    fn label(&self) -> String {
        "dropping".into()
    }
    fn is_remote(&self) -> bool {
        true
    }
    fn call(&self, req: proto::Request) -> host::Result<proto::Response> {
        if self.left.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_err() {
            return Err(host::HostError::Disconnected("connection lost".into()));
        }
        self.inner.call(req)
    }
    fn search(&self, q: proto::SearchQuery) -> host::Result<host::SearchStream> {
        self.inner.search(q)
    }
    fn hash_tree(&self, r: &str, i: Vec<String>, g: bool, l: u32, o: Option<Vec<String>>) -> host::Result<host::HashStream> {
        self.inner.hash_tree(r, i, g, l, o)
    }
    fn cancel(&self, id: u32) {
        self.inner.cancel(id)
    }
    fn pty_spawn(&self, c: u16, r: u16, cwd: Option<String>, sh: Option<String>) -> host::Result<host::Pty> {
        self.inner.pty_spawn(c, r, cwd, sh)
    }
    fn pty_input(&self, p: u32, d: Vec<u8>) {
        self.inner.pty_input(p, d)
    }
    fn pty_resize(&self, p: u32, c: u16, r: u16) {
        self.inner.pty_resize(p, c, r)
    }
    fn pty_kill(&self, p: u32) {
        self.inner.pty_kill(p)
    }
    fn watch(&self, p: Vec<String>) {
        self.inner.watch(p)
    }
    fn fs_events(&self) -> mpsc::Receiver<host::FsChanges> {
        self.inner.fs_events()
    }
    fn status_events(&self) -> mpsc::Receiver<host::ConnStatus> {
        self.inner.status_events()
    }
    fn proc_spawn(&self, p: &str, a: Vec<String>, cwd: Option<String>) -> host::Result<host::Proc> {
        self.inner.proc_spawn(p, a, cwd)
    }
    fn proc_input(&self, p: u32, d: Vec<u8>) {
        self.inner.proc_input(p, d)
    }
    fn proc_kill(&self, p: u32) {
        self.inner.proc_kill(p)
    }
}

/// The connection drops during a discovery: the roots that could not be searched are reported as failed (so the
/// inventory keeps what it knew about them), and nothing is invented.
#[test]
fn a_connection_drop_during_discovery_reports_failed_roots() {
    let w = base("drop");
    let (r1, r2) = (w.join("r1"), w.join("r2"));
    repo_at(&r1.join("x"));
    repo_at(&r2.join("y"));
    let roots = vec![s(&r1), s(&r2)];
    let full = run(&roots, Limits::default());
    assert_eq!(full.repos.len(), 2);
    for budget in 0..12 {
        let host = Dropping { inner: host::local(), left: AtomicUsize::new(budget) };
        let d = discover(&host, &roots, Limits::default(), &AtomicBool::new(false));
        // Whatever was found is real, and a root is never both fully searched and failed.
        for f in &d.repos {
            assert!(full.repos.contains(f), "budget {budget}: {f:?}");
        }
        let failed: Vec<&str> = d.failed.iter().map(|(r, _)| r.as_str()).collect();
        for root in &roots {
            let found_all = full.repos.iter().filter(|f| &f.root == root).all(|f| d.repos.contains(f));
            assert!(found_all || failed.contains(&root.as_str()), "budget {budget}: {root} neither complete nor failed");
        }
        let mut inv = Inventory::default();
        let g = inv.begin();
        inv.apply(g, full.clone());
        let keys: std::collections::BTreeSet<RepoKey> = inv.repos.iter().map(|r| r.id.key).collect();
        let g = inv.begin();
        inv.apply(g, d);
        assert_eq!(inv.repos.iter().map(|r| r.id.key).collect::<std::collections::BTreeSet<_>>(), keys, "budget {budget}: known repositories are kept");
    }
    let _ = std::fs::remove_dir_all(w);
}
