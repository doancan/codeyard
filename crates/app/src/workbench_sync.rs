//! Workbench: Sync Changes. Pull, push, fetch and publish from the status bar, the Source Control view and the commands, the
//! background fetch (`git.autofetch`) and the guard that keeps two network operations from running at once.

use gpui::{AppContext as _, Context, SharedString, Window};

use super::git_glue::first_line;
use super::{Notice, OverlayKind, Workbench};
use crate::git::{self, PostCommit, SyncOp, SyncOptions, SyncStep};
use crate::picker::{PickItem, Picker, Source as PickSource};
use crate::settings::Settings;

/// Which network operation holds the guard. Only a sync the user asked for shows a spinner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Running {
    Sync,
    Autofetch,
}

#[derive(Default)]
pub(super) struct SyncState {
    pub(super) running: Option<Running>,
    /// Why the last background fetch failed; shown as the status bar item's tooltip until a fetch or a sync succeeds.
    autofetch_error: Option<AutofetchError>,
}

struct AutofetchError {
    host: std::sync::Weak<dyn host::Host>,
    root: String,
    message: SharedString,
}

/// How often the background fetch looks at the clock; the period itself comes from `git.autofetchPeriod`.
const AUTOFETCH_TICK: std::time::Duration = std::time::Duration::from_secs(5);

fn options(s: &Settings) -> SyncOptions {
    SyncOptions { rebase: s.git_rebase_when_sync, prune: s.git_prune_on_fetch, tags: s.git_pull_tags, fetch_on_pull: s.git_fetch_on_pull }
}

fn is_pull(op: &SyncOp) -> bool {
    matches!(op, SyncOp::Pull | SyncOp::PullRebase | SyncOp::PullFrom { .. })
}

fn is_push(op: &SyncOp) -> bool {
    matches!(op, SyncOp::Push | SyncOp::PushTo { .. })
}

fn working_text(ops: &[SyncOp]) -> &'static str {
    match ops {
        [op] if is_pull(op) => crate::i18n::t("Pulling…"),
        [op] if is_push(op) => crate::i18n::t("Pushing…"),
        [SyncOp::Fetch | SyncOp::FetchAll] => crate::i18n::t("Fetching…"),
        [SyncOp::Publish] => crate::i18n::t("Publishing branch…"),
        [a, b] if is_pull(a) && is_push(b) => crate::i18n::t("Pulling, Pushing…"),
        _ => crate::i18n::t("Syncing…"),
    }
}

fn done_text(ops: &[SyncOp], steps: &[SyncStep]) -> String {
    if let Some(SyncStep::Published(remote)) = steps.iter().find(|s| matches!(s, SyncStep::Published(_))) {
        return crate::i18n::tf("Published to {}", &[remote]);
    }
    match ops {
        [op] if is_pull(op) => crate::i18n::t("Pulled"),
        [op] if is_push(op) => crate::i18n::t("Pushed"),
        [SyncOp::Fetch | SyncOp::FetchAll] => crate::i18n::t("Fetched"),
        _ => crate::i18n::t("Synced"),
    }
    .to_string()
}

fn failed_text(op: &SyncOp, e: &str) -> String {
    let failed = match op {
        op if is_pull(op) => crate::i18n::tf("Pull failed: {}", &[&first_line(e)]),
        op if is_push(op) => crate::i18n::tf("Push failed: {}", &[&first_line(e)]),
        SyncOp::Publish => crate::i18n::tf("Publish failed: {}", &[&first_line(e)]),
        _ => crate::i18n::tf("Fetch failed: {}", &[&first_line(e)]),
    };
    if e.contains("could not read Username") || e.contains("Permission denied") || e.contains("terminal prompts disabled") {
        format!("{failed} {}", crate::i18n::t("The server wants credentials: run the command once in the terminal (or set up an SSH key)."))
    } else {
        failed
    }
}

