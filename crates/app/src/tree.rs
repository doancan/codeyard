//! Explorer: lazily loaded, virtualized file tree (S02).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, ScrollStrategy, SharedString, UniformListScrollHandle, Window, actions, div,
    prelude::*, px, uniform_list,
};
use host::Host;
use proto::DirEntry;

use crate::ui::{self, c, icon, layout, palette};

#[path = "tree_selection.rs"]
mod selection;

/// Row metrics: a little taller and roomier than the shared list row, so a deep tree stays easy to scan and to hit.
const ROW_H: f32 = 24.;
const ROW_PAD_L: f32 = 10.;
const ROW_PAD_R: f32 = 12.;
const ROW_GAP: f32 = 6.;

actions!(
    tree,
    [
        ExpandAll,
        CollapseAllInside,
        SelectPrev,
        SelectNext,
        SelectFirst,
        SelectLast,
        PageUp,
        PageDown,
        Expand,
        Collapse,
        Confirm,
        RenameEntry,
        DeleteEntry,
        CopyEntry,
        CutEntry,
        PasteEntry,
        ExtendPrev,
        ExtendNext,
        ExtendFirst,
        ExtendLast,
        ExtendPageUp,
        ExtendPageDown,
        SelectAllEntries,
        ToggleSelection
    ]
);

/// A file or folder dragged out of the tree.
#[derive(Clone)]
pub struct DraggedEntry {
    pub paths: Arc<Vec<String>>,
    name: SharedString,
}

impl DraggedEntry {
    fn move_to(&self, to_dir: String) -> TreeEvent {
        match self.paths.as_slice() {
            [from] => TreeEvent::Move { from: from.clone(), to_dir },
            _ => TreeEvent::MoveMany { paths: self.paths.as_ref().clone(), to_dir },
        }
    }
}

impl Render for DraggedEntry {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *crate::ui::palette(cx);
        div()
            .px(px(theme::SPACE[3]))
            .py(px(theme::SPACE[1]))
            .rounded(px(theme::radius::SM))
            .bg(c(p.surface_active))
            .border_1()
            .border_color(c(p.focus_ring))
            .text_size(px(crate::ui::text::SM.0))
            .text_color(c(p.text))
            .child(self.name.clone())
    }
}

pub enum TreeEvent {
    /// `preview`: single click (preview tab), double click/Enter: permanent.
    OpenFile {
        path: String,
        preview: bool,
    },
    /// Right click (or keyboard): `path` is the root itself for the root.
    ContextMenu {
        path: String,
        is_dir: bool,
        position: gpui::Point<gpui::Pixels>,
    },
    Rename(String),
    /// (path, is it a folder?)
    Delete(String, bool),
    /// ⌘C / ⌘X: path to the clipboard (copied or moved on paste).
    Copy(String),
    Cut(String),
    /// ⌘V: into the selected folder (the file's folder if a file is selected).
    Paste {
        dir: String,
    },
    /// Drag and drop: `from` → into the `to_dir` folder.
    Move {
        from: String,
        to_dir: String,
    },
    /// File dropped from Finder/Explorer: upload (remote) or copy (local) into the `to_dir` folder.
    DropFiles {
        paths: Vec<std::path::PathBuf>,
        to_dir: String,
    },
    /// The user expanded/collapsed a folder (expanded folders are persisted).
    ExpandedChanged,
    SelectionChanged {
        root: String,
    },
    CopyMany(Vec<String>),
    CutMany(Vec<String>),
    DeleteMany(Vec<String>),
    MoveMany {
        paths: Vec<String>,
        to_dir: String,
    },
}

enum Load {
    Loading,
    Loaded(Vec<DirEntry>),
    Failed(SharedString),
}

#[derive(Clone)]
struct Row {
    path: String,
    name: SharedString,
    depth: usize,
    is_dir: bool,
    loading: bool,
    error: Option<SharedString>,
    /// Compacted folder chain ("a/b/c"): `path` is the last folder of the chain.
    chain: bool,
}

/// Explorer appearance: comes from settings (`explorer.compactFolders`, `explorer.sortOrder`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewOptions {
    pub compact: bool,
    pub sort: SortOrder,
}

impl Default for ViewOptions {
    fn default() -> Self {
        ViewOptions { compact: true, sort: SortOrder::Default }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOrder {
    /// Folders first, then natural order.
    Default,
    FilesFirst,
    /// Folders and files together, natural order.
    Mixed,
    /// Folders first, files grouped by extension.
    Type,
}

impl SortOrder {
    pub fn from_setting(s: &str) -> SortOrder {
        match s {
            "filesFirst" => SortOrder::FilesFirst,
            "mixed" => SortOrder::Mixed,
            "type" => SortOrder::Type,
            _ => SortOrder::Default,
        }
    }
}

fn extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext,
        _ => "",
    }
}

/// Order in the tree.
pub fn sort_entries(entries: &mut [DirEntry], order: SortOrder) {
    entries.sort_by(|a, b| {
        let (da, db) = (a.is_dir(), b.is_dir());
        let kind = match order {
            SortOrder::Default | SortOrder::Type => (!da).cmp(&!db),
            SortOrder::FilesFirst => da.cmp(&db),
            SortOrder::Mixed => std::cmp::Ordering::Equal,
        };
        kind.then_with(|| match order {
            SortOrder::Type if !da && !db => {
                extension(&a.name).to_lowercase().cmp(&extension(&b.name).to_lowercase()).then_with(|| natural_cmp(&a.name, &b.name))
            }
            _ => natural_cmp(&a.name, &b.name),
        })
    });
}

