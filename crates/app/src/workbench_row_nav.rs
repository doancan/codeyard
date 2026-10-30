//! Keyboard rows of the Run and Debug lists (variables, watches, call stack, breakpoints), the Testing tree, the Remote
//! Explorer and the Ports list. Each list takes the focus as a whole (Tab, or a click on a row) and keeps a cursor row,
//! as the Source Control repository list does: Up/Down/Home/End move it, Enter does what a click on the row does, and
//! Shift+F10 or the Menu key (`menu::ShowContextMenu`) opens the menu a right click opens, under the row. The menu
//! gives the focus back to the list when it closes. The cursor always names a row of the list as it is now, so a row
//! that went away is never acted on: the cursor moves to the last row instead.

use std::collections::HashMap;

use gpui::{AnyElement, FocusHandle, ScrollStrategy, UniformListScrollHandle, div};

use super::a11y::{self, RowFrame};
use super::debug::{StackRow, stack_rows, variable_rows};
use super::ports_glue::port_rows;
use super::remote_glue::tree::RemoteRow;
use super::*;
use crate::debug_model::BpRow;
use crate::testexp::{self, Row};

gpui::actions!(rows, [RowPrevious, RowNext, RowFirst, RowLast, RowActivate]);

/// The key context of these lists.
pub const ROW_LIST: &str = "RowList";

const KINDS: [RowFrame; 7] =
    [RowFrame::DebugVariable, RowFrame::DebugWatch, RowFrame::DebugStack, RowFrame::DebugBreakpoint, RowFrame::Test, RowFrame::Remote, RowFrame::Port];

/// Focus, cursor and scroll position of each list.
pub(super) struct RowNav {
    focus: HashMap<RowFrame, FocusHandle>,
    scroll: HashMap<RowFrame, UniformListScrollHandle>,
    cursor: HashMap<RowFrame, usize>,
}

impl RowNav {
    pub(super) fn new(cx: &mut App) -> Self {
        Self {
            focus: KINDS.into_iter().map(|k| (k, cx.focus_handle().tab_stop(true))).collect(),
            scroll: KINDS.into_iter().map(|k| (k, UniformListScrollHandle::new())).collect(),
            cursor: HashMap::new(),
        }
    }

    pub(super) fn focus(&self, kind: RowFrame) -> &FocusHandle {
        &self.focus[&kind]
    }

    pub(super) fn scroll(&self, kind: RowFrame) -> UniformListScrollHandle {
        self.scroll[&kind].clone()
    }

    /// The list that has the keyboard, and its cursor row.
    pub(super) fn focused(&self, window: &Window) -> Option<(RowFrame, usize)> {
        let kind = KINDS.into_iter().find(|k| self.focus[k].is_focused(window))?;
        Some((kind, self.cursor.get(&kind).copied().unwrap_or(0)))
    }
}

/// Where a keyboard move goes.
#[derive(Clone, Copy)]
enum Step {
    By(isize),
    First,
    Last,
}

impl Workbench {
    /// `list` (a uniform list of `kind` rows) as a keyboard list: it takes the focus, shows the focus ring while it has
    /// it, and runs the row actions.
    pub(super) fn row_list(&self, kind: RowFrame, list: impl IntoElement, cx: &mut Context<Self>) -> AnyElement {
        let p = *crate::ui::palette(cx);
        div()
            .id(("row-list", KINDS.iter().position(|k| *k == kind).unwrap_or(0)))
            .size_full()
            .track_focus(self.chrome.row_nav.focus(kind))
            .key_context(ROW_LIST)
            .border_1()
            .border_color(crate::ui::ca(p.focus_ring, 0.))
            .focus(|d| d.border_color(c(p.focus_ring)))
            .on_action(cx.listener(move |this, _: &RowPrevious, _, cx| this.step_row(kind, Step::By(-1), cx)))
            .on_action(cx.listener(move |this, _: &RowNext, _, cx| this.step_row(kind, Step::By(1), cx)))
            .on_action(cx.listener(move |this, _: &RowFirst, _, cx| this.step_row(kind, Step::First, cx)))
            .on_action(cx.listener(move |this, _: &RowLast, _, cx| this.step_row(kind, Step::Last, cx)))
            .on_action(cx.listener(move |this, _: &RowActivate, window, cx| this.activate_row(kind, window, cx)))
            .on_action(cx.listener(move |this, _: &crate::menu::ShowContextMenu, window, cx| this.row_context_menu(kind, window, cx)))
            .child(list)
            .into_any_element()
    }