impl Workbench {
    /// Pull / push / fetch / publish, several in sequence (sync: pull first, then push). Never two at once.
    pub(super) fn git_sync(&mut self, ops: Vec<SyncOp>, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        let target = root.clone();
        match self.vcs.sync.running {
            Some(Running::Sync) => {
                self.notice = Some(Notice::Info(crate::i18n::t("A sync is already running.").into()));
                cx.notify();
                return;
            }
            Some(Running::Autofetch) => {
                self.notice = Some(Notice::Info(crate::i18n::t("Fetching in the background: try again in a moment.").into()));
                cx.notify();
                return;
            }
            None => {}
        }
        self.vcs.sync.running = Some(Running::Sync);
        self.set_scm_syncing(true, cx);
        self.notice = Some(Notice::Info(working_text(&ops).into()));
        cx.notify();
        let host = self.remote.host.clone();
        let source_host = host.clone();
        let source_root = root.clone();
        let opts = options(cx.global::<Settings>());
        let list = ops.clone();
        let task = cx.background_executor().spawn(async move {
            let mut steps = Vec::new();
            for op in &list {
                let step = git::sync(host.as_ref(), &root, op, opts).map_err(|e| (op.clone(), e))?;
                let stop = step == SyncStep::Conflicts;
                steps.push(step);
                if stop {
                    break;
                }
            }
            Ok::<_, (SyncOp, String)>(steps)
        });
        let picked = self.vcs.repos.inventory.repos.len() > 1;
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.vcs.sync.running = None;
                this.set_scm_syncing(false, cx);
                // Another server, or (with one repository) another folder: the result says nothing about what is shown.
                let elsewhere = !std::sync::Arc::ptr_eq(&this.remote.host, &source_host) || (!picked && !this.git_context_matches(&source_host, &source_root));
                let gone = picked && this.vcs.repos.inventory.by_worktree(&target).is_none() && !this.all_roots(cx).contains(&target);
                if elsewhere || gone {
                    if gone && !elsewhere {
                        // The repository left the workspace while its sync ran.
                        this.notice = Some(Notice::Error(crate::i18n::t("Repository changed while the operation was pending.").into()));
                        this.refresh_git(0, cx);
                    }
                    cx.notify();
                    return;
                }
                this.notice = Some(match result {
                    Ok(steps) if steps.last() == Some(&SyncStep::Conflicts) => {
                        // The conflicted pull is the last step that ran.
                        let rebasing = ops.get(steps.len() - 1).is_some_and(|op| match op {
                            SyncOp::PullRebase => true,
                            _ => is_pull(op) && opts.rebase,
                        });
                        this.chrome.panel = Some(super::Panel::Scm);
                        Notice::Info(
                            if rebasing {
                                crate::i18n::t("Rebase stopped on conflicts: resolve them, then Git: Continue Rebase (or Abort Rebase)")
                            } else {
                                crate::i18n::t("Pull has conflicts: resolve them in Source Control (or Git: Abort Merge)")
                            }
                            .into(),
                        )
                    }
                    Ok(steps) => {
                        this.vcs.sync.autofetch_error = None;
                        Notice::Info(done_text(&ops, &steps).into())
                    }
                    Err((op, e)) => Notice::Error(failed_text(&op, &e).into()),
                });
                this.refresh_git(0, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_scm_syncing(&mut self, syncing: bool, cx: &mut Context<Self>) {
        self.scm.update(cx, |s, cx| s.set_syncing(syncing, cx));
    }

    /// The Sync button, the status bar arrows and "Git: Sync": publish an unpublished branch, otherwise pull and push what is
    /// pending (only fetch when nothing is). Asks first when `git.confirmSync` is on and the sync moves commits.
    pub(super) fn git_sync_auto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.git_status() else {
            self.repo_root(cx);
            return;
        };
        let ops = if st.branch.is_none() { vec![SyncOp::Fetch] } else { git::sync_plan(st.upstream.is_some(), st.ahead, st.behind) };
        let pulls = ops.iter().any(is_pull);
        let pushes = ops.iter().any(is_push);
        if !(pulls || pushes) || !cx.global::<Settings>().git_confirm_sync {
            self.git_sync(ops, cx);
            return;
        }
        let upstream = st.upstream.clone().unwrap_or_default();
        let mut detail = match (pulls, pushes) {
            (true, true) => crate::i18n::tf("This pulls and pushes commits from and to “{}”.", &[&upstream]),
            (true, false) => crate::i18n::tf("This pulls commits from “{}”.", &[&upstream]),
            _ => crate::i18n::tf("This pushes commits to “{}”.", &[&upstream]),
        };
        if self.vcs.repos.inventory.repos.len() > 1 {
            // Several repositories: the question names the one it applies to.
            detail = format!("{}\n{detail}", crate::i18n::tf("Repository: {}", &[&st.root]));
        }
        // The answer applies to the repository and server the question was about, never to one selected while it was open.
        let (host, root) = (self.remote.host.clone(), st.root.clone());
        self.confirm(window, crate::i18n::t("Sync changes?"), &detail, crate::i18n::t("Sync"), cx, move |this, cx| {
            if !this.git_context_matches(&host, &root) {
                this.notice = Some(Notice::Error(crate::i18n::t("Repository changed while the operation was pending.").into()));
                cx.notify();
                return;
            }
            this.git_sync(ops, cx)
        });
    }

    pub(super) fn git_publish_branch(&mut self, cx: &mut Context<Self>) {
        self.git_sync(vec![SyncOp::Publish], cx);
    }

    /// After a commit: `git.postCommitCommand` or "Commit & Push/Sync". Asked for ahead of time, so no confirmation.
    pub(super) fn git_post_commit(&mut self, then: PostCommit, cx: &mut Context<Self>) {
        let has_upstream = self.git_status().is_some_and(|s| s.upstream.is_some());
        self.git_sync(then.ops(has_upstream), cx);
    }

    /// "Pull from…" lists the remote branches, "Push to…" the remotes.
    pub(super) fn show_sync_target_picker(&mut self, pull: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        let host = self.remote.host.clone();
        let source_host = host.clone();
        let source_root = root.clone();
        let task = cx.background_executor().spawn(async move {
            let remotes = git::remotes(host.as_ref(), &root)?;
            let branches = if pull { git::branches(host.as_ref(), &root)?.1 } else { Vec::new() };
            Ok::<_, String>((remotes, branches))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if !this.git_context_matches(&source_host, &source_root) {
                    return;
                }
                let (remotes, branches) = match result {
                    Ok(r) => r,
                    Err(e) => {
                        this.notice = Some(Notice::Error(crate::i18n::tf("Could not list: {}", &[&first_line(&e)]).into()));
                        cx.notify();
                        return;
                    }
                };
                let (mut ops, mut items) = (Vec::new(), Vec::new());
                if pull {
                    for b in branches {
                        // The longest remote name that prefixes it: remote names may hold a slash themselves.
                        let owner = remotes.iter().filter(|(n, _)| b.strip_prefix(n.as_str()).is_some_and(|r| r.starts_with('/'))).max_by_key(|(n, _)| n.len());
                        if let Some((remote, _)) = owner {
                            let branch = b[remote.len() + 1..].to_string();
                            items.push(PickItem { label: b.clone().into(), detail: None, keys: None });
                            ops.push(SyncOp::PullFrom { remote: remote.clone(), branch });
                        }
                    }
                } else {
                    for (remote, url) in remotes {
                        items.push(PickItem { label: remote.clone().into(), detail: Some(url.into()), keys: None });
                        ops.push(SyncOp::PushTo { remote });
                    }
                }
                if ops.is_empty() {
                    this.notice = Some(Notice::Info(crate::i18n::t("Nothing to choose from.").into()));
                    cx.notify();
                    return;
                }
                let title = if pull { crate::i18n::t("Pull from…") } else { crate::i18n::t("Push to…") };
                let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], title, "", None, cx));
                this.show_overlay(picker, OverlayKind::SyncTarget { host: source_host, root: source_root, ops }, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// What the status bar's sync item says when hovered: where the branch stands, and why the last background fetch failed.
    pub(super) fn sync_tooltip(&self) -> SharedString {
        let mut tip = match (self.vcs.sync.running, self.git_status()) {
            (Some(Running::Sync), _) => crate::i18n::t("Syncing…").to_string(),
            (_, Some(st)) => match &st.upstream {
                Some(up) if st.ahead == 0 && st.behind == 0 => crate::i18n::tf("{}: up to date. Click to fetch.", &[up]),
                Some(up) => crate::i18n::tf("{0}: {1} to pull, {2} to push. Click to sync.", &[up, &st.behind, &st.ahead]),
                None if st.branch.is_some() => crate::i18n::t("This branch is not published yet. Click to publish it.").to_string(),
                None => "Fetch".to_string(),
            },
            _ => String::new(),
        };
        if let Some(e) = self.autofetch_error() {
            tip = format!("{tip}\n{}", crate::i18n::tf("The last background fetch failed: {}", &[e]));
        }
        tip.into()
    }

    fn autofetch_error(&self) -> Option<&SharedString> {
        let error = self.vcs.sync.autofetch_error.as_ref()?;
        let host = error.host.upgrade()?;
        self.git_context_matches(&host, &error.root).then_some(&error.message)
    }

    /// Fetch in the background if `git.autofetch` is on, the window is in front, the folder is trusted and nothing else is
    /// talking to the remote. Failures are kept for the sync item's tooltip, never shown as a notice.
    pub(super) fn autofetch_now(&mut self, window_active: bool, cx: &mut Context<Self>) -> bool {
        if !cx.global::<Settings>().git_autofetch || !window_active || self.vcs.sync.running.is_some() {
            return false;
        }
        let Some(root) = self.git_status().map(|s| s.root.clone()) else { return false };
        if !self.is_trusted_root(&root, cx) {
            return false;
        }
        self.vcs.sync.running = Some(Running::Autofetch);
        let host = self.remote.host.clone();
        let source_host = host.clone();
        let source_root = root.clone();
        let opts = options(cx.global::<Settings>());
        let task = cx.background_executor().spawn(async move { git::autofetch(host.as_ref(), &root, opts) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.vcs.sync.running = None;
                if !this.git_context_matches(&source_host, &source_root) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(()) => {
                        this.vcs.sync.autofetch_error = None;
                        this.refresh_git(0, cx);
                    }
                    Err(e) => {
                        this.vcs.sync.autofetch_error = Some(AutofetchError {
                            host: std::sync::Arc::downgrade(&source_host),
                            root: source_root,
                            message: first_line(&e).to_string().into(),
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        true
    }

    /// Runs `autofetch_now` once the period has passed. Like `poll_git`, it only runs while the window is in front.
    pub(super) fn poll_autofetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let mut waited = 0u64;
            loop {
                cx.background_executor().timer(AUTOFETCH_TICK).await;
                waited += AUTOFETCH_TICK.as_secs();
                let ok = this
                    .update_in(cx, |this, window, cx| {
                        let (on, period) = {
                            let s = cx.global::<Settings>();
                            (s.git_autofetch, u64::from(s.git_autofetch_period.max(30)))
                        };
                        // Off: start counting again from the moment it is turned on. Due but blocked (window in the
                        // background, a sync running): keep the count, so it fetches as soon as it can.
                        if !on || (waited >= period && this.autofetch_now(window.is_window_active() || cfg!(test), cx)) {
                            waited = 0;
                        }
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        })
        .detach();
    }

    #[cfg(test)]
    pub fn sync_for_test(&mut self, ops: Vec<SyncOp>, cx: &mut Context<Self>) {
        self.git_sync(ops, cx);
    }

    #[cfg(test)]
    pub fn autofetch_for_test(&mut self, window_active: bool, cx: &mut Context<Self>) -> bool {
        self.autofetch_now(window_active, cx)
    }

    #[cfg(test)]
    pub fn sync_tooltip_for_test(&self) -> String {
        self.sync_tooltip().to_string()
    }

    #[cfg(test)]
    pub fn sync_running_for_test(&self) -> bool {
        self.vcs.sync.running.is_some()
    }

    #[cfg(test)]
    pub fn autofetch_error_for_test(&self) -> Option<String> {
        self.autofetch_error().map(|e| e.to_string())
    }
}
