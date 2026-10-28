//! Source control panel (S06): staged / unstaged changes,
//! stage, unstage, discard change, commit. Commands go through `git/` on the agent.

use std::sync::Arc;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, PromptLevel, SharedString, Subscription, UniformListScrollHandle,
    Window, div, prelude::*, px,
};
use host::Host;

use crate::git::{self, Change, Mark, Status};
use crate::graph::{self, CommitDetail, CommitOp, FileChange, GraphCommit, GraphRow, RefKind};
use crate::input::{FieldEvent, TextField};
use crate::sections::{self, Sections};
use crate::ui::{self, c, icon, layout, palette, text};

pub enum ScmEvent {
    /// An operation finished: refresh the state.
    Refresh,
    /// Show diff: index ↔ HEAD if `staged`, otherwise working tree ↔ index.
    OpenDiff {
        path: String,
        staged: bool,
    },
    /// Open another worktree on the same host, keeping the normal unsaved-file guard.
    OpenWorktree {
        root: String,
        path: String,
    },
    AddWorktree,
    /// The "…" in the header: more git operations (window coordinates).
    MoreActions(gpui::Point<gpui::Pixels>),
    /// The "…" in the GRAPH header: view, filters and history actions (window coordinates).
    GraphMenu(gpui::Point<gpui::Pixels>),
    /// The file list (picker) for a commit in the graph was requested.
    OpenCommit(GraphCommit),
    /// Open one of a commit's files as a diff: (commit, file).
    OpenCommitFile(GraphCommit, FileChange),
    /// Right click on a commit row: operation menu.
    CommitMenu(GraphCommit, gpui::Point<gpui::Pixels>),
    /// Section layout changed (persisted).
    SectionsChanged(String),
    /// Right click on a changed file: its menu (path relative to the repository, window coordinates).
    FileMenu(String, bool, gpui::Point<gpui::Pixels>),
    // --- git-sync ---
    /// The Sync Changes button: pull and/or push, asking first when `git.confirmSync` is on.
    Sync,
    /// The Publish Branch button: the branch has no upstream yet.
    Publish,
    /// A commit went through and `git.postCommitCommand` (or "Commit & Push/Sync") asks for this next.
    PostCommit(git::PostCommit),
}

#[derive(Clone, Copy)]
pub enum FileAction {
    Open,
    Diff(bool),
    Stage,
    Unstage,
    Discard,
}

/// What the loaded graph depends on besides HEAD: the checked-out branch (a new branch on the same commit moves the HEAD
/// label) and every other ref (a fetch, a push, a new tag move labels without moving HEAD).
#[derive(Clone, Debug, PartialEq, Eq)]
struct GraphKey {
    root: String,
    head: Option<String>,
    branch: Option<String>,
    refs: u64,
}

impl GraphKey {
    fn of(st: &Status) -> GraphKey {
        GraphKey { root: st.root.clone(), head: st.head.clone(), branch: st.branch.clone(), refs: st.refs_stamp }
    }
}

/// Graph: commits and the lane layout for each.
pub struct GraphData {
    commits: Vec<GraphCommit>,
    rows: Vec<GraphRow>,
}

pub struct ScmPanel {
    host: Arc<dyn Host>,
    status: Option<Arc<Status>>,
    message: Entity<TextField>,
    busy: bool,
    /// Result of the last operation: (text, is error).
    result: Option<(SharedString, bool)>,
    focus_handle: FocusHandle,
    graph: Option<Arc<GraphData>>,
    graph_loading: bool,
    /// State the graph was loaded for. Reloads when it changes.
    graph_key: Option<GraphKey>,
    graph_error: Option<SharedString>,
    graph_scroll: UniformListScrollHandle,
    /// Open/closed state and heights of the sections (Changes, Stashes, Graph, Details).
    sections: Sections,
    scope: graph::Scope,
    search_open: bool,
    /// GRAPH view and filters: the selection is per session; if none is chosen, the `git.graph.view` setting (read live).
    graph_view: Option<graph::GraphView>,
    only_mine: bool,
    hide_merges: bool,
    query: Entity<TextField>,
    /// Selected commit in the graph and its detail (until loaded, `detail` is stale or empty).
    selected: Option<GraphCommit>,
    detail: Option<Arc<CommitDetail>>,
    detail_error: Option<SharedString>,
    detail_task: Option<gpui::Task<()>>,
    stashes: Vec<(String, String)>,
    worktrees: Box<worktree_ui::Worktrees>,
    /// Identity that will author commits (name, email); `None` if not read yet.
    identity: Option<(Option<String>, Option<String>)>,
    // --- git-sync ---
    /// A sync is running (the workbench owns the guard; the button only shows it).
    syncing: bool,
    /// The repository has a remote: without one there is nothing to publish to, so no Publish Branch button.
    has_remote: bool,
    /// Row tooltips must not cover a context menu opened above them.
    menu_open: bool,
    _subs: Vec<Subscription>,
}

#[path = "scm_graph.rs"]
mod graph_ui;
pub use graph_ui::GraphOption;

#[path = "scm_access.rs"]
mod access;
pub(crate) use access::AccessView;

#[path = "scm_worktrees.rs"]
mod worktree_ui;

impl EventEmitter<ScmEvent> for ScmPanel {}

