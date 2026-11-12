//! Nested files in the explorer's rows: each loaded folder's grouping, which parent files are open, and opening or closing one.
//! What goes together is decided in `tree_nesting.rs`; here it is applied to the tree.

use super::*;

#[derive(Default)]
pub(super) struct NestState {
    /// Parent files whose nested files are listed.
    open: HashSet<String>,
    /// The grouping of each loaded folder, kept until the folder is read again.
    groups: HashMap<String, Arc<nesting::Groups>>,
}

impl NestState {
    pub(super) fn is_open(&self, path: &str) -> bool {
        self.open.contains(path)
    }

    /// The rules changed: every folder is grouped again.
    pub(super) fn forget_groups(&mut self) {
        self.groups.clear();
    }

    /// The folder was read again.
    pub(super) fn forget_dir(&mut self, dir: &str) {
        self.groups.remove(dir);
    }

    /// The folder is gone, with everything under it.
    pub(super) fn forget_under(&mut self, dir: &str) {
        let under = format!("{dir}/");
        let gone = |p: &String| p == dir || p.starts_with(&under);
        self.groups.retain(|p, _| !gone(p));
        self.open.retain(|p| !gone(p));
    }

    pub(super) fn reset(&mut self) {
        self.groups.clear();
        self.open.clear();
    }
}

impl Row {
    /// How many files this row holds (0: it is not a parent).
    pub(super) fn nested_files(&self) -> usize {
        match self.nest {
            nesting::Role::Parent(n) => n,
            _ => 0,
        }
    }
}

impl FileTree {
    /// The grouping of a loaded folder, or `None` when nesting is off or the folder is not loaded.
    pub(super) fn nest_groups(&mut self, dir: &str) -> Option<Arc<nesting::Groups>> {
        if !self.opts.nesting.enabled {
            return None;
        }
        if let Some(groups) = self.nests.groups.get(dir) {
            return Some(groups.clone());
        }
        let Some(Load::Loaded(entries)) = self.children.get(dir) else { return None };
        let groups = Arc::new(self.opts.nesting.group(entries));
        self.nests.groups.insert(dir.to_string(), groups.clone());
        Some(groups)
    }

    /// Whether the row shows its contents: an open folder, or a parent file whose nested files are listed. A filtered tree lists
    /// every folder it shows open.
    pub(super) fn row_expanded(&self, row: &Row) -> bool {
        if row.is_dir { self.filter.is_active() || self.expanded.contains(&row.path) } else { self.nests.is_open(&row.path) }
    }

    /// The marker of a closed parent file that has none itself but holds a changed file: drawn as a dot, as a folder shows its contents.
    pub(super) fn nest_held_mark(&mut self, row: &Row) -> Option<crate::git::Mark> {
        if row.nested_files() == 0 || self.nests.is_open(&row.path) || self.git.contains_key(&row.path) {
            return None;
        }
        let (dir, name) = row.path.rsplit_once('/')?;
        let groups = self.nest_groups(dir)?;
        let Some(Load::Loaded(entries)) = self.children.get(dir) else { return None };
        let parent = entries.iter().position(|e| e.name == name)?;
        groups.children.get(&parent)?.iter().find_map(|&k| self.git.get(&ui::join(dir, &entries.get(k)?.name)).copied())
    }

    /// Forget the open parents of a folder that are gone (deleted or renamed) or hold nothing now, so the saved list does not grow.
    pub(super) fn prune_nests(&mut self, dir: &str) {
        let prefix = format!("{dir}/");
        if !self.nests.open.iter().any(|p| p.strip_prefix(&prefix).is_some_and(|rest| !rest.contains('/'))) {
            return;
        }
        let groups = self.nest_groups(dir);
        let Some(Load::Loaded(entries)) = self.children.get(dir) else { return };
        // With nesting off nothing is a parent, but the choice stays for files that are still there.
        let keep: HashSet<String> = match &groups {
            Some(g) => g.children.keys().filter_map(|&i| entries.get(i)).map(|e| ui::join(dir, &e.name)).collect(),
            None => entries.iter().map(|e| ui::join(dir, &e.name)).collect(),
        };
        self.nests.open.retain(|p| !p.strip_prefix(&prefix).is_some_and(|rest| !rest.contains('/')) || keep.contains(p));
    }

    /// Open or close the nested files of a parent file.
    pub(super) fn toggle_nest(&mut self, path: &str, cx: &mut Context<Self>) {
        let opening = self.nests.open.insert(path.to_string());
        if !opening {
            self.nests.open.remove(path);
            // The selected nested file is about to leave the list: the parent takes the selection.
            let inside = self.selected.is_some_and(|ix| {
                self.rows[ix].nest == nesting::Role::Child
                    && self.rows[..ix].iter().rev().find(|r| r.nest != nesting::Role::Child).is_some_and(|parent| parent.path == path)
            });
            if inside {
                self.selected = self.rows.iter().position(|r| r.path == path);
                self.marked = self.selected.map(|ix| self.rows[ix].path.clone()).into_iter().collect();
                self.anchor = self.marked.iter().next().cloned();
                self.selection_changed(cx);
            }
        }
        self.rebuild();
        cx.emit(TreeEvent::ExpandedChanged);
        cx.notify();
    }

    /// Open the parent file that holds `path`, if there is one (so a file revealed in the tree is not hidden). Returns whether it
    /// changed the rows.
    pub(super) fn open_nest_holding(&mut self, path: &str) -> bool {
        let Some((dir, name)) = path.rsplit_once('/') else { return false };
        let Some(groups) = self.nest_groups(dir) else { return false };
        let Some(Load::Loaded(entries)) = self.children.get(dir) else { return false };
        let Some(child) = entries.iter().position(|e| e.name == name) else { return false };
        let Some(parent) = groups.children.iter().find(|(_, kids)| kids.contains(&child)).and_then(|(p, _)| entries.get(*p)) else { return false };
        let parent = ui::join(dir, &parent.name);
        let opened = self.nests.open.insert(parent);
        if opened {
            self.rebuild();
        }
        opened
    }

    /// Parent files that are open, for persistence.
    pub fn open_nests(&self) -> Vec<String> {
        let mut v: Vec<String> = self.nests.open.iter().cloned().collect();
        v.sort();
        v
    }

    /// Open the parent files that were open in the previous session.
    pub fn restore_nests(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let dirs: HashSet<String> = paths.iter().filter_map(|p| p.rsplit_once('/').map(|(d, _)| d.to_string())).collect();
        self.nests.open.extend(paths);
        // Folders already read have said which files exist.
        for dir in dirs {
            self.prune_nests(&dir);
        }
        self.rebuild();
        cx.notify();
    }
}
