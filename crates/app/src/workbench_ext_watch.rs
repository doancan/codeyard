//! Workbench: watches of code extensions (`watch`, API 1.1; docs/plugin-architecture.md §5.2). Document events come
//! from the editors: `opened` (the documents already open first), `changed` once typing pauses, `saved`. File events
//! come from the window's file watcher, which follows the matching files and their folders, and from listing the
//! folders again when one of those changes. Every event is checked as a read of its path is (`Scope::may_read`: the
//! grants, the user's switches and Workspace Trust) and goes only to the extension whose watch asked for it. A running
//! watch keeps its exthost from being stopped as idle; the watches end with the exthost.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use ext_api::value::{DocumentChange, DocumentEvent, FileChange, FileEvent, Notify};
use ext_manifest::limits;
use ext_proto::ToHost;

use super::*;
use crate::ext_broker::{self, Scope};

/// Files listed per workspace folder for a file watch, as for `workspace-contains`.
const LISTED_FILES: u32 = 20_000;

#[derive(Default)]
pub(super) struct WatchState {
    /// Per extension, its watches by id.
    watches: HashMap<String, BTreeMap<u64, Watch>>,
    /// `changed` events waiting for typing to pause, per document path (dropping one cancels it).
    changes: HashMap<String, gpui::Task<()>>,
    /// The version last sent as `changed` or `saved`, per document path: an edit that left the text as it was sends
    /// nothing.
    sent: HashMap<String, u32>,
    /// The next delivery of file events.
    flush: Option<gpui::Task<()>>,
}

enum Watch {
    Documents { text: bool },
    Files(FileWatch),
}

struct FileWatch {
    glob: String,
    /// Matching files the extension may read, as host paths, from the last listing.
    known: BTreeSet<String>,
    /// The workspace folders and the folders of `known`: a change there may mean files came or went.
    folders: BTreeSet<String>,
    /// Events waiting for the next delivery, one per file.
    pending: BTreeMap<String, FileChange>,
    /// A followed folder changed: list again at the next delivery.
    rescan: bool,
}

/// `new` after `old` for one file, as one event; none when they cancel out.
fn merge(old: Option<FileChange>, new: FileChange) -> Option<FileChange> {
    match (old, new) {
        (Some(FileChange::Created), FileChange::Changed) => Some(FileChange::Created),
        (Some(FileChange::Created), FileChange::Deleted) => None,
        (Some(FileChange::Deleted), FileChange::Created) => Some(FileChange::Changed),
        (_, new) => Some(new),
    }
}

fn parent(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(dir, _)| dir).filter(|d| !d.is_empty())
}

