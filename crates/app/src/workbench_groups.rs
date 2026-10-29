//! Workbench: spatial editor groups, view lifetimes and tab transfers.

use super::*;
use crate::editor_layout::{Direction, GroupId, MAX_GROUPS};

/// Every view in a group shares this lifetime token. Late reads and stale UI
/// actions cannot target a new group that happens to reuse its numeric slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GroupKey {
    pub(super) id: crate::editor_layout::GroupId,
    pub(super) epoch: u64,
}

#[derive(Clone, Copy)]
pub(super) struct GroupResize {
    pub(super) split: u8,
    pub(super) axis: crate::editor_layout::Axis,
    pub(super) bounds: gpui::Bounds<gpui::Pixels>,
    pub(super) anchor: GroupKey,
    pub(super) revision: u64,
}

pub(super) struct Group {
    epoch: u64,
    pub(super) editors: Vec<Entity<EditorView>>,
    pub(super) active: Option<usize>,
    pub(super) opening: Option<String>,
    pub(super) open_error: Option<(String, SharedString)>,
    /// Single diff tab (a new diff replaces the previous one); `diff_active`: that tab is in front.
    pub(super) diff: Option<Entity<crate::diff::DiffView>>,
    pub(super) diff_active: bool,
    /// Single large-file view (read-only; a separate tab like diff).
    pub(super) large: Option<Entity<crate::large::LargeFileView>>,
    pub(super) large_active: bool,
    /// Welcome or Settings page (single tab, like diff); `page_active`: that tab is in front.
    pub(super) page: Option<Page>,
    pub(super) page_active: bool,
    pub(super) _page_sub: Option<Subscription>,
    /// Match to show in the file opened from a search result: (path, line, column, length).
    pub(super) pending_reveal: Option<(String, u32, u32, u32)>,
    /// Tab bar scroll (so the active tab stays visible).
    pub(super) tab_scroll: gpui::ScrollHandle,
}

impl Default for Group {
    fn default() -> Self {
        static NEXT_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            epoch: NEXT_EPOCH.fetch_add(1, Ordering::Relaxed),
            editors: Vec::new(),
            active: None,
            opening: None,
            open_error: None,
            diff: None,
            diff_active: false,
            large: None,
            large_active: false,
            page: None,
            page_active: false,
            _page_sub: None,
            pending_reveal: None,
            tab_scroll: gpui::ScrollHandle::new(),
        }
    }
}

impl state_glue::TabsState {
    pub(super) fn group(&self, id: GroupId) -> Option<&Group> {
        if self.layout.focused() == id { Some(&self.current) } else { self.groups.get(&id) }
    }

    pub(super) fn group_mut(&mut self, id: GroupId) -> Option<&mut Group> {
        if self.layout.focused() == id { Some(&mut self.current) } else { self.groups.get_mut(&id) }
    }

    pub(super) fn key(&self, id: GroupId) -> Option<GroupKey> {
        self.group(id).map(|group| GroupKey { id, epoch: group.epoch })
    }

    pub(super) fn current_key(&self) -> GroupKey {
        GroupKey { id: self.layout.focused(), epoch: self.current.epoch }
    }

    pub(super) fn contains_key(&self, key: GroupKey) -> bool {
        self.key(key.id) == Some(key)
    }

    pub(super) fn focus(&mut self, key: GroupKey) -> bool {
        if !self.contains_key(key) {
            return false;
        }
        let old = self.layout.focused();
        if old != key.id {
            let Some(next) = self.groups.remove(&key.id) else { return false };
            let previous = std::mem::replace(&mut self.current, next);
            self.groups.insert(old, previous);
            self.layout.focus(key.id);
            self.reveal_group.set(Some(key));
        }
        true
    }

    pub(super) fn split(&mut self, direction: Direction) -> Result<GroupKey, crate::editor_layout::Error> {
        let old = self.layout.focused();
        let new = self.layout.split(old, direction)?;
        self.revision = self.revision.wrapping_add(1);
        let previous = std::mem::take(&mut self.current);
        self.groups.insert(old, previous);
        self.reveal_group.set(Some(self.current_key()));
        Ok(self.current_key_with_id(new))
    }

    fn current_key_with_id(&self, id: GroupId) -> GroupKey {
        GroupKey { id, epoch: self.current.epoch }
    }

