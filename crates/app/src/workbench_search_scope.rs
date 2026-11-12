//! Workbench: the files behind the Search view's scopes. The open editors and the changed files come from state the
//! workbench already has (nothing is run to find them, so this is safe before a folder is trusted and on SSH folders);
//! they are handed to the panel again whenever they change.

use super::*;
use crate::search::scope::Scope;

gpui::actions!(search_scope, [SearchInWorkspace, SearchInOpenEditors, SearchInChangedFiles, SearchInCurrentFolder]);

impl Workbench {
    /// *Search: …*: the Search view opens and looks only in `scope`.
    fn search_in_scope(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        self.find_in_files(&FindInFiles, window, cx);
        self.search.update(cx, |s, cx| s.set_scope(scope, cx));
        self.sync_search_scope(cx);
    }

    pub(in crate::workbench) fn search_in_workspace(&mut self, _: &SearchInWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        self.search_in_scope(Scope::Workspace, window, cx);
    }

    pub(in crate::workbench) fn search_in_open_editors(&mut self, _: &SearchInOpenEditors, window: &mut Window, cx: &mut Context<Self>) {
        self.search_in_scope(Scope::OpenEditors, window, cx);
    }

    pub(in crate::workbench) fn search_in_changed_files(&mut self, _: &SearchInChangedFiles, window: &mut Window, cx: &mut Context<Self>) {
        self.search_in_scope(Scope::ChangedFiles, window, cx);
    }

    pub(in crate::workbench) fn search_in_current_folder(&mut self, _: &SearchInCurrentFolder, window: &mut Window, cx: &mut Context<Self>) {
        self.search_in_scope(Scope::CurrentFolder, window, cx);
    }

    /// The absolute files of `scope`; for the current folder, that folder (the front file's, else the workspace's).
    pub(in crate::workbench) fn search_scope_paths(&self, scope: Scope, cx: &App) -> Vec<String> {
        match scope {
            Scope::Workspace => Vec::new(),
            Scope::OpenEditors => {
                let mut paths: Vec<String> = self.all_editors().iter().map(|e| e.read(cx).path.clone()).collect();
                paths.sort();
                paths.dedup();
                paths
            }
            Scope::ChangedFiles => self.vcs.status.as_ref().map(|s| s.changes.iter().map(|c| s.abs(&c.path)).collect()).unwrap_or_default(),
            Scope::CurrentFolder => {
                let front = self.tab_editor().map(|e| e.read(cx).path.clone());
                front.and_then(|p| p.rsplit_once('/').map(|(dir, _)| dir.to_string())).or_else(|| self.root(cx)).into_iter().collect()
            }
        }
    }

    /// Gives the Search view the files of its scope (called when the scope changes and on every frame, since the editors
    /// and the changes move on; the panel searches again only when the files differ).
    pub(crate) fn sync_search_scope(&mut self, cx: &mut Context<Self>) {
        let scope = self.search.read(cx).scope();
        if !scope.needs_paths() {
            return;
        }
        let paths = self.search_scope_paths(scope, cx);
        self.search.update(cx, |s, cx| s.set_scope_paths(paths, cx));
    }
}

#[cfg(test)]
impl Workbench {
    /// The rows of the first Search Editor tab (files and matches as listed).
    pub fn search_editor_rows_for_test(&self, cx: &App) -> Vec<String> {
        self.all_search_editors().first().map(|e| e.read(cx).rows_for_test()).unwrap_or_default()
    }

    /// How many Search Editor tabs are open.
    pub fn search_editor_count_for_test(&self) -> usize {
        self.all_search_editors().len()
    }
}