impl Workbench {
    /// `watch.documents`, `watch.files` and `watch.stop` from extension `ext`. The exthost already refused what its
    /// grants do not allow; the app checks again, per event.
    pub(super) fn on_extension_watch(&mut self, ext: &str, n: Notify, cx: &mut Context<Self>) {
        match n {
            Notify::WatchDocuments { id, text } => {
                if self.add_extension_watch(ext, id, Watch::Documents { text }) {
                    self.replay_open_documents(ext, id, text, cx);
                }
            }
            Notify::WatchFiles { id, glob } => {
                if ext_manifest::check_watch_glob(&glob).is_err() {
                    return;
                }
                let watch = FileWatch { glob, known: BTreeSet::new(), folders: BTreeSet::new(), pending: BTreeMap::new(), rescan: false };
                if self.add_extension_watch(ext, id, Watch::Files(watch)) {
                    self.list_watched_files(ext, id, false, cx);
                }
            }
            Notify::StopWatch { id } => {
                if let Some(mine) = self.cfg.exts.watch.watches.get_mut(ext) {
                    mine.remove(&id);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn add_extension_watch(&mut self, ext: &str, id: u64, watch: Watch) -> bool {
        let mine = self.cfg.exts.watch.watches.entry(ext.to_string()).or_default();
        if mine.len() >= limits::WATCHES && !mine.contains_key(&id) {
            return false;
        }
        mine.insert(id, watch);
        true
    }

    /// Whether `ext` has a running watch: its exthost is not stopped as idle, as with a timer.
    pub(super) fn extension_watches(&self, ext: &str) -> bool {
        self.cfg.exts.watch.watches.get(ext).is_some_and(|w| !w.is_empty())
    }

    /// The exthost of `ext` stopped (idle, a crash, a reload): its watches end with it, and its next activation starts
    /// them again.
    pub(super) fn forget_extension_watches(&mut self, ext: &str) {
        self.cfg.exts.watch.watches.remove(ext);
    }

    /// The scope of each running extension with a watch, built once per batch of events.
    fn watch_scopes(&self, cx: &App) -> HashMap<String, Scope> {
        let reg = crate::ext::current();
        self.cfg
            .exts
            .watch
            .watches
            .iter()
            .filter(|(ext, w)| !w.is_empty() && self.cfg.exts.code.procs.contains_key(ext.as_str()))
            .filter_map(|(ext, _)| reg.active.iter().find(|a| a.id() == ext).map(|a| (ext.clone(), self.extension_scope(a, cx))))
            .collect()
    }

    // ---- documents ---------------------------------------------------------------------------------------------------

    /// A new editor: `opened` for document watches (once per file, whatever the groups), then `changed` and `saved` as
    /// it is edited.
    pub(super) fn attach_ext_watch(&mut self, ed: &Entity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        self._subs.push(cx.subscribe_in(ed, window, |this, ed, ev: &EditorEvent, _, cx| match ev {
            EditorEvent::Changed => this.document_changed_soon(ed, cx),
            EditorEvent::Saved => {
                let path = ed.read(cx).path.clone();
                this.cfg.exts.watch.changes.remove(&path);
                this.send_document_event(ed, DocumentChange::Saved, cx);
            }
            _ => {}
        }));
        let path = ed.read(cx).path.clone();
        if self.all_editors().iter().filter(|e| e.read(cx).path == path).count() <= 1 {
            self.send_document_event(ed, DocumentChange::Opened, cx);
        }
    }

    fn document_watches(&self) -> Vec<(String, u64, bool)> {
        let mut out = Vec::new();
        for (ext, watches) in &self.cfg.exts.watch.watches {
            for (id, w) in watches {
                if let Watch::Documents { text } = w {
                    out.push((ext.clone(), *id, *text));
                }
            }
        }
        out
    }

    /// `changed` once the document has not changed for `limits::DOCUMENT_EVENT_DEBOUNCE_MILLIS`.
    fn document_changed_soon(&mut self, ed: &Entity<EditorView>, cx: &mut Context<Self>) {
        if self.document_watches().is_empty() {
            return;
        }
        let path = ed.read(cx).path.clone();
        let weak = ed.downgrade();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(limits::DOCUMENT_EVENT_DEBOUNCE_MILLIS)).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(ed) = weak.upgrade() {
                    this.send_document_event(&ed, DocumentChange::Changed, cx);
                }
            });
        });
        self.cfg.exts.watch.changes.insert(path, task);
    }

    fn document_event(ed: &EditorView, change: DocumentChange, text: bool) -> DocumentEvent {
        // The language id authors match on, as for `documents.active` (workbench_ext_code.rs).
        let language = crate::lsp::language_of(&ed.path).map_or_else(|| ed.language().to_ascii_lowercase(), |(id, _)| id.to_string());
        // A document larger than one call comes without its text; `workspace.read-file` gets it.
        let text = text.then(|| ed.text()).filter(|t| t.len() <= limits::CALL_BYTES);
        DocumentEvent { change, uri: ext_broker::uri(&ed.path), language, version: ed.lsp_buffer_version() as u32, text }
    }

    /// Send one document event to every document watch whose extension may read the file.
    fn send_document_event(&mut self, ed: &Entity<EditorView>, change: DocumentChange, cx: &mut Context<Self>) {
        let watchers = self.document_watches();
        if watchers.is_empty() {
            return;
        }
        let (path, version) = {
            let e = ed.read(cx);
            (e.path.clone(), e.lsp_buffer_version() as u32)
        };
        if change != DocumentChange::Opened {
            let before = self.cfg.exts.watch.sent.insert(path.clone(), version);
            if change == DocumentChange::Changed && before == Some(version) {
                return;
            }
        }
        let Some(normalized) = ext_broker::normalize(&path) else { return };
        let scopes = self.watch_scopes(cx);
        for (ext, watch, text) in watchers {
            let (Some(scope), Some(p)) = (scopes.get(&ext), self.cfg.exts.code.procs.get(&ext)) else { continue };
            if scope.may_read(&normalized).is_err() {
                continue;
            }
            let event = Self::document_event(ed.read(cx), change, text);
            let _ = p.send(&ToHost::Documents { watch, events: vec![event] });
        }
    }

    /// A new document watch: `opened` for each document already open that the extension may read.
    fn replay_open_documents(&mut self, ext: &str, watch: u64, text: bool, cx: &mut Context<Self>) {
        let scopes = self.watch_scopes(cx);
        let (Some(scope), Some(p)) = (scopes.get(ext), self.cfg.exts.code.procs.get(ext).cloned()) else { return };
        let mut seen = BTreeSet::new();
        for ed in self.all_editors() {
            let e = ed.read(cx);
            let readable = ext_broker::normalize(&e.path).is_some_and(|n| scope.may_read(&n).is_ok());
            if readable && seen.insert(e.path.clone()) {
                let _ = p.send(&ToHost::Documents { watch, events: vec![Self::document_event(e, DocumentChange::Opened, text)] });
            }
        }
    }

    // ---- files -------------------------------------------------------------------------------------------------------

    /// List the workspace folders for file watch `id` of `ext` in the background. The first listing is what later ones
    /// compare with; a later one (`rescan`) reports the files that came or went.
    fn list_watched_files(&mut self, ext: &str, id: u64, rescan: bool, cx: &mut Context<Self>) {
        let roots = self.all_roots(cx);
        let host = self.remote.host.clone();
        let listing = cx.background_executor().spawn(async move {
            let mut files = Vec::new();
            for root in &roots {
                let base = root.trim_end_matches('/');
                if let Ok(list) = host.list_files(root, LISTED_FILES) {
                    files.extend(list.paths.iter().map(|p| format!("{base}/{p}")));
                }
            }
            (roots, files)
        });
        let ext = ext.to_string();
        cx.spawn(async move |this, cx| {
            let (roots, files) = listing.await;
            let _ = this.update(cx, |this, cx| this.watched_files_listed(&ext, id, roots, files, rescan, cx));
        })
        .detach();
    }

    fn watched_files_listed(&mut self, ext: &str, id: u64, roots: Vec<String>, files: Vec<String>, rescan: bool, cx: &mut Context<Self>) {
        let Some(scope) = self.watch_scopes(cx).remove(ext) else { return };
        let Some(Watch::Files(w)) = self.cfg.exts.watch.watches.get_mut(ext).and_then(|m| m.get_mut(&id)) else { return };
        let matching: BTreeSet<String> = files.into_iter().filter(|f| follows(&scope, &w.glob, f)).collect();
        if rescan {
            for gone in w.known.difference(&matching) {
                let old = w.pending.remove(gone);
                if let Some(c) = merge(old, FileChange::Deleted) {
                    w.pending.insert(gone.clone(), c);
                }
            }
            for new in matching.difference(&w.known) {
                let old = w.pending.remove(new);
                if let Some(c) = merge(old, FileChange::Created) {
                    w.pending.insert(new.clone(), c);
                }
            }
        }
        w.folders =
            roots.into_iter().map(|r| r.trim_end_matches('/').to_string()).chain(matching.iter().filter_map(|f| parent(f).map(str::to_string))).collect();
        w.known = matching;
        let pending = !w.pending.is_empty();
        if pending {
            self.schedule_file_events(cx);
        }
        // The watched set grows by the new files and folders at the next render (`sync_watches`).
        cx.notify();
    }

    /// The paths file watches follow, for the window's watcher: the workspace folders and the folders of matching
    /// files first (they show files that come and go), then the files, at most `budget` in all.
    pub(super) fn ext_watched_paths(&self, budget: usize) -> Vec<String> {
        let budget = budget.min(limits::WATCHED_PATHS);
        let mut folders = BTreeSet::new();
        let mut files = BTreeSet::new();
        for w in self.cfg.exts.watch.watches.values().flat_map(|m| m.values()) {
            if let Watch::Files(w) = w {
                folders.extend(w.folders.iter().cloned());
                files.extend(w.known.iter().cloned());
            }
        }
        folders.into_iter().chain(files).take(budget).collect()
    }

    /// The window's watcher saw `changes`: a followed file changed or went, or a followed folder changed (list it
    /// again). Delivered together at most every `limits::FILE_EVENT_INTERVAL_MILLIS`.
    pub(super) fn ext_files_changed(&mut self, changes: &host::FsChanges, cx: &mut Context<Self>) {
        let mut any = false;
        for w in self.cfg.exts.watch.watches.values_mut().flat_map(|m| m.values_mut()) {
            let Watch::Files(w) = w else { continue };
            for (path, kind) in changes {
                if w.known.contains(path) {
                    let change = if *kind == proto::ChangeKind::Removed { FileChange::Deleted } else { FileChange::Changed };
                    let old = w.pending.remove(path);
                    if let Some(c) = merge(old, change) {
                        w.pending.insert(path.clone(), c);
                    }
                    if change == FileChange::Deleted {
                        w.known.remove(path);
                    }
                    any = true;
                } else if w.folders.contains(path.trim_end_matches('/')) {
                    w.rescan = true;
                    any = true;
                }
            }
        }
        if any {
            self.schedule_file_events(cx);
        }
    }

    fn schedule_file_events(&mut self, cx: &mut Context<Self>) {
        if self.cfg.exts.watch.flush.is_some() {
            return;
        }
        self.cfg.exts.watch.flush = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(limits::FILE_EVENT_INTERVAL_MILLIS)).await;
            let _ = this.update(cx, |this, cx| {
                this.cfg.exts.watch.flush = None;
                this.deliver_file_events(cx);
            });
        }));
    }

    /// Send what file watches gathered, checked again against what each extension may read now, and list the folders
    /// that changed (their events come with the next delivery).
    fn deliver_file_events(&mut self, cx: &mut Context<Self>) {
        let scopes = self.watch_scopes(cx);
        let mut rescans = Vec::new();
        let mut sends = Vec::new();
        for (ext, watches) in self.cfg.exts.watch.watches.iter_mut() {
            for (id, w) in watches.iter_mut() {
                let Watch::Files(w) = w else { continue };
                if std::mem::take(&mut w.rescan) {
                    rescans.push((ext.clone(), *id));
                }
                let pending = std::mem::take(&mut w.pending);
                let Some(scope) = scopes.get(ext) else { continue };
                let events: Vec<FileEvent> = pending
                    .into_iter()
                    .filter(|(path, _)| follows(scope, &w.glob, path))
                    .map(|(path, change)| FileEvent { change, uri: ext_broker::uri(&path) })
                    .collect();
                if !events.is_empty() {
                    sends.push((ext.clone(), *id, events));
                }
            }
        }
        for (ext, watch, events) in sends {
            let Some(p) = self.cfg.exts.code.procs.get(&ext) else { continue };
            for chunk in events.chunks(limits::WATCH_EVENTS) {
                let _ = p.send(&ToHost::Files { watch, events: chunk.to_vec() });
            }
        }
        for (ext, id) in rescans {
            self.list_watched_files(&ext, id, true, cx);
        }
    }
}