impl Focusable for ScmPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ScmPanel {
    pub fn new(host: Arc<dyn Host>, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| TextField::new(crate::i18n::tr("Message (↵ to commit)"), cx).multiline());
        let subs = vec![cx.subscribe(&message, |this, _, ev: &FieldEvent, cx| {
            if matches!(ev, FieldEvent::Submit) {
                this.commit(cx);
            }
        })];
        let query = cx.new(|cx| TextField::new(crate::i18n::tr("Search commits (message, author, hash)"), cx));
        let mut subs = subs;
        subs.push(cx.observe(&query, |_, _, cx| cx.notify()));
        ScmPanel {
            host,
            status: None,
            message,
            busy: false,
            result: None,
            focus_handle: cx.focus_handle(),
            graph: None,
            graph_loading: false,
            graph_key: None,
            graph_error: None,
            graph_scroll: UniformListScrollHandle::new(),
            sections: Sections::default(),
            scope: if cx.try_global::<crate::settings::Settings>().is_none_or(|s| s.graph_all_branches) { graph::Scope::All } else { graph::Scope::Current },
            search_open: false,
            graph_view: None,
            only_mine: false,
            hide_merges: false,
            query,
            selected: None,
            detail: None,
            detail_error: None,
            detail_task: None,
            stashes: Vec::new(),
            worktrees: Default::default(),
            identity: None,
            syncing: false,
            has_remote: false,
            menu_open: false,
            _subs: subs,
        }
    }

    pub fn set_status(&mut self, host: Arc<dyn Host>, status: Option<Arc<Status>>, cx: &mut Context<Self>) {
        if !Arc::ptr_eq(&self.host, &host) || self.status.as_ref().map(|s| &s.root) != status.as_ref().map(|s| &s.root) {
            self.worktrees.reset();
        }
        self.host = host;
        self.status = status;
        // Commit, branch switch, pull: the graph refreshes when HEAD or the repo changes.
        let key = self.status.as_deref().map(GraphKey::of);
        if key != self.graph_key {
            self.graph_key = key;
            self.load_graph(cx);
        }
        cx.notify();
    }

    pub fn set_menu_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.menu_open != open {
            self.menu_open = open;
            cx.notify();
        }
    }

    /// Load the graph in the background; the old result stays visible until the new one arrives.
    pub fn load_graph(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.status.as_ref().map(|s| s.root.clone()) else {
            self.graph = None;
            self.graph_error = None;
            return;
        };
        if self.graph_loading {
            // The state changed while loading: it reloads when done.
            self.graph_key = None;
            return;
        }
        self.graph_loading = true;
        let host = self.host.clone();
        let scope = self.scope;
        let limit = cx.global::<crate::settings::Settings>().graph_limit;
        let task = cx.background_executor().spawn(async move {
            let log = graph::log(host.as_ref(), &root, limit, scope);
            let has_remote = git::remotes(host.as_ref(), &root).is_ok_and(|r| !r.is_empty());
            (log, git::identity(host.as_ref(), &root), git::stash_list(host.as_ref(), &root).unwrap_or_default(), has_remote)
        });
        cx.spawn(async move |this, cx| {
            let (result, identity, stashes, has_remote) = task.await;
            this.update(cx, |this, cx| {
                this.identity = Some(identity);
                this.has_remote = has_remote;
                this.stashes = stashes;
                this.graph_loading = false;
                match result {
                    Ok(commits) => {
                        let mut rows = graph::layout(&commits);
                        graph::cap_lanes(&mut rows, graph::MAX_COLS);
                        this.graph = Some(Arc::new(GraphData { commits, rows }));
                        this.graph_error = None;
                    }
                    Err(e) => this.graph_error = Some(e.lines().next().unwrap_or("git log failed").to_string().into()),
                }
                // If the state changed during loading (key was reset), load again.
                let now = this.status.as_deref().map(GraphKey::of);
                if this.graph_key.is_none() && now.is_some() {
                    this.graph_key = now;
                    this.load_graph(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    #[cfg(test)]
    pub fn identity_for_test(&self) -> Option<(Option<String>, Option<String>)> {
        self.identity.clone()
    }

    #[cfg(test)]
    pub fn graph_subjects(&self) -> Vec<String> {
        self.graph.as_ref().map(|g| g.commits.iter().map(|c| c.subject.clone()).collect()).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn graph_cols(&self) -> usize {
        self.graph.as_ref().map_or(0, |g| g.rows.iter().map(|r| r.width).max().unwrap_or(1))
    }

    #[cfg(test)]
    pub fn graph_commit(&self, ix: usize) -> Option<GraphCommit> {
        self.graph.as_ref().and_then(|g| g.commits.get(ix).cloned())
    }

    pub fn focus_message(&self, window: &mut Window, cx: &App) {
        window.focus(&self.message.focus_handle(cx));
    }

    #[cfg(test)]
    pub fn set_message(&mut self, text: &str, cx: &mut Context<Self>) {
        self.message.update(cx, |m, cx| m.set_text(text, cx));
    }

    #[cfg(test)]
    pub fn message_text(&self, cx: &App) -> String {
        self.message.read(cx).text().to_string()
    }

    #[cfg(test)]
    pub fn result_text(&self) -> Option<String> {
        self.result.as_ref().map(|(s, _)| s.to_string())
    }

    #[cfg(test)]
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// A git operation in the background; when done the result is written and the state refreshed.
    fn run(&mut self, op: impl FnOnce(&dyn Host, &str) -> Result<String, String> + Send + 'static, cx: &mut Context<Self>) {
        self.run_then(op, |_, _| {}, cx);
    }

    /// `on_ok`: if the operation succeeded (e.g. clear the message after a commit).
    fn run_then(
        &mut self,
        op: impl FnOnce(&dyn Host, &str) -> Result<String, String> + Send + 'static,
        on_ok: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.status.as_ref().map(|s| s.root.clone()) else { return };
        if self.busy {
            return;
        }
        self.busy = true;
        self.result = None;
        let host = self.host.clone();
        let task = cx.background_executor().spawn(async move { op(host.as_ref(), &root) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                if result.is_ok() {
                    on_ok(this, cx);
                }
                this.result = match result {
                    Ok(msg) if msg.is_empty() => None,
                    Ok(msg) => Some((msg.into(), false)),
                    Err(e) => Some((e.into(), true)),
                };
                // Operations that don't change HEAD, like fetch/pull, can still move the branch tip.
                this.load_graph(cx);
                cx.emit(ScmEvent::Refresh);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Staging also means marking a conflict "resolved": refuse if markers are still inside.
    pub fn stage(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let conflicted: Vec<String> = self
            .status
            .as_ref()
            .map(|st| st.changes.iter().filter(|c| c.unstaged == Some(Mark::Conflict) && paths.contains(&c.path)).map(|c| c.path.clone()).collect())
            .unwrap_or_default();
        self.run(
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
            cx,
        );
    }

    pub fn unstage(&mut self, mut paths: Vec<String>, cx: &mut Context<Self>) {
        // Rename: the old path is also unstaged (as deleted).
        if let Some(st) = &self.status {
            let origs: Vec<String> = st.changes.iter().filter(|c| paths.contains(&c.path)).filter_map(|c| c.orig.clone()).collect();
            paths.extend(origs);
        }
        self.run(move |h, root| git::unstage(h, root, &paths).map(|_| String::new()), cx);
    }

    /// Irreversible: ask first (untracked files are deleted).
    pub fn confirm_discard(&mut self, changes: Vec<Change>, window: &mut Window, cx: &mut Context<Self>) {
        let deletes = changes.iter().any(|c| c.unstaged == Some(Mark::Untracked));
        let title = match changes.as_slice() {
            [one] => crate::i18n::tf("Discard changes in {}?", &[&format!("“{}”", ui::file_name(&one.path))]),
            many => crate::i18n::tf("Discard changes in {} files?", &[&many.len()]),
        };
        let detail = if deletes { crate::i18n::t("Untracked files will be deleted. This can't be undone.") } else { crate::i18n::t("This can't be undone.") };
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(detail),
            &[gpui::PromptButton::cancel(crate::i18n::t("Cancel")), gpui::PromptButton::new(crate::i18n::t("Discard"))],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(1) {
                this.update(cx, |this, cx| this.discard(changes, cx)).ok();
            }
        })
        .detach();
    }

    pub fn discard(&mut self, changes: Vec<Change>, cx: &mut Context<Self>) {
        self.run(move |h, root| git::discard(h, root, &changes).map(|_| String::new()), cx);
    }

    /// "Discard All Changes": everything not staged (asks first).
    pub fn confirm_discard_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Conflicted files are not discarded (checkout fails on an unmerged file and the resolution would be lost).
        let changes: Vec<Change> =
            self.status.as_ref().map(|s| s.unstaged().filter(|c| c.unstaged != Some(Mark::Conflict)).cloned().collect()).unwrap_or_default();
        if changes.is_empty() {
            self.result = Some((crate::i18n::t("No changes to discard").to_string().into(), false));
            cx.notify();
            return;
        }
        self.confirm_discard(changes, window, cx);
    }

    /// Amend the last commit: staged changes are added; if the box is empty the message stays the same.
    pub fn commit_amend(&mut self, cx: &mut Context<Self>) {
        let message = self.message.read(cx).text().trim().to_string();
        self.run_then(
            move |h, root| {
                git::commit(h, root, &message, git::CommitOptions { amend: true, ..Default::default() })
                    .map(|summary| crate::i18n::tf("Amended {}", &[&summary]))
            },
            |this, cx| this.message.update(cx, |m, cx| m.set_text("", cx)),
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
            move |this, cx| {
                let m = std::mem::take(&mut *message.lock().unwrap_or_else(|e| e.into_inner()));
                this.message.update(cx, |f, cx| f.set_text(&m, cx));
            },
            cx,
        );
    }

    pub fn stash(&mut self, untracked: bool, cx: &mut Context<Self>) {
        self.run(move |h, root| git::stash(h, root, untracked).map(|_| crate::i18n::t("Stashed changes").to_string()), cx);
    }

    pub fn stash_apply(&mut self, stash: String, pop: bool, cx: &mut Context<Self>) {
        self.run(
            move |h, root| {
                git::stash_apply(h, root, &stash, pop)
                    .map(|_| if pop { crate::i18n::tf("Popped {}", &[&stash]) } else { crate::i18n::tf("Applied {}", &[&stash]) })
            },
            cx,
        );
    }

    pub fn stash_drop(&mut self, stash: String, cx: &mut Context<Self>) {
        self.run(move |h, root| git::stash_drop(h, root, &stash).map(|_| crate::i18n::tf("Dropped {}", &[&stash])), cx);
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

    fn commit_with(&mut self, flags: git::CommitOptions, then: Option<git::PostCommit>, cx: &mut Context<Self>) {
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
        // The message is cleared only when the commit succeeds (if a hook rejects, the user fixes it and retries).
        self.run_then(
            move |h, root| git::commit(h, root, &message, opts).map(|summary| crate::i18n::tf("Committed {}", &[&summary])),
            move |this, cx| {
                this.message.update(cx, |m, cx| m.set_text("", cx));
                if let Some(then) = then {
                    cx.emit(ScmEvent::PostCommit(then));
                }
            },
            cx,
        );
    }

    /// The workbench runs the syncs; the button shows whether one is running.
    pub fn set_syncing(&mut self, syncing: bool, cx: &mut Context<Self>) {
        if self.syncing != syncing {
            self.syncing = syncing;
            cx.notify();
        }
    }

    fn row(&self, ix: usize, change: &Change, staged: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *palette(cx);
        let mark = if staged { change.staged } else { change.unstaged }.unwrap_or(Mark::Modified);
        let name = ui::file_name(&change.path).to_string();
        let dir = change.path.rsplit_once('/').map(|(d, _)| d.to_string());
        let group: SharedString = format!("scm-row-{staged}-{ix}").into();
        let path = change.path.clone();
        let menu_path = path.clone();
        let action = |id: String, glyph: &'static str, tip: &'static str| {
            div()
                .id(SharedString::from(id))
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .hover(|d| d.bg(c(p.surface_active)))
                .child(icon(glyph, 14., p.text_muted))
                .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.into())).into())
        };
        let actions = if staged {
            let path = path.clone();
            div().child(action(format!("unstage-{ix}"), "minus", crate::i18n::t("Unstage")).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.unstage(vec![path.clone()], cx);
            })))
        } else {
            let (p1, ch) = (path.clone(), change.clone());
            let conflict = change.unstaged == Some(Mark::Conflict);
            div()
                .flex()
                .when(!conflict, |d| {
                    d.child(action(format!("discard-{ix}"), "undo-2", crate::i18n::t("Discard changes")).on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.confirm_discard(vec![ch.clone()], window, cx);
                    })))
                })
                .child(
                    action(
                        format!("stage-{ix}"),
                        if conflict { "check" } else { "plus" },
                        if conflict { crate::i18n::t("Mark as resolved (stage)") } else { crate::i18n::t("Stage") },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.stage(vec![p1.clone()], cx);
                    })),
                )
        };
        div()
            .id(SharedString::from(format!("scm-{staged}-{ix}")))
            .group(group.clone())
            .w_full()
            .h(px(layout::ROW_H))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .pl(px(theme::SPACE[6]))
            .pr(px(theme::SPACE[4]))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_hover)))
            .child(crate::icon_theme::file_icon_el(&name, layout::ICON, &p))
            .child(div().flex_shrink().min_w_0().overflow_hidden().text_color(c(mark.color(&p))).when(mark == Mark::Deleted, |d| d.line_through()).child(name))
            .child(div().flex_1().min_w_0().overflow_hidden().text_size(px(text::XS.0)).text_color(c(p.text_muted)).children(dir))
            .child(div().invisible().group_hover(group, |s| s.visible()).child(actions))
            .child(div().w(px(12.)).flex_none().text_size(px(text::XS.0)).text_color(c(mark.color(&p))).child(mark.letter()))
            .when(!self.menu_open, |d| {
                d.tooltip({
                    let tip = format!("{} · {}", change.path, mark.describe());
                    move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.clone().into())).into()
                })
            })
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ScmEvent::OpenDiff { path: path.clone(), staged })))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_, ev: &MouseDownEvent, _, cx| cx.emit(ScmEvent::FileMenu(menu_path.clone(), staged, ev.position))),
            )
    }

    fn group_header(&self, id: &'static str, title: &str, n: usize, staged: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *palette(cx);
        let paths: Vec<String> = match &self.status {
            Some(st) if staged => st.staged().map(|c| c.path.clone()).collect(),
            Some(st) => {
                let merge = id == "scm-merge";
                st.unstaged().filter(|c| (c.unstaged == Some(Mark::Conflict)) == merge).map(|c| c.path.clone()).collect()
            }
            None => Vec::new(),
        };
        let (glyph, tip) = match (staged, id == "scm-merge") {
            (true, _) => ("minus", crate::i18n::t("Unstage all")),
            (false, true) => ("check", crate::i18n::t("Mark all as resolved (stage)")),
            (false, false) => ("plus", crate::i18n::t("Stage all")),
        };
        let hover_button = |bid: String, glyph: &'static str, tip: &'static str| {
            div()
                .id(SharedString::from(bid))
                .invisible()
                .group_hover(id, |s| s.visible())
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .hover(|d| d.bg(c(p.surface_active)))
                .child(icon(glyph, 14., p.text_muted))
                .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.into())).into())
        };
        div()
            .id(id)
            .group(id)
            .w_full()
            .h(px(layout::ROW_H))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .px(px(theme::SPACE[4]))
            .text_size(px(text::XS.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(c(p.text_muted))
            .cursor_pointer()
            .child(icon(if self.sections.is_open(id, true) { "chevron-down" } else { "chevron-right" }, 14., p.text_muted))
            .child(div().flex_1().child(title.to_string()))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(id, cx)))
            .when(!staged && n > 0 && id != "scm-merge", |d| {
                d.child(hover_button(format!("{id}-discard"), "undo-2", crate::i18n::t("Discard all changes")).on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.confirm_discard_all(window, cx);
                })))
            })
            .child(hover_button(format!("{id}-all"), glyph, tip).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if staged {
                    this.unstage(paths.clone(), cx);
                } else {
                    this.stage(paths.clone(), cx);
                }
            })))
            .when(n > 0, |d| {
                d.child(div().px(px(theme::SPACE[2])).rounded(px(theme::radius::LG)).bg(c(p.surface_active)).text_color(c(p.text)).child(n.to_string()))
            })
    }
}

