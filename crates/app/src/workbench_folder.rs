//! Remote "Open Folder": path input with a live subfolder list (like VS Code).
//! After connecting this picker opens instead of the home directory; ⌘O does the same on a remote host.

use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, Context, Window};

use super::{Notice, Overlay, OverlayKind, Workbench};
use crate::picker::{Pick, PickItem, Picker, Source as PickSource};
use crate::ui;
use host::Host;

/// At most this many folders are shown in the list.
const MAX_DIRS: usize = 200;

/// Split the typed path into (dir, prefix): dir ends with "/", prefix is the last segment.
/// "~" is the home directory; a relative path is relative to home.
pub fn split_query(query: &str, home: &str) -> (String, String) {
    let home = home.trim_end_matches('/');
    let q = match query.strip_prefix('~') {
        Some("") => format!("{home}/"),
        Some(rest) if rest.starts_with('/') => format!("{home}{rest}"),
        _ => query.to_string(),
    };
    match q.rfind('/') {
        Some(i) => (q[..=i].to_string(), q[i + 1..].to_string()),
        None => (format!("{home}/"), q),
    }
}

/// Normal path of the folder to open: "~" is expanded, a relative path is joined to home,
/// the trailing "/" is dropped (except for the root).
pub fn normalize(query: &str, home: &str) -> String {
    let (dir, rest) = split_query(query.trim(), home);
    let joined = format!("{dir}{rest}");
    let trimmed = joined.trim_end_matches('/');
    if trimmed.is_empty() { "/".to_string() } else { trimmed.to_string() }
}

/// Folders in `dir` that start with `partial` (case-insensitive; hidden
/// folders only when the prefix starts with "."), sorted by name.
pub fn subdirs(host: &dyn Host, dir: &str, partial: &str) -> Result<Vec<String>, String> {
    let entries = host.read_dir(dir).map_err(|e| e.to_string())?;
    let p = partial.to_lowercase();
    let mut names: Vec<String> = entries
        .into_iter()
        .filter(|e| e.is_dir())
        .map(|e| e.name)
        .filter(|n| (!n.starts_with('.') || p.starts_with('.')) && n.to_lowercase().starts_with(&p))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.truncate(MAX_DIRS);
    Ok(names)
}

impl Workbench {
    /// Open the folder picker (remote host). Start path: the open folder, else home.
    pub(super) fn open_remote_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.asking() {
            return;
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let start = self.root(cx).unwrap_or_else(|| self.remote.host.info().home.clone());
        let start = format!("{}/", start.trim_end_matches('/'));
        let message = crate::i18n::tf("Open a folder on {}", &[&self.remote.host.label()]);
        let picker = cx.new(|cx| {
            Picker::new(
                vec![PickSource::Dirs { items: Vec::new(), status: crate::i18n::t("Loading…").into() }],
                crate::i18n::t("Path of the folder (~ is your home folder)"),
                &start,
                None,
                cx,
            )
            .with_message(message)
        });
        self.show_overlay(picker, OverlayKind::RemoteFolder { dir: String::new(), names: Vec::new() }, window, cx);
        self.list_remote_dirs(start, cx);
    }

