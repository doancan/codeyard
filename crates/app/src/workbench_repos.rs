//! Workbench: the repositories of the workspace. Discovery runs on a background thread (bounded, a newer search cancels
//! an older one) and only when the roots change, an entry turns out to be gone, or a refresh comes after a while. Each
//! repository's state is refreshed on its own, a few at a time, and selecting a repository makes it the one Source
//! Control, the status bar, Sync and the history act on. Results are applied only while the repository is still the
//! same one (see `crate::repos`).

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::repos::{Discovery, Inventory, Limits, OpState, RepoKey};

/// Repository refreshes that run at the same time (each is about three Git processes).
pub(crate) const MAX_REFRESHES: usize = 8;

/// A refresh click searches the workspace again only when the last search is older than this.
pub(crate) const REDISCOVER_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct RepoState {
    pub(super) inventory: Inventory,
    cancel: Option<Arc<AtomicBool>>,
    task: Option<gpui::Task<()>>,
    /// Repositories with a refresh running, those asked for again meanwhile, and those waiting for a free slot.
    busy: HashSet<RepoKey>,
    again: HashSet<RepoKey>,
    waiting: VecDeque<RepoKey>,
    /// Test override of the discovery bounds.
    pub(super) limits: Option<Limits>,
    /// A search was asked for while Source Control was hidden.
    pending: bool,
    /// When the last search finished.
    last: Option<std::time::Instant>,
    /// Searches started, refreshes started, and the most refreshes that ran at the same time.
    discoveries: usize,
    refreshes: usize,
    peak: usize,
}

impl Workbench {
    /// Source Control is on screen: run the search that was put off.
    pub(super) fn discover_if_pending(&mut self, cx: &mut Context<Self>) {
        if self.vcs.repos.pending {
            self.discover_repos(cx);
        }
    }

    /// The host or the folder changed: every repository identity ends, and nothing pending can apply to the new ones.
    pub(super) fn reset_repos(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.vcs.repos.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.vcs.repos.task = None;
        self.vcs.repos.busy.clear();
        self.vcs.repos.again.clear();
        self.vcs.repos.waiting.clear();
        self.vcs.repos.last = None;
        self.vcs.repos.inventory.reset();
        self.push_repos(cx);
    }

