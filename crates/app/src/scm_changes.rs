//! The change list of a single repository: one uniform row per group header or file, so only the visible rows are
//! built (thousands of generated changes stay within the frame budget). The scroll handle lives on the panel, so a
//! status refresh keeps the position. The list takes the keyboard focus as a whole and keeps a cursor row: Up/Down/
//! Home/End move it, Enter opens a file's diff or folds a group, Space stages or unstages the file (or folds the group),
//! Left/Right go out to the group and back in, Delete (⌘⌫ on macOS) discards the file's changes after the usual
//! question, and Shift+F10 or the Menu key (`menu::ShowContextMenu`) opens a file's menu under it, as a right click
//! does (the menu gives the keyboard back to the list when it closes). The cursor names its row by identity, so a
//! refresh never moves it onto another file.

use std::sync::Arc;

use gpui::{Context, FocusHandle, KeyDownEvent, MouseButton, ScrollStrategy, Window, div, prelude::*, px, uniform_list};

use super::repos_ui::Group;
use super::{ScmEvent, ScmPanel};
use crate::git::{Change, Mark, Status};
use crate::ui::{c, ca, layout, palette, text};

/// One row of the single-repository list. `File` indexes `Status::changes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeLine {
    Group(Group, usize),
    File(Group, usize),
    /// "No changes."
    Empty,
}

/// Rows of `st`: Merge and Staged Changes only when they hold something, Changes always; files of open groups only.
pub(crate) fn change_lines(st: &Status, open: impl Fn(&str) -> bool) -> Vec<ChangeLine> {
    let mut out = Vec::new();
    for g in Group::ALL {
        let files: Vec<usize> = st.changes.iter().enumerate().filter(|(_, ch)| g.holds(ch)).map(|(i, _)| i).collect();
        if files.is_empty() && g != Group::Changes {
            continue;
        }
        out.push(ChangeLine::Group(g, files.len()));
        if open(g.id()) {
            out.extend(files.into_iter().map(|i| ChangeLine::File(g, i)));
        }
    }
    if st.changes.is_empty() {
        out.push(ChangeLine::Empty);
    }
    out
}

/// The row the cursor is on: a group, or a file of a group (the same path can be both staged and changed).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ChangeKey {
    Group(Group),
    File(Group, String),
}

impl ChangeKey {
    pub(crate) fn of(line: ChangeLine, st: &Status) -> Option<ChangeKey> {
        match line {
            ChangeLine::Group(g, _) => Some(ChangeKey::Group(g)),
            ChangeLine::File(g, i) => st.changes.get(i).map(|c| ChangeKey::File(g, c.path.clone())),
            ChangeLine::Empty => None,
        }
    }
}

/// Can the change be discarded from this group? Conflicts and staged rows cannot (as their painted rows show).
pub(crate) fn discardable(g: Group, change: &Change) -> bool {
    g == Group::Changes && change.unstaged.is_some_and(|m| m != Mark::Conflict)
}

/// What a key press on the change list does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListKey {
    Step(isize),
    Open,
    Toggle,
    Out,
    In,
    Discard,
}

pub(crate) fn list_key(key: &str, m: gpui::Modifiers) -> Option<ListKey> {
    if m.control || m.alt || m.shift || m.function {
        return None;
    }
    Some(match (key, m.platform) {
        ("up", false) => ListKey::Step(-1),
        ("down", false) => ListKey::Step(1),
        ("home", false) => ListKey::Step(isize::MIN),
        ("end", false) => ListKey::Step(isize::MAX),
        ("enter", false) => ListKey::Open,
        ("space", false) => ListKey::Toggle,
        ("left", false) => ListKey::Out,
        ("right", false) => ListKey::In,
        ("delete", false) | ("backspace", true) => ListKey::Discard,
        _ => return None,
    })
}

