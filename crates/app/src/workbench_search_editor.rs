//! Workbench: Search Editor tabs. Opens and restores them, keeps them tied to the host and folder they searched, and does
//! the work that needs the rest of the window: previews read open editors' buffers (unsaved text wins over the disk),
//! Apply edits open buffers as one undo step each and writes closed files through the host, and the batch is kept for one
//! conflict-checked Undo. A preview, apply or undo made for another host or folder than the one shown is refused.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{EntityId, WeakEntity};

use super::*;
use crate::replace_apply::{self, Job, Outcome, Report, UndoDisk};
use crate::replace_plan::{LIMITS, Pattern, Plan, PlanError, Source, Version};
use crate::search_doc::{self, Provenance, SearchDoc, SearchSpec};
use crate::search_editor::{Busy, Review, SearchEditor, SearchEditorEvent};

/// Saved Search Editors of a workspace (state database key prefix).
const SAVED_KEY: &str = "search_editors";

/// An open editor changed by a replacement: undo takes back that one step, only if nothing was typed since.
struct UndoBuffer {
    path: String,
    editor: WeakEntity<EditorView>,
    after: crate::editor::DocIdentity,
}

/// The last applied replacement, for Undo.
pub(super) struct ReplaceBatch {
    host: Arc<dyn Host>,
    root: String,
    editor: WeakEntity<SearchEditor>,
    disk: Vec<UndoDisk>,
    buffers: Vec<UndoBuffer>,
}

#[derive(Default)]
pub(super) struct SearchEditors {
    subs: Vec<(EntityId, Subscription)>,
    batch: Option<ReplaceBatch>,
    /// Last saved text per workspace, so unchanged state is not written again.
    saved: std::cell::RefCell<Option<(i64, String)>>,
    restored: Option<i64>,
    persist_task: Option<gpui::Task<()>>,
    /// The plan each editor's preview was read with, by editor.
    plan_hosts: Vec<(EntityId, Arc<dyn Host>)>,
    /// Tests write through a store that injects failures.
    #[cfg(test)]
    pub(super) store: Option<Arc<dyn replace_apply::Store>>,
}

/// How a search's host and folder relate to what the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Same,
    /// Same host name and folder, but the connection was made again: reading is fine, earlier previews are not.
    Reconnected,
    Other,
}

impl Workbench {
    fn all_search_editors(&self) -> Vec<Entity<SearchEditor>> {
        let mut out: Vec<Entity<SearchEditor>> = self.tabs.searches.clone();
        for g in self.tabs.groups.values() {
            out.extend(g.searches.iter().filter(|e| !out.contains(e)).cloned().collect::<Vec<_>>());
        }
        out
    }

    fn place_of(&self, ed: &SearchEditor, cx: &App) -> Place {
        let prov = &ed.doc.provenance;
        // Every folder the search covered must still be in the workspace (one added since is simply not searched yet).
        if self.root(cx).as_deref() != Some(prov.root.as_str())
            || self.remote.host.label() != prov.host
            || !prov.extra_roots.iter().all(|r| self.roots.extra_roots.contains(r))
        {
            return Place::Other;
        }
        if Arc::ptr_eq(ed.host(), &self.remote.host) { Place::Same } else { Place::Reconnected }
    }

    fn provenance(&self, cx: &App) -> Option<Provenance> {
        Some(Provenance { host: self.remote.host.label(), root: self.root(cx)?, extra_roots: self.roots.extra_roots.clone() })
    }

    /// Open a Search Editor for `spec` in the first folder and run it. `review`: make a replacement preview once it finishes.
    pub(super) fn open_search_editor(
        &mut self,
        spec: SearchSpec,
        review: Option<Review>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SearchEditor>> {
        let Some(provenance) = self.provenance(cx) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a folder to search in it.").into()));
            cx.notify();
            return None;
        };
        let run = !spec.pattern.is_empty();
        let host = self.remote.host.clone();
        // The same query in the same folder reuses its tab (a second Replace from the sidebar does not pile up tabs).
        let existing = self.tabs.searches.iter().find(|e| {
            let e = e.read(cx);
            e.busy.is_none() && e.doc.provenance == provenance && !e.edited_spec(cx).differs_in_query(&spec) && !spec.pattern.is_empty()
        });
        let ed = match existing.cloned() {
            Some(ed) => {
                ed.update(cx, |e, cx| e.load_spec(&spec, cx));
                ed
            }
            None => {
                let ed = cx.new(|cx| SearchEditor::new(SearchDoc::new(spec, provenance), host.clone(), cx));
                self.add_search_editor(ed.clone(), window, cx);
                ed
            }
        };
        self.show_search_editor(ed.clone(), window, cx);
        if run {
            ed.update(cx, |e, cx| {
                e.review = review;
                e.run(host, cx);
            });
        } else {
            ed.read(cx).focus_query(window, cx);
        }
        self.persist_search_editors_soon(cx);
        Some(ed)
    }