impl Render for ScmPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.worktrees.loaded {
            self.refresh_worktrees(cx);
        }
        let p = *palette(cx);
        let header = div()
            .h(px(layout::VIEW_HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .px(px(theme::SPACE[6]))
            .text_size(px(text::XXS.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(c(p.text_muted))
            .child(div().flex_1().child(crate::i18n::upper(crate::i18n::t("Source Control"))))
            .child(
                div()
                    .id("scm-refresh")
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::radius::SM))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(if self.busy {
                        ui::spinner("scm-busy", 14., p.text_muted).into_any_element()
                    } else {
                        icon("refresh-cw", 14., p.text_muted).into_any_element()
                    })
                    .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Refresh").into())).into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        // A remote added since the graph was read shows up here (HEAD did not move, so nothing else reloads it).
                        this.load_graph(cx);
                        this.refresh_worktrees(cx);
                        cx.emit(ScmEvent::Refresh);
                    })),
            )
            .child(
                div()
                    .id("scm-more")
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::radius::SM))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(icon("ellipsis", 14., p.text_muted))
                    .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("More Actions…").into())).into())
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|_, ev: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            cx.emit(ScmEvent::MoreActions(ev.position));
                        }),
                    ),
            );
        let Some(st) = self.status.clone() else {
            return div().key_context("ScmPanel").track_focus(&self.focus_handle).size_full().flex().flex_col().child(header).child(
                div()
                    .px(px(theme::SPACE[6]))
                    .text_size(px(text::SM.0))
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::t("This folder is not a git repository.")),
            );
        };
        let staged: Vec<Change> = st.staged().cloned().collect();
        // Conflicts get their own section (VS Code: "Merge Changes"); staging means "resolved".
        let (merge, unstaged): (Vec<Change>, Vec<Change>) = st.unstaged().cloned().partition(|c| c.unstaged == Some(Mark::Conflict));
        let (commit_label, can_commit) = self.commit_state(&st, cx);
        let sync_button = self.sync_button(&st, &p, cx);
        let commit_label: SharedString = commit_label.into();
        // Latest commit in the repo: so the panel visibly shows it really read the repo.
        let last = self.graph.as_ref().and_then(|g| g.commits.iter().find(|c| c.refs.iter().any(|r| r.kind == RefKind::Head)).cloned());
        let branch = match (&st.branch, st.ahead, st.behind) {
            (Some(b), 0, 0) => b.clone(),
            (Some(b), a, d) => format!("{b}  {}", git::counts_label(a, d)),
            (None, ..) => crate::i18n::t("detached HEAD").into(),
        };
        let graph_fills =
            staged.is_empty() && merge.is_empty() && unstaged.is_empty() && self.sections.is_open("graph", true) && !self.sections.has_height("graph");
        let field_box = ui::input_box("scm-message-box", &p, p.bg, self.message.focus_handle(cx).is_focused(window)).child(self.message.clone());
        div()
            .key_context("ScmPanel")
            .track_focus(&self.focus_handle)
            .tab_group()
            .on_key_down(|event: &gpui::KeyDownEvent, window, cx| {
                let modifiers = event.keystroke.modifiers;
                if event.keystroke.key == "tab" && !modifiers.control && !modifiers.alt && !modifiers.platform && !modifiers.function {
                    if modifiers.shift {
                        window.focus_prev();
                    } else {
                        window.focus_next();
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                }
            })
            .overflow_hidden()
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, _, cx| {
                if !this.sections.dragging() {
                    return;
                }
                if ev.pressed_button != Some(gpui::MouseButton::Left) {
                    // Button was released elsewhere: consider the drag finished.
                    if this.sections.end_drag() {
                        this.emit_sections(cx);
                    }
                } else if this.sections.drag_to(ev.position.y / px(1.), 700.) {
                    cx.notify();
                }
            }))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.sections.end_drag() {
                    this.emit_sections(cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(text::SM.0))
            .child(header)
            .child(
                div()
                    .px(px(theme::SPACE[6]))
                    .pb(px(theme::SPACE[3]))
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[3]))
                    // Single row: branch (ahead/behind counts) on the left, identity icon on the right (name and email in the tooltip).
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[3]))
                            .text_size(px(text::XS.0))
                            .child(icon("git-branch", 14., p.text_muted))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(c(p.text)).child(branch))
                            .when_some(self.identity.clone(), |d, (name, email)| match (name, email) {
                                (Some(n), Some(e)) => {
                                    let tip: SharedString = crate::i18n::tf("Commits as {0} <{1}>", &[&n, &e]).into();
                                    d.child(
                                        div()
                                            .id("scm-identity")
                                            .flex_none()
                                            .child(avatar(&n, 16., &p))
                                            .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.clone())).into()),
                                    )
                                }
                                _ => d.child(
                                    div()
                                        .id("scm-identity")
                                        .flex_none()
                                        .child(icon("alert", 14., p.warning))
                                        .tooltip(|_, cx| {
                                            cx.new(|_| {
                                                crate::workbench::Tooltip(
                                                    crate::i18n::t("Git identity is not set on this host: commits will be refused. Set user.name and user.email (git config --global …).").into(),
                                                )
                                            })
                                            .into()
                                        }),
                                ),
                            }),
                    )
                    .when_some(last, |d, lc| {
                        d.child(
                            div()
                                .flex()
                                .gap(px(theme::SPACE[2]))
                                .text_size(px(text::XS.0))
                                .text_color(c(p.text_muted))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .items_center()
                                .child(icon("git-commit", 14., p.text_muted))
                                .child(div().flex_none().font_family(ui::MONO_FONT).text_color(c(p.accent)).child(lc.short.clone()))
                                .child(div().flex_1().min_w_0().overflow_hidden().child(lc.subject.clone())),
                        )
                    })
                    .child(field_box)
                    .child(
                        div()
                            .id("scm-commit")
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(theme::SPACE[2]))
                            .rounded(px(theme::radius::SM))
                            .bg(c(p.accent))
                            .text_color(c(p.on_accent))
                            .cursor_pointer()
                            .when(!can_commit, |d| d.opacity(0.5))
                            .hover(|d| d.opacity(0.9))
                            .child(icon("check", 14., p.on_accent))
                            .child(commit_label)
                            .on_click(cx.listener(|this, _, _, cx| this.commit(cx))),
                    )
                    .children(sync_button)
                    .when_some(self.result.clone(), |d, (msg, err)| {
                        d.child(div().text_size(px(text::XS.0)).text_color(c(if err { p.danger } else { p.text_muted })).child(msg))
                    }),
            )
            .child(
                div()
                    .id("scm-list")
                    // Nothing to commit: the list takes only its line and the graph gets the room (until its height is set by hand).
                    .when(!graph_fills, |d| d.flex_1())
                    .min_h(px(if graph_fills { 0. } else { 72. }))
                    .overflow_y_scroll()
                    .when(!merge.is_empty(), |d| {
                        let rows: Vec<_> = if self.sections.is_open("scm-merge", true) { merge.iter().enumerate().map(|(i, ch)| self.row(1000 + i, ch, false, cx).into_any_element()).collect() } else { Vec::new() };
                        d.child(self.group_header("scm-merge", crate::i18n::t("Merge Changes"), merge.len(), false, cx)).children(rows)
                    })
                    .when(!staged.is_empty(), |d| {
                        let rows: Vec<_> = if self.sections.is_open("scm-staged", true) { staged.iter().enumerate().map(|(i, ch)| self.row(i, ch, true, cx).into_any_element()).collect() } else { Vec::new() };
                        d.child(self.group_header("scm-staged", crate::i18n::t("Staged Changes"), staged.len(), true, cx)).children(rows)
                    })
                    .child(self.group_header("scm-changes", crate::i18n::t("Changes"), unstaged.len(), false, cx))
                    .children(if self.sections.is_open("scm-changes", true) { unstaged.iter().enumerate().map(|(i, ch)| self.row(i, ch, false, cx).into_any_element()).collect::<Vec<_>>() } else { Vec::new() })
                    .when(staged.is_empty() && unstaged.is_empty() && merge.is_empty(), |d| {
                        d.child(div().px(px(theme::SPACE[6])).py(px(theme::SPACE[3])).text_color(c(p.text_muted)).child(crate::i18n::t("No changes.")))
                    }),
            )
            .children(self.stash_section(cx))
            .child(self.worktree_section(cx))
            .child(self.graph_section(self.query.focus_handle(cx).is_focused(window), graph_fills, cx))
            .children(self.detail_section(cx))
    }
}