    pub(super) fn remove_empty(&mut self, key: GroupKey) -> bool {
        if !self.contains_key(key) || !self.group(key.id).is_some_and(Group::is_empty) {
            return false;
        }
        let was_current = key.id == self.layout.focused();
        if !self.layout.remove(key.id) {
            return false;
        }
        self.revision = self.revision.wrapping_add(1);
        if was_current {
            self.current = self.groups.remove(&self.layout.focused()).expect("layout group has state");
        } else {
            self.groups.remove(&key.id);
        }
        self.reveal_group.set(Some(self.current_key()));
        true
    }
}

impl Group {
    pub(super) fn is_empty(&self) -> bool {
        self.editors.is_empty() && self.diff.is_none() && self.large.is_none() && self.page.is_none() && self.opening.is_none()
    }
}

impl Workbench {
    pub(super) fn focused_slot(&self) -> usize {
        usize::from(self.tabs.layout.focused())
    }

    pub(super) fn sync_slots(&self, cx: &mut App) {
        let ids = self.tabs.layout.groups();
        for ed in self.all_editors() {
            let slots: Vec<usize> = ids.iter().copied().filter(|id| self.tabs.group(*id).is_some_and(|g| g.editors.contains(&ed))).map(usize::from).collect();
            ed.update(cx, |e, cx| {
                for slot in 0..MAX_GROUPS {
                    if slots.contains(&slot) {
                        e.open_slot(slot, cx);
                    } else {
                        e.close_slot(slot);
                    }
                }
                let focused = self.focused_slot();
                if let Some(home) = slots.iter().copied().find(|slot| *slot == focused).or_else(|| slots.first().copied()) {
                    e.set_home(home);
                }
            });
        }
    }

    pub(super) fn refresh_active_flags(&self, cx: &mut App) {
        let front: Vec<_> = self
            .tabs
            .layout
            .groups()
            .into_iter()
            .filter_map(|id| {
                let group = self.tabs.group(id)?;
                if group.diff_active || group.large_active || group.page_active {
                    return None;
                }
                group.active.and_then(|ix| group.editors.get(ix)).cloned()
            })
            .collect();
        for ed in self.all_editors() {
            ed.update(cx, |e, _| e.set_active(front.contains(&ed)));
        }
    }

    pub(super) fn also_in_other(&self, ed: &Entity<EditorView>) -> bool {
        self.tabs.groups.values().any(|g| g.editors.contains(ed))
    }

    pub(super) fn ensure_group(&mut self, key: GroupKey) -> bool {
        self.tabs.focus(key)
    }

    pub(super) fn focus_group_key(&mut self, key: GroupKey, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ensure_group(key) {
            return;
        }
        self.sync_slots(cx);
        self.refresh_active_flags(cx);
        if self.tabs.diff_active {
            if let Some(view) = &self.tabs.diff {
                window.focus(&view.focus_handle(cx));
            }
        } else if self.tabs.large_active {
            if let Some(view) = &self.tabs.large {
                window.focus(&view.focus_handle(cx));
            }
        } else if !self.tabs.page_active {
            if let Some(ed) = self.active_editor() {
                window.focus(&ed.focus_handle(cx));
            } else {
                window.focus(&self.focus_handle);
            }
        } else {
            let handle = match &self.tabs.page {
                Some(Page::Settings(view)) => view.focus_handle(cx),
                Some(Page::Markdown(view)) => view.focus_handle(cx),
                Some(Page::Shortcuts(view)) => view.focus_handle(cx),
                Some(Page::Merge(view)) => view.focus_handle(cx),
                Some(Page::Compare(view)) => view.focus_handle(cx),
                _ => self.focus_handle.clone(),
            };
            window.focus(&handle);
        }
        self.persist_session(cx);
        cx.notify();
    }

    fn focus_direction(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.tabs.layout.neighbor(direction).and_then(|id| self.tabs.key(id)) {
            self.focus_group_key(key, window, cx);
        }
    }

    pub(super) fn focus_left_group(&mut self, _: &FocusLeftGroup, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_direction(Direction::Left, window, cx);
    }
    pub(super) fn focus_right_group(&mut self, _: &FocusRightGroup, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_direction(Direction::Right, window, cx);
    }
    pub(super) fn focus_up_group(&mut self, _: &FocusUpGroup, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_direction(Direction::Up, window, cx);
    }
    pub(super) fn focus_down_group(&mut self, _: &FocusDownGroup, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_direction(Direction::Down, window, cx);
    }