/// Whether a file watch with `glob` reports `path`: it matches the glob relative to its workspace folder, and the
/// extension may read it.
fn follows(scope: &Scope, glob: &str, path: &str) -> bool {
    scope.relative(path).is_some_and(|rel| crate::globs::path_matches(glob, &rel)) && scope.may_read(path).is_ok()
}

#[cfg(test)]
impl Workbench {
    /// The paths the window's file watcher follows.
    pub(crate) fn watched_for_test(&self) -> Vec<String> {
        self.watched.clone()
    }

    pub(crate) fn extension_watching_for_test(&self, extension: &str) -> bool {
        self.extension_watches(extension)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_of_one_file_merge_into_one() {
        assert_eq!(merge(None, FileChange::Changed), Some(FileChange::Changed));
        assert_eq!(merge(Some(FileChange::Created), FileChange::Changed), Some(FileChange::Created));
        assert_eq!(merge(Some(FileChange::Created), FileChange::Deleted), None, "came and went between deliveries");
        assert_eq!(merge(Some(FileChange::Deleted), FileChange::Created), Some(FileChange::Changed));
        assert_eq!(merge(Some(FileChange::Changed), FileChange::Deleted), Some(FileChange::Deleted));
        assert_eq!(parent("/work/src/a.rs"), Some("/work/src"));
        assert_eq!(parent("a.rs"), None);
    }
}
