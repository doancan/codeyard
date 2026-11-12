//! Workbench: reviewing and applying a language server's refactoring (rename, quick fix, a command's edit).
//!
//! The edit is turned into a plan on a background thread (`lsp::refactor::plan`), from the open editors' text and the host's
//! files. Under `editor.refactorPreview` the plan is shown on the Refactor Preview page, or applied at once when it is a change
//! inside one open file. Applying is all or nothing: open editors are changed first (every one checked against the state the plan
//! was made from), then the host's files and file operations run as one transaction (`lsp::refactor::apply`); if it fails, the
//! editors' changes are taken back too.
//!
//! Undo: each open editor's change is one undo step (the editor's own undo works, and so does the page's Undo button, which is
//! refused for a file typed into since); each file written on the host is written back if it is still what the refactoring
//! wrote. File operations (create, rename, delete) are not undone: a refactoring that has any offers no Undo.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::WeakEntity;

use super::*;
use crate::i18n::t;
use crate::lsp::refactor::apply::{self, Fs, Step, Transaction};
use crate::lsp::refactor::edit;
use crate::lsp::refactor::env::HostEnv;
use crate::lsp::refactor::plan::{self, Chosen, OpenDoc, PlanError, RefactorPlan};
use crate::lsp::refactor::preview::{RefactorEvent, RefactorPreview};
use crate::replace_apply::{Outcome, Report, UndoDisk};

/// What runs after the edit was applied (`true`) or turned down (`false`): a code action's command, or the answer to the server.
pub(crate) type Done = Box<dyn FnOnce(&mut Workbench, bool, &mut Context<Workbench>)>;

/// Where an edit came from: its title, the server that made it and the state of each open document that server was told.
pub(super) struct EditSource {
    pub(super) title: String,
    pub(super) client: Option<Arc<crate::lsp::Client>>,
    /// Path -> (the editor's buffer version, the document version sent to the server) when the request was sent.
    pub(super) snapshot: HashMap<String, (u64, i64)>,
}

/// An open editor changed by a refactoring: undo takes back that one step, only if nothing was typed since.
struct UndoBuffer {
    path: String,
    editor: WeakEntity<EditorView>,
    after: crate::editor::DocIdentity,
}

/// The last applied refactoring, for the page's Undo.
struct Batch {
    host: Arc<dyn Host>,
    root: String,
    view: WeakEntity<RefactorPreview>,
    disk: Vec<UndoDisk>,
    buffers: Vec<UndoBuffer>,
}

/// The plan under review. The page is held weakly: closing its tab (or opening another page in its place) drops it, and
/// that answers the edit's `done` with `false`.
struct Review {
    view: WeakEntity<RefactorPreview>,
    plan: Arc<RefactorPlan>,
    host: Arc<dyn Host>,
    done: Option<Done>,
}

#[derive(Default)]
pub(super) struct RefactorState {
    review: Option<Review>,
    /// The page's events; kept after the apply, for the report's Undo and Close.
    page_sub: Option<Subscription>,
    /// Told when the page is dropped.
    release_sub: Option<Subscription>,
    /// What had the keyboard before the page opened (the editor the rename was made in): it gets it back when the page closes.
    return_focus: Option<gpui::FocusHandle>,
    batch: Option<Batch>,
    /// Stops the transaction that is running (the page's Stop).
    cancel: Option<Arc<AtomicBool>>,
    /// What the server was told of each open document when the quick fixes now listed were asked for.
    pub(super) listed: HashMap<String, (u64, i64)>,
    /// Tests write through a host that injects failures.
    #[cfg(test)]
    pub(super) fs: Option<Arc<dyn Fs>>,
}

impl Workbench {
    /// Before a request whose answer may edit many files: the state of each open document the server was told.
    pub(super) fn refactor_snapshot(&mut self, client: &Arc<crate::lsp::Client>, cx: &mut Context<Self>) -> HashMap<String, (u64, i64)> {
        let mut out = HashMap::new();
        for ed in self.all_editors() {
            let attached = ed.read(cx).lsp_client().is_some_and(|c| Arc::ptr_eq(c, client));
            if attached && let Some(state) = ed.update(cx, |e, _| e.lsp_snapshot()) {
                out.insert(ed.read(cx).path.clone(), state);
            }
        }
        out
    }