    /// One row of such a list: its painted bounds are recorded, the cursor row is highlighted while the list has the
    /// keyboard, and a press on it moves the cursor there (the row's own click handler still runs).
    pub(super) fn nav_row(&self, kind: RowFrame, ix: usize, row: AnyElement, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let nav = &self.chrome.row_nav;
        let current = nav.focus(kind).is_focused(window) && self.row_cursor(kind, cx) == Some(ix);
        let p = *crate::ui::palette(cx);
        div()
            .relative()
            .w_full()
            .when(current, |d| d.bg(c(crate::ui::focused_selection(&p))))
            .child(row)
            .when(current, |d| d.child(div().absolute().top_0().left_0().size_full().border_1().border_color(c(p.focus_ring))))
            .child(a11y::row_frame(kind, ix, cx))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, _| {
                    this.chrome.row_nav.cursor.insert(kind, ix);
                    window.focus(&this.chrome.row_nav.focus(kind).clone());
                }),
            )
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, _, window, _| {
                    this.chrome.row_nav.cursor.insert(kind, ix);
                    window.focus(&this.chrome.row_nav.focus(kind).clone());
                }),
            )
            .into_any_element()
    }

    /// The cursor row of `kind`, kept inside the list as it is now (`None` for an empty list).
    pub(super) fn row_cursor(&self, kind: RowFrame, cx: &App) -> Option<usize> {
        let n = self.row_count(kind, cx);
        (n > 0).then(|| self.chrome.row_nav.cursor.get(&kind).copied().unwrap_or(0).min(n - 1))
    }

    fn step_row(&mut self, kind: RowFrame, step: Step, cx: &mut Context<Self>) {
        let n = self.row_count(kind, cx);
        if n == 0 {
            return;
        }
        let at = self.row_cursor(kind, cx).unwrap_or(0) as isize;
        let to = match step {
            Step::By(d) => (at + d).clamp(0, n as isize - 1) as usize,
            Step::First => 0,
            Step::Last => n - 1,
        };
        self.chrome.row_nav.cursor.insert(kind, to);
        self.row_scroll(kind).scroll_to_item(to, ScrollStrategy::Center);
        cx.notify();
    }

    /// The scroll handle a list of `kind` is drawn with.
    pub(super) fn row_scroll(&self, kind: RowFrame) -> UniformListScrollHandle {
        match kind {
            RowFrame::Test => self.jobs.tests.scroll.clone(),
            RowFrame::Remote => self.remote.view.scroll.clone(),
            _ => self.chrome.row_nav.scroll(kind),
        }
    }

    /// How many rows the list of `kind` shows now (the same rows it draws).
    fn row_count(&self, kind: RowFrame, cx: &App) -> usize {
        let d = &self.dbg;
        match kind {
            RowFrame::DebugVariable => d.cur().map(variable_rows).map_or(0, |r| r.len()),
            RowFrame::DebugWatch => d.watch_exprs.len(),
            RowFrame::DebugStack => stack_rows(&d.sessions, d.cur().map(|s| s.id)).len(),
            RowFrame::DebugBreakpoint => d.bps.rows().len(),
            RowFrame::Test => self.test_rows(cx).len(),
            RowFrame::Remote => self.remote_rows(cx).len(),
            RowFrame::Port => self.ssh_alias().map_or(0, |alias| port_rows(&self.ports.set, &self.ports.listening, &alias).len()),
        }
    }

    fn test_rows(&self, cx: &App) -> Vec<Row> {
        let t = &self.jobs.tests;
        let filter = t.filter.as_ref().map(|f| f.read(cx).text().to_string()).unwrap_or_default();
        testexp::rows(&t.files, &t.results, &t.expanded, &filter, t.only_failed)
    }

    /// Enter: what a click on the cursor row does.
    fn activate_row(&mut self, kind: RowFrame, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.row_cursor(kind, cx) else { return };
        match kind {
            RowFrame::DebugVariable => {
                let Some(r) = self.dbg.cur().map(variable_rows).and_then(|rows| rows.into_iter().nth(ix)) else { return };
                match r.more {
                    Some(paged) => self.load_more_variables(paged, cx),
                    None if r.reference > 0 => self.toggle_variable(r.reference, cx),
                    None => {}
                }
            }
            RowFrame::DebugWatch => {}
            RowFrame::DebugStack => match stack_rows(&self.dbg.sessions, self.dbg.cur().map(|s| s.id)).into_iter().nth(ix) {
                Some(StackRow::Session { id, .. }) => self.focus_debug_session(id, cx),
                Some(StackRow::Thread { id, .. }) => self.select_thread(id, cx),
                Some(StackRow::Frame { index, .. }) => self.select_frame(index, cx),
                Some(StackRow::LoadMore) => self.load_more_frames(cx),
                _ => {}
            },
            RowFrame::DebugBreakpoint => {
                if let Some(BpRow::Line { path, line, .. }) = self.dbg.bps.rows().into_iter().nth(ix) {
                    self.open_location(path, line as u32, 0, window, cx);
                }
            }
            RowFrame::Test => match self.test_rows(cx).into_iter().nth(ix) {
                Some(Row::File { file, .. }) => {
                    let Some(rel) = self.jobs.tests.files.get(file).map(|f| f.rel.clone()) else { return };
                    let expanded = &mut self.jobs.tests.expanded;
                    if !expanded.remove(&rel) {
                        expanded.insert(rel);
                    }
                    cx.notify();
                }
                Some(Row::Test { file, test, .. }) => self.open_test(file, test, false, window, cx),
                Some(Row::Message { file, test, .. }) => self.open_test(file, test, true, window, cx),
                None => {}
            },
            RowFrame::Remote => match self.remote_rows(cx).into_iter().nth(ix) {
                Some(RemoteRow::Group { group, .. }) => self.toggle_remote(group.key().to_string(), cx),
                Some(RemoteRow::Host { alias, .. }) => self.toggle_remote(remote_glue::tree::host_key(&alias), cx),
                Some(RemoteRow::Folder { alias, path, last_opened, .. }) => {
                    let recent = state::RecentWorkspace { host: Some(alias), path, last_opened_at: last_opened };
                    self.open_recent(recent, window, cx);
                }
                Some(RemoteRow::PortsHeader { .. }) => self.show_ports(&ShowPorts, window, cx),
                _ => {}
            },
            // A port row has no click of its own; its buttons and menu are the actions.
            RowFrame::Port => {}
        }
    }

    /// Shift+F10 or the Menu key: the cursor row's menu, as a right click on it opens it.
    fn row_context_menu(&mut self, kind: RowFrame, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.row_cursor(kind, cx) {
            self.open_row_menu(kind, ix, window, cx);
        }
    }

    /// The menu of row `ix` of `kind` as the list shows it now, under the row; `false` if that row has no menu.
    pub(super) fn open_row_menu(&mut self, kind: RowFrame, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let at = self.row_anchor(kind, ix, window);
        match kind {
            RowFrame::DebugVariable => match self.dbg.cur().map(variable_rows).and_then(|rows| rows.into_iter().nth(ix)) {
                Some(r) if r.parent != 0 && r.more.is_none() => self.open_variable_menu(r.parent, r.name, at, window, cx),
                _ => return false,
            },
            RowFrame::DebugStack => match stack_rows(&self.dbg.sessions, self.dbg.cur().map(|s| s.id)).into_iter().nth(ix) {
                Some(StackRow::Frame { index, .. }) => self.open_frame_menu(index, at, window, cx),
                _ => return false,
            },
            RowFrame::DebugBreakpoint => match self.dbg.bps.rows().into_iter().nth(ix) {
                Some(BpRow::Line { path, line, meta }) => self.open_breakpoint_menu(path, line, meta.disabled, at, window, cx),
                _ => return false,
            },
            RowFrame::Remote => match self.remote_rows(cx).into_iter().nth(ix) {
                Some(RemoteRow::Host { alias, group, .. }) => self.open_host_menu(&alias, group, at, window, cx),
                _ => return false,
            },
            RowFrame::Port => {
                let Some(alias) = self.ssh_alias() else { return false };
                let Some(row) = port_rows(&self.ports.set, &self.ports.listening, &alias).into_iter().nth(ix) else { return false };
                self.open_port_menu(&row, at, window, cx);
            }
            RowFrame::DebugWatch | RowFrame::Test => return false,
        }
        true
    }

    /// Where a menu opened without the pointer appears: at the bottom left of its row as last painted, else (the row is
    /// scrolled out of view) at the middle of the window.
    pub(super) fn row_anchor(&self, kind: RowFrame, ix: usize, window: &Window) -> gpui::Point<gpui::Pixels> {
        match self.chrome.a11y_bounds.row(kind, ix) {
            Some(bounds) => bounds.bottom_left(),
            None => {
                let size = window.viewport_size();
                gpui::point(size.width / 2., size.height / 2.)
            }
        }
    }

    /// The list that has the keyboard and its cursor row, for the accessibility tree's focus.
    pub(super) fn focused_row(&self, window: &Window, cx: &App) -> Option<(RowFrame, usize)> {
        let (kind, _) = self.chrome.row_nav.focused(window)?;
        Some((kind, self.row_cursor(kind, cx)?))
    }

    /// The list `kind` has the keyboard on row `ix` (tests).
    #[cfg(test)]
    pub(in crate::workbench) fn focus_row_list_for_test(&mut self, kind: RowFrame, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.row_nav.cursor.insert(kind, ix);
        window.focus(&self.chrome.row_nav.focus(kind).clone());
        cx.notify();
    }
}

#[cfg(test)]
#[path = "workbench_row_nav_tests.rs"]
mod tests;
