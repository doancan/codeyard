//! Workbench: Sync Changes. Pull, push, fetch and publish from the status bar, the Source Control view and the commands, the
//! background fetch (`git.autofetch`) and the guard that keeps two network operations from running at once.

use gpui::{AppContext as _, Context, SharedString, Window};
use std::sync::Arc;

use super::git_glue::first_line;
use super::{Notice, NoticeSource, OverlayKind, Workbench};
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
    /// What the user asked for while a background fetch ran (a sync, or a push after a commit): it starts when the fetch is done.
    queued: Option<Vec<SyncOp>>,
    /// Why the last automatic pull did nothing although the branch is behind; shown as the status bar item's tooltip until it
    /// pulls, or finds nothing to pull. Same lifetime rule as `autofetch_error`.
    auto_pull_note: Option<AutofetchError>,
    /// Git commands started from the workbench (checkout, stash, discard…) that have not finished: a sync does not start under them.
    pub(super) local_ops: u32,
    /// Repositories (by root) whose first automatic pull of this session is done: `git.autoPull = "open"` pulls once.
    auto_pulled_on_open: std::collections::HashSet<String>,
}

struct AutofetchError {
    host: std::sync::Weak<dyn host::Host>,
    root: String,
    message: SharedString,
}

/// How often the background fetch looks at the clock; the period itself comes from `git.autofetchPeriod`.
const AUTOFETCH_TICK: std::time::Duration = std::time::Duration::from_secs(5);
/// How soon it looks again while a fetch is due and only waiting for something (the repository's status to be read, a sync to end):
/// what others pushed shows up within a second of the window opening, not after a whole tick.
const AUTOFETCH_EAGER: std::time::Duration = std::time::Duration::from_secs(1);
/// A count of seconds that is past any period: the next tick fetches.
const DUE: u64 = 1 << 32;

/// Automatic pull implies fetching: there is nothing to pull until something was fetched.
fn background_fetch_on(s: &Settings) -> bool {
    s.git_autofetch || s.git_auto_pull != "off"
}

fn options(s: &Settings) -> SyncOptions {
    SyncOptions { rebase: s.git_rebase_when_sync, prune: s.git_prune_on_fetch, tags: s.git_pull_tags, fetch_on_pull: s.git_fetch_on_pull }
}

fn is_pull(op: &SyncOp) -> bool {
    matches!(op, SyncOp::Pull | SyncOp::PullRebase | SyncOp::PullFrom { .. })
}

fn is_push(op: &SyncOp) -> bool {
    matches!(op, SyncOp::Push | SyncOp::PushTo { .. } | SyncOp::ForcePush(_))
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
    if git::credentials_rejected(e) {
        format!("{failed} {}", crate::i18n::t("The server refused the login: check the account and that the token may push."))
    } else if git::needs_credentials(e) {
        format!("{failed} {}", crate::i18n::t("The server wants credentials that were not given: try again and enter them when asked, or set up an SSH key."))
    } else {
        failed
    }
}