    /// Search the workspace roots for repositories in the background (the roots changed: a running search is replaced).
    /// Until Source Control is shown the search only becomes pending: nothing is listed on the host that no one looks at.
    pub fn discover_repos(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.vcs.repos.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        if !self.view_shown(Panel::Scm) {
            // A search already running for the old roots is dropped; the next one starts when the panel is shown.
            self.vcs.repos.inventory.begin();
            self.vcs.repos.inventory.discovering = false;
            self.vcs.repos.task = None;
            self.vcs.repos.pending = true;
            return;
        }
        self.vcs.repos.pending = false;
        self.vcs.repos.discoveries += 1;
        let generation = self.vcs.repos.inventory.begin();
        let roots = self.all_roots(cx);
        if roots.is_empty() {
            self.vcs.repos.inventory.apply(generation, Discovery::default());
            self.vcs.repos.last = Some(std::time::Instant::now());
            self.push_repos(cx);
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.vcs.repos.cancel = Some(cancel.clone());
        let host = self.remote.host.clone();
        let limits = self.vcs.repos.limits.unwrap_or_default();
        let task = cx.background_executor().spawn(async move { crate::repos::discover(host.as_ref(), &roots, limits, &cancel) });
        self.vcs.repos.task = Some(cx.spawn(async move |this, cx| {
            let found = task.await;
            this.update(cx, |this, cx| {
                if this.vcs.repos.inventory.apply(generation, found) {
                    this.vcs.repos.cancel = None;
                    this.vcs.repos.last = Some(std::time::Instant::now());
                    this.inventory_changed(cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// An entry turned out to be gone or replaced: search again, unless a search is already running.
    pub(super) fn rediscover(&mut self, cx: &mut Context<Self>) {
        if !self.vcs.repos.inventory.discovering {
            self.discover_repos(cx);
        }
    }

    /// The Refresh button: the inventory is searched again only when the last search is old (a plain refresh of the
    /// repositories' state never lists the workspace).
    pub(super) fn refresh_inventory(&mut self, cx: &mut Context<Self>) {
        if self.vcs.repos.last.is_none_or(|t| t.elapsed() >= REDISCOVER_AFTER) {
            self.rediscover(cx);
        }
    }

    /// After a discovery: the selection follows what Source Control shows, a picked repository that is gone is let go
    /// of, and repositories not read yet are refreshed.
    fn inventory_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(st) = self.vcs.status.clone() {
            self.vcs.repos.inventory.set_selected_status(&st);
        }
        if let Some(dir) = self.vcs.scm_root.clone()
            && !self.all_roots(cx).contains(&dir)
            && self.vcs.repos.inventory.by_worktree(&dir).is_none()
        {
            self.vcs.scm_root = None;
            self.notice = Some(Notice::Error(crate::i18n::tf("Repository is no longer available: {}", &[&dir]).into()));
            self.refresh_git(0, cx);
        }
        self.push_repos(cx);
        // The folder Source Control looks at is no repository (a folder of repositories): show the first one found.
        let dir = self.vcs.scm_root.clone().or_else(|| self.root(cx));
        if self.vcs.status.is_none()
            && let Some(dir) = dir
            && self.vcs.repos.inventory.containing(&dir).is_none()
            && let Some(first) = self.vcs.repos.inventory.repos.first().map(|r| r.id.key)
        {
            self.select_repository(first, cx);
        }
        if self.vcs.repos.inventory.repos.len() > 1 {
            let unread: Vec<RepoKey> = self.vcs.repos.inventory.repos.iter().filter(|r| r.status.is_none() && r.error.is_none()).map(|r| r.id.key).collect();
            for key in unread {
                self.refresh_repo(key, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn push_repos(&mut self, cx: &mut Context<Self>) {
        let repos = Arc::new(self.vcs.repos.inventory.repos.clone());
        self.scm.update(cx, |s, cx| s.set_repos(repos, cx));
    }

    /// Refresh one repository's state (status, branch, upstream, operation in progress). Only its entry changes. At
    /// most `MAX_REFRESHES` run at once; the others wait their turn (each only once).
    pub fn refresh_repo(&mut self, key: RepoKey, cx: &mut Context<Self>) {
        let Some(id) = self.vcs.repos.inventory.get(key).map(|r| r.id.clone()) else { return };
        let state = &mut self.vcs.repos;
        if state.busy.contains(&key) {
            state.again.insert(key);
            return;
        }
        if state.busy.len() >= MAX_REFRESHES {
            if !state.waiting.contains(&key) {
                state.waiting.push_back(key);
            }
            return;
        }
        state.waiting.retain(|k| *k != key);
        state.busy.insert(key);
        state.refreshes += 1;
        state.peak = state.peak.max(state.busy.len());
        let trusted = self.is_trusted_root(&id.root, cx);
        let host = self.remote.host.clone();
        let target = id.clone();
        let task = cx.background_executor().spawn(async move {
            let status = crate::git::status_for(host.as_ref(), &target.worktree, trusted);
            let op = OpState::read(host.as_ref(), &target.git_dir);
            (status, op)
        });
        cx.spawn(async move |this, cx| {
            let (status, op) = task.await;
            this.update(cx, |this, cx| {
                // A reset (another folder or host) cleared the guard: this result belongs to an identity that is gone.
                if !this.vcs.repos.busy.remove(&key) {
                    return;
                }
                let gone = matches!(&status, Ok(None)) || matches!(&status, Ok(Some(st)) if st.root.trim_end_matches('/') != id.worktree);
                if this.vcs.repos.inventory.set_state(&id, status, op) {
                    this.push_repos(cx);
                }
                if gone && this.vcs.repos.inventory.is_current(&id) {
                    // Deleted (or no longer a repository of its own): search again so the entry goes away.
                    this.rediscover(cx);
                }
                if this.vcs.repos.again.remove(&key) {
                    this.refresh_repo(key, cx);
                }
                while this.vcs.repos.busy.len() < MAX_REFRESHES {
                    let Some(next) = this.vcs.repos.waiting.pop_front() else { break };
                    this.refresh_repo(next, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Refresh the repositories of the inventory besides the selected one (the main refresh reads that one). Polling
    /// calls this while Source Control shows several; a repository still waiting from the last poll is not queued twice.
    pub fn refresh_repos(&mut self, cx: &mut Context<Self>) {
        if self.vcs.repos.inventory.repos.len() < 2 {
            return;
        }
        let selected = self.vcs.status.as_ref().and_then(|s| self.vcs.repos.inventory.by_worktree(&s.root)).map(|r| r.id.key);
        let keys: Vec<RepoKey> = self.vcs.repos.inventory.repos.iter().map(|r| r.id.key).filter(|k| Some(*k) != selected).collect();
        for key in keys {
            if !self.vcs.repos.waiting.contains(&key) {
                self.refresh_repo(key, cx);
            }
        }
    }

    /// Files changed on disk: refresh the repositories they belong to (worktree or git directory), and only those.
    pub(super) fn refresh_repos_for(&mut self, paths: &[String], cx: &mut Context<Self>) {
        if self.vcs.repos.inventory.repos.len() < 2 {
            return;
        }
        let keys: HashSet<RepoKey> = paths.iter().filter_map(|p| self.vcs.repos.inventory.containing(p)).map(|r| r.id.key).collect();
        for key in keys {
            self.refresh_repo(key, cx);
        }
    }

    /// Make a repository of the inventory the one Source Control acts on. Its known state shows at once (the status
    /// bar's branch included), then it is refreshed. `false`: it is no longer in the inventory (nothing changes, and
    /// nothing falls back to another one).
    pub fn select_repository(&mut self, key: RepoKey, cx: &mut Context<Self>) -> bool {
        let Some(repo) = self.vcs.repos.inventory.get(key).cloned() else {
            self.notice = Some(Notice::Error(crate::i18n::t("Repository changed while the operation was pending.").into()));
            cx.notify();
            return false;
        };
        if self.vcs.status.as_ref().is_some_and(|s| s.root.trim_end_matches('/') == repo.id.worktree) {
            return true;
        }
        self.vcs.repos.inventory.select(key);
        self.vcs.scm_root = Some(repo.id.worktree.clone());
        self.vcs.error = None;
        self.apply_git(repo.status.clone(), cx);
        // The branch comes from this repository's own status (a detached HEAD has none), not from an earlier one.
        self.vcs.branch = repo.status.as_ref().and_then(|s| s.branch.clone()).map(Into::into);
        self.refresh_git(0, cx);
        self.push_repos(cx);
        cx.notify();
        true
    }

    /// The git directories to watch for the inventory's repositories (index and HEAD of each).
    pub(super) fn repo_watch_paths(&self) -> Vec<String> {
        if self.vcs.repos.inventory.repos.len() < 2 {
            return Vec::new();
        }
        self.vcs.repos.inventory.repos.iter().flat_map(|r| [format!("{}/index", r.id.git_dir), format!("{}/HEAD", r.id.git_dir)]).collect()
    }

    pub fn repo_inventory(&self) -> &Inventory {
        &self.vcs.repos.inventory
    }

    #[cfg(test)]
    pub fn refresh_repos_for_test(&mut self, paths: &[String], cx: &mut Context<Self>) {
        self.refresh_repos_for(paths, cx);
    }

    #[cfg(test)]
    pub fn refresh_inventory_for_test(&mut self, cx: &mut Context<Self>) {
        self.refresh_inventory(cx);
    }

    #[cfg(test)]
    pub fn set_repo_limits_for_test(&mut self, limits: Limits) {
        self.vcs.repos.limits = Some(limits);
    }

    #[cfg(test)]
    pub fn repo_key_for_test(&self, worktree: &str) -> Option<RepoKey> {
        self.vcs.repos.inventory.by_worktree(worktree).map(|r| r.id.key)
    }

    /// (searches started, refreshes started, most refreshes at once).
    #[cfg(test)]
    pub fn repo_counters_for_test(&self) -> (usize, usize, usize) {
        (self.vcs.repos.discoveries, self.vcs.repos.refreshes, self.vcs.repos.peak)
    }

    /// (refreshes running, refreshes waiting).
    #[cfg(test)]
    pub fn repo_queue_for_test(&self) -> (Vec<RepoKey>, Vec<RepoKey>) {
        (self.vcs.repos.busy.iter().copied().collect(), self.vcs.repos.waiting.iter().copied().collect())
    }

    /// Pretend the last search finished long ago (`old`) or just now (a slow machine can take a minute between steps).
    #[cfg(test)]
    pub fn age_discovery_for_test(&mut self, old: bool) {
        let now = std::time::Instant::now();
        self.vcs.repos.last = if old { now.checked_sub(REDISCOVER_AFTER * 2) } else { Some(now) };
    }

    /// Pick a row of the open context menu by its label, as a click would.
    #[cfg(test)]
    pub fn select_menu_item_for_test(&mut self, label: &str, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_ref().map(|m| m.menu.clone()) else { panic!("no menu open") };
        let index = menu.read(cx).labels().iter().position(|l| l == label).unwrap_or_else(|| panic!("no menu item {label}"));
        menu.update(cx, |_, cx| cx.emit(crate::menu::MenuEvent::Select(index)));
    }

    #[cfg(test)]
    pub fn status_bar_branch_for_test(&self) -> Option<String> {
        self.vcs.branch.as_ref().map(|b| b.to_string())
    }
}
