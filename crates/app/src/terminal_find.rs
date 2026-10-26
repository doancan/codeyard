//! Find in the terminal: a small bar over the top right corner. The text is searched literally in the screen and the
//! scrollback (smart case: a query with capitals matches case); the current match is selected, so ⌘C copies it, and every
//! visible match is tinted. Submodule of `terminal.rs`: reads the view's private fields.

use alacritty_terminal::grid::{Dimensions as _, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use gpui::{Context, Entity, Focusable as _, SharedString, Subscription, Window, div, prelude::*, px};

use super::{TermFind, TermFindNext, TermFindPrevious, TerminalView};
use crate::input::{FieldEvent, TextField};
use crate::ui::{self, Palette, c, icon, text};

/// Counting stops here: the label then says "999+".
const MAX_COUNT: usize = 999;

pub(super) struct FindState {
    field: Entity<TextField>,
    /// `None` while the query is empty (or, defensively, if the escaped query does not compile).
    regex: Option<RegexSearch>,
    current: Option<Match>,
    /// Matches in the whole buffer when the query last changed (capped at `MAX_COUNT + 1`).
    total: usize,
    _sub: Subscription,
}

impl FindState {
    /// The regex for drawing: matches on screen are tinted while the bar is open.
    pub(super) fn regex(&self) -> Option<RegexSearch> {
        self.regex.clone()
    }

    /// The selected match: drawn stronger than the others.
    pub(super) fn current(&self) -> Option<Match> {
        self.current.clone()
    }
}

/// The query is plain text, not a pattern.
fn compile(query: &str) -> Option<RegexSearch> {
    if query.is_empty() {
        return None;
    }
    RegexSearch::new(&regex::escape(query)).ok()
}

/// "3 results", "No results", "999+ results".
fn count_label(total: usize) -> String {
    match total {
        0 => crate::i18n::t("No results").to_string(),
        n if n > MAX_COUNT => crate::i18n::tf("{}+ results", &[&MAX_COUNT]),
        n => crate::i18n::tn("{} result", "{} results", n, &[]),
    }
}

impl TerminalView {
    pub(super) fn find(&mut self, _: &TermFind, window: &mut Window, cx: &mut Context<Self>) {
        let field = match &self.find {
            Some(f) => f.field.clone(),
            None => {
                let field = cx.new(|cx| TextField::new(crate::i18n::tr("Find"), cx));
                let sub = cx.subscribe_in(&field, window, |this, _, ev: &FieldEvent, window, cx| match ev {
                    FieldEvent::Changed => this.find_query_changed(cx),
                    FieldEvent::Submit => this.find_step(Direction::Left, cx),
                    FieldEvent::Escape => this.close_find(window, cx),
                });
                self.find = Some(FindState { field: field.clone(), regex: None, current: None, total: 0, _sub: sub });
                field
            }
        };
        // Opening the bar with a selection searches for it.
        if let Some(selected) = self.term.selection_to_string().filter(|s| !s.is_empty() && !s.contains('\n')) {
            field.update(cx, |f, cx| f.set_text(&selected, cx));
            self.find_query_changed(cx);
        }
        window.focus(&field.focus_handle(cx));
        cx.notify();
    }

    pub(super) fn find_next(&mut self, _: &TermFindNext, _: &mut Window, cx: &mut Context<Self>) {
        // Newer output is further down: "next" walks toward the bottom, like a terminal's history read top to bottom.
        self.find_step(Direction::Right, cx);
    }

    pub(super) fn find_previous(&mut self, _: &TermFindPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.find_step(Direction::Left, cx);
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            self.term.selection = None;
            window.focus(&self.focus_handle);
            cx.notify();
        }
    }

    /// The next step starts again from the bottom (after new output; the tint of the other matches is recomputed per frame).
    pub(super) fn forget_find_match(&mut self) {
        if let Some(find) = &mut self.find {
            find.current = None;
        }
    }

    /// The find field has focus: keys typed there are not for the shell.
    pub(super) fn find_has_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        self.find.as_ref().is_some_and(|f| f.field.focus_handle(cx).is_focused(window))
    }

    /// New query: count the matches and jump to the one nearest the bottom (the latest output).
    fn find_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        let query = find.field.read(cx).text().to_string();
        find.regex = compile(&query);
        find.current = None;
        find.total = 0;
        if let Some(regex) = &mut find.regex {
            let (start, end) = buffer_bounds(&self.term);
            find.total = RegexIter::new(start, end, Direction::Right, &self.term, regex).take(MAX_COUNT + 1).count();
        }
        self.term.selection = None;
        self.find_step(Direction::Left, cx);
    }

    /// Select the next match in `direction`, starting after the current one (or at the bottom of the buffer), wrapping around.
    fn find_step(&mut self, direction: Direction, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        let Some(regex) = &mut find.regex else {
            cx.notify();
            return;
        };
        let (top, bottom) = buffer_bounds(&self.term);
        let origin = match (&find.current, direction) {
            (Some(m), Direction::Right) => step(&self.term, *m.end(), true),
            (Some(m), Direction::Left) => step(&self.term, *m.start(), false),
            (None, Direction::Right) => top,
            (None, Direction::Left) => bottom,
        };
        let side = if direction == Direction::Right { Side::Left } else { Side::Right };
        find.current = self.term.search_next(regex, origin, direction, side, None);
        match &find.current {
            Some(m) => {
                let mut selection = Selection::new(SelectionType::Simple, *m.start(), Side::Left);
                selection.update(*m.end(), Side::Right);
                self.term.selection = Some(selection);
                self.term.scroll_to_point(*m.start());
            }
            None => self.term.selection = None,
        }
        cx.notify();
    }

    pub(super) fn find_bar(&self, p: &Palette, focused: bool, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let find = self.find.as_ref()?;
        let has_query = find.regex.is_some();
        let label: SharedString = if has_query { count_label(find.total).into() } else { SharedString::default() };
        let none = has_query && find.total == 0;
        let button = |id: &'static str, glyph: &'static str, tip: &'static str| {
            div()
                .id(id)
                .size(px(22.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(icon(glyph, 14., p.text_muted))
                .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.into())).into())
        };
        Some(
            div()
                .absolute()
                .top(px(theme::SPACE[2]))
                .right(px(theme::SPACE[8]))
                .w(px(320.))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .p(px(theme::SPACE[2]))
                .bg(c(p.surface_raised))
                .border_1()
                .border_color(c(p.border))
                .rounded(px(theme::radius::MD))
                .shadow_md()
                .font_family(ui::UI_FONT)
                .text_size(px(text::XS.0))
                .child(
                    ui::input_box("term-find-box", p, p.bg, focused)
                        .flex_1()
                        .min_w_0()
                        .when(none, |d| d.border_color(c(p.danger)))
                        .child(div().flex_1().min_w_0().child(find.field.clone())),
                )
                .child(div().flex_none().min_w(px(64.)).text_color(c(if none { p.danger } else { p.text_muted })).child(label))
                .child(
                    button("term-find-prev", "arrow-up", crate::i18n::t("Previous Match"))
                        .on_click(cx.listener(|this, _, _, cx| this.find_step(Direction::Left, cx))),
                )
                .child(
                    button("term-find-next", "arrow-down", crate::i18n::t("Next Match"))
                        .on_click(cx.listener(|this, _, _, cx| this.find_step(Direction::Right, cx))),
                )
                .child(button("term-find-close", "x", crate::i18n::t("Close")).on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx)))),
        )
    }

    #[cfg(test)]
    pub fn find_for_test(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) -> (usize, Option<String>) {
        self.find(&TermFind, window, cx);
        if let Some(f) = &self.find {
            f.field.update(cx, |f, cx| f.set_text(query, cx));
        }
        self.find_query_changed(cx);
        (self.find.as_ref().map_or(0, |f| f.total), self.term.selection_to_string())
    }

    #[cfg(test)]
    pub fn find_step_for_test(&mut self, down: bool, cx: &mut Context<Self>) -> Option<String> {
        self.find_step(if down { Direction::Right } else { Direction::Left }, cx);
        self.term.selection_to_string()
    }

    /// Scroll the view through the history (keys bound in `main.rs`).
    pub(super) fn scroll_by(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        self.term.scroll_display(scroll);
        cx.notify();
    }
}

