//! Path-based multi-selection. `selected` is the keyboard focus, independent of the marked set.

use super::*;

impl FileTree {
    pub fn selected_paths(&self) -> Vec<String> {
        self.rows.iter().filter(|r| self.marked.contains(&r.path)).map(|r| r.path.clone()).collect()
    }

    pub fn path_is_selected(&self, path: &str) -> bool {
        self.marked.contains(path)
    }

    pub(super) fn selection_changed(&self, cx: &mut Context<Self>) {
        if let Some(root) = &self.root {
            cx.emit(TreeEvent::SelectionChanged { root: root.clone() });
        }
    }

    pub(super) fn select_with(&mut self, ix: usize, extend: bool, toggle: bool, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        let path = row.path.clone();
        if extend {
            let anchor = self.anchor.as_ref().and_then(|a| self.rows.iter().position(|r| &r.path == a)).or(self.selected).unwrap_or(ix);
            if !toggle {
                self.marked.clear();
            }
            self.marked.extend(self.rows[anchor.min(ix)..=anchor.max(ix)].iter().map(|r| r.path.clone()));
            if self.anchor.is_none() {
                self.anchor = Some(self.rows[anchor].path.clone());
            }
        } else if toggle {
            if !self.marked.remove(&path) {
                self.marked.insert(path.clone());
            }
            self.anchor = Some(path);
        } else {
            self.marked.clear();
            self.marked.insert(path.clone());
            self.anchor = Some(path);
        }
        self.selected = Some(ix);
        self.root_selected = false;
        self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        self.selection_changed(cx);
        cx.notify();
    }

    pub(super) fn reconcile_selection(&mut self) {
        let visible: HashSet<&str> = self.rows.iter().map(|r| r.path.as_str()).collect();
        self.marked.retain(|p| visible.contains(p.as_str()));
        if self.anchor.as_ref().is_some_and(|p| !visible.contains(p.as_str())) {
            self.anchor = None;
        }
    }

    pub(super) fn extend_prev(&mut self, _: &ExtendPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.select_with(self.selected.unwrap_or(0).saturating_sub(1), true, false, cx);
    }
    pub(super) fn extend_next(&mut self, _: &ExtendNext, _: &mut Window, cx: &mut Context<Self>) {
        self.select_with(self.selected.map_or(0, |i| (i + 1).min(self.rows.len().saturating_sub(1))), true, false, cx);
    }
    pub(super) fn extend_first(&mut self, _: &ExtendFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.select_with(0, true, false, cx);
    }
    pub(super) fn extend_last(&mut self, _: &ExtendLast, _: &mut Window, cx: &mut Context<Self>) {
        self.select_with(self.rows.len().saturating_sub(1), true, false, cx);
    }
    pub(super) fn extend_page_up(&mut self, _: &ExtendPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.select_with(self.selected.unwrap_or(0).saturating_sub(Self::PAGE), true, false, cx);
    }
    pub(super) fn extend_page_down(&mut self, _: &ExtendPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.select_with((self.selected.unwrap_or(0) + Self::PAGE).min(self.rows.len().saturating_sub(1)), true, false, cx);
    }
    pub(super) fn select_all_entries(&mut self, _: &SelectAllEntries, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = self.rows.iter().map(|r| r.path.clone()).collect();
        if self.selected.is_none() && !self.rows.is_empty() {
            self.selected = Some(0);
        }
        self.anchor = self.selected.and_then(|i| self.rows.get(i)).map(|r| r.path.clone());
        self.selection_changed(cx);
        cx.notify();
    }
    pub(super) fn toggle_selection(&mut self, _: &ToggleSelection, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected {
            self.select_with(ix, false, true, cx);
        }
    }
}

#[cfg(test)]
#[path = "tree_selection_tests.rs"]
mod tests;