    pub(super) fn drop_tab(
        &mut self,
        ed: Entity<EditorView>,
        from: GroupKey,
        to: GroupKey,
        before: Option<Entity<EditorView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tabs.contains_key(from) || !self.tabs.contains_key(to) || Some(&ed) == before.as_ref() {
            return;
        }
        if let Some(before) = &before
            && !self.tabs.group(to.id).is_some_and(|g| g.editors.contains(before))
        {
            return;
        }
        let source = self.tabs.group_mut(from.id).expect("validated group");
        let Some(index) = source.editors.iter().position(|e| *e == ed) else { return };
        source.editors.remove(index);
        source.active = match source.active {
            _ if source.editors.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) => Some(active.min(source.editors.len() - 1)),
            None => None,
        };
        let target = self.tabs.group_mut(to.id).expect("validated group");
        if let Some(index) = target.editors.iter().position(|e| *e == ed) {
            target.editors.remove(index);
        }
        let index = before.and_then(|b| target.editors.iter().position(|e| *e == b)).unwrap_or(target.editors.len());
        target.editors.insert(index, ed);
        target.active = Some(index);
        if from != to {
            self.tabs.remove_empty(from);
        }
        if self.tabs.focus(to) {
            self.activate(index, window, cx);
        }
        self.persist_session(cx);
        cx.notify();
    }

    fn split_direction(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active_editor().cloned();
        let key = match self.tabs.split(direction) {
            Ok(key) => key,
            Err(_) => {
                self.notice = Some(Notice::Info(crate::i18n::tf("A window supports up to {} editor groups.", &[&MAX_GROUPS.to_string()]).into()));
                cx.notify();
                return;
            }
        };
        if let Some(ed) = active {
            self.tabs.editors.push(ed);
            self.activate(0, window, cx);
        } else {
            self.focus_group_key(key, window, cx);
        }
        self.persist_session(cx);
        cx.notify();
    }

    pub(super) fn split_editor(&mut self, _: &SplitEditor, window: &mut Window, cx: &mut Context<Self>) {
        self.split_direction(Direction::Right, window, cx);
    }

    pub(super) fn open_editor_beside(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        match self.tabs.split(Direction::Right) {
            Ok(key) => {
                self.focus_group_key(key, window, cx);
                self.open_file(path, window, cx);
            }
            Err(_) => {
                self.notice = Some(Notice::Info(crate::i18n::tf("A window supports up to {} editor groups.", &[&MAX_GROUPS.to_string()]).into()));
                cx.notify();
            }
        }
    }
    pub(super) fn split_editor_left(&mut self, _: &SplitEditorLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.split_direction(Direction::Left, window, cx);
    }
    pub(super) fn split_editor_up(&mut self, _: &SplitEditorUp, window: &mut Window, cx: &mut Context<Self>) {
        self.split_direction(Direction::Up, window, cx);
    }
    pub(super) fn split_editor_down(&mut self, _: &SplitEditorDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split_direction(Direction::Down, window, cx);
    }

    pub(super) fn close_empty_editor_group(&mut self, _: &CloseEmptyEditorGroup, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.tabs.current_key();
        if self.tabs.editors.is_empty() && self.tabs.diff.is_none() && self.tabs.large.is_none() && self.tabs.page.is_none() {
            self.tabs.opening = None;
            self.tabs.open_error = None;
        }
        if self.tabs.remove_empty(key) {
            self.focus_group_key(self.tabs.current_key(), window, cx);
        }
    }

    pub(super) fn move_editor_to_other_group(&mut self, _: &MoveEditorToOtherGroup, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.tabs.current_key();
        let ed = self.active_editor().cloned();
        let was_single = self.tabs.layout.groups().len() == 1;
        let target = self
            .tabs
            .layout
            .neighbor(Direction::Right)
            .or_else(|| self.tabs.layout.groups().into_iter().find(|id| *id != from.id))
            .and_then(|id| self.tabs.key(id));
        let target = match target.or_else(|| self.tabs.split(Direction::Right).ok()) {
            Some(target) => target,
            None => return,
        };
        if let Some(ed) = ed.filter(|_| !was_single || self.tabs.group(from.id).is_some_and(|g| g.editors.len() > 1)) {
            self.drop_tab(ed, from, target, None, window, cx);
        } else {
            self.focus_group_key(target, window, cx);
        }
    }
}