/// Commit button's label and whether it can be pressed: "Commit (n)" if something is staged, otherwise
/// "Commit All (n)" for tracked changes, a dimmed "Commit" if there are none.
pub(crate) fn commit_button(staged: usize, changed: usize) -> (String, bool) {
    if staged > 0 {
        (crate::i18n::tf("Commit ({})", &[&staged]), true)
    } else if changed > 0 {
        (crate::i18n::tf("Commit All ({})", &[&changed]), true)
    } else {
        (crate::i18n::t("Commit").into(), false)
    }
}

/// A stable lane color index from the author name.
fn author_color(name: &str) -> usize {
    name.bytes().fold(7usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize))
}

/// Round avatar with the author's initials.
fn avatar(name: &str, size: f32, p: &ui::Palette) -> gpui::Div {
    let initials: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(c(graph::lane_color(p, author_color(name))))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * 0.5))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(c(ui::readable_on(graph::lane_color(p, author_color(name)))))
        .child(initials)
}

/// Change letter → color.
fn status_color(status: char, p: &ui::Palette) -> u32 {
    match status {
        'A' => crate::git::Mark::Added.color(p),
        'D' => crate::git::Mark::Deleted.color(p),
        'R' | 'C' => crate::git::Mark::Renamed.color(p),
        _ => crate::git::Mark::Modified.color(p),
    }
}

