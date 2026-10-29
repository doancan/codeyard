//! Git operations on one repository of the Source Control panel: which repository an action goes to (checked again on
//! the host before Git runs), running it, and staging, discarding, stashing and committing there.

use super::*;

impl ScmPanel {
    /// An operation is running in the selected repository.
    pub(super) fn busy(&self) -> bool {
        self.status.as_ref().is_some_and(|s| self.busy.contains(&s.root))
    }

    /// The inventory entry of a worktree.
    pub(super) fn repo_by_root(&self, root: &str) -> Option<&Repo> {
        let root = root.trim_end_matches('/');
        self.repos.iter().find(|r| r.id.worktree == root)
    }

    /// The identity is still in the inventory: a result for it may change what the panel shows.
    pub(super) fn repo_current(&self, id: &RepoId) -> bool {
        self.repos.iter().any(|r| *r.id == *id)
    }

    /// The selected repository's inventory key.
    pub(crate) fn selected_repo_key(&self) -> Option<RepoKey> {
        self.status.as_ref().and_then(|s| self.repo_by_root(&s.root)).map(|r| r.id.key)
    }

    /// Where an operation goes: `None` is the selected repository, `Some` a row's repository. Never another one.
    pub(crate) fn target(&self, repo: Option<RepoKey>) -> Result<Target, String> {
        match repo {
            None => {
                let root = self.status.as_ref().map(|s| s.root.clone()).ok_or_else(|| crate::i18n::t("Select a repository first").to_string())?;
                let id = self.repo_by_root(&root).map(|r| r.id.clone());
                Ok(Target { root, id })
            }
            Some(key) => match self.repos.iter().find(|r| r.id.key == key) {
                Some(r) if r.status.is_some() => Ok(Target { root: r.id.worktree.clone(), id: Some(r.id.clone()) }),
                Some(r) => Err(crate::i18n::tf("Repository is no longer available: {}", &[&r.id.worktree])),
                None => Err(crate::i18n::t("Repository changed while the operation was pending.").to_string()),
            },
        }
    }

    /// Show a refused operation where its repository's results go.
    pub(super) fn refuse(&mut self, repo: Option<RepoKey>, why: String, cx: &mut Context<Self>) {
        match repo.filter(|k| Some(*k) != self.selected_repo_key()) {
            Some(key) if self.repos.iter().any(|r| r.id.key == key) => {
                self.repo_results.insert(key, (why.into(), true));
            }
            _ => self.result = Some((why.into(), true)),
        }
        cx.notify();
    }

    /// A git operation in the background in the selected repository; when done the result is written and the state refreshed.
    pub(super) fn run(&mut self, op: impl FnOnce(&dyn Host, &str) -> Result<String, String> + Send + 'static, cx: &mut Context<Self>) {
        self.run_then(op, |_, _, _| {}, cx);
    }