    /// List the typed path's subfolders after a 100 ms pause, not per keystroke;
    /// a new query drops the old one.
    pub(super) fn list_remote_dirs(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(picker) = self.overlay.as_ref().map(|o| o.picker.downgrade()) else { return };
        let host: Arc<dyn Host> = self.remote.host.clone();
        let home = host.info().home.clone();
        self.folder_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let (dir, partial) = split_query(&query, &home);
            let d = dir.clone();
            let found = cx.background_executor().spawn(async move { subdirs(host.as_ref(), &d, &partial) }).await;
            this.update(cx, |this, cx| {
                let Some(Overlay { kind: OverlayKind::RemoteFolder { dir: shown, names }, .. }) = &mut this.overlay else { return };
                let (list, status) = match found {
                    Ok(list) if list.is_empty() => (list, crate::i18n::t("No matching folders").to_string()),
                    Ok(list) => (list, String::new()),
                    Err(e) => (Vec::new(), crate::i18n::tf("Can't read {0}: {1}", &[&dir, &e])),
                };
                *shown = dir;
                *names = list.clone();
                let items = list.into_iter().map(|n| PickItem { label: format!("{n}/").into(), detail: None, keys: None }).collect();
                picker.update(cx, |p, cx| p.set_dirs(items, status, cx)).ok();
            })
            .ok();
        }));
    }

    /// Picker events. `true`: handled.
    pub(super) fn on_folder_event(&mut self, event: &crate::picker::PickerEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use crate::picker::PickerEvent;
        let Some(Overlay { kind: OverlayKind::RemoteFolder { dir, names }, picker, .. }) = &self.overlay else { return false };
        match event {
            PickerEvent::Query(q) => {
                let q = q.clone();
                self.list_remote_dirs(q, cx);
            }
            // Picking a subfolder descends into it: the path becomes "dir/name/" and the list refreshes.
            PickerEvent::Confirm(Pick::Item { index, .. }) => {
                if let Some(name) = names.get(*index) {
                    let next = format!("{dir}{name}/");
                    picker.clone().update(cx, |p, cx| p.set_query(next, cx));
                }
            }
            PickerEvent::Confirm(Pick::Input) => {
                let typed = picker.read(cx).query().to_string();
                self.close_overlay(window, cx);
                self.open_remote_path(typed, cx);
            }
            PickerEvent::Dismiss => {
                self.close_overlay(window, cx);
            }
            _ => {}
        }
        true
    }

    /// Validate the typed path (is it a folder, is it readable) and make it the root.
    fn open_remote_path(&mut self, typed: String, cx: &mut Context<Self>) {
        let host: Arc<dyn Host> = self.remote.host.clone();
        let path = normalize(&typed, &host.info().home);
        let p = path.clone();
        self.folder_task = Some(cx.spawn(async move |this, cx| {
            let check = cx.background_executor().spawn(async move { host.read_dir(&p).map(|_| ()).map_err(|e| e.to_string()) }).await;
            this.update(cx, |this, cx| match check {
                Ok(()) => this.set_root(path, cx),
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Can't open {0}: {1}", &[&path, &e]).into()));
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

impl Workbench {
    /// `.code-workspace`: first folder becomes the root, the rest are added as workspace folders in this window.
    pub(super) fn open_workspace_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let base = std::path::Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("Could not read the workspace file: {}", &[&e]).into()));
                cx.notify();
                return;
            }
        };
        let folders = crate::workspace_file::folders(&text, &base, |p| std::path::Path::new(p).is_dir());
        let Some((first, rest)) = folders.split_first() else {
            self.notice = Some(Notice::Error(crate::i18n::t("The workspace file lists no folder that exists on this computer.").into()));
            cx.notify();
            return;
        };
        self.open_path(first, window, cx);
        // `set_root` clears additional folders, so the file's own are added after it.
        let canonical = |p: &str| std::fs::canonicalize(p).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string());
        for folder in rest {
            self.add_workspace_folder_now(canonical(folder), window, cx);
        }
        if !rest.is_empty() {
            self.notice = Some(Notice::Info(
                crate::i18n::tn("Workspace: {} folder in the Explorer.", "Workspace: {} folders in the Explorer.", folders.len(), &[]).into(),
            ));
        }
        cx.notify();
    }

    /// "File: Open Workspace from File…"
    pub(super) fn pick_workspace_file(&mut self, _: &super::OpenWorkspaceFile, _: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(crate::i18n::t("Open").to_string().into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let path = path.to_string_lossy().into_owned();
                let handle = this.update(cx, |t, _| t.window_handle).ok();
                if let Some(h) = handle {
                    cx.update_window(h, |_, window, cx| {
                        this.update(cx, |t, cx| t.open_path(&path, window, cx)).ok();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// "File: Add Folder in New Window…": open another folder alongside, in a separate window.
    pub(super) fn add_folder_in_new_window(&mut self, _: &super::AddFolderInNewWindow, _: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some(crate::i18n::t("Open").to_string().into()),
        });
        cx.spawn(async move |_this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                cx.update(|cx| {
                    for p in paths {
                        if let Some(w) = crate::open_workbench_window(cx) {
                            let p = p.to_string_lossy().into_owned();
                            w.update(cx, |wb, window, cx| wb.open_path(&p, window, cx)).ok();
                        }
                    }
                })
                .ok();
            }
        })
        .detach();
    }
}

/// At most this many additional folders.
pub(super) const MAX_EXTRA_ROOTS: usize = 32;

impl Workbench {
    /// Explorer trees: the first root and additional folders.
    pub(super) fn all_trees(&self) -> Vec<gpui::Entity<crate::tree::FileTree>> {
        std::iter::once(self.roots.primary.clone()).chain(self.roots.extra_trees.iter().cloned()).collect()
    }

    /// All roots of the workspace: the first and the added ones.
    pub fn all_roots(&self, cx: &gpui::App) -> Vec<String> {
        self.root(cx).into_iter().chain(self.roots.extra_roots.iter().cloned()).collect()
    }

    /// The root containing the file (the longest match for nested roots).
    pub fn root_of(&self, path: &str, cx: &gpui::App) -> Option<String> {
        self.all_roots(cx)
            .into_iter()
            .chain(self.roots.pending_extra_roots.iter().cloned())
            .filter(|r| path == r || path.starts_with(&format!("{}/", r.trim_end_matches('/'))))
            .max_by_key(|r| r.len())
    }

    /// Give the path as relative to its root (works for additional folders too).
    pub fn rel_in_roots(&self, path: &str, cx: &gpui::App) -> Option<String> {
        let root = self.root_of(path, cx)?;
        path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).map(str::to_string)
    }

    /// Go to file pick: `folder › path` resolves to the additional folder, a plain path to the first root.
    pub(super) fn palette_path(&self, root: &str, rel: &str) -> String {
        if let Some((label, inner)) = rel.split_once(super::ROOT_SEP)
            && let Some(extra) = self.roots.extra_roots.iter().find(|e| ui::file_name(e) == label)
        {
            return ui::join(extra, inner);
        }
        ui::join(root, rel)
    }

    /// Target for a new file/folder: the tree with a selection; if several, the one at the active file's root comes first.
    pub(super) fn target_dir_any(&self, cx: &gpui::App) -> Option<String> {
        let active = self.active_root(cx);
        let mut trees = self.all_trees();
        trees.sort_by_key(|t| t.read(cx).root().map(str::to_string) != active);
        trees.iter().find_map(|t| t.read(cx).target_dir())
    }

    /// The path's explorer tree (else the first).
    pub(super) fn tree_for(&self, path: &str, cx: &gpui::App) -> gpui::Entity<crate::tree::FileTree> {
        let root = self.root_of(path, cx);
        self.all_trees().into_iter().find(|t| t.read(cx).root() == root.as_deref()).unwrap_or_else(|| self.roots.primary.clone())
    }

    /// The active file's root (else the first root): tasks, tests and debugging run from there.
    pub fn active_root(&self, cx: &gpui::App) -> Option<String> {
        self.active_editor().and_then(|e| self.root_of(&e.read(cx).path, cx)).or_else(|| self.root(cx))
    }

    /// Reload the path in every tree that has it loaded; `false` if none did.
    pub(super) fn reload_in_trees(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        let mut any = false;
        for t in self.all_trees() {
            if t.read(cx).is_loaded_dir(path) {
                t.update(cx, |t, cx| t.reload(path, cx));
                any = true;
            }
        }
        any
    }

    /// Reveal the active file in its own root's tree (the others are left alone).
    pub(super) fn reveal_in_trees(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(root) = self.root_of(path, cx) else { return };
        for t in self.all_trees() {
            if t.read(cx).root() == Some(root.as_str()) {
                t.update(cx, |t, cx| t.reveal(path, cx));
            }
        }
    }

    /// Add the folder to the workspace without checking it exists (out-of-bounds entries and duplicates are dropped).
    pub fn add_workspace_folder_now(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let path = if self.remote.host.is_remote() { path } else { ui::local_path(std::path::Path::new(&path)) };
        let path = path.trim_end_matches('/').to_string();
        let refuse = |this: &mut Self, why: String, cx: &mut Context<Self>| {
            this.notice = Some(Notice::Info(why.into()));
            cx.notify();
            false
        };
        if self.root(cx).is_none() {
            return refuse(self, crate::i18n::t("Open a folder first; folders are added to it.").into(), cx);
        }
        if self.all_roots(cx).iter().any(|r| r.trim_end_matches('/') == path) {
            return refuse(self, crate::i18n::tf("{} is already in the workspace.", &[&ui::file_name(&path)]), cx);
        }
        if self.roots.extra_roots.len() >= MAX_EXTRA_ROOTS {
            return refuse(self, crate::i18n::tf("At most {} extra folders fit in one workspace.", &[&MAX_EXTRA_ROOTS]), cx);
        }
        let host = self.remote.host.clone();
        let tree = cx.new(|cx| crate::tree::FileTree::new(host.clone(), cx));
        let settings = cx.global::<crate::settings::Settings>();
        let view = crate::tree::ViewOptions { compact: settings.compact_folders, sort: crate::tree::SortOrder::from_setting(&settings.sort_order) };
        tree.update(cx, |t, cx| {
            t.set_options(view, cx);
            t.set_root(host, Some(path.clone()), cx);
        });
        self._subs.push(cx.subscribe_in(&tree, window, |this, _, event: &crate::tree::TreeEvent, window, cx| this.on_tree_event(event, window, cx)));
        self.roots.pending_extra_roots.retain(|p| *p != path);
        self.load_folder_settings(path.clone(), cx);
        self.roots.extra_roots.push(path);
        self.roots.extra_trees.push(tree);
        self.roots.file_index = None;
        self.persist_extra_roots();
        self.sync_search_roots(cx);
        self.refresh_git(0, cx);
        cx.notify();
        true
    }

    pub fn remove_workspace_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.roots.extra_roots.iter().position(|r| r == path) {
            self.roots.extra_roots.remove(i);
            self.roots.extra_trees.remove(i);
            self.roots.file_index = None;
            self.persist_extra_roots();
            self.sync_search_roots(cx);
            self.vcs.tree_marks.remove(path);
            self.cfg.folders.remove(path);
            self.apply_folder_settings(cx);
            if self.vcs.scm_root.as_deref() == Some(path) {
                self.vcs.scm_root = None;
                self.refresh_git(0, cx);
            }
            // The closed folder's language servers and diagnostics must not linger.
            self.lsp.diagnostics.retain(|p, _| !p.starts_with(&format!("{}/", path.trim_end_matches('/'))));
            cx.notify();
        }
    }

    /// Read the additional folder's settings files (in the background), parse them and apply to that folder's open files.
    pub(super) fn load_folder_settings(&mut self, root: String, cx: &mut Context<Self>) {
        let (host, paths) = (self.remote.host.clone(), super::folder_settings_paths(&root));
        let task = cx
            .background_executor()
            .spawn(async move { paths.iter().filter_map(|p| host.read(p).ok()).map(|(d, _)| String::from_utf8_lossy(&d).into_owned()).collect::<Vec<_>>() });
        cx.spawn(async move |this, cx| {
            let texts = task.await;
            this.update(cx, |this, cx| {
                if !this.roots.extra_roots.contains(&root) && !this.roots.pending_extra_roots.contains(&root) {
                    return;
                }
                let layers: Vec<&str> = texts.iter().map(String::as_str).collect();
                let parsed = crate::folder_settings::FolderSettings::parse(&layers);
                if parsed.is_empty() {
                    this.cfg.folders.remove(&root);
                } else {
                    this.cfg.folders.insert(root, Arc::new(parsed));
                }
                this.apply_folder_settings(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Give each open editor its own folder's settings (none for those in the first folder and folderless ones).
    pub(super) fn apply_folder_settings(&mut self, cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            self.apply_folder_settings_to(&ed, cx);
        }
    }

    pub(super) fn apply_folder_settings_to(&mut self, ed: &gpui::Entity<crate::editor::EditorView>, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        let primary = self.root(cx);
        let folder = self.root_of(&path, cx).filter(|r| Some(r) != primary.as_ref()).and_then(|r| self.cfg.folders.get(&r).cloned());
        ed.update(cx, |e, cx| e.set_folder_settings(folder, cx));
    }

    fn sync_search_roots(&mut self, cx: &mut Context<Self>) {
        let roots = self.roots.extra_roots.clone();
        self.search.update(cx, |s, cx| s.set_extra_roots(roots, cx));
        self.sync_search_editors(cx);
    }

    fn persist_extra_roots(&self) {
        if let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) {
            let _ = s.set(&format!("folders:{ws}"), &self.roots.extra_roots.join("\n"));
        }
    }

    /// Restore additional folders from the previous session (deferred because it needs a window).
    pub(super) fn restore_extra_roots(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) else { return };
        let Some(text) = s.get(&format!("folders:{ws}")) else { return };
        let folders: Vec<String> = text.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        if folders.is_empty() {
            return;
        }
        self.roots.pending_extra_roots = folders.clone();
        let (handle, host) = (self.window_handle, self.remote.host.clone());
        cx.spawn(async move |this, cx| {
            // Deleted folders are dropped silently.
            let alive = cx.background_executor().spawn(async move { folders.into_iter().filter(|f| host.stat(f).is_ok()).collect::<Vec<_>>() }).await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    for f in alive {
                        this.add_workspace_folder_now(f, window, cx);
                    }
                    this.roots.pending_extra_roots.clear();
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    #[cfg(test)]
    pub fn extra_tree_rows_for_test(&self, i: usize, cx: &gpui::App) -> Vec<String> {
        self.roots.extra_trees.get(i).map(|t| t.read(cx).row_names()).unwrap_or_default()
    }

    /// Go to file list (tests): empty if there is no cache.
    #[cfg(test)]
    pub fn palette_path_for_test(&self, root: &str, rel: &str) -> String {
        self.palette_path(root, rel)
    }

    #[cfg(test)]
    pub fn editor_for_path_for_test(&self, path: &str, cx: &gpui::App) -> Option<gpui::Entity<crate::editor::EditorView>> {
        self.all_editors().into_iter().find(|e| e.read(cx).path == path)
    }

    #[cfg(test)]
    pub fn load_folder_settings_for_test(&mut self, root: String, cx: &mut Context<Self>) {
        self.load_folder_settings(root, cx);
    }

    #[cfg(test)]
    pub fn folder_settings_for_test(&self, root: &str) -> Option<crate::folder_settings::FolderSettings> {
        self.cfg.folders.get(root).map(|f| (**f).clone())
    }

    #[cfg(test)]
    pub fn extra_roots_for_test(&self) -> Vec<String> {
        self.roots.extra_roots.clone()
    }

    /// "File: Add Folder to Workspace…"
    pub(super) fn add_folder_to_workspace(&mut self, _: &super::AddFolderToWorkspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.root(cx).is_none() {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a folder first; folders are added to it.").into()));
            cx.notify();
            return;
        }
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some(crate::i18n::t("Add").to_string().into()),
        });
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                cx.update_window(handle, |_, window, cx| {
                    this.update(cx, |this, cx| {
                        for p in paths {
                            let p = std::fs::canonicalize(&p).unwrap_or(p).to_string_lossy().into_owned();
                            this.add_workspace_folder_now(p, window, cx);
                        }
                    })
                    .ok();
                })
                .ok();
            }
        })
        .detach();
    }

    /// "File: Remove Folder from Workspace…"
    pub(super) fn show_remove_folder(&mut self, _: &super::RemoveFolderFromWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.roots.extra_roots.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("The workspace has no extra folders.").into()));
            cx.notify();
            return;
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let items = self
            .roots
            .extra_roots
            .iter()
            .map(|r| PickItem { label: ui::file_name(r).to_string().into(), detail: Some(r.clone().into()), keys: None })
            .collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Remove folder from workspace"), "", None, cx));
        self.show_overlay(picker, OverlayKind::RemoveFolder(self.roots.extra_roots.clone()), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_splits_into_directory_and_prefix() {
        let home = "/home/ada";
        assert_eq!(split_query("/srv/app/sr", home), ("/srv/app/".into(), "sr".into()));
        assert_eq!(split_query("/srv/app/", home), ("/srv/app/".into(), String::new()));
        assert_eq!(split_query("/", home), ("/".into(), String::new()));
        assert_eq!(split_query("~", home), ("/home/ada/".into(), String::new()));
        assert_eq!(split_query("~/pro", home), ("/home/ada/".into(), "pro".into()));
        assert_eq!(split_query("~/a/b", "/home/ada/"), ("/home/ada/a/".into(), "b".into()));
        assert_eq!(split_query("work", home), ("/home/ada/".into(), "work".into()));
    }

    #[test]
    fn typed_path_is_normalized() {
        let home = "/home/ada";
        assert_eq!(normalize("/srv/app/", home), "/srv/app");
        assert_eq!(normalize("  /srv/app  ", home), "/srv/app");
        assert_eq!(normalize("/", home), "/");
        assert_eq!(normalize("~", home), "/home/ada");
        assert_eq!(normalize("~/code/", home), "/home/ada/code");
        assert_eq!(normalize("code", home), "/home/ada/code");
    }

    #[test]
    fn lists_matching_folders_only() {
        let dir = std::env::temp_dir().join(format!("codeyard-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["Alpha", "alps", "beta", ".hidden"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("alpha.txt"), "").unwrap();
        let host = host::local();
        let d = format!("{}/", dir.display());
        assert_eq!(subdirs(&host, &d, "").unwrap(), ["Alpha", "alps", "beta"], "no files or hidden folders");
        assert_eq!(subdirs(&host, &d, "AL").unwrap(), ["Alpha", "alps"], "case-insensitive");
        assert_eq!(subdirs(&host, &d, ".").unwrap(), [".hidden"], "a dot shows hidden folders");
        assert!(subdirs(&host, &format!("{d}missing/"), "").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