    fn add_search_editor(&mut self, ed: Entity<SearchEditor>, window: &mut Window, cx: &mut Context<Self>) {
        let live: Vec<EntityId> = self.all_search_editors().iter().map(|e| e.entity_id()).collect();
        self.searches.subs.retain(|(id, _)| live.contains(id));
        self.searches.plan_hosts.retain(|(id, _)| live.contains(id));
        let sub = cx.subscribe_in(&ed, window, |this, ed, ev: &SearchEditorEvent, window, cx| this.on_search_editor_event(ed.clone(), ev, window, cx));
        self.searches.subs.push((ed.entity_id(), sub));
        self.tabs.searches.push(ed);
    }

    pub(super) fn show_search_editor(&mut self, ed: Entity<SearchEditor>, window: &mut Window, cx: &mut Context<Self>) {
        let place = self.place_of(ed.read(cx), cx);
        ed.update(cx, |e, cx| e.set_workspace_matches(place != Place::Other, cx));
        self.tabs.page = Some(Page::Search(ed.clone()));
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        if ed.read(cx).doc.files.is_empty() {
            ed.read(cx).focus_query(window, cx);
        } else {
            ed.update(cx, |e, cx| e.focus_list(window, cx));
        }
        self.refresh_active_flags(cx);
        cx.notify();
    }

    /// Close a Search Editor tab: its search stops; an undo record it made stays available from the command palette.
    pub(super) fn close_search_editor(&mut self, ed: &Entity<SearchEditor>, cx: &mut Context<Self>) {
        ed.update(cx, |e, _| e.stop_search());
        self.tabs.searches.retain(|e| e != ed);
        for g in self.tabs.groups.values_mut() {
            g.searches.retain(|e| e != ed);
            if matches!(&g.page, Some(Page::Search(p)) if p == ed) {
                g.page = None;
                g.page_active = false;
            }
        }
        if matches!(&self.tabs.page, Some(Page::Search(p)) if p == ed) {
            self.tabs.page = None;
            self.tabs.page_active = false;
        }
        self.searches.subs.retain(|(id, _)| *id != ed.entity_id());
        self.searches.plan_hosts.retain(|(id, _)| *id != ed.entity_id());
        self.persist_search_editors(cx);
        cx.notify();
    }

    /// The window's host or folder changed: Search Editors elsewhere are marked stale (and unmarked when it comes back).
    pub(super) fn sync_search_editors(&mut self, cx: &mut Context<Self>) {
        for ed in self.all_search_editors() {
            let place = self.place_of(ed.read(cx), cx);
            ed.update(cx, |e, cx| e.set_workspace_matches(place != Place::Other, cx));
        }
    }

    fn on_search_editor_event(&mut self, ed: Entity<SearchEditor>, ev: &SearchEditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            SearchEditorEvent::Open { path, line, column, len } => {
                if self.place_of(ed.read(cx), cx) == Place::Other {
                    ed.update(cx, |e, cx| e.set_workspace_matches(false, cx));
                    return;
                }
                self.tabs.pending_reveal = Some((path.clone(), *line, *column, *len));
                self.open_file(path.clone(), window, cx);
            }
            SearchEditorEvent::Rerun => self.rerun_search_editor(&ed, cx),
            SearchEditorEvent::Preview => self.preview_replace(ed, cx),
            SearchEditorEvent::Apply => self.apply_replace(ed, cx),
            SearchEditorEvent::Cancel => {
                if let Some(Busy::Applying(flag) | Busy::Undoing(flag)) = &ed.read(cx).busy {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            SearchEditorEvent::Undo => self.undo_replace(cx),
            SearchEditorEvent::Changed => self.persist_search_editors_soon(cx),
        }
    }

    fn rerun_search_editor(&mut self, ed: &Entity<SearchEditor>, cx: &mut Context<Self>) {
        if ed.read(cx).busy.is_some() {
            return;
        }
        match self.place_of(ed.read(cx), cx) {
            Place::Other => ed.update(cx, |e, cx| {
                e.set_workspace_matches(false, cx);
                e.set_message(crate::i18n::t("Open this search's folder on its host to run it again."), true, cx);
            }),
            Place::Same | Place::Reconnected => {
                let host = self.remote.host.clone();
                ed.update(cx, |e, cx| e.run(host, cx));
            }
        }
    }

    /// "Search: Open Search Editor": a new tab with the sidebar's query (run at once if there is one).
    pub(super) fn open_search_editor_from_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let spec = self.search.read(cx).spec(cx);
        self.open_search_editor(spec, None, window, cx);
    }

