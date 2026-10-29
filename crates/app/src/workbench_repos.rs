//! Workbench: the workspace's repositories. They are found below every open folder (`git::repos::nested`), each one's
//! status is read on its own, and Source Control shows the selected one: the one the user picked, or else the
//! repository of the active file. Operations name their repository and are refused if the folder changed meanwhile.

use super::*;
use crate::git::repos;

/// One repository of the inventory.
pub(super) struct RepoEntry {
    pub(super) root: String,
    /// The workspace folder it was found in (trust is that folder's).
    pub(super) found_in: String,
    pub(super) status: Option<Arc<crate::git::Status>>,
    pub(super) error: Option<String>,
    pub(super) loaded: bool,
}

impl Workbench {
    /// Search the open folders for repositories when they changed since the last search.
    pub(super) fn discover_repos(&mut self, force: bool, cx: &mut Context<Self>) {
        let roots = self.all_roots(cx);
        if self.vcs.discovering || (!force && self.vcs.repos_for == roots) {
            return;
        }
        self.vcs.discovering = true;
        self.vcs.repos_for = roots.clone();
        let host = self.remote.host.clone();
        let task = cx.background_executor().spawn(async move {
            let mut found: Vec<(String, String)> = roots.iter().map(|r| (r.clone(), r.clone())).collect();
            let mut truncated = false;
            for root in &roots {
                let left = repos::MAX_REPOS.saturating_sub(found.len());
                let nested = repos::nested(host.as_ref(), root, left);
                truncated |= nested.truncated;
                found.extend(nested.roots.into_iter().map(|r| (r, root.clone())));
            }
            (found, truncated)
        });
        let host = self.remote.host.clone();
        cx.spawn(async move |this, cx| {
            let (found, truncated) = task.await;
            this.update(cx, |this, cx| {
                this.vcs.discovering = false;
                // The folders or the connection changed while searching: search again for the new ones.
                if !Arc::ptr_eq(&host, &this.remote.host) || this.vcs.repos_for != this.all_roots(cx) {
                    return this.discover_repos(true, cx);
                }
                let mut old: std::collections::HashMap<String, RepoEntry> = this.vcs.repos.drain(..).map(|r| (r.root.clone(), r)).collect();
                this.vcs.repos = found
                    .into_iter()
                    .map(|(root, found_in)| old.remove(&root).unwrap_or(RepoEntry { root, found_in, status: None, error: None, loaded: false }))
                    .collect();
                this.vcs.repos_truncated = truncated;
                this.refresh_repo_statuses(cx);
                this.publish_repos(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Another folder or connection: the list (and the pick) belonged to the old one.
    pub(super) fn forget_repos(&mut self, cx: &mut Context<Self>) {
        self.vcs.repos.clear();
        self.vcs.repos_for.clear();
        self.vcs.repos_busy.clear();
        self.vcs.pinned = false;
        self.scm.update(cx, |s, cx| s.set_repos(Vec::new(), false, cx));
    }

    /// Read the status of every listed repository but the selected one (Source Control refreshes that one itself).
    pub(super) fn refresh_repo_statuses(&mut self, cx: &mut Context<Self>) {
        let selected = self.vcs.status.as_ref().map(|s| s.root.clone());
        let roots: Vec<(String, String)> =
            self.vcs.repos.iter().filter(|r| Some(&r.root) != selected.as_ref()).map(|r| (r.root.clone(), r.found_in.clone())).collect();
        for (root, found_in) in roots {
            self.refresh_repo_status(root, found_in, cx);
        }
    }

    fn refresh_repo_status(&mut self, root: String, found_in: String, cx: &mut Context<Self>) {
        if !self.vcs.repos_busy.insert(root.clone()) {
            return;
        }
        let host = self.remote.host.clone();
        let trusted = self.is_trusted_root(&found_in, cx);
        let r = root.clone();
        let task = cx.background_executor().spawn(async move { crate::git::status_for(host.as_ref(), &r, trusted) });
        let host = self.remote.host.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.vcs.repos_busy.remove(&root);
                if !Arc::ptr_eq(&host, &this.remote.host) {
                    return;
                }
                let Some(entry) = this.vcs.repos.iter_mut().find(|e| e.root == root) else { return };
                entry.loaded = true;
                match result {
                    Ok(st) => {
                        entry.status = st.map(Arc::new);
                        entry.error = None;
                    }
                    Err(e) => entry.error = Some(e),
                }
                this.publish_repos(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The list Source Control shows: one row per real repository (a folder that is not one, or that belongs to a
    /// repository already listed, is left out).
    pub(super) fn publish_repos(&mut self, cx: &mut Context<Self>) {
        let selected = self.vcs.status.as_ref().map(|s| s.root.clone());
        // The selected repository's status is the fresher one.
        if let Some(st) = &self.vcs.status
            && let Some(entry) = self.vcs.repos.iter_mut().find(|e| e.root == st.root)
        {
            entry.status = Some(st.clone());
            entry.loaded = true;
            entry.error = None;
        }
        let mut seen = std::collections::HashSet::new();
        let mut rows = Vec::new();
        for e in &self.vcs.repos {
            let root = e.status.as_ref().map(|s| s.root.clone()).unwrap_or_else(|| e.root.clone());
            if (e.loaded && e.status.is_none() && e.error.is_none()) || !seen.insert(root.clone()) {
                continue;
            }
            let label = root.trim_end_matches('/').rsplit('/').next().unwrap_or(&root).to_string();
            let location = root.strip_prefix(&format!("{}/", e.found_in.trim_end_matches('/'))).map(str::to_string);
            let st = e.status.as_deref();
            rows.push(crate::scm::RepoRow {
                label,
                location,
                branch: st.and_then(|s| s.branch.clone()),
                ahead: st.map_or(0, |s| s.ahead),
                behind: st.map_or(0, |s| s.behind),
                changes: st.map_or(0, |s| s.changes.len()),
                error: e.error.clone(),
                loading: !e.loaded,
                selected: selected.as_deref() == Some(root.as_str()),
                root,
            });
        }
        let truncated = self.vcs.repos_truncated;
        self.scm.update(cx, |s, cx| s.set_repos(rows, truncated, cx));
    }

    /// The user picked a repository: Source Control shows it until they pick another (the active file no longer moves it).
    pub(super) fn select_repo(&mut self, root: String, cx: &mut Context<Self>) {
        if !self.vcs.repos.iter().any(|r| r.root == root || r.status.as_ref().is_some_and(|s| s.root == root)) {
            self.notice = Some(Notice::Error(crate::i18n::tf("Repository is no longer available: {}", &[&root]).into()));
            return cx.notify();
        }
        self.vcs.pinned = true;
        if self.vcs.status.as_ref().map(|s| s.root.as_str()) == Some(root.as_str()) {
            return;
        }
        self.vcs.scm_root = Some(root.clone());
        self.vcs.error = None;
        // Until the new status arrives the panel shows nothing of the previous repository.
        self.apply_git(None, cx);
        self.load_git_branch(&root, cx);
        self.refresh_git(0, cx);
        self.publish_repos(cx);
    }

    /// "Git: Select Repository…": the same choice by keyboard.
    pub(super) fn show_repo_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.scm.read(cx).repos().to_vec();
        if rows.len() < 2 {
            self.notice = Some(Notice::Info(crate::i18n::t("This workspace has one repository.").into()));
            return cx.notify();
        }
        let items = rows.iter().map(|r| crate::picker::PickItem { label: r.label.clone().into(), detail: Some(r.description().into()), keys: None }).collect();
        let picker = cx
            .new(|cx| crate::picker::Picker::new(vec![crate::picker::Source::List { prefix: "", items }], crate::i18n::t("Select a repository"), "", None, cx));
        self.show_overlay(picker, OverlayKind::Repositories(rows.into_iter().map(|r| r.root).collect()), window, cx);
    }

    /// The repository Source Control should show for the active file, unless the user picked one.
    pub(super) fn wanted_scm_root(&self, cx: &App) -> Option<String> {
        let roots = self.all_roots(cx);
        if self.vcs.pinned
            && let Some(current) = self.vcs.scm_root.clone()
            && (self.vcs.repos.iter().any(|r| r.root == current) || roots.contains(&current))
        {
            return Some(current);
        }
        let file = self.active_editor().map(|e| e.read(cx).path.clone());
        let listed = self.vcs.repos.iter().filter(|r| !(r.loaded && r.status.is_none())).map(|r| r.root.as_str());
        file.as_deref().and_then(|f| repos::containing(listed.chain(roots.iter().map(String::as_str)), f).map(str::to_string)).or_else(|| self.active_root(cx))
    }
}

#[cfg(test)]
#[path = "workbench_repos_tests.rs"]
mod tests;