/// Commit matches on subject, author, short/full hash.
pub fn commit_matches(c: &GraphCommit, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || c.subject.to_lowercase().contains(&q)
        || c.author.to_lowercase().contains(&q)
        || c.hash.starts_with(&q)
        || c.refs.iter().any(|r| r.name.to_lowercase().contains(&q))
}

impl ScmPanel {
    /// "Sync Changes ↓N ↑M" when the branch has an upstream and differs from it, "Publish Branch" when it has none
    /// (and there is a remote to publish to and a commit to publish).
    fn sync_action(&self, st: &Status) -> Option<(SharedString, &'static str, fn() -> ScmEvent)> {
        match (&st.upstream, st.branch.is_some()) {
            (Some(_), _) if st.ahead > 0 || st.behind > 0 => {
                Some((crate::i18n::tf("Sync Changes {}", &[&git::counts_label(st.ahead, st.behind)]).into(), "refresh-cw", || ScmEvent::Sync))
            }
            (None, true) if self.has_remote && st.head.is_some() => Some((crate::i18n::t("Publish Branch").into(), "cloud-upload", || ScmEvent::Publish)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn has_remote_for_test(&self) -> bool {
        self.has_remote
    }

    #[cfg(test)]
    pub fn sync_label_for_test(&self) -> Option<String> {
        self.status.as_ref().and_then(|st| self.sync_action(st)).map(|(label, ..)| label.to_string())
    }

    fn sync_button(&self, st: &Status, p: &ui::Palette, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let (label, glyph, event) = self.sync_action(st)?;
        let p = *p;
        Some(
            div()
                .id("scm-sync")
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(theme::SPACE[2]))
                .rounded(px(theme::radius::SM))
                .bg(c(p.surface_active))
                .text_color(c(p.text))
                .cursor_pointer()
                .when(self.syncing, |d| d.opacity(0.6))
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(if self.syncing { ui::spinner("scm-sync-busy", 14., p.text).into_any_element() } else { icon(glyph, 14., p.text).into_any_element() })
                .child(label)
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(event()))),
        )
    }