    /// `on_ok`: if the operation succeeded (e.g. clear the message after a commit); its flag says whether the repository
    /// is still the selected one.
    pub(super) fn run_then(
        &mut self,
        op: impl FnOnce(&dyn Host, &str) -> Result<String, String> + Send + 'static,
        on_ok: impl FnOnce(&mut Self, bool, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Ok(target) = self.target(None) else { return };
        self.run_in(target, op, on_ok, cx);
    }

    /// Run `op` in `target`, one operation at a time per repository. On the host the target is checked first (still
    /// there, still the same repository). When it finishes, a result for a repository that was removed, replaced or
    /// reconnected meanwhile is rejected, and the result for a repository that is no longer selected is kept for it
    /// instead of being shown for another one.
    pub(crate) fn run_in(
        &mut self,
        target: Target,
        op: impl FnOnce(&dyn Host, &str) -> Result<String, String> + Send + 'static,
        on_ok: impl FnOnce(&mut Self, bool, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        if !self.busy.insert(target.root.clone()) {
            return;
        }
        if self.status.as_ref().is_some_and(|s| s.root == target.root) {
            self.result = None;
        } else if let Some(id) = &target.id {
            self.repo_results.remove(&id.key);
        }
        let host = self.host.clone();
        let (root, id) = (target.root.clone(), target.id.clone());
        let task = cx.background_executor().spawn(async move {
            if let Some(id) = &id {
                crate::repos::revalidate(host.as_ref(), id).map_err(|e| (true, e))?;
            }
            op(host.as_ref(), &root).map_err(|e| (false, e))
        });
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.finish(target, host, result, on_ok, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn finish(
        &mut self,
        target: Target,
        host: Arc<dyn Host>,
        result: Result<String, (bool, String)>,
        on_ok: impl FnOnce(&mut Self, bool, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) {
        if Arc::ptr_eq(&self.host, &host) {
            self.busy.remove(&target.root);
        }
        let stale = !Arc::ptr_eq(&self.host, &host) || target.id.as_ref().is_some_and(|id| !self.repo_current(id));
        if stale || matches!(result, Err((true, _))) {
            // Removed, replaced or reconnected while the operation was pending: nothing is applied to the repositories
            // the panel shows now, and the inventory is searched again.
            let why = match result {
                Err((true, e)) => e,
                _ => crate::i18n::t("Repository changed while the operation was pending.").to_string(),
            };
            self.result = Some((why.into(), true));
            cx.emit(ScmEvent::Rediscover);
            cx.notify();
            return;
        }
        let selected = self.status.as_ref().is_some_and(|s| s.root == target.root);
        if result.is_ok() {
            on_ok(self, selected, cx);
        }
        let shown = match result {
            Ok(msg) if msg.is_empty() => None,
            Ok(msg) => Some((msg.into(), false)),
            Err((_, e)) => Some((e.into(), true)),
        };
        match (&target.id, selected) {
            (Some(id), false) => {
                match shown {
                    Some(r) => self.repo_results.insert(id.key, r),
                    None => self.repo_results.remove(&id.key),
                };
                cx.emit(ScmEvent::RefreshRepo(id.key));
            }
            _ => {
                if selected {
                    self.result = shown;
                    // Operations that don't change HEAD, like fetch/pull, can still move the branch tip.
                    self.load_graph(cx);
                }
                cx.emit(ScmEvent::Refresh);
            }
        }
        cx.notify();
    }

    /// The status of an operation's repository: the selected one's (`None`) or a row's.
    pub(super) fn status_of(&self, repo: Option<RepoKey>) -> Option<Arc<Status>> {
        match repo {
            None => self.status.clone(),
            Some(key) => self.repos.iter().find(|r| r.id.key == key).and_then(|r| r.status.clone()),
        }
    }

    /// Staging also means marking a conflict "resolved": refuse if markers are still inside.
    pub fn stage(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.stage_in(None, paths, cx);
    }

    /// Stage in the selected repository (`None`) or in a row's repository.
    pub fn stage_in(&mut self, repo: Option<RepoKey>, paths: Vec<String>, cx: &mut Context<Self>) {
        let target = match self.target(repo) {
            Ok(t) => t,
            Err(why) if repo.is_some() => return self.refuse(repo, why, cx),
            Err(_) => return,
        };
        let conflicted: Vec<String> = self
            .status_of(repo)
            .map(|st| st.changes.iter().filter(|c| c.unstaged == Some(Mark::Conflict) && paths.contains(&c.path)).map(|c| c.path.clone()).collect())
            .unwrap_or_default();
        self.run_in(
            target,
            move |h, root| {
                for rel in &conflicted {
                    let abs = format!("{}/{rel}", root.trim_end_matches('/'));
                    let still = h.read(&abs).is_ok_and(|(d, _)| !crate::conflict::find(&buffer::Rope::from_str(&String::from_utf8_lossy(&d))).is_empty());
                    if still {
                        return Err(crate::i18n::tf(
                            "{} still has merge conflict markers. Resolve them (Accept Current / Incoming / Both) and save first.",
                            &[&ui::file_name(rel)],
                        ));
                    }
                }
                git::stage(h, root, &paths).map(|_| String::new())
            },
            |_, _, _| {},
            cx,
        );
    }

    pub fn unstage(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.unstage_in(None, paths, cx);
    }

    pub fn unstage_in(&mut self, repo: Option<RepoKey>, mut paths: Vec<String>, cx: &mut Context<Self>) {
        let target = match self.target(repo) {
            Ok(t) => t,
            Err(why) if repo.is_some() => return self.refuse(repo, why, cx),
            Err(_) => return,
        };
        // Rename: the old path is also unstaged (as deleted).
        if let Some(st) = self.status_of(repo) {
            let origs: Vec<String> = st.changes.iter().filter(|c| paths.contains(&c.path)).filter_map(|c| c.orig.clone()).collect();
            paths.extend(origs);
        }
        self.run_in(target, move |h, root| git::unstage(h, root, &paths).map(|_| String::new()), |_, _, _| {}, cx);
    }

    /// Irreversible: ask first (untracked files are deleted).
    pub fn confirm_discard(&mut self, changes: Vec<Change>, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard_in(None, changes, window, cx);
    }

    /// The target is fixed when the question is asked: the answer applies to that repository even if the selection
    /// changes while the dialog is open, and to nothing if it was removed or replaced meanwhile.
    pub fn confirm_discard_in(&mut self, repo: Option<RepoKey>, changes: Vec<Change>, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.target(repo) {
            Ok(t) => Some(t),
            Err(why) if repo.is_some() => return self.refuse(repo, why, cx),
            Err(_) => None,
        };
        let deletes = changes.iter().any(|c| c.unstaged == Some(Mark::Untracked));
        let title = match changes.as_slice() {
            [one] => crate::i18n::tf("Discard changes in {}?", &[&format!("“{}”", ui::file_name(&one.path))]),
            many => crate::i18n::tf("Discard changes in {} files?", &[&many.len()]),
        };
        let mut detail =
            if deletes { crate::i18n::t("Untracked files will be deleted. This can't be undone.") } else { crate::i18n::t("This can't be undone.") }
                .to_string();
        if let Some(t) = target.as_ref().filter(|_| self.repos.len() > 1) {
            // Several repositories: the question names the one it applies to.
            detail = format!("{}\n{detail}", crate::i18n::tf("Repository: {}", &[&t.root]));
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &[gpui::PromptButton::cancel(crate::i18n::t("Cancel")), gpui::PromptButton::new(crate::i18n::t("Discard"))],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(1)
                && let Some(target) = target
            {
                this.update(cx, |this, cx| this.discard_target(target, changes, cx)).ok();
            }
        })
        .detach();
    }

    #[cfg(test)]
    pub fn discard(&mut self, changes: Vec<Change>, cx: &mut Context<Self>) {
        if let Ok(target) = self.target(None) {
            self.discard_target(target, changes, cx);
        }
    }

    pub(super) fn discard_target(&mut self, target: Target, changes: Vec<Change>, cx: &mut Context<Self>) {
        self.run_in(target, move |h, root| git::discard(h, root, &changes).map(|_| String::new()), |_, _, _| {}, cx);
    }

    /// "Discard All Changes": everything not staged (asks first).
    pub fn confirm_discard_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard_all_in(None, window, cx);
    }

    pub(super) fn confirm_discard_all_in(&mut self, repo: Option<RepoKey>, window: &mut Window, cx: &mut Context<Self>) {
        // Conflicted files are not discarded (checkout fails on an unmerged file and the resolution would be lost).
        let changes: Vec<Change> =
            self.status_of(repo).map(|s| s.unstaged().filter(|c| c.unstaged != Some(Mark::Conflict)).cloned().collect()).unwrap_or_default();
        if changes.is_empty() {
            self.result = Some((crate::i18n::t("No changes to discard").to_string().into(), false));
            cx.notify();
            return;
        }
        self.confirm_discard_in(repo, changes, window, cx);
    }

    /// Amend the last commit: staged changes are added; if the box is empty the message stays the same.
    pub fn commit_amend(&mut self, cx: &mut Context<Self>) {
        let message = self.message.read(cx).text().trim().to_string();
        self.run_then(
            move |h, root| {
                git::commit(h, root, &message, git::CommitOptions { amend: true, ..Default::default() })
                    .map(|summary| crate::i18n::tf("Amended {}", &[&summary]))
            },
            |this, selected, cx| {
                if selected {
                    this.message.update(cx, |m, cx| m.set_text("", cx));
                }
            },
            cx,
        );
    }

    /// Undo the last commit: changes stay staged, its message returns to the box.
    pub fn undo_last_commit(&mut self, cx: &mut Context<Self>) {
        // The message comes from the background job; it is written to the box only on success.
        let message = Arc::new(std::sync::Mutex::new(String::new()));
        let out = message.clone();
        self.run_then(
            move |h, root| {
                let m = git::undo_last_commit(h, root)?;
                let summary = crate::i18n::tf("Undid commit “{}”", &[&m.lines().next().unwrap_or("")]);
                *out.lock().unwrap_or_else(|e| e.into_inner()) = m;
                Ok(summary)
            },
            move |this, selected, cx| {
                let m = std::mem::take(&mut *message.lock().unwrap_or_else(|e| e.into_inner()));
                if selected {
                    this.message.update(cx, |f, cx| f.set_text(&m, cx));
                }
            },
            cx,
        );
    }

    pub fn stash(&mut self, untracked: bool, cx: &mut Context<Self>) {
        self.run(move |h, root| git::stash(h, root, untracked).map(|_| crate::i18n::t("Stashed changes").to_string()), cx);
    }

    /// Apply or pop a stash of the selected repository, right now ("pop the latest stash").
    pub fn stash_apply(&mut self, stash: String, pop: bool, cx: &mut Context<Self>) {
        self.stash_action_in(None, stash, None, if pop { StashAction::Pop } else { StashAction::Apply }, cx);
    }

    /// A stash action pinned to the repository the stash was listed in (`None`: the selected one, now). It runs there
    /// or nowhere, and only on the same entry: `stash@{N}` is a position, so when `message` is given the entry at that
    /// position must still carry it (another stash pushed or dropped meanwhile shifts the positions).
    pub fn stash_action_in(&mut self, repo: Option<Target>, stash: String, message: Option<String>, action: StashAction, cx: &mut Context<Self>) {
        let target = match repo.map_or_else(|| self.target(None), Ok) {
            Ok(t) => t,
            Err(why) => {
                self.result = Some((why.into(), true));
                cx.notify();
                return;
            }
        };
        self.run_in(
            target,
            move |h, root| {
                if let Some(message) = &message
                    && !git::stash_list(h, root)?.iter().any(|(n, m)| *n == stash && m == message)
                {
                    return Err(crate::i18n::t("Repository changed while the operation was pending.").to_string());
                }
                match action {
                    StashAction::Apply => git::stash_apply(h, root, &stash, false).map(|_| crate::i18n::tf("Applied {}", &[&stash])),
                    StashAction::Pop => git::stash_apply(h, root, &stash, true).map(|_| crate::i18n::tf("Popped {}", &[&stash])),
                    StashAction::Drop => git::stash_drop(h, root, &stash).map(|_| crate::i18n::tf("Dropped {}", &[&stash])),
                }
            },
            |_, _, _| {},
            cx,
        );
    }

    /// If nothing is staged, commit all tracked changes ("Commit All"), unless `git.enableSmartCommit` is off.
    pub fn commit(&mut self, cx: &mut Context<Self>) {
        self.commit_with(git::CommitOptions::default(), None, cx);
    }

    pub fn commit_signed_off(&mut self, cx: &mut Context<Self>) {
        self.commit_with(git::CommitOptions { signoff: true, ..Default::default() }, None, cx);
    }

    /// Skips the pre-commit and commit-msg hooks.
    pub fn commit_no_verify(&mut self, cx: &mut Context<Self>) {
        self.commit_with(git::CommitOptions { no_verify: true, ..Default::default() }, None, cx);
    }

    /// "Commit & Push" / "Commit & Sync": what follows the commit is asked for explicitly, so no confirmation.
    pub fn commit_and(&mut self, then: git::PostCommit, cx: &mut Context<Self>) {
        self.commit_with(git::CommitOptions::default(), Some(then), cx);
    }

    pub(super) fn commit_with(&mut self, flags: git::CommitOptions, then: Option<git::PostCommit>, cx: &mut Context<Self>) {
        let message = self.message.read(cx).text().trim().to_string();
        let Some(st) = &self.status else { return };
        if message.is_empty() {
            self.result = Some((crate::i18n::t("Type a commit message first").to_string().into(), true));
            cx.notify();
            return;
        }
        let nothing_staged = st.staged().next().is_none();
        if nothing_staged && !st.unstaged().any(|c| c.unstaged != Some(Mark::Untracked)) {
            self.result = Some((crate::i18n::t("Nothing to commit").to_string().into(), true));
            cx.notify();
            return;
        }
        if nothing_staged && !cx.global::<crate::settings::Settings>().git_smart_commit {
            self.result =
                Some((crate::i18n::t("Nothing is staged: stage the changes to commit first (or turn on git.enableSmartCommit)").to_string().into(), true));
            cx.notify();
            return;
        }
        let then = then.or_else(|| git::PostCommit::from_setting(&cx.global::<crate::settings::Settings>().git_post_commit));
        let opts = git::CommitOptions { all: nothing_staged, ..flags };
        let root = st.root.clone();
        // The message is cleared only when the commit succeeds (if a hook rejects, the user fixes it and retries).
        self.run_then(
            move |h, root| git::commit(h, root, &message, opts).map(|summary| crate::i18n::tf("Committed {}", &[&summary])),
            move |this, selected, cx| {
                if selected {
                    this.message.update(cx, |m, cx| m.set_text("", cx));
                    if let Some(then) = then {
                        cx.emit(ScmEvent::PostCommit(then));
                    }
                } else {
                    // The committed draft is gone. A push or sync meant to follow would now reach the selected
                    // repository instead, so it does not run.
                    this.drafts.remove(&root);
                    if then.is_some() {
                        this.result = Some((crate::i18n::t("Repository changed while the operation was pending.").into(), true));
                    }
                }
            },
            cx,
        );
    }
}