impl ScmPanel {
    pub(super) fn change_list(&self, st: Arc<Status>, fit: bool, focused: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let lines = Arc::new(self.current_lines(&st));
        let n = lines.len();
        let cursor = if focused { self.cursor_in(&lines, &st) } else { None };
        div()
            .id("scm-changes")
            .track_focus(&self.change_focus)
            .tab_index(0)
            .key_context("ScmChanges")
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if let Some(key) = list_key(&ev.keystroke.key, ev.keystroke.modifiers)
                    && this.list_key(key, window, cx)
                {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &crate::menu::ShowContextMenu, window, cx| {
                this.open_row_menu(None, window, cx);
            }))
            .child(
                uniform_list(
                    "scm-list",
                    n,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range.filter_map(|ix| Some(this.change_line(ix, *lines.get(ix)?, &st, cursor == Some(ix), cx))).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.change_scroll.clone())
                .debug_selector(|| "scm-list".into())
                .size_full(),
            )
            // Nothing to commit: the list takes only its lines and the graph gets the room (until its height is set by hand).
            .when(fit, |d| d.h(px(layout::ROW_H * n as f32)))
            .when(!fit, |d| d.flex_1().min_h(px(72.)))
    }

    fn change_line(&self, ix: usize, line: ChangeLine, st: &Status, current: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let row = match line {
            ChangeLine::Group(g, n) => self.group_header(g.id(), g.label(), n, g.staged(), None, cx).into_any_element(),
            ChangeLine::File(g, i) => match st.changes.get(i) {
                Some(change) => self.row(ix, change, g.staged(), None, cx).into_any_element(),
                None => div().h(px(layout::ROW_H)).into_any_element(),
            },
            ChangeLine::Empty => {
                let p = *palette(cx);
                div()
                    .h(px(layout::ROW_H))
                    .flex()
                    .items_center()
                    .px(px(theme::SPACE[6]))
                    .text_size(px(text::SM.0))
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::t("No changes."))
                    .into_any_element()
            }
        };
        let Some(key) = ChangeKey::of(line, st) else { return row };
        let ring = palette(cx).focus_ring;
        // A press puts the cursor on the row; the click itself still opens the diff or folds the group.
        div()
            .relative()
            .child(row)
            .child(div().absolute().inset_0().border_1().border_color(if current { c(ring) } else { ca(ring, 0.) }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.change_cursor = Some((key.clone(), ix));
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The rows of the single-repository change list, as drawn.
    pub(crate) fn current_lines(&self, st: &Status) -> Vec<ChangeLine> {
        change_lines(st, |id| self.sections.is_open(id, true))
    }

    /// Index of the cursor row: the same group or file as before, or else the nearest row at or above its old place.
    pub(crate) fn cursor_in(&self, lines: &[ChangeLine], st: &Status) -> Option<usize> {
        let (key, last) = self.change_cursor.as_ref()?;
        lines
            .iter()
            .position(|l| ChangeKey::of(*l, st).as_ref() == Some(key))
            .or_else(|| (0..lines.len().min(last + 1)).rev().find(|ix| ChangeKey::of(lines[*ix], st).is_some()))
    }

    pub(crate) fn change_focus(&self) -> &FocusHandle {
        &self.change_focus
    }

    /// The status the change list shows: none while the aggregated list of several repositories or a provider's
    /// section takes its place.
    pub(crate) fn listed_status(&self) -> Option<Arc<Status>> {
        self.status.clone().filter(|_| !self.git_hidden() && !self.multi())
    }

    /// Put the cursor on `ix` and bring that row into view.
    fn set_cursor(&mut self, ix: usize, lines: &[ChangeLine], st: &Status, cx: &mut Context<Self>) {
        if let Some(key) = lines.get(ix).and_then(|l| ChangeKey::of(*l, st)) {
            self.change_cursor = Some((key, ix));
            self.change_scroll.scroll_to_item(ix, ScrollStrategy::Center);
            cx.notify();
        }
    }

    /// Focus the change list with the cursor on `key` (false if that row is not in the list now: nothing changes then),
    /// or, without a key, where the cursor was (the first row the first time).
    pub(crate) fn focus_changes(&mut self, key: Option<&ChangeKey>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(st) = self.listed_status() else { return false };
        let lines = self.current_lines(&st);
        let ix = match key {
            Some(key) => lines.iter().position(|l| ChangeKey::of(*l, &st).as_ref() == Some(key)),
            None => self.cursor_in(&lines, &st).or_else(|| (!lines.is_empty()).then_some(0)),
        };
        let Some(ix) = ix else { return false };
        window.focus(&self.change_focus);
        self.set_cursor(ix, &lines, &st, cx);
        true
    }

    /// One key on the focused list; false if it means nothing there (it then goes on to the panel).
    pub(crate) fn list_key(&mut self, key: ListKey, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(st) = self.listed_status() else { return false };
        let lines = self.current_lines(&st);
        let rows: Vec<usize> = (0..lines.len()).filter(|ix| ChangeKey::of(lines[*ix], &st).is_some()).collect();
        if rows.is_empty() {
            return false;
        }
        let at = self.cursor_in(&lines, &st);
        if let ListKey::Step(delta) = key {
            let pos = at.and_then(|ix| rows.iter().position(|r| *r == ix));
            let last = rows.len() - 1;
            let next = match (pos, delta) {
                (_, isize::MIN) => 0,
                (_, isize::MAX) => last,
                (Some(p), d) => (p as isize + d).clamp(0, last as isize) as usize,
                (None, _) => 0,
            };
            self.set_cursor(rows[next], &lines, &st, cx);
            return true;
        }
        let Some(ix) = at else { return false };
        match (key, lines[ix]) {
            (ListKey::Open | ListKey::Toggle, ChangeLine::Group(g, _)) => self.a11y_toggle_group(g.id(), cx),
            (ListKey::Out, ChangeLine::Group(g, _)) if self.sections.is_open(g.id(), true) => self.a11y_toggle_group(g.id(), cx),
            (ListKey::In, ChangeLine::Group(g, _)) if !self.sections.is_open(g.id(), true) => self.a11y_toggle_group(g.id(), cx),
            (ListKey::In, ChangeLine::Group(_, n)) if n > 0 => self.set_cursor(ix + 1, &lines, &st, cx),
            (ListKey::Out, ChangeLine::File(g, _)) => {
                if let Some(h) = lines.iter().position(|l| matches!(l, ChangeLine::Group(hg, _) if *hg == g)) {
                    self.set_cursor(h, &lines, &st, cx);
                }
            }
            (ListKey::Open, ChangeLine::File(g, i)) => cx.emit(ScmEvent::OpenDiff { path: st.changes[i].path.clone(), staged: g.staged() }),
            (ListKey::Toggle, ChangeLine::File(g, i)) if !self.busy() => {
                let paths = vec![st.changes[i].path.clone()];
                if g.staged() { self.unstage(paths, cx) } else { self.stage(paths, cx) }
            }
            (ListKey::Discard, ChangeLine::File(g, i)) if !self.busy() && discardable(g, &st.changes[i]) => {
                self.confirm_discard_in(None, vec![st.changes[i].clone()], window, cx);
            }
            _ => {}
        }
        true
    }

    /// The menu of the file row `key` names (else of the cursor row), under the row, as a right click on it opens it;
    /// false if that row is not listed now or has no menu (a group header). The menu returns the focus to whatever had
    /// it, so a caller that wants the keyboard back on the list focuses the list first.
    pub(crate) fn open_row_menu(&mut self, key: Option<&ChangeKey>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(st) = self.listed_status() else { return false };
        let lines = self.current_lines(&st);
        let ix = match key {
            Some(key) => lines.iter().position(|l| ChangeKey::of(*l, &st).as_ref() == Some(key)),
            None => self.cursor_in(&lines, &st),
        };
        let Some((ix, ChangeLine::File(g, i))) = ix.and_then(|ix| Some((ix, *lines.get(ix)?))) else { return false };
        let Some(change) = st.changes.get(i) else { return false };
        let at = self.line_anchor(ix).unwrap_or_else(|| {
            let size = window.viewport_size();
            gpui::point(size.width / 2., size.height / 2.)
        });
        cx.emit(ScmEvent::FileMenu(change.path.clone(), g.staged(), at));
        true
    }

    /// The bottom left of line `ix` as last painted, if it is on screen.
    fn line_anchor(&self, ix: usize) -> Option<gpui::Point<gpui::Pixels>> {
        let state = self.change_scroll.0.borrow();
        let bounds = state.base_handle.bounds();
        if bounds.size.height <= px(0.) {
            return None;
        }
        let bottom = bounds.top() + state.base_handle.offset().y + px(layout::ROW_H * (ix + 1) as f32);
        (bottom > bounds.top() && bottom - px(layout::ROW_H) < bounds.bottom()).then(|| gpui::point(bounds.left(), bottom.min(bounds.bottom())))
    }

    /// First and last line of the change list on screen in the last layout.
    pub(crate) fn visible_change_lines(&self, total: usize) -> Option<(usize, usize)> {
        let state = self.change_scroll.0.borrow();
        let height = f32::from(state.last_item_size?.item.height);
        let first = (-f32::from(state.base_handle.offset().y) / layout::ROW_H).max(0.) as usize;
        let last = (first + (height / layout::ROW_H).ceil() as usize).saturating_sub(1).min(total.saturating_sub(1));
        Some((first.min(last), last))
    }

    /// Accessibility scroll action: the list moves `pages` screens down (up when negative); the cursor stays.
    pub(crate) fn a11y_scroll_changes(&mut self, pages: isize, cx: &mut Context<Self>) {
        let Some(st) = self.listed_status() else { return };
        let total = self.current_lines(&st).len();
        let Some((first, last)) = self.visible_change_lines(total) else { return };
        let page = (last - first).max(1) as isize;
        let max = total.saturating_sub(1) as isize;
        self.change_scroll.scroll_to_item_strict((first as isize + pages * page).clamp(0, max) as usize, ScrollStrategy::Top);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn change_scroll_top(&self) -> f32 {
        -f32::from(self.change_scroll.0.borrow().base_handle.offset().y)
    }

    #[cfg(test)]
    pub(crate) fn scroll_changes_to(&self, ix: usize) {
        self.change_scroll.scroll_to_item_strict(ix, gpui::ScrollStrategy::Top);
    }

    #[cfg(test)]
    pub(crate) fn change_cursor_key(&self) -> Option<ChangeKey> {
        let st = self.listed_status()?;
        let lines = self.current_lines(&st);
        self.cursor_in(&lines, &st).and_then(|ix| ChangeKey::of(lines[ix], &st))
    }
}

#[cfg(test)]
#[path = "scm_changes_tests.rs"]
mod tests;
