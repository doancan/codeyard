//! Workbench: Session: saving tabs, backups and restoring.

use super::*;

impl Workbench {
    /// Tabs and views (when the folder changes, when a tab is opened or closed, on exit).
    pub(super) fn persist_session(&self, cx: &App) {
        if let (Some(b), Some(s)) = (self.chrome.window_bounds, &self.persist.state) {
            s.set(WINDOW_BOUNDS_KEY, &format_bounds(b)).ok();
        }
        let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) else { return };
        let mut tabs = Vec::new();
        // Left group 0, right group 1 (if split); each group's active tab is marked.
        let focused = (&self.tabs.editors[..], self.tabs.active);
        let other = self.tabs.other.as_ref().map(|g| (&g.editors[..], g.active));
        let groups = if self.tabs.focused_left { [Some(focused), other] } else { [other, Some(focused)] };
        for (group, (editors, active)) in groups.into_iter().enumerate().filter_map(|(g, x)| x.map(|x| (g, x))) {
            for (i, ed) in editors.iter().enumerate() {
                let e = ed.read(cx);
                if !e.uses_host(&self.remote.host) {
                    continue; // those that don't belong to this folder, like the settings file
                }
                if let Some(rel) = self.rel(&e.path, cx) {
                    let _ = s.save_view(ws, &rel, e.view());
                    self.save_bookmarks(ed, cx);
                    tabs.push(state::Tab { rel_path: rel, active: active == Some(i), pinned: e.pinned, group: group as u8 });
                }
            }
        }
        let _ = s.save_tabs(ws, &tabs);
        let _ = s.set(&format!("focus_left:{ws}"), if self.tabs.focused_left { "1" } else { "0" });
    }

    /// Back up 1 s after an edit. The file write (fsync included, hundreds of ms for a big file) runs in the background; only scheduling and
    /// the DB record stay on the UI thread.
    pub(super) fn schedule_backup(&mut self, cx: &mut Context<Self>) {
        self.persist.backup_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            let Ok(Some(plan)) = this.update(cx, |this, cx| this.plan_backups(cx)) else { return };
            let (dir, ws) = (plan.dir.clone(), plan.ws);
            let writes = plan.writes;
            let removals = plan.removals;
            let written = cx.background_executor().spawn(async move { write_blobs(&dir, writes) }).await;
            this.update(cx, |this, _| this.finish_backups(ws, written, removals)).ok();
        }));
    }

    /// Write dirty contents and delete backups of cleaned files. Synchronous, for exit and folder switch where the task
    /// can't be awaited.
    pub(super) fn flush_backups(&mut self, cx: &App) {
        let Some(plan) = self.plan_backups(cx) else { return };
        let written = write_blobs(&plan.dir, plan.writes);
        self.finish_backups(plan.ws, written, plan.removals);
    }

    /// Collect what to write and delete (so the map doesn't change while editors are read); doesn't touch files.
    fn plan_backups(&self, cx: &App) -> Option<BackupPlan> {
        if !cx.global::<Settings>().hot_exit {
            return None;
        }
        let (Some(ws), Some(data)) = (self.persist.workspace_id, self.persist.data_dir.clone()) else { return None };
        self.persist.state.as_ref()?;
        let dir = data.join("backups");
        let mut writes = Vec::new();
        let mut removals = Vec::new();
        for ed in &self.all_editors() {
            let e = ed.read(cx);
            if !e.uses_host(&self.remote.host) {
                continue;
            }
            let Some(rel) = self.rel(&e.path, cx) else { continue };
            if e.is_dirty() {
                if self.persist.backed_up.get(&e.path) != Some(&e.version()) {
                    let blob = dir.join(format!("{ws}-{:016x}.bak", proto::content_hash(rel.as_bytes())));
                    writes.push(BackupWrite { path: e.path.clone(), rel, version: e.version(), bytes: e.content_bytes(), base_hash: e.disk_hash(), blob });
                }
            } else if self.persist.backed_up.contains_key(&e.path) {
                removals.push((e.path.clone(), rel));
            }
        }
        Some(BackupPlan { ws, dir, writes, removals })
    }

    /// Commit the written blobs to the database, delete the backups of cleaned ones.
    fn finish_backups(&mut self, ws: i64, written: Vec<(BackupWrite, bool)>, removals: Vec<(String, String)>) {
        let Some(s) = &self.persist.state else { return };
        for (w, ok) in written {
            let backup = state::Backup { rel_path: w.rel, blob_path: w.blob.to_string_lossy().into_owned(), base_hash: w.base_hash };
            if ok && s.save_backup(ws, &backup).is_ok() {
                self.persist.backed_up.insert(w.path, w.version);
            }
        }
        for (path, rel) in removals {
            if let Ok(Some(blob)) = s.remove_backup(ws, &rel) {
                let _ = std::fs::remove_file(blob);
            }
            self.persist.backed_up.remove(&path);
        }
    }

    /// A tab was closed on purpose: its backup should go too.
    pub(super) fn drop_backup(&mut self, path: &str, cx: &App) {
        let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) else { return };
        if let Some(rel) = self.rel(path, cx)
            && let Ok(Some(blob)) = s.remove_backup(ws, &rel)
        {
            let _ = std::fs::remove_file(blob);
        }
        self.persist.backed_up.remove(path);
    }

    /// Restore the folder's tabs and backups (in order, in one go).
    pub(super) fn restore_tabs(&mut self, ws: i64, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(s), Some(root)) = (&self.persist.state, self.root(cx)) else { return };
        if !cx.global::<Settings>().restore_session {
            return;
        }
        let mut tabs = s.tabs(ws).unwrap_or_default();
        let backups = if cx.global::<Settings>().hot_exit { s.backups(ws).unwrap_or_default() } else { Vec::new() };
        // Tab not saved but has a backup (crash): open it anyway, so changes aren't lost.
        for b in &backups {
            if !tabs.iter().any(|t| t.rel_path == b.rel_path) {
                tabs.push(state::Tab { rel_path: b.rel_path.clone(), active: false, pinned: false, group: 0 });
            }
        }
        if tabs.is_empty() {
            return;
        }
        // Don't load a file that's being opened from the command line (or is already open)
        // twice: a 500 KB file's syntax tree is ~20 MB, and the dropped copy
        // lingered in the allocator.
        let busy = |rel: &str| {
            let path = ui::join(&root, rel);
            self.tabs.opening.as_deref() == Some(path.as_str()) || self.all_editors().iter().any(|e| e.read(cx).path == path)
        };
        tabs.retain(|t| !busy(&t.rel_path));
        if tabs.is_empty() {
            return;
        }
        // Layout: the tab order and active tabs of the two groups; the file is loaded once.
        let layout: Vec<(String, u8, bool)> = tabs.iter().map(|t| (ui::join(&root, &t.rel_path), t.group, t.active)).collect();
        let focus_left = s.get(&format!("focus_left:{ws}")).is_none_or(|v| v != "0");
        let mut seen = std::collections::HashSet::new();
        tabs.retain(|t| seen.insert(t.rel_path.clone()));
        let views: Vec<Option<state::View>> = tabs.iter().map(|t| s.view(ws, &t.rel_path)).collect();
        let pinned: Vec<bool> = tabs.iter().map(|t| t.pinned).collect();
        let host = self.remote.host.clone();
        let jobs: Vec<(String, Option<state::Backup>)> =
            tabs.iter().map(|t| (ui::join(&root, &t.rel_path), backups.iter().find(|b| b.rel_path == t.rel_path).cloned())).collect();
        let fallback = cx.global::<Settings>().fallback_encoding;
        let task = cx.background_executor().spawn(async move {
            jobs.into_iter()
                .map(|(path, backup)| {
                    let loaded = editor::load(host.as_ref(), &path, fallback);
                    let blob = backup.as_ref().and_then(|b| std::fs::read(&b.blob_path).ok().map(|d| (d, b.base_hash)));
                    (path, loaded, blob)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.persist.workspace_id != Some(ws) {
                    return; // switched to another folder in the meantime
                }
                for (((path, loaded, blob), view), pin) in results.into_iter().zip(views).zip(pinned) {
                    if this.all_editors().iter().any(|e| e.read(cx).path == path) {
                        continue;
                    }
                    let (loaded, missing) = match (loaded, &blob) {
                        (Ok(l), _) => (l, false),
                        // File deleted but a backup exists: keep the content.
                        (Err(_), Some(_)) => (
                            editor::Loaded {
                                buffer: buffer::Buffer::empty(),
                                hash: 0,
                                syntax: None,
                                encoding: crate::encoding::Encoding::Utf8,
                                rules: Default::default(),
                            },
                            true,
                        ),
                        (Err(_), None) => continue,
                    };
                    let ed = this.add_editor(this.remote.host.clone(), path.clone(), loaded, window, cx);
                    ed.update(cx, |e, cx| {
                        if let Some((bytes, base)) = &blob {
                            e.apply_backup(bytes, if missing { 0 } else { *base }, cx);
                        }
                        if missing {
                            e.mark_deleted_on_disk();
                        }
                        if let Some(v) = view {
                            e.set_view(v);
                        }
                        e.pinned = pin;
                    });
                    // Replacing the whole text moves line marks to its start; the saved breakpoints match the backed-up text.
                    if blob.is_some() {
                        this.apply_breakpoints_to(&ed, cx);
                    }
                }
                this.restore_layout(&layout, focus_left, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// At app startup (if there are no arguments): the last folder and its tabs.
    pub fn restore_last(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Default: the welcome screen. Only `window.startup = "continue"` opens the last folder (including SSH).
        if cx.global::<Settings>().startup != crate::settings::Startup::Continue {
            return;
        }
        let Some(last) = self.persist.recent.first().cloned() else { return };
        self.reopen(last.host, last.path, window, cx);
    }

    /// Opens a folder from an earlier session (this computer, or an SSH host: it connects, which may ask for a password).
    pub fn reopen(&mut self, host: Option<String>, path: String, window: &mut Window, cx: &mut Context<Self>) {
        match host {
            None if std::path::Path::new(&path).is_dir() => self.set_root(path, cx),
            // Folder deleted or moved: stay on the welcome screen and say why.
            None => {
                self.notice = Some(Notice::Info(crate::i18n::tf("{} no longer exists, so it was not reopened.", &[&tilde(&path)]).into()));
                cx.notify();
            }
            Some(alias) => {
                self.remote.pending_path = Some(path);
                self.connect(alias, window, cx);
            }
        }
    }

    /// Before exit: write backups and the session. If hot exit is off and there are dirty
    /// files, `false`: the caller should ask.
    pub fn prepare_quit(&mut self, cx: &App) -> bool {
        let hot_exit = cx.global::<Settings>().hot_exit;
        if !hot_exit && self.any_dirty(cx) {
            return false;
        }
        self.flush_backups(cx);
        self.persist_session(cx);
        // Hot exit only covers what it could actually write down (files outside the folders, no database, a full disk): ask for the rest.
        if self.has_unbacked_dirty(cx) {
            return false;
        }
        self.cancel_all_forwards();
        true
    }

    /// A dirty buffer whose current text is not in a backup: closing or switching away would lose it.
    pub(super) fn has_unbacked_dirty(&self, cx: &App) -> bool {
        self.all_editors().iter().any(|ed| {
            let e = ed.read(cx);
            e.is_dirty() && self.persist.backed_up.get(&e.path) != Some(&e.version())
        })
    }
}

/// A single backup to be written.
struct BackupWrite {
    path: String,
    rel: String,
    version: u64,
    bytes: Vec<u8>,
    base_hash: u64,
    blob: std::path::PathBuf,
}

struct BackupPlan {
    ws: i64,
    dir: std::path::PathBuf,
    writes: Vec<BackupWrite>,
    removals: Vec<(String, String)>,
}

/// Write blobs atomically (a partial backup must not overwrite the real one); success info for each.
fn write_blobs(dir: &std::path::Path, writes: Vec<BackupWrite>) -> Vec<(BackupWrite, bool)> {
    writes
        .into_iter()
        .map(|w| {
            let ok = std::fs::create_dir_all(dir).and_then(|_| crate::fsutil::write_atomic(&w.blob, &w.bytes)).is_ok();
            (w, ok)
        })
        .collect()
}