    // --- Preview ---

    /// Make the replacement plan: open editors' text for open files, the disk for the rest (read on the host, bounded).
    fn preview_replace(&mut self, ed: Entity<SearchEditor>, cx: &mut Context<Self>) {
        let editor = ed.read(cx);
        if editor.busy.is_some() || editor.is_running() {
            return;
        }
        if self.place_of(editor, cx) == Place::Other {
            ed.update(cx, |e, cx| e.set_workspace_matches(false, cx));
            return;
        }
        let edited = editor.edited_spec(cx);
        let mut spec = editor.doc.spec.clone();
        spec.replacement = edited.replacement;
        spec.preserve_case = edited.preserve_case;
        let settings = cx.global::<Settings>();
        let (smart, fallback) = (settings.search_smart_case, settings.fallback_encoding);
        let Some(query) = spec.query(&editor.doc.provenance.root, smart, &[], 1) else { return };
        let pattern = match Pattern::new(&query, &spec.replacement, spec.preserve_case) {
            Ok(p) => p,
            Err(e) => return ed.update(cx, |e2, cx| e2.set_message(e, true, cx)),
        };
        let generation = editor.doc.generation;
        let (prov_host, prov_root) = (editor.doc.provenance.host.clone(), editor.doc.provenance.root.clone());
        let paths: Vec<String> = editor.doc.files.iter().map(|f| f.path.clone()).collect();
        if paths.len() > LIMITS.files {
            return ed.update(cx, |e, cx| e.set_message(crate::i18n::tf("Too many files to preview ({}). Narrow the search.", &[&paths.len()]), true, cx));
        }
        let host = self.remote.host.clone();
        // Open documents are the source of truth: their current text, even unsaved.
        let open: Vec<(String, Source)> = paths
            .iter()
            .filter_map(|p| {
                let e = self.all_editors().into_iter().find(|e| e.read(cx).path == *p && e.read(cx).uses_host(&host))?;
                let e = e.read(cx);
                Some((p.clone(), Source::Buffer { text: e.buffer_text(), epoch: e.epoch, version: e.version(), encoding: e.encoding() }))
            })
            .collect();
        ed.update(cx, |e, cx| e.set_busy(Some(Busy::Previewing), cx));
        self.searches.plan_hosts.retain(|(id, _)| *id != ed.entity_id());
        let read_host = host.clone();
        let task = cx.background_executor().spawn(async move {
            let mut open = open;
            let mut budget = LIMITS.bytes;
            let mut sources = Vec::with_capacity(paths.len());
            for path in paths {
                if let Some(i) = open.iter().position(|(p, _)| *p == path) {
                    let (p, s) = open.swap_remove(i);
                    sources.push((p, Ok(s)));
                    continue;
                }
                match read_host.read(&path) {
                    Ok((bytes, hash)) => {
                        if bytes.len() > budget {
                            return Err(PlanError::TooLarge);
                        }
                        budget -= bytes.len();
                        sources.push((path, Ok(Source::Disk { bytes, hash })));
                    }
                    Err(e) => sources.push((path, Err(e.to_string()))),
                }
            }
            Plan::build(prov_host, prov_root, generation, sources, &pattern, fallback, &LIMITS)
        });
        let weak = ed.downgrade();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(ed) = weak.upgrade() else { return };
                // A reply for an older run, or one that arrives after the window moved to another host or folder, is dropped.
                let stale =
                    ed.read(cx).doc.generation != generation || !Arc::ptr_eq(&this.remote.host, &host) || this.place_of(ed.read(cx), cx) == Place::Other;
                ed.update(cx, |e, cx| {
                    e.set_busy(None, cx);
                    if stale {
                        e.set_message(crate::i18n::t("The search or the connection changed while the preview was being made. Preview again."), true, cx);
                        return;
                    }
                    match result {
                        Ok(plan) if plan.files.iter().all(|f| f.changes.is_empty()) && plan.files.iter().all(|f| f.problem.is_none()) => {
                            e.set_message(crate::i18n::t("Nothing to replace: the files no longer contain matches."), false, cx)
                        }
                        Ok(plan) => e.set_plan(Arc::new(plan), cx),
                        Err(PlanError::TooLarge) => {
                            e.set_message(crate::i18n::t("This replacement is too large to preview. Narrow the search (fewer files or matches)."), true, cx)
                        }
                    }
                });
                if ed.read(cx).has_plan() {
                    this.searches.plan_hosts.push((ed.entity_id(), host));
                }
            })
            .ok();
        })
        .detach();
    }

    // --- Apply ---

    pub(super) fn apply_replace(&mut self, ed: Entity<SearchEditor>, cx: &mut Context<Self>) {
        let Some((plan, selection)) = ed.read(cx).plan() else { return };
        if ed.read(cx).busy.is_some() {
            return;
        }
        if let Some(why) = ed.read(cx).apply_blocker() {
            return ed.update(cx, |e, cx| e.set_message(why, true, cx));
        }
        let plan_host = self.searches.plan_hosts.iter().find(|(id, _)| *id == ed.entity_id()).map(|(_, h)| h.clone());
        let host = self.remote.host.clone();
        if plan_host.is_none_or(|h| !Arc::ptr_eq(&h, &host))
            || self.place_of(ed.read(cx), cx) != Place::Same
            || self.root(cx).as_deref() != Some(plan.root.as_str())
            || host.label() != plan.host
        {
            return ed.update(cx, |e, cx| {
                e.clear_plan();
                e.set_message(crate::i18n::t("The connection or folder changed since the preview. Rerun the search and preview again."), true, cx);
            });
        }
        let mut report = Report::default();
        let mut buffers = Vec::new();
        // Disk files: (plan file index, expected hash, selected changes). Their bytes are built one at a time on the
        // background thread, so the plan's text and the new bytes of every file are never all in memory at once.
        let mut disk_targets: Vec<(usize, u64, usize)> = Vec::new();
        let editors = self.all_editors();
        let open_editor = |path: &str, cx: &App| editors.iter().find(|e| e.read(cx).path == path && e.read(cx).uses_host(&host)).cloned();
        for (fi, file) in plan.files.iter().enumerate() {
            let selected = selection.file(fi);
            let n = selected.iter().filter(|x| **x).count();
            if n == 0 {
                continue;
            }
            if let Some(problem) = &file.problem {
                report.files.push((file.path.clone(), Outcome::Skipped(problem.message())));
                continue;
            }
            let open = open_editor(&file.path, cx);
            match file.version {
                Version::Buffer { epoch, version, hash } => match open {
                    Some(e) if e.read(cx).doc_identity() == crate::editor::DocIdentity { epoch, version, hash } => {
                        let edits = file.buffer_edits(selected);
                        e.update(cx, |e, cx| e.apply_buffer_edits(|_| edits, cx));
                        buffers.push(UndoBuffer { path: file.path.clone(), editor: e.downgrade(), after: e.read(cx).doc_identity() });
                        report.files.push((file.path.clone(), Outcome::Changed(n)));
                    }
                    _ => report.files.push((file.path.clone(), Outcome::Conflict)),
                },
                Version::Disk { hash } => {
                    // A file opened since the preview: its editor holds the truth now. Unsaved edits, or a buffer read
                    // from another version, mean the preview is out of date.
                    if open.as_ref().is_some_and(|e| e.read(cx).is_dirty() || e.read(cx).disk_hash() != hash) {
                        report.files.push((file.path.clone(), Outcome::Conflict));
                        continue;
                    }
                    disk_targets.push((fi, hash, n));
                }
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        ed.update(cx, |e, cx| e.set_busy(Some(Busy::Applying(cancel.clone())), cx));
        let store: Arc<dyn replace_apply::Store> = self.searches_store(&host);
        let (bg_plan, bg_selection) = (plan.clone(), selection.clone());
        let task = cx.background_executor().spawn(async move {
            let mut skipped = Vec::new();
            let jobs = disk_targets.into_iter().filter_map(|(fi, expect, changes)| {
                let file = &bg_plan.files[fi];
                match file.output_bytes(bg_selection.file(fi)) {
                    Ok(bytes) => Some(Job { path: file.path.clone(), bytes, expect, original: file.original(), changes }),
                    Err(problem) => {
                        skipped.push((file.path.clone(), Outcome::Skipped(problem.message())));
                        None
                    }
                }
            });
            let (mut outcomes, undo) = replace_apply::write_all(store.as_ref(), jobs, &cancel);
            outcomes.extend(skipped);
            (outcomes, undo)
        });
        let weak = ed.downgrade();
        let root = plan.root.clone();
        cx.spawn(async move |this, cx| {
            let (outcomes, disk) = task.await;
            this.update(cx, |this, cx| {
                let written: Vec<String> = disk.iter().map(|u| u.path.clone()).collect();
                report.files.extend(outcomes);
                let can_undo = !disk.is_empty() || !buffers.is_empty();
                if can_undo {
                    this.searches.batch = Some(ReplaceBatch { host: host.clone(), root, editor: weak.clone(), disk, buffers });
                }
                for other in this.all_search_editors() {
                    if Some(other.entity_id()) != weak.upgrade().map(|e| e.entity_id()) {
                        other.update(cx, |e, cx| e.set_can_undo(false, cx));
                    }
                }
                this.notify_files_changed(&written);
                this.refresh_git(300, cx);
                this.search.update(cx, |s, cx| s.rerun(cx));
                let summary = report_notice(&report);
                match weak.upgrade() {
                    Some(ed) => ed.update(cx, |e, cx| e.set_report(report, can_undo, cx)),
                    None => this.notice = Some(summary),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The store apply and undo write through. Tests swap in one that injects failures.
    fn searches_store(&self, host: &Arc<dyn Host>) -> Arc<dyn replace_apply::Store> {
        #[cfg(test)]
        if let Some(store) = &self.searches.store {
            return store.clone();
        }
        Arc::new(host.clone())
    }

    // --- Undo ---

    /// Undo the last replacement: each open editor's step if nothing was typed since, each file if it is still what the
    /// replacement wrote. Anything else is reported as an undo conflict and left alone.
    pub(super) fn undo_replace(&mut self, cx: &mut Context<Self>) {
        let Some(batch) = self.searches.batch.take() else {
            self.notice = Some(Notice::Info(crate::i18n::t("There is no replacement to undo.").into()));
            cx.notify();
            return;
        };
        let target = batch.editor.upgrade();
        if !Arc::ptr_eq(&batch.host, &self.remote.host) || self.root(cx).as_deref() != Some(batch.root.as_str()) {
            // Kept: the window may come back to that folder on that connection.
            self.searches.batch = Some(batch);
            let msg = crate::i18n::t("Undo is not available here: the replacement was made in another folder or connection.");
            match target {
                Some(ed) => ed.update(cx, |e, cx| e.set_message(msg, true, cx)),
                None => self.notice = Some(Notice::Error(msg.into())),
            }
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
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(ed) = &target {
            ed.update(cx, |e, cx| e.set_busy(Some(Busy::Undoing(cancel.clone())), cx));
        }
        // A file reopened and edited since holds unsaved work: writing the old bytes under it would set up a conflict on
        // its next save, so it is reported instead.
        let editors = self.all_editors();
        let (entries, busy): (Vec<UndoDisk>, Vec<UndoDisk>) = batch
            .disk
            .iter()
            .cloned()
            .partition(|u| !editors.iter().any(|e| e.read(cx).path == u.path && e.read(cx).uses_host(&batch.host) && e.read(cx).is_dirty()));
        report.files.extend(busy.into_iter().map(|u| (u.path, Outcome::Conflict)));
        let store = self.searches_store(&batch.host);
        let task = cx.background_executor().spawn(async move { replace_apply::undo_all(store.as_ref(), &entries, &cancel) });
        let weak = batch.editor.clone();
        cx.spawn(async move |this, cx| {
            let outcomes = task.await;
            this.update(cx, |this, cx| {
                let restored: Vec<String> = outcomes.iter().filter(|(_, o)| o.changed()).map(|(p, _)| p.clone()).collect();
                report.files.extend(outcomes);
                this.notify_files_changed(&restored);
                this.refresh_git(300, cx);
                this.search.update(cx, |s, cx| s.rerun(cx));
                let summary = report_notice(&report);
                match weak.upgrade() {
                    Some(ed) => ed.update(cx, |e, cx| e.set_report(report, false, cx)),
                    None => this.notice = Some(summary),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // --- Saving and restoring ---

    fn persist_search_editors_soon(&mut self, cx: &mut Context<Self>) {
        self.searches.persist_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
            this.update(cx, |this, cx| this.persist_search_editors(cx)).ok();
        }));
    }

    /// Save the workspace's Search Editors (query, options and a bounded part of the results).
    pub(super) fn persist_search_editors(&self, cx: &App) {
        let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) else { return };
        // Not yet restored: saving now would replace the saved tabs with none.
        if self.searches.restored != Some(ws) {
            return;
        }
        let eds = self.all_search_editors();
        let docs: Vec<&SearchDoc> = eds.iter().map(|e| &e.read(cx).doc).collect();
        let text = search_doc::save(&docs);
        if self.searches.saved.borrow().as_ref().is_some_and(|(w, t)| *w == ws && *t == text) {
            return;
        }
        if s.set(&format!("{SAVED_KEY}:{ws}"), &text).is_ok() {
            *self.searches.saved.borrow_mut() = Some((ws, text));
        }
    }

    /// Bring back the workspace's Search Editors as tabs, stale: their results are from the last session and nothing runs
    /// or is replaced until asked.
    pub(super) fn restore_search_editors(&mut self, ws: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.searches.restored == Some(ws) {
            return;
        }
        self.searches.restored = Some(ws);
        let Some(text) = self.persist.state.as_ref().and_then(|s| s.get(&format!("{SAVED_KEY}:{ws}"))) else { return };
        *self.searches.saved.borrow_mut() = Some((ws, text.clone()));
        let Some(here) = self.provenance(cx) else { return };
        let host = self.remote.host.clone();
        for doc in search_doc::restore(&text) {
            if (&doc.provenance.host, &doc.provenance.root) != (&here.host, &here.root) {
                continue;
            }
            let ed = cx.new(|cx| SearchEditor::new(doc, host.clone(), cx));
            self.add_search_editor(ed, window, cx);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn search_editors_for_test(&self) -> Vec<Entity<SearchEditor>> {
        self.all_search_editors()
    }

    #[cfg(test)]
    pub fn open_search_editor_for_test(
        &mut self,
        spec: SearchSpec,
        review: Option<Review>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SearchEditor>> {
        self.open_search_editor(spec, review, window, cx)
    }

    #[cfg(test)]
    pub fn active_search_editor_for_test(&self) -> Option<Entity<SearchEditor>> {
        match &self.tabs.page {
            Some(Page::Search(e)) if self.tabs.page_active => Some(e.clone()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn show_search_editor_for_test(&mut self, ed: Entity<SearchEditor>, window: &mut Window, cx: &mut Context<Self>) {
        self.show_search_editor(ed, window, cx)
    }

    #[cfg(test)]
    pub fn host_for_test(&self) -> Arc<dyn Host> {
        self.remote.host.clone()
    }

    #[cfg(test)]
    pub fn has_replace_undo_for_test(&self) -> bool {
        self.searches.batch.is_some()
    }

    #[cfg(test)]
    pub fn set_replace_store_for_test(&mut self, store: Arc<dyn replace_apply::Store>) {
        self.searches.store = Some(store);
    }

    #[cfg(test)]
    pub fn persist_search_editors_for_test(&self, cx: &App) {
        self.persist_search_editors(cx)
    }
}

/// A one-line summary for the status bar (when the Search Editor that asked is gone).
fn report_notice(report: &Report) -> Notice {
    let lines = report.lines(|p| ui::file_name(p).to_string());
    let text = lines.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>().join(" ");
    if lines.iter().any(|(_, bad)| *bad) { Notice::Error(text.into()) } else { Notice::Info(text.into()) }
}