pub struct FileTree {
    host: Arc<dyn Host>,
    root: Option<String>,
    children: HashMap<String, Load>,
    expanded: HashSet<String>,
    rows: Vec<Row>,
    selected: Option<usize>,
    marked: HashSet<String>,
    anchor: Option<String>,
    load_epochs: HashMap<String, u64>,
    reload_queued: HashSet<String>,
    next_load: u64,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Git markers (absolute path → marker; folders carry those of their contents).
    git: Arc<HashMap<String, crate::git::Mark>>,
    /// File to reveal: picked once its parent folders are loaded.
    pending_reveal: Option<String>,
    opts: ViewOptions,
    /// Type-to-search: the prefix built from the latest keystrokes and the time of the last keystroke.
    type_ahead: (String, Option<std::time::Instant>),
    /// Clicked on empty space: no selection, root is the target.
    root_selected: bool,
    /// While "expand all" runs: folders still waiting to load and the remaining budget (folder count).
    recursive: HashSet<String>,
    budget: usize,
    /// The list's visible area in the last frame, to put a menu opened by keyboard under the selected row.
    bounds: Option<gpui::Bounds<gpui::Pixels>>,
}

impl EventEmitter<TreeEvent> for FileTree {}

impl Focusable for FileTree {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FileTree {
    pub fn set_git(&mut self, marks: Arc<HashMap<String, crate::git::Mark>>, cx: &mut Context<Self>) {
        self.git = marks;
        cx.notify();
    }

    pub fn new(host: Arc<dyn Host>, cx: &mut Context<Self>) -> Self {
        FileTree {
            host,
            root: None,
            children: HashMap::new(),
            expanded: HashSet::new(),
            pending_reveal: None,
            rows: Vec::new(),
            selected: None,
            marked: HashSet::new(),
            anchor: None,
            load_epochs: HashMap::new(),
            reload_queued: HashSet::new(),
            next_load: 0,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            git: Arc::default(),
            opts: cx
                .try_global::<crate::settings::Settings>()
                .map_or_else(ViewOptions::default, |s| ViewOptions { compact: s.compact_folders, sort: SortOrder::from_setting(&s.sort_order) }),
            type_ahead: (String::new(), None),
            root_selected: false,
            recursive: HashSet::new(),
            budget: 0,
            bounds: None,
        }
    }

    /// Setting changed: rebuild the order and chains from scratch.
    pub fn set_options(&mut self, opts: ViewOptions, cx: &mut Context<Self>) {
        if self.opts == opts {
            return;
        }
        self.opts = opts;
        for load in self.children.values_mut() {
            if let Load::Loaded(entries) = load {
                sort_entries(entries, opts.sort);
            }
        }
        self.rebuild();
        // Compaction turned on: also load the child of loaded folders that have a single subfolder.
        if opts.compact {
            self.prefetch_chains(cx);
        }
        cx.notify();
    }

    /// If a loaded folder's only child is a folder, load that too (so the chain "a/b" is one row).
    fn prefetch_chains(&mut self, cx: &mut Context<Self>) {
        let todo: Vec<(String, bool)> = self
            .children
            .iter()
            .filter_map(|(dir, l)| match l {
                Load::Loaded(es) if es.len() == 1 && es[0].is_dir() => Some((ui::join(dir, &es[0].name), self.expanded.contains(dir))),
                _ => None,
            })
            .filter(|(child, _)| !self.children.contains_key(child))
            .collect();
        for (child, parent_open) in todo {
            if parent_open {
                self.expanded.insert(child.clone());
            }
            self.load(child, cx);
        }
    }

    pub fn root(&self) -> Option<&str> {
        self.root.as_deref()
    }