/// First and last cell of the screen plus scrollback.
fn buffer_bounds<T>(term: &alacritty_terminal::Term<T>) -> (GridPoint, GridPoint) {
    (GridPoint::new(term.topmost_line(), Column(0)), GridPoint::new(term.bottommost_line(), term.last_column()))
}

/// The cell after (or before) `point`, wrapping around the ends of the buffer.
fn step<T>(term: &alacritty_terminal::Term<T>, point: GridPoint, forward: bool) -> GridPoint {
    let (top, bottom) = buffer_bounds(term);
    let last = term.last_column();
    if forward {
        if point.column < last {
            GridPoint::new(point.line, point.column + 1)
        } else if point.line < bottom.line {
            GridPoint::new(point.line + 1, Column(0))
        } else {
            top
        }
    } else if point.column > Column(0) {
        GridPoint::new(point.line, point.column - 1)
    } else if point.line > top.line {
        GridPoint::new(point.line - 1, last)
    } else {
        bottom
    }
}

/// Matches inside the visible rows (for tinting): `offset` is the display offset, `rows` the screen height.
pub(super) fn visible_matches<T>(term: &alacritty_terminal::Term<T>, regex: &mut RegexSearch, offset: i32, rows: usize) -> Vec<Match> {
    let first = Line(-offset);
    let last = Line(-offset + rows as i32 - 1).min(term.bottommost_line());
    if last < first {
        return Vec::new();
    }
    let (start, end) = (GridPoint::new(first, Column(0)), GridPoint::new(last, term.last_column()));
    RegexIter::new(start, end, Direction::Right, term, regex).take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_is_literal_text() {
        assert!(compile("").is_none());
        // Characters that mean something in a regex are searched as they are.
        for q in ["a.b", "(x)", "[1]", "*", "c++", "$HOME", r"C:\Users"] {
            assert!(compile(q).is_some(), "{q}");
        }
    }

    #[test]
    fn counts_read_naturally() {
        assert_eq!(count_label(0), "No results");
        assert_eq!(count_label(1), "1 result");
        assert_eq!(count_label(7), "7 results");
        assert_eq!(count_label(MAX_COUNT + 1), "999+ results");
    }
}
