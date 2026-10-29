//! Workbench: a named workspace opened from a workspace file. Its folders open in this window in the file's order and
//! under their display names; its shared settings sit between the user's settings and each folder's own; its tasks and
//! launch configurations join the folder's. Adding or removing a folder writes the file back through the host
//! (atomically, refusing when someone else changed it meanwhile) and only the folder list changes in the text. A file
//! that cannot be read is never written. The file is a Recents entry of its own: restarting (or reopening it from
//! Recents) reads it again, while opening one of its folders directly opens just that folder.

use std::sync::Arc;

use gpui::{AppContext as _, Context, Window};
use host::Host;

use super::{Notice, Workbench};
use crate::ui;
use crate::workspace_file::{Document, Edit, Target};

/// A folder listed in the file that is not open.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Unavailable {
    pub(super) item: usize,
    pub(super) label: String,
    /// Resolved path, or what the file says when it does not resolve.
    pub(super) path: String,
}

/// The workspace file this window was opened through.
pub(super) struct OpenWorkspace {
    /// Path of the file on `host`.
    pub(super) path: String,
    pub(super) host: Arc<dyn Host>,
    /// The last version read or written: the base of the next save.
    pub(super) doc: Document,
    /// Hash of `doc` on disk; a save expects the disk to still have it.
    hash: u64,
    /// The latest read failed: nothing is written until a reload succeeds.
    pub(super) error: Option<String>,
    /// Open root for each folder entry (`Folder::item`), as the Explorer has it.
    pub(super) opened: Vec<(usize, String)>,
    pub(super) unavailable: Vec<Unavailable>,
    /// Open roots the file does not list (the first folder after the file dropped it): saving does not add them back.
    unlisted: Vec<String>,
    /// Entries removed in this window, written out by the next save.
    removed: Vec<usize>,
    /// Bumped whenever the file is applied again (open, reload, Save As): older reload and save replies are dropped.
    generation: u64,
    /// A write is running; another change waits for it.
    saving: bool,
    save_again: bool,
    /// The text of the running write: when the watcher reports it, it is our own and changes nothing.
    writing: Option<String>,
    /// Writes started so far: a watcher read that saw one start (or run) may hold the text from before it.
    writes: u64,
    /// The watcher saw a change while a write ran: read again once it is done.
    reload_after_save: bool,
    /// The file's own folders are being opened: not an edit to save.
    applying: bool,
}

/// What a background read of the file found.
struct Loaded {
    doc: Document,
    hash: u64,
    /// (item, path to open) for each folder that exists; local paths are canonical.
    available: Vec<(usize, String)>,
    unavailable: Vec<Unavailable>,
}

/// Folder of `path` (either separator).
fn parent_dir(path: &str) -> String {
    match path.rfind(['/', '\\']) {
        Some(0) => path[..1].to_string(),
        Some(i) => path[..i].to_string(),
        None => ".".to_string(),
    }
}

fn unavailable(f: &crate::workspace_file::Folder) -> Unavailable {
    Unavailable {
        item: f.item,
        label: f.label(),
        path: f.path().map(str::to_string).unwrap_or_else(|| match &f.source {
            crate::workspace_file::Source::Path(s) | crate::workspace_file::Source::Uri(s) => s.clone(),
        }),
    }
}

/// Read and resolve the file on `host` (run in the background). `Err`: the reason for "Workspace file could not be read: {}".
fn load(host: &dyn Host, path: &str) -> Result<Loaded, String> {
    let (bytes, hash) = host.read(path).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| crate::i18n::t("the file is not UTF-8 text").to_string())?;
    let doc = Document::parse(&text, &parent_dir(path))?;
    let (mut available, mut missing) = (Vec::new(), Vec::new());
    for f in doc.folders() {
        let open = match &f.target {
            // A directory, or a link to one (`read_dir` follows it); a file or a broken link is unavailable.
            Target::Path(p) => {
                let is_dir = host.stat(p).is_ok_and(|m| m.kind == proto::Kind::Dir || (m.kind == proto::Kind::Symlink && host.read_dir(p).is_ok()));
                is_dir.then(|| if host.is_remote() { p.clone() } else { std::fs::canonicalize(p).map(|c| ui::local_path(&c)).unwrap_or_else(|_| p.clone()) })
            }
            Target::Unsupported => None,
        };
        match open {
            Some(p) if !available.iter().any(|(_, q)| *q == p) => available.push((f.item, p)),
            // The same folder twice: the first entry opens it, the second stays in the file.
            Some(_) => {}
            None => missing.push(unavailable(f)),
        }
    }
    Ok(Loaded { doc, hash, available, unavailable: missing })
}