    /// Reveal the file: expand parents (loading if needed), select, scroll.
    pub fn reveal(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let Some(rel) = path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))) else { return };
        if self.selected_row().is_some_and(|r| r.path == path) {
            return;
        }
        let mut dir = root.clone();
        let parts: Vec<&str> = rel.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            dir = ui::join(&dir, part);
            self.expanded.insert(dir.clone());
            if !self.children.contains_key(&dir) {
                self.load(dir.clone(), cx);
            }
        }
        self.pending_reveal = Some(path.to_string());
        self.rebuild();
        self.try_reveal(cx);
        cx.notify();
    }

    /// Reveal the folder: expand its parents and itself, select it.
    pub fn reveal_dir(&mut self, path: &str, cx: &mut Context<Self>) {
        self.reveal(path, cx);
        if !self.expanded.contains(path) {
            self.toggle(path, cx);
        }
    }

    fn try_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(path) = &self.pending_reveal else { return };
        // Folder in the middle of a compacted chain: select the chain's row.
        let inside = format!("{path}/");
        let found = self.rows.iter().position(|r| r.path == *path).or_else(|| self.rows.iter().position(|r| r.chain && r.path.starts_with(&inside)));
        if let Some(ix) = found {
            self.selected = Some(ix);
            self.marked.clear();
            self.marked.insert(self.rows[ix].path.clone());
            self.anchor = Some(self.rows[ix].path.clone());
            self.root_selected = false;
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
            self.pending_reveal = None;
            self.selection_changed(cx);
        }
    }

    /// Collapse every folder except the root.
    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        self.expanded.retain(|d| Some(d) == root.as_ref());
        self.recursive.clear();
        self.rebuild();
        cx.emit(TreeEvent::ExpandedChanged);
        cx.notify();
    }

    /// Empty rows below the list (like VS Code: click there to clear the selection).
    const SPACER_ROWS: usize = 3;

    /// Clear the selection; the root folder becomes "selected".
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.marked.clear();
        self.anchor = None;
        self.root_selected = true;
        cx.notify();
    }

    /// Target for a new file/folder: the selected folder (its folder if a file); the root if empty space was clicked.
    pub fn target_dir(&self) -> Option<String> {
        match self.selected_row() {
            Some(r) if r.is_dir => Some(r.path.clone()),
            Some(r) => r.path.rsplit_once('/').map(|(d, _)| d.to_string()),
            None if self.root_selected => self.root.clone(),
            None => None,
        }
    }

    /// Folders below these that are expensive to load and are skipped by "expand all".
    const HEAVY: &'static [&'static str] = &["node_modules", "target", ".git", ".venv", "venv", "__pycache__", "dist", "build"];
    /// "Expand all" opens at most this many folders (so the tree doesn't blow up on huge repos).
    const EXPAND_BUDGET: usize = 400;

    /// Expand the selected folder (its folder if a file is selected, the root if nothing is) with all its subfolders.
    pub fn expand_selected_recursive(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let dir = match self.selected_row() {
            Some(r) if r.is_dir => r.path.clone(),
            Some(r) => r.path.rsplit_once('/').map_or(root.clone(), |(d, _)| d.to_string()),
            None => root,
        };
        self.expand_recursive(&dir, cx);
    }

    pub fn expand_recursive(&mut self, dir: &str, cx: &mut Context<Self>) {
        self.budget = Self::EXPAND_BUDGET;
        self.expand_rec(dir, cx);
        self.rebuild();
        cx.emit(TreeEvent::ExpandedChanged);
        cx.notify();
    }

    fn expand_rec(&mut self, dir: &str, cx: &mut Context<Self>) {
        self.expanded.insert(dir.to_string());
        match self.children.get(dir) {
            Some(Load::Loaded(es)) => {
                let dirs: Vec<String> = es.iter().filter(|e| e.is_dir() && !Self::HEAVY.contains(&e.name.as_str())).map(|e| ui::join(dir, &e.name)).collect();
                for d in dirs {
                    if self.budget == 0 {
                        return;
                    }
                    self.budget -= 1;
                    self.expand_rec(&d, cx);
                }
            }
            Some(Load::Loading) => {
                self.recursive.insert(dir.to_string());
            }
            _ => {
                self.recursive.insert(dir.to_string());
                self.load(dir.to_string(), cx);
            }
        }
    }

    /// Collapse the folder and all folders below it (the root does not collapse).
    pub fn collapse_recursive(&mut self, dir: &str, cx: &mut Context<Self>) {
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        let root = self.root.clone();
        self.expanded.retain(|d| Some(d) == root.as_ref() || (d != dir && !d.starts_with(&prefix)));
        self.recursive.retain(|d| !d.starts_with(&prefix));
        if Some(dir) != root.as_deref() {
            self.expanded.remove(dir);
        }
        self.rebuild();
        cx.emit(TreeEvent::ExpandedChanged);
        cx.notify();
    }

    /// Expanded folders (excluding the root), for persistence.
    pub fn open_dirs(&self) -> Vec<String> {
        let mut v: Vec<String> = self.expanded.iter().filter(|d| Some(d.as_str()) != self.root.as_deref()).cloned().collect();
        v.sort();
        v
    }

    /// Re-expand the folders that were open in the previous session (after the root has loaded).
    pub fn restore_expanded(&mut self, dirs: Vec<String>, cx: &mut Context<Self>) {
        for d in dirs {
            self.expanded.insert(d.clone());
            if !self.children.contains_key(&d) {
                self.load(d, cx);
            }
        }
        self.rebuild();
        cx.notify();
    }

    /// Selected folder (its folder if a file is selected), the root if there is no selection: target for upload/paste.
    pub fn selected_dir(&self) -> Option<String> {
        match self.selected_row() {
            Some(r) if r.is_dir => Some(r.path.clone()),
            Some(r) => r.path.rsplit_once('/').map(|(d, _)| d.to_string()),
            None => self.root.clone(),
        }
    }

    #[cfg(test)]
    pub fn selected_path(&self) -> Option<String> {
        self.selected_row().map(|r| r.path.clone())
    }

    /// Re-read a folder (after file operations). Do nothing if it was never opened.
    pub fn reload(&mut self, dir: &str, cx: &mut Context<Self>) {
        let dir = dir.trim_end_matches('/').to_string();
        if self.children.contains_key(&dir) {
            if self.load_epochs.contains_key(&dir) {
                self.reload_queued.insert(dir);
            } else {
                self.load(dir, cx);
            }
        }
    }

    /// Re-read all loaded folders (the exclude setting changed).
    pub fn reload_all(&mut self, cx: &mut Context<Self>) {
        let dirs: Vec<String> = self.children.keys().cloned().collect();
        for d in dirs {
            self.load(d, cx);
        }
    }

    /// Folders that need watching: the root and the expanded ones.
    pub fn expanded_dirs(&self) -> Vec<String> {
        self.expanded.iter().filter(|d| matches!(self.children.get(*d), Some(Load::Loaded(_)))).cloned().collect()
    }

    /// Loaded folders get re-read on a change event.
    pub fn is_loaded_dir(&self, dir: &str) -> bool {
        matches!(self.children.get(dir.trim_end_matches('/')), Some(Load::Loaded(_)))
    }

    /// For the accessibility tree (a11y.rs): rows (path, name, depth, is folder, is expanded) and the selected row.
    pub fn a11y_rows(&self) -> (Vec<(String, String, usize, bool, bool)>, Option<usize>) {
        let rows = self.rows.iter().map(|r| (r.path.clone(), r.name.to_string(), r.depth, r.is_dir, r.is_dir && self.expanded.contains(&r.path))).collect();
        (rows, self.selected)
    }

    /// Resolve a snapshot's path again: a refresh may have inserted or removed rows since AT read it.
    pub fn a11y_select(&mut self, path: &str, activate: bool, cx: &mut Context<Self>) {
        let Some(ix) = self.rows.iter().position(|r| r.path == path) else { return };
        self.select(ix, cx);
        if activate {
            self.activate(ix, false, cx);
        }
    }

    #[cfg(test)]
    pub fn row_errors(&self) -> Vec<(String, String)> {
        self.rows.iter().filter_map(|r| r.error.as_ref().map(|e| (r.name.to_string(), e.to_string()))).collect()
    }

    #[cfg(test)]
    pub fn row_names(&self) -> Vec<String> {
        self.rows.iter().map(|r| r.name.to_string()).collect()
    }

    /// Hierarchy line to draw prominently: (column, first row, last row). If the selected folder
    /// is expanded, its own children's line; otherwise its parent folder's.
    fn active_guide(&self) -> Option<(usize, usize, usize)> {
        let s = self.selected?;
        let row = self.rows.get(s)?;
        let folder = if row.is_dir && self.expanded.contains(&row.path) {
            s
        } else if row.depth > 0 {
            (0..s).rev().find(|&i| self.rows[i].depth < row.depth)?
        } else {
            return None;
        };
        let col = self.rows[folder].depth;
        let last = (folder + 1..self.rows.len()).take_while(|&i| self.rows[i].depth > col).last()?;
        Some((col, folder + 1, last))
    }

    #[cfg(test)]
    pub fn guide_for_test(&self) -> Option<(usize, usize, usize)> {
        self.active_guide()
    }

    fn selected_row(&self) -> Option<&Row> {
        self.selected.and_then(|i| self.rows.get(i))
    }

    fn rename_entry(&mut self, _: &RenameEntry, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_row() {
            cx.emit(TreeEvent::Rename(row.path.clone()));
        }
    }

    fn delete_entry(&mut self, _: &DeleteEntry, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_empty() {
            return;
        }
        if self.marked.len() > 1 {
            cx.emit(TreeEvent::DeleteMany(self.selected_paths()));
            return;
        }
        if let Some(row) = self.selected_row() {
            cx.emit(TreeEvent::Delete(row.path.clone(), row.is_dir));
        }
    }

    fn copy_entry(&mut self, _: &CopyEntry, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_empty() {
            return;
        }
        if self.marked.len() > 1 {
            cx.emit(TreeEvent::CopyMany(self.selected_paths()));
            return;
        }
        if let Some(row) = self.selected_row() {
            cx.emit(TreeEvent::Copy(row.path.clone()));
        }
    }

    fn cut_entry(&mut self, _: &CutEntry, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_empty() {
            return;
        }
        if self.marked.len() > 1 {
            cx.emit(TreeEvent::CutMany(self.selected_paths()));
            return;
        }
        if let Some(row) = self.selected_row() {
            cx.emit(TreeEvent::Cut(row.path.clone()));
        }
    }

    fn paste_entry(&mut self, _: &PasteEntry, _: &mut Window, cx: &mut Context<Self>) {
        let dir = match self.selected_row() {
            Some(r) if r.is_dir => Some(r.path.clone()),
            Some(r) => r.path.rsplit_once('/').map(|(d, _)| d.to_string()),
            None => self.root.clone(),
        };
        if let Some(dir) = dir {
            cx.emit(TreeEvent::Paste { dir });
        }
    }

    pub fn set_root(&mut self, host: Arc<dyn Host>, root: Option<String>, cx: &mut Context<Self>) {
        self.host = host;
        self.root = root.clone();
        self.children.clear();
        self.expanded.clear();
        self.rows.clear();
        self.selected = None;
        self.marked.clear();
        self.anchor = None;
        self.load_epochs.clear();
        self.reload_queued.clear();
        self.pending_reveal = None;
        self.recursive.clear();
        self.root_selected = false;
        if let Some(root) = root {
            self.expanded.insert(root.clone());
            self.load(root, cx);
        }
        cx.notify();
    }

    fn load(&mut self, path: String, cx: &mut Context<Self>) {
        // Keep loaded rows while refreshing: selection must not disappear between
        // the request and reply. Per-request epochs reject old roots and old reads.
        if !matches!(self.children.get(&path), Some(Load::Loaded(_))) {
            self.children.insert(path.clone(), Load::Loading);
        }
        self.next_load = self.next_load.wrapping_add(1);
        let epoch = self.next_load;
        self.load_epochs.insert(path.clone(), epoch);
        self.rebuild();
        let host = self.host.clone();
        let p = path.clone();
        let task = cx.background_executor().spawn(async move { host.read_dir(&p) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.finish_load(path, epoch, result, cx)).ok();
        })
        .detach();
    }

    fn finish_load(&mut self, path: String, epoch: u64, result: host::Result<Vec<DirEntry>>, cx: &mut Context<Self>) {
        if self.load_epochs.get(&path) != Some(&epoch) {
            return;
        }
        self.load_epochs.remove(&path);
        // The folder is gone (deleted, here or outside Codeyard): forget it and read its parent again, which drops its
        // row. The watcher may report only the folder's own change, so this is the only read that sees it.
        if matches!(&result, Err(host::HostError::Op(proto::Error::NotFound(_)))) && self.root.as_deref() != Some(path.as_str()) {
            self.forget(&path);
            if let Some((parent, _)) = path.rsplit_once('/') {
                let parent = if parent.is_empty() { "/" } else { parent };
                self.reload(parent, cx);
            }
            self.rebuild();
            cx.notify();
            return;
        }
        let loaded = result.is_ok();
        let load = match result {
            Ok(mut entries) => {
                let excludes = &cx.global::<crate::settings::Settings>().files_exclude;
                let rel_dir = self.root.as_deref().and_then(|r| path.strip_prefix(r)).unwrap_or("").trim_start_matches('/').to_string();
                entries.retain(|e| {
                    let rel = if rel_dir.is_empty() { e.name.clone() } else { format!("{rel_dir}/{}", e.name) };
                    !crate::globs::is_excluded(excludes, &rel)
                });
                sort_entries(&mut entries, self.opts.sort);
                Load::Loaded(entries)
            }
            Err(e) => Load::Failed(e.to_string().into()),
        };
        // Folder with a single subfolder: also load the child so the chain is one row;
        // if the folder is expanded the child expands too (so the chain's row doesn't look collapsed).
        let only_dir = match &load {
            Load::Loaded(es) if es.len() == 1 && es[0].is_dir() && self.opts.compact => Some(ui::join(&path, &es[0].name)),
            _ => None,
        };
        let open = self.expanded.contains(&path);
        self.children.insert(path.clone(), load);
        if self.recursive.remove(&path) {
            // A failed read stays failed until an explicit retry. Re-entering
            // expansion here would immediately retry the same unreadable folder.
            if loaded {
                self.expand_rec(&path, cx);
            }
            if self.recursive.is_empty() {
                // The last folder has loaded too: the fully expanded state is saved.
                cx.emit(TreeEvent::ExpandedChanged);
            }
        }
        if let Some(child) = only_dir.filter(|c| !self.children.contains_key(c)) {
            if open {
                self.expanded.insert(child.clone());
            }
            self.load(child, cx);
        }
        self.rebuild();
        self.try_reveal(cx);
        if self.reload_queued.remove(&path) {
            self.load(path, cx);
        }
        cx.notify();
    }

    /// Drop a folder that no longer exists, with everything under it, from what the tree holds.
    fn forget(&mut self, dir: &str) {
        let under = format!("{dir}/");
        let gone = |p: &String| p == dir || p.starts_with(&under);
        self.children.retain(|p, _| !gone(p));
        self.expanded.retain(|p| !gone(p));
        self.reload_queued.retain(|p| !gone(p));
        self.recursive.retain(|p| !gone(p));
    }

    /// Rebuild the visible rows from scratch (expanded folders, depth first).
    fn rebuild(&mut self) {
        let selected_path = self.selected.and_then(|i| self.rows.get(i)).map(|r| r.path.clone());
        self.rows.clear();
        if let Some(root) = self.root.clone() {
            self.push_children(&root, 0);
        }
        self.selected = selected_path.and_then(|p| self.rows.iter().position(|r| r.path == p));
        self.reconcile_selection();
    }

    fn push_children(&mut self, dir: &str, depth: usize) {
        let Some(Load::Loaded(entries)) = self.children.get(dir) else { return };
        let entries: Vec<(String, SharedString, bool)> =
            entries.iter().map(|e| (ui::join(dir, &e.name), SharedString::from(e.name.clone()), e.is_dir())).collect();
        for (mut path, mut name, is_dir) in entries {
            // Compaction: folders with a single subfolder appear as "a/b/c" on one row.
            let mut chain = false;
            if is_dir && self.opts.compact {
                while let Some(Load::Loaded(es)) = self.children.get(&path) {
                    let [only] = es.as_slice() else { break };
                    if !only.is_dir() {
                        break;
                    }
                    name = format!("{name}/{}", only.name).into();
                    path = ui::join(&path, &only.name);
                    chain = true;
                }
            }
            let expanded = is_dir && self.expanded.contains(&path);
            let (loading, error) = match self.children.get(&path) {
                Some(Load::Loading) => (true, None),
                Some(Load::Failed(e)) => (false, Some(e.clone())),
                _ => (false, None),
            };
            self.rows.push(Row { path: path.clone(), name, depth, is_dir, loading, error, chain });
            if expanded {
                self.push_children(&path, depth + 1);
            }
        }
    }

    fn toggle(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.expanded.remove(path) {
            self.rebuild();
        } else {
            self.expanded.insert(path.to_string());
            match self.children.get(path) {
                Some(Load::Loaded(_)) => self.rebuild(),
                // If it errored, retry.
                _ => self.load(path.to_string(), cx),
            }
        }
        cx.emit(TreeEvent::ExpandedChanged);
        cx.notify();
    }

    fn activate(&mut self, ix: usize, preview: bool, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else { return };
        self.select(ix, cx);
        if row.is_dir {
            if !preview || self.selected == Some(ix) {
                self.toggle(&row.path, cx);
            }
        } else {
            cx.emit(TreeEvent::OpenFile { path: row.path, preview });
        }
        cx.notify();
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let ix = ix.min(self.rows.len() - 1);
        self.select_with(ix, false, false, cx);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        cx.notify();
    }

    /// Shift+F10 or the Menu key: the selected row's menu (the root folder's without a selection), under the row, as a
    /// right click opens it.
    fn show_context_menu(&mut self, _: &crate::menu::ShowContextMenu, _: &mut Window, cx: &mut Context<Self>) {
        let bounds = self.bounds.unwrap_or_default();
        let (path, is_dir, position) = match self.selected.filter(|_| !self.root_selected).and_then(|ix| Some((ix, self.rows.get(ix)?))) {
            Some((ix, row)) => {
                // Scrolled to the top first, so the menu opens just under the first visible row.
                self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
                let y = (bounds.top() + px(ROW_H)).min(bounds.bottom());
                (row.path.clone(), row.is_dir, gpui::point(bounds.left() + px(ROW_PAD_L), y))
            }
            None => match self.root.clone() {
                Some(root) => (root, true, gpui::point(bounds.left() + px(ROW_PAD_L), bounds.top() + px(ROW_H))),
                None => return,
            },
        };
        cx.emit(TreeEvent::ContextMenu { path, is_dir, position });
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        let ix = self.selected.map_or(0, |i| i.saturating_sub(1));
        self.select(ix, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let ix = self.selected.map_or(0, |i| i + 1);
        self.select(ix, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.select(0, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.select(self.rows.len().saturating_sub(1), cx);
    }

    /// Page: as many rows as are visible (approximate; row height is fixed).
    const PAGE: usize = 20;

    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.select(self.selected.map_or(0, |i| i.saturating_sub(Self::PAGE)), cx);
    }

    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.select(self.selected.map_or(0, |i| i + Self::PAGE), cx);
    }

    fn expand(&mut self, _: &Expand, _: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.selected.and_then(|i| self.rows.get(i)).cloned() else { return };
        if row.is_dir && !self.expanded.contains(&row.path) {
            self.toggle(&row.path, cx);
        } else if row.is_dir && self.rows.get(self.selected.unwrap() + 1).is_some_and(|child| child.depth > row.depth) {
            self.select(self.selected.unwrap() + 1, cx); // step into the first child
        }
    }

    fn collapse(&mut self, _: &Collapse, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected else { return };
        let row = self.rows[ix].clone();
        if row.is_dir && self.expanded.contains(&row.path) {
            self.toggle(&row.path, cx);
        } else if row.depth > 0 {
            // Go up to the parent folder.
            let parent = (0..ix).rev().find(|&i| self.rows[i].depth < row.depth);
            if let Some(p) = parent {
                self.select(p, cx);
            }
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected {
            self.activate(ix, false, cx);
        }
    }

    /// Type-to-search: jump to the first row starting with the letters typed while the tree has focus
    /// (letters typed within 1 s combine; pressing the same letter again moves to the next match).
    fn type_ahead_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = ev.keystroke.modifiers;
        if m.control || m.platform || m.alt || m.function {
            return;
        }
        let Some(ch) = ev.keystroke.key_char.as_deref().filter(|c| c.chars().all(|c| !c.is_control())) else { return };
        let now = std::time::Instant::now();
        let fresh = self.type_ahead.1.is_none_or(|t| now.duration_since(t) > std::time::Duration::from_millis(1000));
        if fresh {
            self.type_ahead.0.clear();
        }
        if ch == " " && self.type_ahead.0.is_empty() {
            return;
        }
        self.type_ahead.0.push_str(&ch.to_lowercase());
        self.type_ahead.1 = Some(now);
        if let Some(ix) = self.type_ahead_match(self.selected) {
            self.select(ix, cx);
            cx.stop_propagation();
        }
    }

    /// Row starting with `buffer`: for a single letter, from the one after the selection, then starting from the selection.
    fn type_ahead_match(&self, from: Option<usize>) -> Option<usize> {
        let buf = &self.type_ahead.0;
        let n = self.rows.len();
        if n == 0 {
            return None;
        }
        let same_letter = buf.chars().count() > 1 && buf.chars().all(|c| buf.starts_with(c));
        let (probe, skip_current): (String, bool) = if same_letter { (buf.chars().take(1).collect(), true) } else { (buf.clone(), buf.chars().count() == 1) };
        let start = from.map_or(0, |i| if skip_current { i + 1 } else { i });
        (0..n).map(|k| (start + k) % n).find(|&i| self.rows[i].name.to_lowercase().starts_with(&probe))
    }
}