    /// The open documents on this host, as the plan needs them.
    fn refactor_docs(&self, source: &EditSource, cx: &App) -> HashMap<String, OpenDoc> {
        let host = &self.remote.host;
        self.all_editors()
            .into_iter()
            .filter(|ed| ed.read(cx).uses_host(host))
            .map(|ed| {
                let e = ed.read(cx);
                let seen = source.snapshot.get(&e.path).copied();
                let attached = source.client.as_ref().zip(e.lsp_client()).is_some_and(|(a, b)| Arc::ptr_eq(a, b));
                let doc = OpenDoc {
                    text: e.buffer_text(),
                    epoch: e.epoch,
                    version: e.version(),
                    encoding: e.encoding(),
                    dirty: e.is_dirty(),
                    synced: attached && seen.is_some(),
                    server_version: seen.map(|(_, v)| v),
                    edited_since_request: seen.is_some_and(|(buffer, _)| buffer != e.version()),
                };
                (e.path.clone(), doc)
            })
            .collect()
    }

    /// A language server's edit: make the plan, then review it or apply it, as `editor.refactorPreview` says. `done` runs with
    /// whether it was applied.
    pub(super) fn review_workspace_edit(&mut self, source: EditSource, edit: &serde_json::Value, done: Option<Done>, cx: &mut Context<Self>) {
        let parsed = edit::parse(edit);
        if parsed.ops.is_empty() && parsed.skipped.is_empty() {
            self.notify(NoticeSource::Language, Notice::Info(t("The language server proposed no changes.").into()));
            cx.notify();
            return finish(done, true, self, cx);
        }
        if self.restricted_root(cx).is_some() {
            self.notify(NoticeSource::Language, Notice::Error(t("This folder is not trusted, so a language server's edits are not applied.").into()));
            cx.notify();
            return finish(done, false, self, cx);
        }
        let docs = self.refactor_docs(&source, cx);
        let host = self.remote.host.clone();
        let (roots, root) = (self.all_roots(cx), self.root(cx).unwrap_or_default());
        let fallback = cx.global::<Settings>().fallback_encoding;
        let (task_host, label) = (host.clone(), host.label());
        let task = cx.background_executor().spawn(async move {
            let env = HostEnv::new(task_host, docs);
            plan::build(parsed, &env, &roots, label, root, fallback)
        });
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let result = task.await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| this.planned(source, host, result, done, window, cx)).ok();
            })
            .ok();
        })
        .detach();
    }

    fn planned(
        &mut self,
        source: EditSource,
        host: Arc<dyn Host>,
        result: Result<RefactorPlan, PlanError>,
        done: Option<Done>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plan = match result {
            Ok(plan) => Arc::new(plan),
            Err(e) => {
                self.notify(NoticeSource::Language, Notice::Error(e.message().into()));
                cx.notify();
                return finish(done, false, self, cx);
            }
        };
        // The window moved to another connection or folder while the plan was made: its paths mean something else now.
        if !Arc::ptr_eq(&self.remote.host, &host) || self.root(cx).as_deref() != Some(plan.root.as_str()) {
            self.notify(
                NoticeSource::Language,
                Notice::Error(t("The connection or folder changed while the refactoring was prepared. Nothing was changed.").into()),
            );
            cx.notify();
            return finish(done, false, self, cx);
        }
        let mode = cx.global::<Settings>().refactor_preview;
        if plan.items.iter().all(|i| !i.usable()) && !plan::needs_review(&plan, mode) {
            self.notify(NoticeSource::Language, Notice::Info(t("The language server proposed no changes.").into()));
            cx.notify();
            return finish(done, true, self, cx);
        }
        if plan::needs_review(&plan, mode) {
            self.show_refactor_page(source.title, plan, host, done, window, cx);
        } else {
            let chosen = Chosen::initial(&plan);
            self.apply_refactor(plan, chosen, None, host, done, window, cx);
        }
    }

    fn show_refactor_page(
        &mut self,
        title: String,
        plan: Arc<RefactorPlan>,
        host: Arc<dyn Host>,
        done: Option<Done>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A page already open is replaced: its edit is turned down.
        if let Some(old) = self.lsp.refactor.review.take() {
            finish(old.done, false, self, cx);
        }
        // A page replacing a page keeps the first one's return place.
        if self.lsp.refactor.return_focus.is_none() || !matches!(self.tabs.page, Some(Page::Refactor(_))) {
            self.lsp.refactor.return_focus = window.focused(cx);
        }
        let view = cx.new(|cx| RefactorPreview::new(title, plan.clone(), cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &RefactorEvent, window, cx| this.on_refactor_event(ev, window, cx));
        self.lsp.refactor.review = Some(Review { view: view.downgrade(), plan, host, done });
        self.lsp.refactor.release_sub = Some(cx.observe_release(&view, |this, _, cx| this.refactor_page_released(cx)));
        self.lsp.refactor.page_sub = Some(sub);
        self.tabs.page = Some(Page::Refactor(view.clone()));
        self.tabs._page_sub = None;
        self.reveal_editor_area(window, cx);
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        view.update(cx, |v, cx| v.focus_list(window, cx));
        self.refresh_active_flags(cx);
        cx.notify();
    }

    /// The page was dropped without a choice (its tab closed, another page opened in its place): the edit is turned down.
    fn refactor_page_released(&mut self, cx: &mut Context<Self>) {
        if self.lsp.refactor.review.as_ref().is_some_and(|r| r.view.upgrade().is_none())
            && let Some(review) = self.lsp.refactor.review.take()
        {
            finish(review.done, false, self, cx);
        }
    }

    fn on_refactor_event(&mut self, ev: &RefactorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            RefactorEvent::Apply => {
                let Some(review) = &self.lsp.refactor.review else { return };
                let Some(view) = review.view.upgrade() else { return };
                let (plan, host) = (review.plan.clone(), review.host.clone());
                let chosen = view.read(cx).chosen().clone();
                if !Arc::ptr_eq(&self.remote.host, &host) || self.root(cx).as_deref() != Some(plan.root.as_str()) {
                    let why = t("The connection or folder changed since the preview. Nothing was changed.").to_string();
                    return view.update(cx, |v, cx| v.set_stale(why, cx));
                }
                let done = self.lsp.refactor.review.as_mut().and_then(|r| r.done.take());
                self.apply_refactor(plan, chosen, Some(view), host, done, window, cx);
            }
            RefactorEvent::Discard => {
                // Also the page of an edit that was applied: only the report is left, and closing it answers nobody.
                if let Some(Page::Refactor(view)) = &self.tabs.page
                    && self.lsp.refactor.review.as_ref().is_none_or(|r| r.view.entity_id() == view.entity_id())
                {
                    self.close_page(cx);
                    self.lsp.refactor.page_sub = None;
                    match self.lsp.refactor.return_focus.take() {
                        Some(handle) => window.focus(&handle),
                        None => window.focus(&self.focus_handle),
                    }
                }
                if let Some(review) = self.lsp.refactor.review.take() {
                    finish(review.done, false, self, cx);
                }
            }
            RefactorEvent::Stop => {
                if let Some(flag) = &self.lsp.refactor.cancel {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            RefactorEvent::Undo => self.undo_refactor(window, cx),
            RefactorEvent::Open { path, line, column, len } => {
                self.tabs.pending_reveal = Some((path.clone(), *line, *column, *len));
                self.open_file(path.clone(), window, cx);
            }
        }
    }

    // --- Apply ---

    /// Write the selected parts: open editors first (all or none), then the host's files and operations as one transaction.
    #[allow(clippy::too_many_arguments)]
    fn apply_refactor(
        &mut self,
        plan: Arc<RefactorPlan>,
        chosen: Chosen,
        view: Option<Entity<RefactorPreview>>,
        host: Arc<dyn Host>,
        done: Option<Done>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prepared = apply::prepare(&plan, &chosen);
        let editors = self.all_editors();
        let open = |path: &str, cx: &App| editors.iter().find(|e| e.read(cx).path == path && e.read(cx).uses_host(&host)).cloned();
        // Every editor must still hold exactly the text the plan was made from; one that does not stops everything.
        let mut targets = Vec::new();
        for job in &prepared.buffers {
            match open(&job.path, cx) {
                Some(e) if e.read(cx).doc_identity() == (crate::editor::DocIdentity { epoch: job.epoch, version: job.version, hash: job.hash }) => {
                    targets.push((e, job));
                }
                _ => {
                    let lines = vec![(crate::i18n::tf("File changed since preview: {}", &[&name_in(&plan, &job.path)]), true)];
                    return self.refactor_failed(lines, view, done, window, cx);
                }
            }
        }
        let mut buffers = Vec::new();
        let mut buffer_counts = Vec::new();
        for (ed, job) in targets {
            let edits = job.edits.clone();
            ed.update(cx, |e, cx| e.apply_buffer_edits(|_| edits, cx));
            buffers.push(UndoBuffer { path: job.path.clone(), editor: ed.downgrade(), after: ed.read(cx).doc_identity() });
            buffer_counts.push((job.path.clone(), job.changes));
        }
        let skipped = prepared.skipped;
        if prepared.steps.is_empty() {
            let tx = Transaction::default();
            return self.refactor_done(plan, tx, buffers, buffer_counts, skipped, view, host, done, window, cx);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.lsp.refactor.cancel = Some(cancel.clone());
        let fs = self.refactor_fs(&host);
        let steps = prepared.steps;
        let task = cx.background_executor().spawn(async move { apply::run(fs.as_ref(), steps, &cancel) });
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let tx = task.await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    this.lsp.refactor.cancel = None;
                    this.refactor_done(plan, tx, buffers, buffer_counts, skipped, view, host, done, window, cx)
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    fn refactor_fs(&self, host: &Arc<dyn Host>) -> Arc<dyn Fs> {
        #[cfg(test)]
        if let Some(fs) = &self.lsp.refactor.fs {
            return fs.clone();
        }
        Arc::new(host.clone())
    }

    /// Nothing was applied: say why, on the page or as a notice.
    fn refactor_failed(
        &mut self,
        lines: Vec<(String, bool)>,
        view: Option<Entity<RefactorPreview>>,
        done: Option<Done>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match view {
            Some(view) => {
                // The page stays for another try; its edit is still waiting for an answer.
                view.update(cx, |v, cx| v.set_report(lines, false, false, cx));
                view.update(cx, |v, cx| v.focus_list(window, cx));
                if let Some(review) = &mut self.lsp.refactor.review {
                    review.done = done;
                }
            }
            None => {
                if let Some((line, _)) = lines.first() {
                    self.notify(NoticeSource::Language, Notice::Error(line.clone().into()));
                }
                finish(done, false, self, cx);
            }
        }
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn refactor_done(
        &mut self,
        plan: Arc<RefactorPlan>,
        tx: Transaction,
        buffers: Vec<UndoBuffer>,
        buffer_counts: Vec<(String, usize)>,
        skipped: Vec<(String, String)>,
        view: Option<Entity<RefactorPreview>>,
        host: Arc<dyn Host>,
        done: Option<Done>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = |p: &str| name_in(&plan, p);
        if !tx.ok() {
            // The transaction took its own steps back; the editors' changes are taken back too.
            let mut lines = apply::report(&tx, &[], name);
            for b in buffers.iter().rev() {
                let taken_back = b.editor.upgrade().is_some_and(|e| e.update(cx, |e, cx| e.undo_replace_step(b.after, cx)));
                if !taken_back {
                    lines.push((
                        crate::i18n::tf("Could not take back the change in {}: it was edited since. Undo it in the editor.", &[&name_in(&plan, &b.path)]),
                        true,
                    ));
                }
            }
            return self.refactor_failed(lines, view, done, window, cx);
        }
        let mut lines = apply::report(&tx, &buffer_counts, name);
        if !buffers.is_empty() {
            lines.push((t("Changes in open files are not saved yet.").to_string(), false));
        }
        for (path, why) in &skipped {
            lines.push((crate::i18n::tf("Not changed: {0} ({1})", &[&name_in(&plan, path), why]), true));
        }
        let mut touched = Vec::new();
        for (step, _) in &tx.steps {
            match step {
                Step::Write { path, .. } | Step::Create { path } => touched.push(path.clone()),
                Step::Rename { from, to } => {
                    self.after_move(from.clone(), to.clone(), cx);
                    touched.push(to.clone());
                }
                Step::Delete { path } => {
                    self.after_delete(path.clone(), window, cx);
                    touched.push(path.clone());
                }
            }
        }
        for path in touched.iter().filter(|p| tx.steps.iter().any(|(s, _)| matches!(s, Step::Create { path } if path == *p))) {
            self.reload_in_trees(&parent_of(path), cx);
        }
        self.notify_files_changed(&touched);
        self.refresh_git(300, cx);
        let can_undo = !tx.has_resource_steps() && (!tx.undo.is_empty() || !buffers.is_empty());
        self.lsp.refactor.batch = can_undo.then(|| Batch {
            host: host.clone(),
            root: plan.root.clone(),
            view: view.as_ref().map_or_else(WeakEntity::new_invalid, |v| v.downgrade()),
            disk: tx.undo.clone(),
            buffers,
        });
        match view {
            Some(view) => {
                view.update(cx, |v, cx| v.set_report(lines, true, can_undo, cx));
                // Closing the tab of a deleted file moved the window to another tab: the report is what the user looks for.
                self.tabs.page = Some(Page::Refactor(view.clone()));
                self.tabs.page_active = true;
                self.tabs.diff_active = false;
                self.tabs.large_active = false;
                self.refresh_active_flags(cx);
                window.focus(&view.read(cx).focus_handle(cx));
                self.lsp.refactor.review = None;
                self.tabs._page_sub = None;
            }
            None => {
                self.notify(NoticeSource::Language, Notice::Info(lines.first().map_or_else(|| t("Done.").into(), |(l, _)| l.clone().into())));
            }
        }
        finish(done, true, self, cx);
        cx.notify();
    }

    // --- Undo ---

    /// Take back the last refactoring's text changes: each editor's step if nothing was typed since, each file if it is still
    /// what the refactoring wrote.
    fn undo_refactor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch) = self.lsp.refactor.batch.take() else {
            self.notify(NoticeSource::Language, Notice::Info(t("There is no refactoring to undo.").into()));
            cx.notify();
            return;
        };
        let view = batch.view.upgrade();
        if !Arc::ptr_eq(&batch.host, &self.remote.host) || self.root(cx).as_deref() != Some(batch.root.as_str()) {
            // Kept: the window may come back to that folder on that connection.
            let message = t("Undo is not available here: the refactoring was made in another folder or connection.");
            self.lsp.refactor.batch = Some(batch);
            self.notify(NoticeSource::Language, Notice::Error(message.into()));
            cx.notify();
            return;
        }
        let mut report = Report { undo: true, files: Vec::new() };
        for b in &batch.buffers {
            let outcome = match b.editor.upgrade() {
                Some(e) if e.update(cx, |e, cx| e.undo_replace_step(b.after, cx)) => Outcome::Changed(0),
                _ => Outcome::Conflict,
            };
            report.files.push((b.path.clone(), outcome));
        }
        // A file reopened and edited since holds unsaved work: it is reported instead of written under.
        let editors = self.all_editors();
        let (entries, busy): (Vec<UndoDisk>, Vec<UndoDisk>) = batch
            .disk
            .iter()
            .cloned()
            .partition(|u| !editors.iter().any(|e| e.read(cx).path == u.path && e.read(cx).uses_host(&batch.host) && e.read(cx).is_dirty()));
        report.files.extend(busy.into_iter().map(|u| (u.path, Outcome::Conflict)));
        let fs = self.refactor_fs(&batch.host);
        let cancel = Arc::new(AtomicBool::new(false));
        let task = cx.background_executor().spawn(async move { apply::undo(fs.as_ref(), &entries, &cancel) });
        let root = batch.root.clone();
        let handle = self.window_handle;
        let _ = window;
        cx.spawn(async move |this, cx| {
            let outcomes = task.await;
            cx.update_window(handle, |_, _, cx| {
                this.update(cx, |this, cx| {
                    let restored: Vec<String> = outcomes.iter().filter(|(_, o)| o.changed()).map(|(p, _)| p.clone()).collect();
                    report.files.extend(outcomes);
                    this.notify_files_changed(&restored);
                    this.refresh_git(300, cx);
                    let lines = report.lines(|p| {
                        let root = root.replace('\\', "/");
                        let p = p.replace('\\', "/");
                        p.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).map(str::to_string).unwrap_or(p)
                    });
                    match view {
                        Some(v) => v.update(cx, |v, cx| v.set_undone(lines, cx)),
                        None => {
                            if let Some((line, bad)) = lines.first() {
                                this.notify(NoticeSource::Language, if *bad { Notice::Error(line.clone().into()) } else { Notice::Info(line.clone().into()) });
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    /// The window's host or folder changed: a page open for the old one cannot apply (it says so when Apply is pressed).
    #[cfg(test)]
    pub fn refactor_page_for_test(&self) -> Option<Entity<RefactorPreview>> {
        self.lsp.refactor.review.as_ref().and_then(|r| r.view.upgrade())
    }

    #[cfg(test)]
    pub fn review_workspace_edit_for_test(&mut self, title: &str, edit: &serde_json::Value, done: Option<Done>, cx: &mut Context<Self>) {
        let source = EditSource { title: title.into(), client: None, snapshot: HashMap::new() };
        self.review_workspace_edit(source, edit, done, cx);
    }

    #[cfg(test)]
    pub fn set_refactor_fs_for_test(&mut self, fs: Arc<dyn Fs>) {
        self.lsp.refactor.fs = Some(fs);
    }
}

/// Run what was waiting for the edit.
pub(super) fn finish(done: Option<Done>, applied: bool, wb: &mut Workbench, cx: &mut Context<Workbench>) {
    if let Some(done) = done {
        done(wb, applied, cx);
    }
}

/// A path as shown: relative to the plan's folder when it is inside it.
fn name_in(plan: &RefactorPlan, path: &str) -> String {
    let root = plan.root.replace('\\', "/");
    let path = path.replace('\\', "/");
    path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).map(str::to_string).unwrap_or(path)
}