    fn emit_sections(&self, cx: &mut Context<Self>) {
        cx.emit(ScmEvent::SectionsChanged(self.sections.encode()));
    }

    pub fn restore_sections(&mut self, text: &str, cx: &mut Context<Self>) {
        self.sections = Sections::decode(text);
        cx.notify();
    }

    fn toggle_section(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.sections.toggle(id, true);
        self.emit_sections(cx);
        cx.notify();
    }

    fn set_scope(&mut self, scope: graph::Scope, cx: &mut Context<Self>) {
        if self.scope != scope {
            self.scope = scope;
            self.load_graph(cx);
            cx.notify();
        }
    }

    /// A commit was selected in the graph: its detail (message, files, line counts) is read in the background.
    pub fn select_commit(&mut self, commit: GraphCommit, cx: &mut Context<Self>) {
        let Some(root) = self.status.as_ref().map(|s| s.root.clone()) else { return };
        self.sections.set_open("details", true);
        self.detail_error = None;
        if self.selected.as_ref().is_some_and(|c| c.hash == commit.hash) && self.detail.is_some() {
            cx.notify();
            return;
        }
        self.detail = None;
        let hash = commit.hash.clone();
        self.selected = Some(commit);
        let host = self.host.clone();
        let task = cx.background_executor().spawn(async move { graph::commit_detail(host.as_ref(), &root, &hash) });
        self.detail_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(d) if this.selected.as_ref().is_some_and(|c| c.hash == d.hash) => this.detail = Some(Arc::new(d)),
                    Ok(_) => {}
                    Err(e) => this.detail_error = Some(e.lines().next().unwrap_or("git show failed").to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    #[cfg(test)]
    pub fn selected_hash(&self) -> Option<String> {
        self.selected.as_ref().map(|c| c.hash.clone())
    }

    #[cfg(test)]
    pub fn detail_for_test(&self) -> Option<Arc<CommitDetail>> {
        self.detail.clone()
    }

    #[cfg(test)]
    pub fn stash_count(&self) -> usize {
        self.stashes.len()
    }

    #[cfg(test)]
    pub fn set_query_for_test(&mut self, q: &str, cx: &mut Context<Self>) {
        self.search_open = true;
        self.query.update(cx, |f, cx| f.set_text(q, cx));
    }

    #[cfg(test)]
    pub fn visible_subjects(&self, cx: &App) -> Vec<String> {
        let q = self.query.read(cx).text().to_string();
        self.graph.as_ref().map(|g| g.commits.iter().filter(|c| commit_matches(c, &q)).map(|c| c.subject.clone()).collect()).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn set_scope_for_test(&mut self, current: bool, cx: &mut Context<Self>) {
        self.set_scope(if current { graph::Scope::Current } else { graph::Scope::All }, cx);
    }

    pub fn commit_by_hash(&self, hash: &str) -> Option<GraphCommit> {
        self.graph.as_ref().and_then(|g| g.commits.iter().find(|c| c.hash == hash).cloned())
    }

    /// Commit operation; irreversible ones (hard reset) ask first.
    pub fn commit_op(&mut self, hash: String, op: CommitOp, window: &mut Window, cx: &mut Context<Self>) {
        if op == CommitOp::Reset(graph::ResetMode::Hard) {
            let short = hash[..hash.len().min(7)].to_string();
            let answer = window.prompt(
                PromptLevel::Warning,
                &crate::i18n::tf("Hard reset to {}?", &[&short]),
                Some(crate::i18n::t("Commits after it leave this branch and every uncommitted change is lost. This can't be undone.")),
                &[crate::i18n::t("Reset"), crate::i18n::t("Cancel")],
                cx,
            );
            cx.spawn(async move |this, cx| {
                if answer.await == Ok(0) {
                    this.update(cx, |this, cx| this.run_commit_op(hash, op, cx)).ok();
                }
            })
            .detach();
            return;
        }
        self.run_commit_op(hash, op, cx);
    }

    fn run_commit_op(&mut self, hash: String, op: CommitOp, cx: &mut Context<Self>) {
        self.run(move |h, root| graph::run_op(h, root, &hash, &op), cx);
    }

    fn section_height(&self, id: &str, default: f32) -> f32 {
        self.sections.height(id, default)
    }

    /// Handle on the section's top edge: dragging up enlarges it.
    fn top_sash(&self, id: &'static str, p: &ui::Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let h = self.section_height(id, if id == "graph" { 260. } else { 170. });
        sections::sash(SharedString::from(format!("sash-{id}")), p, self.sections.dragging_section(id)).on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.sections.begin_drag(id, ev.position.y / px(1.), h, -1.);
            }),
        )
    }