impl Render for FileTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let focused = self.focus_handle.is_focused(window);
        let root_state = self.root.as_ref().and_then(|r| self.children.get(r));

        let body = match root_state {
            None => div().into_any_element(),
            Some(Load::Loading) => {
                div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).child(crate::i18n::t("Loading…")).into_any_element()
            }
            Some(Load::Failed(e)) => div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.danger)).child(e.clone()).into_any_element(),
            Some(Load::Loaded(entries)) if entries.is_empty() => {
                div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).child(crate::i18n::t("Empty folder")).into_any_element()
            }
            Some(Load::Loaded(_)) => uniform_list(
                "tree-rows",
                self.rows.len() + Self::SPACER_ROWS,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    let active = this.active_guide();
                    let marked_paths = Arc::new(this.selected_paths());
                    range
                        .map(|ix| {
                            // Empty space below the list: clicking clears the selection (the root counts as selected).
                            if ix >= this.rows.len() {
                                return div()
                                    .id(("tree-spacer", ix))
                                    .w_full()
                                    .h(px(ROW_H))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            cx.stop_propagation();
                                            window.focus(&this.focus_handle);
                                            this.clear_selection(cx);
                                        }),
                                    )
                                    .into_any_element();
                            }
                            let row = &this.rows[ix];
                            let selected = this.marked.contains(&row.path);
                            let bg = match (selected, focused) {
                                // Selection while focused: a fill sliding toward the accent color (no border; a bright 1 px border felt harsh on every click).
                                (true, true) => Some(crate::ui::focused_selection(&p)),
                                (true, false) => Some(p.surface_hover),
                                _ => None,
                            };
                            let expanded = this.expanded.contains(&row.path);
                            // Folder: arrow + folder icon. File: spacing the width of the arrow + icon by type. A theme without
                            // folder icons puts the file's icon in the arrow's column instead, so every name in a level lines up.
                            let folder_icons = crate::icon_theme::current().folder_icons();
                            let lead = if !row.is_dir {
                                if folder_icons {
                                    div().w(px(layout::ICON)).flex_none().into_any_element()
                                } else {
                                    crate::icon_theme::file_icon_el(&row.name, layout::ICON, &p)
                                }
                            } else if row.loading {
                                ui::spinner(("tree-load", ix), layout::ICON, p.text_muted).into_any_element()
                            } else {
                                icon(if expanded { "chevron-down" } else { "chevron-right" }, layout::ICON, p.text_muted).into_any_element()
                            };
                            let indent = cx.global::<crate::settings::Settings>().tree_indent;
                            let leading_icon = if !folder_icons {
                                None
                            } else if row.is_dir {
                                Some(crate::icon_theme::folder_icon_el(row.name.rsplit('/').next().unwrap_or(&row.name), expanded, layout::ICON, &p))
                            } else {
                                Some(crate::icon_theme::file_icon_el(&row.name, layout::ICON, &p))
                            };
                            // The dropped item goes into the folder; if dropped on a file, into that file's folder.
                            let drop_dir = if row.is_dir { row.path.clone() } else { row.path.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string()) };
                            let paths = if selected { marked_paths.clone() } else { Arc::new(vec![row.path.clone()]) };
                            let name = if paths.len() > 1 { crate::i18n::tn("{} entry", "{} entries", paths.len(), &[]).into() } else { row.name.clone() };
                            let drag = DraggedEntry { paths, name };
                            div()
                                .id(ix)
                                .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
                                .drag_over::<DraggedEntry>(move |s, _, _, _| s.bg(c(p.surface_active)))
                                .on_drop(cx.listener({
                                    let drop_dir = drop_dir.clone();
                                    move |_, d: &DraggedEntry, _, cx| {
                                        cx.emit(d.move_to(drop_dir.clone()));
                                    }
                                }))
                                .drag_over::<gpui::ExternalPaths>(move |s, _, _, _| s.bg(c(p.surface_active)))
                                .on_drop(cx.listener(move |_, d: &gpui::ExternalPaths, _, cx| {
                                    cx.emit(TreeEvent::DropFiles { paths: d.paths().to_vec(), to_dir: drop_dir.clone() });
                                }))
                                .relative()
                                .w_full()
                                .h(px(ROW_H))
                                .flex()
                                .items_center()
                                .gap(px(ROW_GAP))
                                .pl(px(ROW_PAD_L + row.depth as f32 * indent))
                                .pr(px(ROW_PAD_R))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .when_some(bg, |d, bg| d.bg(c(bg)))
                                .hover(|d| d.bg(c(p.surface_hover)))
                                // Hierarchy lines: one vertical line per ancestor folder (below the arrow);
                                // the selected folder's line is prominent.
                                .children((0..if cx.global::<crate::settings::Settings>().tree_guides { row.depth } else { 0 }).map(|col| {
                                    let on = active.is_some_and(|(c0, a, b)| c0 == col && (a..=b).contains(&ix));
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .left(px(ROW_PAD_L + col as f32 * indent + layout::ICON / 2.))
                                        .w(px(1.))
                                        .bg(ui::ca(p.text_muted, if on { 0.6 } else { 0.16 }))
                                }))
                                .child(lead)
                                .children(leading_icon)
                                .child({
                                    let mark = this.git.get(&row.path).copied();
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when_some(mark, |d, m| d.text_color(c(m.color(&p))))
                                        .when(mark == Some(crate::git::Mark::Deleted), |d| d.line_through())
                                        .child(row.name.clone())
                                })
                                .when_some(this.git.get(&row.path).copied(), |d, m| {
                                    // File: letter (M orange, N green, D red…); folder: a dot meaning it contains changes.
                                    if row.is_dir {
                                        d.child(div().flex_none().mr(px(theme::SPACE[2])).size(px(7.)).rounded_full().bg(c(m.color(&p))))
                                    } else {
                                        d.child(div().flex_none().w(px(12.)).text_size(px(crate::ui::text::XS.0)).text_color(c(m.color(&p))).child(m.letter()))
                                    }
                                })
                                .when_some(row.error.clone(), |d, e| d.child(div().ml_auto().text_color(c(p.danger)).child(e)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                                        cx.stop_propagation();
                                        this.root_selected = false;
                                        window.focus(&this.focus_handle);
                                        let toggle = if cfg!(target_os = "macos") { ev.modifiers.platform } else { ev.modifiers.control };
                                        if toggle || ev.modifiers.shift {
                                            this.select_with(ix, ev.modifiers.shift, toggle, cx);
                                            return;
                                        }
                                        if this.marked.len() > 1 && this.rows.get(ix).is_some_and(|r| this.marked.contains(&r.path)) {
                                            this.selected = Some(ix);
                                            cx.notify();
                                            return;
                                        }
                                        if ev.click_count >= 2 {
                                            // Double click: make the preview permanent (don't re-open and close the folder).
                                            if let Some(row) = this.rows.get(ix).filter(|r| !r.is_dir).cloned() {
                                                cx.emit(TreeEvent::OpenFile { path: row.path, preview: false });
                                            }
                                        } else {
                                            this.activate(ix, true, cx);
                                        }
                                    }),
                                )
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                                        cx.stop_propagation();
                                        window.focus(&this.focus_handle);
                                        if !this.rows.get(ix).is_some_and(|r| this.marked.contains(&r.path)) {
                                            this.select(ix, cx);
                                        } else {
                                            this.selected = Some(ix);
                                        }
                                        let row = this.rows[ix].clone();
                                        cx.emit(TreeEvent::ContextMenu { path: row.path, is_dir: row.is_dir, position: ev.position });
                                        cx.notify();
                                    }),
                                )
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(self.scroll.clone())
            .size_full()
            .into_any_element(),
        };

        div()
            .id("file-tree")
            .key_context("FileTree")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::extend_prev))
            .on_action(cx.listener(Self::extend_next))
            .on_action(cx.listener(Self::extend_first))
            .on_action(cx.listener(Self::extend_last))
            .on_action(cx.listener(Self::extend_page_up))
            .on_action(cx.listener(Self::extend_page_down))
            .on_action(cx.listener(Self::select_all_entries))
            .on_action(cx.listener(Self::toggle_selection))
            .on_action(cx.listener(Self::expand))
            .on_action(cx.listener(Self::collapse))
            .on_action(cx.listener(|this, _: &ExpandAll, _, cx| this.expand_selected_recursive(cx)))
            .on_action(cx.listener(|this, _: &CollapseAllInside, _, cx| {
                let dir = match this.selected_row() {
                    Some(r) if r.is_dir => r.path.clone(),
                    Some(r) => r.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default(),
                    None => return,
                };
                this.collapse_recursive(&dir, cx);
            }))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::show_context_menu))
            .on_action(cx.listener(Self::rename_entry))
            .on_action(cx.listener(Self::delete_entry))
            .on_action(cx.listener(Self::copy_entry))
            .on_action(cx.listener(Self::cut_entry))
            .on_action(cx.listener(Self::paste_entry))
            .on_key_down(cx.listener(Self::type_ahead_key))
            // Drop on empty space: into the root folder.
            .on_drop(cx.listener(|this, d: &DraggedEntry, _, cx| {
                if let Some(root) = this.root.clone() {
                    cx.emit(d.move_to(root));
                }
            }))
            .on_drop(cx.listener(|this, d: &gpui::ExternalPaths, _, cx| {
                if let Some(root) = this.root.clone() {
                    cx.emit(TreeEvent::DropFiles { paths: d.paths().to_vec(), to_dir: root });
                }
            }))
            // Click on empty space: selection clears, the root folder counts as selected (a new file goes to the root).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle);
                    this.clear_selection(cx);
                }),
            )
            // Right click on empty space: root folder.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &gpui::MouseDownEvent, _, cx| {
                    if let Some(root) = this.root.clone() {
                        cx.emit(TreeEvent::ContextMenu { path: root, is_dir: true, position: ev.position });
                    }
                }),
            )
            .size_full()
            .relative()
            .text_color(c(p.text))
            .child(body)
            .child({
                let tree = cx.entity().downgrade();
                gpui::canvas(
                    |bounds, window, _| bounds.intersect(&window.content_mask().bounds),
                    move |_, bounds, _, cx| {
                        tree.update(cx, |t, _| t.bounds = Some(bounds)).ok();
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            })
    }
}

/// Natural sort: numbers as numbers (2 < 10), letters case-insensitively.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                match m.len().cmp(&n.len()).then_with(|| m.cmp(n)) {
                    Ordering::Equal => {}
                    o => return o,
                }
            }
            (Some(c), Some(d)) => {
                let (lc, ld) = (c.to_lowercase().next().unwrap_or(c), d.to_lowercase().next().unwrap_or(d));
                if lc != ld {
                    return lc.cmp(&ld);
                }
                x.next();
                y.next();
            }
        }
    }
}

#[cfg(test)]
mod natural_tests {
    #[test]
    fn numbers_sort_naturally() {
        let mut v = vec!["file_10", "file_2", "File_1", "file_02b", "a"];
        v.sort_by(|a, b| super::natural_cmp(a, b));
        assert_eq!(v, vec!["a", "File_1", "file_2", "file_02b", "file_10"]);
    }
}
