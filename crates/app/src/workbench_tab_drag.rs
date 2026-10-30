//! Workbench: an editor tab dragged out of the window. GPUI's drag and drop stays inside one window, but the release
//! of a drag reaches the window it started in even outside its bounds (macOS keeps the mouse with the window that took
//! the press; Windows captures it with `SetCapture` on the press; X11 and Wayland grab the pointer while a button is
//! held), with the position relative to that window. So the workbench remembers which tab a drag carries, and when
//! the drag ends outside the window:
//! - over another Codeyard window (by its last drawn bounds, front-most first), the file opens there and its tab here
//!   closes; a file with unsaved changes or from another machine stays (the other window has its own documents);
//! - anywhere else, the tab becomes an editor window of its own at that place (`workbench_editor_windows.rs`): the
//!   same document, nothing copied.

use super::*;
use gpui::{Bounds, Pixels, Point};

/// The first window in `windows` (front to back) whose bounds contain `point` (screen coordinates).
pub(super) fn window_at<T: Copy>(point: Point<Pixels>, windows: &[(T, Bounds<Pixels>)]) -> Option<T> {
    windows.iter().find(|(_, b)| b.contains(&point)).map(|(t, _)| *t)
}

impl Workbench {
    /// The tab a drag carries from now on (set when the drag starts).
    pub(super) fn start_tab_drag(&mut self, editor: Entity<EditorView>, from: GroupKey) {
        self.chrome.tab_drag = Some((editor, from));
    }

    /// A left button release: when a tab drag ends outside this window, the tab leaves for another Codeyard window or
    /// a window of its own. A release inside the window is the drop targets' business.
    pub(super) fn end_tab_drag(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((editor, from)) = self.chrome.tab_drag.take() else { return };
        let inside = Bounds::new(gpui::point(px(0.), px(0.)), window.viewport_size()).contains(&position);
        if inside {
            return;
        }
        let screen = window.bounds().origin + position;
        // Deferred: the release is still being delivered to this window.
        cx.defer_in(window, move |this, window, cx| this.drop_tab_outside(editor, from, screen, window, cx));
    }

    /// The other Codeyard windows, front to back, with the bounds they were last drawn at.
    fn other_workbenches(&self, cx: &App) -> Vec<(gpui::WindowHandle<Workbench>, Bounds<Pixels>)> {
        let me = self.window_handle;
        let order = cx.window_stack().unwrap_or_else(|| cx.windows());
        order
            .into_iter()
            .filter(|w| *w != me)
            .filter_map(|w| w.downcast::<Workbench>())
            .filter_map(|w| Some((w, w.read(cx).ok()?.chrome.window_bounds?.get_bounds())))
            .collect()
    }

    /// The tab of `editor` in group `from`, dropped at `screen` outside this window.
    pub(super) fn drop_tab_outside(&mut self, editor: Entity<EditorView>, from: GroupKey, screen: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.group(from.id).is_some_and(|g| g.editors.contains(&editor)) || !self.tabs.contains_key(from) {
            return; // the tab closed or moved meanwhile
        }
        let targets = self.other_workbenches(cx);
        match window_at(screen, &targets) {
            Some(target) => self.move_tab_to_window(editor, from, target, window, cx),
            None => {
                let bounds = Bounds::new(screen - gpui::point(px(40.), px(12.)), gpui::size(px(820.), px(620.)));
                if let Err(e) = self.show_in_editor_window(editor.clone(), bounds, None, cx) {
                    self.notice = Some(Notice::Error(e.into()));
                    return cx.notify();
                }
                self.take_tab(&editor, from, window, cx);
                self.refresh_active_flags(cx);
                self.persist_session(cx);
                cx.notify();
            }
        }
    }

    /// Opens the tab's file in the Codeyard window `target` and closes the tab here. Each window has its own
    /// documents, so a file with unsaved changes stays (moving it would leave two different versions), and so does a
    /// file of another machine than the target's folder.
    pub(super) fn move_tab_to_window(
        &mut self,
        editor: Entity<EditorView>,
        from: GroupKey,
        target: gpui::WindowHandle<Workbench>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (path, dirty) = {
            let e = editor.read(cx);
            (e.path.clone(), e.is_dirty())
        };
        let same_host = target.read(cx).is_ok_and(|wb| editor.read(cx).on_same_machine(&wb.remote.host));
        let refusal = if dirty {
            Some(crate::i18n::t("Save the file first: a tab with unsaved changes cannot move to another window."))
        } else if !same_host {
            Some(crate::i18n::t("This tab cannot move to that window: its folder is on another machine."))
        } else {
            None
        };
        if let Some(why) = refusal {
            self.notice = Some(Notice::Info(why.into()));
            return cx.notify();
        }
        let opened = target.update(cx, |wb, window, cx| {
            window.activate_window();
            let host = wb.remote.host.clone();
            wb.open_file_on(host, path, window, cx);
        });
        if opened.is_err() {
            self.notice = Some(Notice::Info(crate::i18n::t("The destination window is no longer available.").into()));
            return cx.notify();
        }
        self.take_tab(&editor, from, window, cx);
    }

    /// Takes `editor`'s tab out of group `from` without asking anything (the document lives on elsewhere, or it was
    /// clean and opened in another window).
    fn take_tab(&mut self, editor: &Entity<EditorView>, from: GroupKey, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.contains_key(from) {
            return;
        }
        if from == self.tabs.current_key() {
            if let Some(ix) = self.tabs.editors.iter().position(|e| e == editor) {
                self.remove_editor(ix, window, cx);
            }
            return;
        }
        let Some(group) = self.tabs.group_mut(from.id) else { return };
        let Some(ix) = group.editors.iter().position(|e| e == editor) else { return };
        group.editors.remove(ix);
        group.active = match group.active {
            _ if group.editors.is_empty() => None,
            Some(a) if a > ix => Some(a - 1),
            Some(a) => Some(a.min(group.editors.len() - 1)),
            None => None,
        };
        self.forget_closed_view(editor, cx);
        self.tabs.remove_empty(from);
        self.sync_slots(cx);
        self.persist_session(cx);
        cx.notify();
    }
}

#[cfg(test)]
#[path = "workbench_tab_drag_tests.rs"]
mod tests;
