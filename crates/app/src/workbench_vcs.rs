//! Workbench: the git status of the open folders. A refresh runs `git status` in the background (one at a time, with a
//! watchdog for a hung git), applies the result to Source Control, the explorer marks and the editors' change marks, and
//! keeps the marks of the other folders of the workspace current.

use super::*;

impl Workbench {
    /// Refresh git status after `delay` ms (back-to-back events coalesce into one run).
    pub fn refresh_git(&mut self, delay: u64, cx: &mut Context<Self>) {
        if let Some(root) = self.vcs.scm_root.clone().or_else(|| self.root(cx)) {
            self.refresh_git_at(&root, delay, cx);
            // Tree marks of inactive roots stay current too (Source Control only shows the active root).
            for other in self.all_roots(cx).into_iter().filter(|r| *r != root) {
                self.refresh_marks_only(other, cx);
            }
        }
    }

    /// Explorer marks only: one root's `git status`, without touching Source Control.
    pub(super) fn refresh_marks_only(&mut self, root: String, cx: &mut Context<Self>) {
        if !self.vcs.marks_busy.insert(root.clone()) {
            return;
        }
        let host = self.remote.host.clone();
        let r = root.clone();
        let trusted = self.is_trusted_root(&root, cx);
        let task = cx.background_executor().spawn(async move { crate::git::status_for(host.as_ref(), &r, trusted) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.vcs.marks_busy.remove(&root);
                if !this.all_roots(cx).contains(&root) {
                    return;
                }
                if let Ok(st) = result {
                    let marks = Arc::new(st.map(|s| s.marks()).unwrap_or_default());
                    if this.vcs.tree_marks.get(&root).map(|m| **m != *marks).unwrap_or(!marks.is_empty()) {
                        this.vcs.tree_marks.insert(root.clone(), marks.clone());
                        for t in this.all_trees() {
                            if t.read(cx).root() == Some(root.as_str()) {
                                t.update(cx, |t, cx| t.set_git(marks.clone(), cx));
                            }
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Source control shows the active file's repo: if the file is in another root, branch and status are refreshed.
    pub(super) fn sync_scm_root(&mut self, cx: &mut Context<Self>) {
        if self.roots.extra_roots.is_empty() {
            return;
        }
        let Some(root) = self.active_root(cx) else { return };
        if self.vcs.scm_root.as_deref().or(self.root(cx).as_deref()) == Some(root.as_str()) {
            return;
        }
        self.vcs.scm_root = Some(root.clone());
        self.load_git_branch(&root, cx);
        self.vcs.error = None;
        self.refresh_git(0, cx);
    }

    pub(super) fn refresh_git_at(&mut self, dir: &str, delay: u64, cx: &mut Context<Self>) {
        if self.vcs.refreshing {
            self.vcs.refresh_again = true;
            return;
        }
        let (host, dir) = (self.remote.host.clone(), dir.to_string());
        let trusted = self.is_trusted_root(&dir, cx);
        #[cfg(test)]
        let hangs = self.git_status_hangs;
        // A task in its waiting phase is replaced by the newer one (cancelled); a running `git status` isn't.
        self.vcs.refresh = Some(cx.spawn(async move |this, cx| {
            if delay > 0 {
                cx.background_executor().timer(Duration::from_millis(delay)).await;
            }
            if this.update(cx, |this, _| this.vcs.refreshing = true).is_err() {
                return;
            }
            let d = dir.clone();
            let status = cx.background_executor().spawn(async move { crate::git::status_for(host.as_ref(), &d, trusted) });
            #[cfg(test)]
            let status: std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<crate::git::Status>, String>>>> =
                if hangs { Box::pin(futures::future::pending()) } else { Box::pin(status) };
            // Watchdog: if no status arrives (hung git, slow network disk) the lock is released; later
            // refreshes run and the late result isn't applied.
            let timer = cx.background_executor().timer(GIT_STATUS_TIMEOUT);
            let result = match futures::future::select(status, timer).await {
                futures::future::Either::Left((result, _)) => result,
                futures::future::Either::Right(_) => Err(format!("git status did not finish in {} s", GIT_STATUS_TIMEOUT.as_secs())),
            };
            this.update(cx, |this, cx| {
                this.vcs.refreshing = false;
                if std::mem::take(&mut this.vcs.refresh_again) {
                    this.refresh_git(0, cx);
                }
                if !this.all_roots(cx).contains(&dir) {
                    return; // another folder was opened meanwhile
                }
                if this.vcs.scm_root.clone().or_else(|| this.root(cx)).as_deref() != Some(dir.as_str()) {
                    this.refresh_git(0, cx); // the active file's root changed meanwhile
                    return;
                }
                match result {
                    // No change (most polls): no redraw. Every frame makes Metal allocate ~110 MB of
                    // temporary buffers; polling every 4 s kept inflating memory.
                    Ok(st) if this.vcs.error.is_none() && st.as_ref() == this.vcs.status.as_deref() => {
                        if this.chrome.panel == Some(Panel::Scm) {
                            this.scm.update(cx, |s, cx| s.refresh_worktrees(cx));
                        }
                        return;
                    }
                    Ok(st) => {
                        this.vcs.error = None;
                        this.apply_git(st.map(Arc::new), cx);
                        if this.chrome.panel == Some(Panel::Scm) {
                            this.scm.update(cx, |s, cx| s.refresh_worktrees(cx));
                        }
                    }
                    Err(e) => {
                        // Say it only the first time (polling retries every 4 s).
                        if this.vcs.error.is_none() {
                            this.notice = Some(Notice::Error(format!("Git: {e}").into())); // i18n-keep: "Git: {}" would read the same in every language; {e} is git's own text
                        }
                        this.vcs.error = Some(e.into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn apply_git(&mut self, st: Option<Arc<crate::git::Status>>, cx: &mut Context<Self>) {
        let head_changed = st.as_ref().map(|s| (&s.root, &s.head)) != self.vcs.status.as_ref().map(|s| (&s.root, &s.head));
        let marks = Arc::new(st.as_ref().map(|s| s.marks()).unwrap_or_default());
        // Each tree shows the marks of its own root: the new status updates the cache of roots inside the repo
        // (clears the active root's if there is no repo); other roots' marks are kept.
        let scm_dir = self.vcs.scm_root.clone().or_else(|| self.root(cx));
        let repo_root = st.as_ref().map(|s| format!("{}/", s.root.trim_end_matches('/')));
        for r in self.all_roots(cx) {
            let inside = repo_root.as_ref().is_some_and(|p| format!("{}/", r.trim_end_matches('/')).starts_with(p.as_str()));
            if inside {
                self.vcs.tree_marks.insert(r, marks.clone());
            } else if repo_root.is_none() && scm_dir.as_deref() == Some(r.as_str()) {
                self.vcs.tree_marks.remove(&r);
            }
        }
        for t in self.all_trees() {
            let m = t.read(cx).root().and_then(|r| self.vcs.tree_marks.get(r).cloned()).unwrap_or_default();
            t.update(cx, |t, cx| t.set_git(m, cx));
        }
        if let Some(s) = &st {
            self.vcs.branch = s.branch.clone().map(Into::into).or(self.vcs.branch.take());
        }
        // Gutter base is re-read only when HEAD (or the repo) changes.
        if head_changed {
            for ed in self.all_editors() {
                if ed.read(cx).uses_host(&self.remote.host) {
                    let (root, changed) = git_context(st.as_deref(), &ed.read(cx).path);
                    ed.update(cx, |e, cx| e.set_git_root(root, changed, cx));
                }
            }
        }
        let host = self.remote.host.clone();
        self.scm.update(cx, |s, cx| s.set_status(host, st.clone(), cx));
        self.vcs.status = st;
    }

    pub fn git_status(&self) -> Option<Arc<crate::git::Status>> {
        self.vcs.status.clone()
    }

    /// Read `.git/HEAD` through the host (works remotely too); if the root is a
    /// subfolder, looks up at most 6 parent levels.
    pub(super) fn load_git_branch(&mut self, root: &str, cx: &mut Context<Self>) {
        self.vcs.branch = None;
        let host = self.remote.host.clone();
        let root = root.trim_end_matches('/').to_string();
        let task = cx.background_executor().spawn(async move {
            let mut dir = root;
            for _ in 0..6 {
                if let Ok((data, _)) = host.read(&format!("{dir}/.git/HEAD")) {
                    let head = String::from_utf8_lossy(&data).trim().to_string();
                    return Some(match head.strip_prefix("ref: refs/heads/") {
                        Some(branch) => branch.to_string(),
                        None => head.chars().take(7).collect(), // detached: short sha
                    });
                }
                match dir.rsplit_once('/') {
                    Some((parent, _)) if !parent.is_empty() => dir = parent.to_string(),
                    _ => break,
                }
            }
            None
        });
        cx.spawn(async move |this, cx| {
            let branch = task.await;
            this.update(cx, |this, cx| {
                this.vcs.branch = branch.map(Into::into);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