    /// "GRAPH": history of all branches; two-line commit rows like GitLab.
    /// "STASHES": saved stashes; apply, pop, drop.
    fn stash_section(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if self.stashes.is_empty() {
            return None;
        }
        let p = *palette(cx);
        let open = self.sections.is_open("stashes", false);
        let header =
            sections::header("scm-stash-header", crate::i18n::t("Stashes"), Some(self.stashes.len()), open, &p).on_click(cx.listener(|this, _, _, cx| {
                this.sections.toggle("stashes", false);
                this.emit_sections(cx);
                cx.notify();
            }));
        let rows: Vec<_> = self
            .stashes
            .iter()
            .enumerate()
            .map(|(i, (name, msg))| {
                let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
                let group: SharedString = format!("stash-{i}").into();
                let btn = |id: &'static str, glyph: &'static str, tip: &'static str| {
                    ui::icon_button((id, i), glyph, tip, &p).invisible().group_hover(group.clone(), |s| s.visible())
                };
                div()
                    .id(("scm-stash", i))
                    .group(group.clone())
                    .h(px(layout::ROW_H))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .pl(px(theme::SPACE[6]))
                    .pr(px(theme::SPACE[2]))
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(div().flex_none().font_family(ui::MONO_FONT).text_size(px(text::XXS.0)).text_color(c(p.accent)).child(name.clone()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(text::XS.0)).child(msg.clone()))
                    .child(
                        btn("stash-apply", "undo-2", crate::i18n::t("Apply (keep the stash)"))
                            .on_click(cx.listener(move |this, _, _, cx| this.stash_apply(n1.clone(), false, cx))),
                    )
                    .child(
                        btn("stash-pop", "arrow-up", crate::i18n::t("Pop (apply and drop)"))
                            .on_click(cx.listener(move |this, _, _, cx| this.stash_apply(n2.clone(), true, cx))),
                    )
                    .child(btn("stash-drop", "trash", crate::i18n::t("Drop")).on_click(cx.listener(move |this, _, _, cx| this.stash_drop(n3.clone(), cx))))
            })
            .collect();
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .child(header)
                .when(open, |d| d.child(div().id("scm-stashes").max_h(px(layout::ROW_H * 6.)).overflow_y_scroll().children(rows))),
        )
    }

    /// "COMMIT": the selected commit's message, author, files and line counts.
    fn detail_section(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let commit = self.selected.clone()?;
        let p = *palette(cx);
        let open = self.sections.is_open("details", true);
        let title = crate::i18n::tf("Commit {}", &[&commit.short]);
        let header = sections::header("scm-detail-header", &title, None, open, &p)
            .child(ui::icon_button("scm-detail-close", "x", crate::i18n::t("Close details"), &p).on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.selected = None;
                this.detail = None;
                cx.notify();
            })))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_section("details", cx)));
        let height = self.section_height("details", 170.);
        let hash = commit.hash.clone();
        let body = open.then(|| {
            let mut col = div()
                .id("scm-detail")
                .size_full()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE[3]))
                .px(px(theme::SPACE[5]))
                .py(px(theme::SPACE[3]));
            let head = |col: gpui::Stateful<gpui::Div>, d: Option<&CommitDetail>| {
                let message = d.map_or(commit.subject.clone(), |d| d.message.clone());
                let (subject, rest) = message.split_once('\n').map_or((message.as_str(), ""), |(a, b)| (a, b.trim()));
                col.child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_size(px(text::SM.0)).child(subject.to_string()))
                    .when(!rest.is_empty(), |c0| c0.child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(rest.to_string())))
                    // Single-line meta: author (email and date in the tooltip), time and hash; icons instead of text.
                    .child({
                        let who: SharedString = format!("{} <{}>\n{}", commit.author, commit.email, commit.date).into();
                        div()
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[4]))
                            .text_size(px(text::XS.0))
                            .child(
                                div()
                                    .id("scm-detail-author")
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(theme::SPACE[2]))
                                    .child(avatar(&commit.author, 16., &p))
                                    .child(div().overflow_hidden().whitespace_nowrap().child(commit.author.clone()))
                                    .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(who.clone())).into()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(theme::SPACE[1]))
                                    .text_color(c(p.text_muted))
                                    .child(icon("clock", 12., p.text_muted))
                                    .child(crate::git::short_ago(&commit.when)),
                            )
                            .child(
                                div()
                                    .id("scm-detail-hash")
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(theme::SPACE[1]))
                                    .cursor_pointer()
                                    .font_family(ui::MONO_FONT)
                                    .text_size(px(text::XXS.0))
                                    .text_color(c(p.accent))
                                    .child(icon("git-commit", 12., p.text_muted))
                                    .child(commit.short.clone())
                                    .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Copy the full hash").into())).into())
                                    .on_click({
                                        let h = hash.clone();
                                        move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(h.clone()))
                                    }),
                            )
                    })
            };
            match (&self.detail, &self.detail_error) {
                (_, Some(e)) => {
                    head(col, None).child(div().text_color(c(p.danger)).child(crate::i18n::tf("Could not read the commit: {}", &[&e]))).into_any_element()
                }
                (None, None) => head(col, None).child(div().text_color(c(p.text_muted)).child(crate::i18n::t("Loading…"))).into_any_element(),
                (Some(d), None) => {
                    let (added, deleted) = d.totals();
                    col = head(col, Some(d)).child(
                        div()
                            .flex()
                            .gap(px(theme::SPACE[3]))
                            .text_size(px(text::XS.0))
                            .text_color(c(p.text_muted))
                            .items_center()
                            .child(icon("files", 12., p.text_muted))
                            .child(d.files.len().to_string())
                            .child(div().text_color(c(p.success)).child(format!("+{added}")))
                            .child(div().text_color(c(p.danger)).child(format!("−{deleted}"))),
                    );
                    let rows: Vec<_> = d
                        .files
                        .iter()
                        .enumerate()
                        .map(|(i, f)| {
                            let (entry, file) = (commit.clone(), f.clone());
                            let dir = f.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
                            div()
                                .id(("scm-detail-file", i))
                                .h(px(layout::ROW_H))
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(theme::SPACE[3]))
                                .px(px(theme::SPACE[2]))
                                .rounded(px(theme::radius::SM))
                                .cursor_pointer()
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .child(crate::icon_theme::file_icon_el(ui::file_name(&f.path), 14., &p))
                                .child(div().flex_none().text_size(px(text::SM.0)).child(ui::file_name(&f.path).to_string()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(text::XXS.0))
                                        .text_color(c(p.text_muted))
                                        .child(dir),
                                )
                                .when_some(f.added, |d, a| {
                                    d.child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.success)).child(format!("+{a}")))
                                })
                                .when_some(f.deleted, |d, a| {
                                    d.child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.danger)).child(format!("−{a}")))
                                })
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(12.))
                                        .text_size(px(text::XS.0))
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(c(status_color(f.status, &p)))
                                        .child(f.status.to_string()),
                                )
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ScmEvent::OpenCommitFile(entry.clone(), file.clone()))))
                        })
                        .collect();
                    col.children(rows).into_any_element()
                }
            }
        });
        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink()
                .min_h(px(layout::ROW_H + 4.))
                .child(self.top_sash("details", &p, cx))
                .child(header)
                .when_some(body, |d, body| d.child(div().h(px(height)).flex_shrink().min_h(px(60.)).child(body))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::commit_button;

    #[gpui::test]
    fn discard_confirmation_defaults_to_cancel(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(crate::ui::ActiveTheme(theme::DARK));
            cx.set_prompt_builder(|level, message, detail, buttons, handle, window, cx| {
                assert!(matches!(&buttons[0], gpui::PromptButton::Cancel(label) if label == "Cancel"));
                assert_eq!(buttons[1].label(), "Discard");
                gpui::fallback_prompt_renderer(level, message, detail, buttons, handle, window, cx)
            });
        });
        let (panel, cx) = cx.add_window_view(|_, cx| super::ScmPanel::new(std::sync::Arc::new(host::local()), cx));
        panel.update_in(cx, |panel, window, cx| panel.confirm_discard(Vec::new(), window, cx));
    }

    #[test]
    fn commit_button_says_what_will_be_committed() {
        assert_eq!(commit_button(2, 5), ("Commit (2)".into(), true));
        assert_eq!(commit_button(0, 3), ("Commit All (3)".into(), true));
        assert_eq!(commit_button(0, 0), ("Commit".into(), false));
    }
}