impl Workbench {
    /// Pull / push / fetch / publish, several in sequence (sync: pull first, then push). Never two at once.
    pub(super) fn git_sync(&mut self, ops: Vec<SyncOp>, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        let target = root.clone();
        // A pull rewrites the working tree and the index: not while a commit, a stash or a checkout is still at it (the
        // other one would fail on the lock, or worse, stop half way).
        if self.vcs.sync.local_ops > 0 || self.scm.read(cx).is_busy_any() {
            self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("Another Git command is still running: try again when it is done.").into()));
            cx.notify();
            return;
        }
        match self.vcs.sync.running {
            Some(Running::Sync) => {
                self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("A sync is already running.").into()));
                cx.notify();
                return;
            }
            Some(Running::Autofetch) => {
                self.vcs.sync.queued = Some(ops);
                self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("Fetching in the background: the sync starts when it is done.").into()));
                cx.notify();
                return;
            }
            None => {}
        }
        self.vcs.sync.running = Some(Running::Sync);
        self.set_scm_syncing(true, cx);
        self.notify(NoticeSource::App, Notice::Info(working_text(&ops).into()));
        cx.notify();
        let host = self.remote.host.clone();
        let source_host = host.clone();
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
                // Another connection was opened while the sync ran: its result belongs to neither what is shown nor
                // that connection, and must not refresh or keep it.
                if !Arc::ptr_eq(&source_host, &this.remote.host) {
                    return;
                }
                if picked && this.vcs.repos.inventory.by_worktree(&target).is_none() && !this.all_roots(cx).contains(&target) {
                    // The repository left the workspace while its sync ran: its result says nothing about what is shown.
                    this.notify(NoticeSource::App, Notice::Error(crate::i18n::t("Repository changed while the operation was pending.").into()));
                    this.refresh_git(0, cx);
                    cx.notify();
                    return;
                }
                // Another repository is shown now: the old result must not replace its notice.
                if !this.git_context_matches(&source_host, &target) {
                    cx.notify();
                    return;
                }
                // A failure keeps the command's whole output as its details.
                let detail = result.as_ref().err().map(|(_, e)| e.clone());
                let notice = match result {
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
                };
                match detail {
                    Some(detail) => this.notify_detailed(NoticeSource::Git, notice, detail),
                    None => this.notify(NoticeSource::Git, notice),
                }
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
        // The answer applies to the connection and repository the question was about, never to one selected while it
        // was open.
        let root = st.root.clone();
        let host = self.remote.host.clone();
        self.confirm(window, crate::i18n::t("Sync changes?"), &detail, crate::i18n::t("Sync"), cx, move |this, cx| {
            if !Arc::ptr_eq(&host, &this.remote.host) {
                return;
            }
            if this.git_status().map(|s| s.root.clone()) != Some(root.clone()) {
                this.notify(NoticeSource::App, Notice::Error(crate::i18n::t("Repository changed while the operation was pending.").into()));
                cx.notify();
                return;
            }
            this.git_sync(ops, cx)
        });
    }

    /// "Git: Push (Force)…": only after a confirmation that names what is lost. The push is pinned to the commit the remote branch
    /// is at when the question opens (`--force-with-lease=<branch>:<commit>`), so a push by someone else while the question is open
    /// is never overwritten unseen. What a background fetch already brought in is not "unseen", which is why the question counts it.
    pub(super) fn git_push_force(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.git_status() else {
            self.repo_root(cx);
            return;
        };
        let (Some(branch), Some(upstream)) = (st.branch.clone(), st.upstream.clone()) else {
            self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("Only a published branch can be force-pushed: use Publish Branch first.").into()));
            cx.notify();
            return;
        };
        let behind = st.behind as usize;
        let host = self.remote.host.clone();
        let source_host = host.clone();
        let root = st.root.clone();
        let task = cx.background_executor().spawn({
            let root = root.clone();
            async move { git::force_target(host.as_ref(), &root) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let target = task.await;
            this.update_in(cx, |this, window, cx| {
                if !this.git_context_matches(&source_host, &root) {
                    return;
                }
                let target = match target {
                    Ok(target) => target,
                    Err(e) => {
                        this.notify(NoticeSource::App, Notice::Error(crate::i18n::tf("Push failed: {}", &[&first_line(&e)]).into()));
                        cx.notify();
                        return;
                    }
                };
                let lost = if behind == 0 {
                    crate::i18n::t("Commits that exist only on the remote branch are lost for everyone using it.").to_string()
                } else {
                    crate::i18n::tn(
                        "{} commit on the remote branch is not in yours and is lost for everyone using it.",
                        "{} commits on the remote branch are not in yours and are lost for everyone using it.",
                        behind,
                        &[],
                    )
                };
                let detail = format!("{lost} {}", crate::i18n::t("If the remote branch moves while this question is open, git refuses."));
                this.confirm_git(
                    window,
                    &crate::i18n::tf("Force-push {0} to {1}?", &[&branch, &upstream]),
                    &detail,
                    crate::i18n::t("Force Push"),
                    cx,
                    move |this, cx| this.git_sync(vec![SyncOp::ForcePush(target)], cx),
                );
            })
            .ok();
        })
        .detach();
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
                        this.notify(NoticeSource::App, Notice::Error(crate::i18n::tf("Could not list: {}", &[&first_line(&e)]).into()));
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
                    this.notify(NoticeSource::App, Notice::Info(crate::i18n::t("Nothing to choose from.").into()));
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
        if let Some(note) = self.auto_pull_note() {
            tip = format!("{tip}\n{note}");
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
        if !background_fetch_on(cx.global::<Settings>()) || !window_active || self.vcs.sync.running.is_some() {
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
        let pull = match cx.global::<Settings>().git_auto_pull.as_str() {
            "always" => true,
            "open" => !self.vcs.sync.auto_pulled_on_open.contains(&root),
            _ => false,
        };
        let pulled_root = root.clone();
        let task = cx.background_executor().spawn(async move {
            // A prompt that arrives during the periodic fetch is declined: nobody asked for it, and the error says why.
            let _quiet = host::askpass::Quiet::begin();
            let changed = git::autofetch(host.as_ref(), &root, opts)?;
            // The fetch is done: a fast-forward needs no network. Its own failure is its own result, not the fetch's.
            Ok::<_, String>((changed, pull.then(|| git::auto_pull(host.as_ref(), &root))))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.vcs.sync.running = None;
                let queued = this.vcs.sync.queued.take();
                if !this.git_context_matches(&source_host, &source_root) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok((changed, pulled)) => {
                        this.vcs.sync.autofetch_error = None;
                        let moved = pulled.is_some_and(|r| this.note_auto_pull(&source_host, &pulled_root, r));
                        // Nothing new came: the status and the graph on screen are still right, so they are not read again.
                        if changed || moved {
                            this.refresh_git(0, cx);
                        }
                    }
                    Err(e) => {
                        this.vcs.sync.autofetch_error = Some(AutofetchError {
                            host: std::sync::Arc::downgrade(&source_host),
                            root: source_root,
                            message: first_line(&e).to_string().into(),
                        });
                    }
                }
                if let Some(ops) = queued {
                    this.git_sync(ops, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        true
    }

    /// What an automatic pull did: a notice when it moved the branch, a line in the sync tooltip when it had to wait for the
    /// user. `true`: the branch moved, so what the window shows is out of date.
    fn note_auto_pull(&mut self, host: &Arc<dyn host::Host>, root: &str, result: Result<git::AutoPull, String>) -> bool {
        use crate::i18n::{t, tn};
        let wait = |why: &str| Some(crate::i18n::tf("Automatic pull is waiting: {}", &[&why]));
        let (note, moved) = match result {
            Ok(git::AutoPull::Pulled(n)) => {
                self.notify(NoticeSource::App, Notice::Info(tn("Pulled {} new commit.", "Pulled {} new commits.", n as usize, &[]).into()));
                (None, true)
            }
            Ok(git::AutoPull::UpToDate) => (None, false),
            // A branch that is not on a branch or has no upstream is an ordinary state, not something waiting.
            Ok(git::AutoPull::Skipped(git::Skip::DetachedHead | git::Skip::NoUpstream)) => (None, false),
            Ok(git::AutoPull::Skipped(git::Skip::Dirty)) => (wait(t("tracked files have changes")), false),
            Ok(git::AutoPull::Skipped(git::Skip::Diverged)) => (wait(t("this branch has commits the upstream does not")), false),
            Ok(git::AutoPull::Skipped(git::Skip::Busy)) => (wait(t("a merge, rebase, cherry-pick or another git command is running")), false),
            Err(e) => (Some(crate::i18n::tf("Automatic pull failed: {}", &[&first_line(&e)])), false),
        };
        // "open" pulls once a session: it is done once the branch is up to date, not while something kept it waiting.
        if note.is_none() {
            self.vcs.sync.auto_pulled_on_open.insert(root.to_string());
        }
        self.vcs.sync.auto_pull_note = note.map(|message| AutofetchError { host: Arc::downgrade(host), root: root.to_string(), message: message.into() });
        moved
    }

    fn auto_pull_note(&self) -> Option<&SharedString> {
        let note = self.vcs.sync.auto_pull_note.as_ref()?;
        let host = note.host.upgrade()?;
        self.git_context_matches(&host, &note.root).then_some(&note.message)
    }

    /// Runs `autofetch_now` once the period has passed. Like `poll_git`, it only runs while the window is in front.
    pub(super) fn poll_autofetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            // Due from the start, and again whenever the window comes back to the front or another repository comes on screen:
            // the first look at a repository (or the return to it) shows what others pushed meanwhile, as VS Code does,
            // instead of waiting a period.
            let mut waited = DUE;
            let mut was_active = false;
            let mut last_root: Option<String> = None;
            let mut delay = AUTOFETCH_EAGER;
            loop {
                cx.background_executor().timer(delay).await;
                waited = waited.saturating_add(delay.as_secs());
                let next = this.update_in(cx, |this, window, cx| {
                    let (on, period) = {
                        let s = cx.global::<Settings>();
                        (background_fetch_on(s), u64::from(s.git_autofetch_period.max(30)))
                    };
                    let active = window.is_window_active() || cfg!(test);
                    let root = this.git_status().map(|s| s.root.clone());
                    if (active && !was_active) || (root.is_some() && root != last_root) {
                        waited = waited.max(DUE);
                    }
                    was_active = active;
                    last_root = root;
                    // Off: start counting again from the moment it is turned on. Due but blocked (window in the
                    // background, a sync running): keep the count, so it fetches as soon as it can.
                    if !on || (waited >= period && this.autofetch_now(active, cx)) {
                        waited = 0;
                    }
                    // Due and only waiting (the status is not read yet, a sync is running): look again soon.
                    if on && active && waited >= period { AUTOFETCH_EAGER } else { AUTOFETCH_TICK }
                });
                match next {
                    Ok(next) => delay = next,
                    Err(_) => break,
                }
            }
        })
        .detach();
    }

    #[cfg(test)]
    pub fn git_task_for_test(&mut self, f: impl FnOnce(&dyn host::Host) -> Result<String, String> + Send + 'static, cx: &mut Context<Self>) {
        self.git_task("Working…", cx, f);
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