impl Workbench {
    /// A new request that replaces whatever workspace was being opened: replies of older ones are dropped.
    fn next_workspace_request(&mut self) -> u64 {
        self.roots.workspace_request += 1;
        self.roots.workspace_request
    }

    /// Open a workspace file on the window's host: its first folder that exists becomes the root, the others join it.
    pub(super) fn open_workspace_file(&mut self, path: &str, _: &mut Window, cx: &mut Context<Self>) {
        let request = self.next_workspace_request();
        let host = self.remote.host.clone();
        let (h, p) = (host.clone(), path.to_string());
        let task = cx.background_executor().spawn(async move { load(h.as_ref(), &p) });
        let path = path.to_string();
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            with_window(&this, cx, move |this, window, cx| {
                // Another open, another folder or another host since: this reply is stale.
                if this.roots.workspace_request != request || !Arc::ptr_eq(&this.remote.host, &host) {
                    return;
                }
                match loaded {
                    Err(e) => {
                        this.notice = Some(Notice::Error(crate::i18n::tf("Workspace file could not be read: {}", &[&e]).into()));
                        // Shown so it can be repaired; the file is not touched.
                        if this.root(cx).is_none() && host.stat(&path).is_ok() {
                            this.open_file_on(host, path, window, cx);
                        }
                        cx.notify();
                    }
                    Ok(loaded) => this.open_loaded_workspace(path, host, loaded, window, cx),
                }
            });
        })
        .detach();
    }

    fn open_loaded_workspace(&mut self, path: String, host: Arc<dyn Host>, loaded: Loaded, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, first)) = loaded.available.first().cloned() else {
            self.notice = Some(Notice::Error(crate::i18n::t("The workspace file lists no folder that exists on this computer.").into()));
            cx.notify();
            return;
        };
        self.set_root(first.clone(), cx);
        if self.root(cx).as_deref() != Some(first.as_str()) {
            return; // Refused (unsaved files).
        }
        // `set_root` started restoring the folder's own extra folders: the file decides instead.
        self.persist.pending_restore_workspace = None;
        self.attach_workspace(path, host, loaded, true, window, cx);
        self.remember_workspace_file(cx);
    }

    /// Make `loaded` the window's workspace: the root is already open; extra folders follow the file.
    fn attach_workspace(&mut self, path: String, host: Arc<dyn Host>, loaded: Loaded, report: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.roots.workspace_generation += 1;
        let generation = self.roots.workspace_generation;
        let Loaded { doc, hash, available, unavailable: mut missing } = loaded;
        self.roots.workspace = Some(OpenWorkspace {
            path,
            host,
            doc,
            hash,
            error: None,
            opened: Vec::new(),
            unavailable: Vec::new(),
            unlisted: Vec::new(),
            removed: Vec::new(),
            generation,
            saving: false,
            save_again: false,
            writing: None,
            writes: 0,
            reload_after_save: false,
            applying: true,
        });
        self.roots.pending_extra_roots.clear();
        let primary = self.root(cx).unwrap_or_default();
        let wanted: Vec<String> = available.iter().map(|(_, p)| p.clone()).filter(|p| *p != primary).collect();
        for extra in self.roots.extra_roots.clone() {
            if !wanted.contains(&extra) {
                self.remove_workspace_folder(&extra, cx);
            }
        }
        let mut skipped = Vec::new();
        for p in &wanted {
            if self.roots.extra_roots.contains(p) {
                continue;
            }
            // Past the limit nothing is tried (no refusal notice per folder); the rest are reported together below.
            let full = self.roots.extra_roots.len() >= super::folder_glue::MAX_EXTRA_ROOTS;
            if full || !self.add_workspace_folder_now(p.clone(), window, cx) {
                skipped.push(p.clone());
            }
        }
        // Extra folders in the file's order.
        let order = |r: &String| wanted.iter().position(|w| w == r).unwrap_or(usize::MAX);
        let mut pairs: Vec<(String, gpui::Entity<crate::tree::FileTree>)> = self.roots.extra_roots.drain(..).zip(self.roots.extra_trees.drain(..)).collect();
        pairs.sort_by_key(|(r, _)| order(r));
        (self.roots.extra_roots, self.roots.extra_trees) = pairs.into_iter().unzip();
        self.roots.file_index = None;
        let open_roots = self.all_roots(cx);
        let mut over_limit = false;
        if let Some(ws) = self.roots.workspace.as_mut() {
            // Folders that did not fit stay listed as unavailable, so they are visible and stay in the file.
            for (item, p) in available.iter().filter(|(_, p)| skipped.contains(p)) {
                if let Some(f) = ws.doc.folders().iter().find(|f| f.item == *item) {
                    missing.push(Unavailable { path: p.clone(), ..unavailable(f) });
                    over_limit = true;
                }
            }
            missing.sort_by_key(|u| u.item);
            ws.unavailable = missing;
            ws.unlisted = open_roots.iter().filter(|r| !available.iter().any(|(_, p)| p == *r)).cloned().collect();
            ws.opened = available.into_iter().filter(|(_, p)| open_roots.contains(p) && !skipped.contains(p)).collect();
            ws.applying = false;
        }
        self.after_workspace_change(report, window, cx);
        if over_limit {
            let limit = crate::i18n::tf("At most {} extra folders fit in one workspace.", &[&super::folder_glue::MAX_EXTRA_ROOTS]);
            self.notice = Some(Notice::Error(format!("{limit} {}", self.notice_text().unwrap_or_default()).trim().to_string().into()));
        }
    }

    /// Settings, folder settings, search roots and the notice after the folders or the file changed.
    fn after_workspace_change(&mut self, report: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_search_roots(cx);
        // Reloading settings replaces the notice with its own warnings: an earlier message (a conflict) stays otherwise.
        let earlier = self.notice.take();
        self.reload_config(window, cx);
        if self.notice.is_none() {
            self.notice = earlier;
        }
        self.resolve_folder_settings(cx);
        if let Some(ws) = self.roots.workspace.as_ref()
            && report
        {
            self.notice = if !ws.unavailable.is_empty() {
                let names: Vec<String> = ws.unavailable.iter().map(|u| u.label.clone()).collect();
                Some(Notice::Error(crate::i18n::tf("Workspace folder is unavailable: {}", &[&names.join(", ")]).into()))
            } else {
                let n = ws.opened.len();
                Some(Notice::Info(crate::i18n::tn("Workspace: {} folder in the Explorer.", "Workspace: {} folders in the Explorer.", n, &[]).into()))
            };
        }
        cx.notify();
    }

    /// The workspace file goes to Recents as an entry of its own (after its first folder, so it leads the list): a
    /// restart or "continue" reopens the workspace, while opening one of its folders opens only that folder.
    fn remember_workspace_file(&mut self, cx: &mut Context<Self>) {
        let alias = self.host_alias();
        if alias.as_deref() == Some(super::LOCAL_AGENT_ALIAS) {
            return;
        }
        if let (Some(s), Some(ws)) = (&self.persist.state, &self.roots.workspace) {
            let _ = s.record_workspace(alias.as_deref(), &ws.path);
            self.reload_recent(cx);
        }
    }

    /// "File: Reload Workspace" (`quiet`: the watcher saw the file change): read the file again and apply its folders,
    /// names and settings. A file that cannot be read leaves the window as it is and blocks saving until it can be read.
    pub(super) fn reload_workspace(&mut self, quiet: bool, cx: &mut Context<Self>) {
        let request = self.roots.workspace_request;
        // An explicit reload supersedes earlier reads and writes; the watcher's own check does not.
        let bumped = self.roots.workspace_generation + 1;
        let Some(ws) = self.roots.workspace.as_mut() else {
            if !quiet {
                self.notice = Some(Notice::Info(crate::i18n::t("No workspace file is open.").into()));
                cx.notify();
            }
            return;
        };
        if !quiet {
            ws.generation = bumped;
            self.roots.workspace_generation = bumped;
        } else if ws.saving {
            // Read once our write is done: a read now may land before it and bring back the text it replaces.
            ws.reload_after_save = true;
            return;
        }
        let (generation, host, path, writes) = (ws.generation, ws.host.clone(), ws.path.clone(), ws.writes);
        let (h, p) = (host.clone(), path.clone());
        let task = cx.background_executor().spawn(async move { load(h.as_ref(), &p) });
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            with_window(&this, cx, move |this, window, cx| {
                if this.roots.workspace_request != request {
                    return;
                }
                let Some(ws) = this.roots.workspace.as_mut().filter(|w| w.generation == generation && w.path == path && Arc::ptr_eq(&w.host, &host)) else {
                    return;
                };
                if quiet && (ws.saving || ws.writes != writes) {
                    // A write started after this read did: what it read may predate that write. Read again after it.
                    let saving = ws.saving;
                    ws.reload_after_save |= saving;
                    if !saving {
                        this.reload_workspace(true, cx);
                    }
                    return;
                }
                match loaded {
                    Err(e) => {
                        ws.error = Some(e.clone());
                        this.notice = Some(Notice::Error(crate::i18n::tf("Workspace file could not be read: {}", &[&e]).into()));
                        cx.notify();
                    }
                    // What this window wrote, or is writing: nothing changed.
                    Ok(loaded) if quiet && ws.error.is_none() && loaded.doc.text() == ws.doc.text() => ws.hash = loaded.hash,
                    Ok(loaded) if quiet && ws.writing.as_deref() == Some(loaded.doc.text()) => {}
                    Ok(loaded) => {
                        let primary = this.root(cx);
                        let dropped_primary = !loaded.available.iter().any(|(_, p)| Some(p) == primary.as_ref());
                        this.attach_workspace(path, host, loaded, !quiet, window, cx);
                        if dropped_primary {
                            // It stays open (closing it closes the editors); saving does not add it back.
                            this.notice = Some(Notice::Info(crate::i18n::t("The open folder is no longer listed in the workspace file.").into()));
                        }
                    }
                }
            });
        })
        .detach();
    }

    /// The folder list as it is now in this window, as edits of the file's list: entries keep their place (unavailable
    /// ones included) unless removed here; folders added here go at the end.
    fn workspace_edits(&self, cx: &gpui::App) -> Option<Vec<Edit>> {
        let ws = self.roots.workspace.as_ref()?;
        let mut edits: Vec<Edit> = ws.doc.folders().iter().filter(|f| !ws.removed.contains(&f.item)).map(|f| Edit::Keep(f.item)).collect();
        for r in self.all_roots(cx) {
            if !ws.opened.iter().any(|(_, p)| *p == r) && !ws.unlisted.contains(&r) {
                edits.push(Edit::Add { path: r, name: None });
            }
        }
        Some(edits)
    }

    /// Write the folder list back to the workspace file (after Add/Remove Folder, or "File: Save Workspace"). Nothing is
    /// written when the text would not change, when the file could not be read, or when it changed on disk meanwhile.
    pub(super) fn save_workspace(&mut self, cx: &mut Context<Self>) {
        let request = self.roots.workspace_request;
        let Some(edits) = self.workspace_edits(cx) else { return };
        let Some(ws) = self.roots.workspace.as_mut() else { return };
        if ws.applying {
            return;
        }
        if let Some(e) = &ws.error {
            self.notice = Some(Notice::Error(crate::i18n::tf("Workspace file could not be read: {}", &[e]).into()));
            cx.notify();
            return;
        }
        if ws.saving {
            ws.save_again = true;
            return;
        }
        let next = match ws.doc.with_folders(&edits) {
            Ok(d) => d,
            Err(e) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("Workspace file could not be read: {}", &[&e]).into()));
                cx.notify();
                return;
            }
        };
        if next.text() == ws.doc.text() {
            return;
        }
        ws.saving = true;
        ws.writes += 1;
        ws.writing = Some(next.text().to_string());
        let (generation, host, path, hash) = (ws.generation, ws.host.clone(), ws.path.clone(), ws.hash);
        let bytes = next.text().as_bytes().to_vec();
        let h = host.clone();
        let p = path.clone();
        let task = cx.background_executor().spawn(async move { h.write(&p, bytes, Some(hash)) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                // Another workspace or folder since: nothing here belongs to it.
                if this.roots.workspace_request != request {
                    return;
                }
                let current = this.roots.workspace.as_mut().filter(|w| w.path == path && Arc::ptr_eq(&w.host, &host));
                let again = current.as_ref().is_some_and(|w| w.save_again);
                let same_generation = current.as_ref().is_some_and(|w| w.generation == generation);
                let mut reread = false;
                if let Some(w) = current {
                    w.saving = false;
                    w.save_again = false;
                    w.writing = None;
                    reread = std::mem::take(&mut w.reload_after_save);
                }
                let open_now = this.all_roots(cx);
                match result {
                    Ok(new_hash) if same_generation => {
                        if let Some(w) = this.roots.workspace.as_mut() {
                            let remapped = remap(&edits, &w.opened, |e| match e {
                                Edit::Add { path, .. } => Some(path.clone()),
                                Edit::Keep(_) => None,
                            });
                            // A folder removed while this write ran is removed by the next one, under its new position.
                            let (still, gone): (Vec<_>, Vec<_>) = remapped.into_iter().partition(|(_, p)| open_now.contains(p));
                            let carried = w.removed.iter().filter_map(|old| edits.iter().position(|e| *e == Edit::Keep(*old)));
                            let removed: Vec<usize> = gone.into_iter().map(|(i, _)| i).chain(carried).collect();
                            w.opened = still;
                            w.unavailable = w
                                .unavailable
                                .iter()
                                .filter_map(|u| edits.iter().position(|e| *e == Edit::Keep(u.item)).map(|i| Unavailable { item: i, ..u.clone() }))
                                .collect();
                            w.doc = next;
                            w.hash = new_hash;
                            w.removed = removed;
                        }
                    }
                    // The file was applied again while this write ran: read what is on disk now, so the next save
                    // expects the right content instead of reporting a conflict with itself.
                    Ok(_) => this.reload_workspace(true, cx),
                    Err(host::HostError::Op(proto::Error::Conflict { .. })) => {
                        this.notice = Some(Notice::Error(crate::i18n::t("Workspace changed on disk. Reload before saving.").into()));
                    }
                    Err(e) => {
                        this.notice = Some(Notice::Error(crate::i18n::tf("Could not save the workspace file: {}", &[&e]).into()));
                    }
                }
                if again {
                    this.save_workspace(cx);
                }
                // While the next write runs this only marks it, so the file is read after that one.
                if reread {
                    this.reload_workspace(true, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// "File: Open Active File as Workspace": the workspace file open in the editor, on its host (the way to open one
    /// that lives on an SSH server).
    pub(super) fn open_active_workspace_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active_editor().map(|e| {
            let e = e.read(cx);
            (e.path.clone(), e.uses_host(&self.remote.host))
        });
        match active {
            Some((path, true)) if crate::workspace_file::is_workspace_file(&path) => self.open_workspace_file(&path, window, cx),
            _ => {
                self.notice = Some(Notice::Info(crate::i18n::t("Open a .code-workspace file in the editor first.").into()));
                cx.notify();
            }
        }
    }

    /// "File: Save Workspace".
    pub(super) fn save_workspace_now(&mut self, cx: &mut Context<Self>) {
        if self.roots.workspace.is_none() {
            self.notice = Some(Notice::Info(crate::i18n::t("No workspace file is open. Use File: Save Workspace As… to create one.").into()));
            cx.notify();
            return;
        }
        self.save_workspace(cx);
    }

    /// Save the workspace (this window's folders, and the open file's other content) as a new file at `target` on the
    /// window's host: relative folder paths are rewritten for the new place. Without an open workspace file, a new one is made.
    pub(super) fn save_workspace_as(&mut self, target: String, cx: &mut Context<Self>) {
        if self.root(cx).is_none() {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a folder first; folders are added to it.").into()));
            cx.notify();
            return;
        }
        let new_dir = parent_dir(&target);
        let added = |e: &Edit| match e {
            Edit::Add { path, .. } => Some(path.clone()),
            Edit::Keep(_) => None,
        };
        let built = match self.roots.workspace.as_ref() {
            Some(ws) => match (&ws.error, self.workspace_edits(cx)) {
                (Some(e), _) => Err(e.clone()),
                (None, None) => Err(String::new()),
                (None, Some(edits)) => ws.doc.with_folders(&edits).and_then(|d| d.rebased(&new_dir)).map(|d| {
                    let opened = remap(&edits, &ws.opened, added);
                    let unavailable = ws
                        .unavailable
                        .iter()
                        .filter_map(|u| edits.iter().position(|e| *e == Edit::Keep(u.item)).map(|i| Unavailable { item: i, ..u.clone() }))
                        .collect();
                    (d, opened, unavailable)
                }),
            },
            None => {
                let edits: Vec<Edit> = self.all_roots(cx).into_iter().map(|path| Edit::Add { path, name: None }).collect();
                Document::empty(&new_dir).with_folders(&edits).map(|d| (d, remap(&edits, &[], added), Vec::new()))
            }
        };
        let (doc, opened, unavailable) = match built {
            Ok(d) => d,
            Err(e) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("Workspace file could not be read: {}", &[&e]).into()));
                cx.notify();
                return;
            }
        };
        let host = self.remote.host.clone();
        // Saving over the open file itself still refuses to replace someone else's edit.
        let expect = self.roots.workspace.as_ref().filter(|w| w.path == target && Arc::ptr_eq(&w.host, &host)).map(|w| w.hash);
        let unlisted = self.roots.workspace.as_ref().map(|w| w.unlisted.clone()).unwrap_or_default();
        // The window this file describes: if the folder or workspace changes before the write ends, it is not attached.
        let (request, root) = (self.roots.workspace_request, self.root(cx));
        let (h, t, bytes) = (host.clone(), target.clone(), doc.text().as_bytes().to_vec());
        let task = cx.background_executor().spawn(async move { h.write(&t, bytes, expect) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, move |this, cx| {
                let same_window = this.roots.workspace_request == request && this.root(cx) == root && Arc::ptr_eq(&this.remote.host, &host);
                match result {
                    Ok(hash) if same_window => {
                        this.roots.workspace_generation += 1;
                        let generation = this.roots.workspace_generation;
                        this.roots.workspace = Some(OpenWorkspace {
                            path: target.clone(),
                            host,
                            doc,
                            hash,
                            error: None,
                            opened,
                            unavailable,
                            unlisted,
                            removed: Vec::new(),
                            generation,
                            saving: false,
                            save_again: false,
                            writing: None,
                            writes: 0,
                            reload_after_save: false,
                            applying: false,
                        });
                        this.remember_workspace_file(cx);
                        this.notice = Some(Notice::Info(crate::i18n::tf("Workspace saved as {}", &[&ui::file_name(&target)]).into()));
                    }
                    // Written, but the window shows something else now: the new file is left as written, not attached.
                    Ok(_) => {}
                    Err(host::HostError::Op(proto::Error::Conflict { .. })) => {
                        this.notice = Some(Notice::Error(crate::i18n::t("Workspace changed on disk. Reload before saving.").into()));
                    }
                    Err(e) => this.notice = Some(Notice::Error(crate::i18n::tf("Could not save the workspace file: {}", &[&e]).into())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// "File: Save Workspace As…": a native save dialog (this computer only).
    pub(super) fn pick_save_workspace_as(&mut self, _: &super::SaveWorkspaceAs, _: &mut Window, cx: &mut Context<Self>) {
        if self.remote.host.is_remote() {
            self.notice = Some(Notice::Info(crate::i18n::t("Save Workspace As… needs a folder on this computer.").into()));
            cx.notify();
            return;
        }
        let Some(root) = self.root(cx) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a folder first; folders are added to it.").into()));
            cx.notify();
            return;
        };
        let (dir, name) = match self.roots.workspace.as_ref() {
            Some(ws) => (parent_dir(&ws.path), ui::file_name(&ws.path).to_string()),
            None => (parent_dir(&root), format!("{}.code-workspace", ui::file_name(&root))),
        };
        let rx = cx.prompt_for_new_path(std::path::Path::new(&dir), Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let mut path = ui::local_path(&path);
                if !crate::workspace_file::is_workspace_file(&path) {
                    path.push_str(".code-workspace");
                }
                this.update(cx, |this, cx| this.save_workspace_as(path, cx)).ok();
            }
        })
        .detach();
    }

    /// Display name of an open root: its name in the workspace file, else the folder's own name.
    pub(super) fn root_label(&self, root: &str) -> String {
        self.roots
            .workspace
            .as_ref()
            .and_then(|ws| {
                let item = ws.opened.iter().find(|(_, p)| p == root)?.0;
                ws.doc.folders().iter().find(|f| f.item == item).map(|f| f.label())
            })
            .unwrap_or_else(|| ui::file_name(root).to_string())
    }

    /// Remove a folder the file lists but that is not available (the Explorer's "Remove"); the file is saved.
    pub(super) fn remove_unavailable_folder(&mut self, item: usize, cx: &mut Context<Self>) {
        let Some(ws) = self.roots.workspace.as_mut() else { return };
        if !ws.unavailable.iter().any(|u| u.item == item) {
            return; // A stale click: the list changed since.
        }
        ws.unavailable.retain(|u| u.item != item);
        ws.removed.push(item);
        self.save_workspace(cx);
        cx.notify();
    }

    /// A folder left the window (Remove Folder from Workspace): drop it from the file too.
    pub(super) fn workspace_folder_removed(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(ws) = self.roots.workspace.as_mut() else { return };
        if ws.applying {
            return;
        }
        if let Some(i) = ws.opened.iter().position(|(_, p)| p == path) {
            let (item, _) = ws.opened.remove(i);
            ws.removed.push(item);
        }
        self.save_workspace(cx);
    }

    /// A folder joined the window: add it to the file.
    pub(super) fn workspace_folder_added(&mut self, cx: &mut Context<Self>) {
        if self.roots.workspace.as_ref().is_some_and(|w| !w.applying) {
            self.save_workspace(cx);
        }
    }

    /// The workspace file's folder when it defines tasks or debug configurations and is not trusted yet: running a task
    /// or the debugger asks for it as well.
    pub(super) fn untrusted_workspace_dir(&self, cx: &gpui::App) -> Option<String> {
        let ws = self.roots.workspace.as_ref()?;
        if !ws.doc.defines_commands() || !Arc::ptr_eq(&ws.host, &self.remote.host) {
            return None;
        }
        let dir = ws.doc.dir().to_string();
        (!self.is_trusted_root(&dir, cx)).then_some(dir)
    }

    /// Whether the workspace file's settings may set `RESTRICTED_KEYS`.
    pub(super) fn workspace_settings_trusted(&self, cx: &gpui::App) -> bool {
        self.roots.workspace.as_ref().is_none_or(|ws| self.is_trusted_root(ws.doc.dir(), cx))
    }

    /// The workspace file's shared settings (a layer between the user's and each folder's).
    pub(super) fn workspace_settings(&self) -> Option<(String, String)> {
        let ws = self.roots.workspace.as_ref()?;
        Some((ui::file_name(&ws.path).to_string(), ws.doc.settings_json()?))
    }

    /// The workspace file's shared tasks and launch sections as task / launch file texts.
    pub(super) fn workspace_tasks_and_launch(&self) -> (Option<String>, Option<String>) {
        match self.roots.workspace.as_ref().filter(|ws| Arc::ptr_eq(&ws.host, &self.remote.host)) {
            Some(ws) => (ws.doc.tasks_json(), ws.doc.launch_json()),
            None => (None, None),
        }
    }

    /// The workspace file's path when this change event touches it.
    pub(super) fn workspace_file_changed(&self, changes: &host::FsChanges) -> bool {
        // Also while a write runs: our own write is recognized by its text, anyone else's is not missed.
        self.roots.workspace.as_ref().is_some_and(|ws| changes.iter().any(|(p, _)| *p == ws.path))
    }

    pub(super) fn workspace_watch_path(&self) -> Option<String> {
        self.roots.workspace.as_ref().filter(|ws| Arc::ptr_eq(&ws.host, &self.remote.host)).map(|ws| ws.path.clone())
    }

    #[cfg(test)]
    pub fn workspace_snapshot_for_test(&self, cx: &gpui::App) -> Option<(String, Vec<String>, Vec<String>, Option<String>)> {
        let ws = self.roots.workspace.as_ref()?;
        let labels = self.all_roots(cx).iter().map(|r| self.root_label(r)).collect();
        let missing = ws.unavailable.iter().map(|u| u.label.clone()).collect();
        Some((ws.path.clone(), labels, missing, ws.error.clone()))
    }

    #[cfg(test)]
    pub fn unavailable_items_for_test(&self) -> Vec<usize> {
        self.roots.workspace.as_ref().map(|ws| ws.unavailable.iter().map(|u| u.item).collect()).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn save_workspace_for_test(&mut self, cx: &mut Context<Self>) {
        self.save_workspace(cx);
    }

    #[cfg(test)]
    pub fn save_workspace_as_for_test(&mut self, target: String, cx: &mut Context<Self>) {
        self.save_workspace_as(target, cx);
    }

    #[cfg(test)]
    pub fn reload_workspace_for_test(&mut self, cx: &mut Context<Self>) {
        self.reload_workspace(false, cx);
    }

    #[cfg(test)]
    pub fn remove_unavailable_for_test(&mut self, item: usize, cx: &mut Context<Self>) {
        self.remove_unavailable_folder(item, cx);
    }

    #[cfg(test)]
    pub fn quiet_reload_workspace_for_test(&mut self, cx: &mut Context<Self>) {
        self.reload_workspace(true, cx);
    }

    /// Whether a watcher event for the workspace file would be acted on now.
    #[cfg(test)]
    pub fn workspace_file_changed_for_test(&self) -> bool {
        let Some(path) = self.roots.workspace.as_ref().map(|w| w.path.clone()) else { return false };
        self.workspace_file_changed(&vec![(path, proto::ChangeKind::Modified)])
    }

    #[cfg(test)]
    pub fn workspace_on_remote_for_test(&self) -> bool {
        self.roots.workspace.as_ref().is_some_and(|w| w.host.is_remote() && Arc::ptr_eq(&w.host, &self.remote.host))
    }

    #[cfg(test)]
    pub fn workspace_saving_for_test(&self) -> bool {
        self.roots.workspace.as_ref().is_some_and(|w| w.saving)
    }
}

/// The open root of each entry of the new list: kept entries keep theirs, added ones are the folder they added.
fn remap(edits: &[Edit], opened: &[(usize, String)], added: impl Fn(&Edit) -> Option<String>) -> Vec<(usize, String)> {
    edits
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Edit::Keep(old) => opened.iter().find(|(item, _)| item == old).map(|(_, p)| (i, p.clone())),
            Edit::Add { .. } => added(e).map(|p| (i, p)),
        })
        .collect()
}

/// Run `f` on the workbench with its window (replies of background reads).
fn with_window(this: &gpui::WeakEntity<Workbench>, cx: &mut gpui::AsyncApp, f: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) {
    let Ok(handle) = this.update(cx, |t, _| t.window_handle) else { return };
    let this = this.clone();
    cx.update_window(handle, |_, window, cx| {
        this.update(cx, |t, cx| f(t, window, cx)).ok();
    })
    .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dirs_cover_both_separators() {
        assert_eq!(parent_dir("/a/b/w.code-workspace"), "/a/b");
        assert_eq!(parent_dir("/w.code-workspace"), "/");
        assert_eq!(parent_dir(r"C:\a\w.code-workspace"), r"C:\a");
        assert_eq!(parent_dir("w.code-workspace"), ".");
    }

    #[test]
    fn remapping_follows_the_new_list() {
        let opened = vec![(0, "/a".to_string()), (2, "/c".to_string())];
        let edits = vec![Edit::Keep(2), Edit::Keep(1), Edit::Add { path: "/d".into(), name: None }];
        let got = remap(&edits, &opened, |e| match e {
            Edit::Add { path, .. } => Some(path.clone()),
            Edit::Keep(_) => None,
        });
        assert_eq!(got, [(0, "/c".to_string()), (2, "/d".to_string())]);
    }
}
