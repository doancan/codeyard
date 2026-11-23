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
const REFRESH_PAUSE: std::time::Duration = std::time::Duration::from_millis(150);
/// Recount a long history in short UI turns, with a chance to cancel between them.
const COUNT_ROWS_PER_TURN: usize = 64;

pub(super) struct FindState {
    field: Entity<TextField>,
    /// `None` while the query is empty (or, defensively, if the escaped query does not compile).
    regex: Option<RegexSearch>,
    query: String,
    current: Option<Match>,
    /// Matches in the whole buffer when the query last changed (capped at `MAX_COUNT + 1`).
    total: usize,
    /// A scan of a long history waits for the typing to pause; typing again drops it.
    pending: Option<gpui::Task<()>>,
    refresh_pending: Option<gpui::Task<()>>,
    refresh_dirty: bool,
    generation: u64,
    _sub: Subscription,
}

struct CountScan {
    regex: RegexSearch,
    next: GridPoint,
    bottom: Line,
    overlap: usize,
    total: usize,
}

impl CountScan {
    fn advance<T>(&mut self, term: &alacritty_terminal::Term<T>) -> bool {
        let last = Line(self.next.line.0 + COUNT_ROWS_PER_TURN as i32 - 1).min(self.bottom);
        // A literal match can straddle a wrapped row at the boundary. Look ahead far
        // enough for that query, counting it only in the batch where it starts.
        let end = (last + self.overlap).min(self.bottom);
        let end = GridPoint::new(end, term.last_column());
        for found in
            RegexIter::new(self.next, end, Direction::Right, term, &mut self.regex).take_while(|m| m.start().line <= last).take(MAX_COUNT + 1 - self.total)
        {
            self.total += 1;
            // Continue after the last consumed match, even if it crossed the batch
            // boundary. Restarting at Column(0) would count overlapping matches twice.
            let end = *found.end();
            self.next = if end.column < term.last_column() { GridPoint::new(end.line, end.column + 1) } else { GridPoint::new(end.line + 1, Column(0)) };
        }
        self.next = self.next.max(GridPoint::new(last + 1, Column(0)));
        self.total > MAX_COUNT || self.next.line > self.bottom
    }
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
                    FieldEvent::Changed => this.find_query_changed_soon(cx),
                    FieldEvent::Submit => this.find_step(Direction::Left, cx),
                    FieldEvent::Escape => this.close_find(window, cx),
                });
                self.find = Some(FindState {
                    field: field.clone(),
                    regex: None,
                    query: String::new(),
                    current: None,
                    total: 0,
                    pending: None,
                    refresh_pending: None,
                    refresh_dirty: false,
                    generation: 0,
                    _sub: sub,
                });
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

    /// Keep a surviving selected result at its new grid position after output; recount
    /// later without selecting another result or moving the user's scroll position.
    pub(super) fn forget_find_match(&mut self) {
        if let Some(find) = &mut self.find {
            if find.current.is_some() {
                find.current = self.term.selection.as_ref().and_then(|s| s.to_range(&self.term)).and_then(|range| {
                    self.term
                        .regex_search_right(find.regex.as_mut()?, range.start, range.end)
                        .filter(|found| *found.start() == range.start && *found.end() == range.end)
                });
            }
            find.generation = find.generation.wrapping_add(1);
            find.refresh_pending = None;
            find.refresh_dirty = find.regex.is_some();
        }
    }

    /// Schedule once per quiet output burst. Parsing only invalidates this work;
    /// it never scans the scrollback for every PTY chunk.
    pub(super) fn refresh_find_soon(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        if !find.refresh_dirty || find.pending.is_some() {
            return;
        }
        find.refresh_dirty = false;
        let Some(regex) = find.regex.clone() else { return };
        let generation = find.generation;
        let mut scan = CountScan {
            regex,
            next: GridPoint::new(self.term.topmost_line(), Column(0)),
            bottom: self.term.bottommost_line(),
            overlap: find.query.chars().count().saturating_mul(2).div_ceil(self.term.columns()).max(1),
            total: 0,
        };
        find.refresh_pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_PAUSE).await;
            loop {
                let done = this
                    .update(cx, |this, cx| {
                        let Some(find) = &mut this.find else { return true };
                        if find.generation != generation {
                            return true;
                        }
                        if scan.advance(&this.term) {
                            find.total = scan.total;
                            find.refresh_pending = None;
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
                cx.background_executor().timer(std::time::Duration::from_millis(1)).await;
            }
        }));
    }

    /// The find field has focus: keys typed there are not for the shell.
    pub(super) fn find_has_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        self.find.as_ref().is_some_and(|f| f.field.focus_handle(cx).is_focused(window))
    }

    /// A query typed: a short buffer is searched at once, a long one (a scan reads every line of it) when the typing pauses.
    fn find_query_changed_soon(&mut self, cx: &mut Context<Self>) {
        const PAUSE: std::time::Duration = std::time::Duration::from_millis(150);
        const LONG: usize = 5_000;
        if let Some(find) = &mut self.find {
            find.generation = find.generation.wrapping_add(1);
            find.refresh_pending = None;
            find.refresh_dirty = false;
        }
        if cfg!(test) || self.term.grid().history_size() < LONG {
            if let Some(find) = &mut self.find {
                find.pending = None;
            }
            return self.find_query_changed(cx);
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PAUSE).await;
            this.update(cx, |this, cx| {
                if let Some(find) = &mut this.find {
                    find.pending = None;
                }
                this.find_query_changed(cx);
            })
            .ok();
        });
        if let Some(find) = &mut self.find {
            find.pending = Some(task);
        }
    }

    /// New query: count the matches and jump to the one nearest the bottom (the latest output).
    fn find_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        let query = find.field.read(cx).text().to_string();
        find.regex = compile(&query);
        find.query = query;
        find.generation = find.generation.wrapping_add(1);
        find.refresh_pending = None;
        find.refresh_dirty = false;
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
                .debug_selector(move || id.into())
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
            div().absolute().top(px(theme::SPACE[2])).left(px(theme::SPACE[2])).right(px(theme::SPACE[2])).flex().justify_end().child(
                div()
                    .id("term-find-bar")
                    .debug_selector(|| "term-find-bar".into())
                    .w(px(320.))
                    .max_w_full()
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
                    .child({
                        let handle = find.field.focus_handle(cx);
                        let press = handle.clone();
                        ui::input_box("term-find-box", p, p.bg, focused)
                            .flex_1()
                            .min_w_0()
                            .when(none, |d| d.border_color(c(p.danger)))
                            .child(div().flex_1().min_w_0().child(find.field.clone()))
                            .relative()
                            .child(crate::a11y_controls::control(
                                crate::a11y_controls::Control::new(
                                    crate::i18n::t("Find in Terminal"),
                                    "field",
                                    accesskit::Role::SearchInput,
                                    crate::i18n::t("Find"),
                                )
                                .value(find.field.read(cx).text().to_string())
                                .focus(handle)
                                .on_entity(cx.entity().downgrade(), move |_: &mut Self, w, _| w.focus(&press)),
                            ))
                    })
                    .child(
                        div()
                            .flex_none()
                            .min_w(px(64.))
                            .text_color(c(if none { p.danger } else { p.text_muted }))
                            .relative()
                            .child(crate::a11y_controls::said(crate::i18n::t("Find in Terminal"), "count", accesskit::Role::Status, label.to_string()))
                            .child(label),
                    )
                    .child(crate::a11y_controls::press_on(
                        button("term-find-prev", "arrow-up", crate::i18n::t("Previous Match")),
                        crate::a11y_controls::Control::button(crate::i18n::t("Find in Terminal"), "prev", crate::i18n::t("Previous Match"), |_, _| {}),
                        cx,
                        |this, _, cx| this.find_step(Direction::Left, cx),
                    ))
                    .child(crate::a11y_controls::press_on(
                        button("term-find-next", "arrow-down", crate::i18n::t("Next Match")),
                        crate::a11y_controls::Control::button(crate::i18n::t("Find in Terminal"), "next", crate::i18n::t("Next Match"), |_, _| {}),
                        cx,
                        |this, _, cx| this.find_step(Direction::Right, cx),
                    ))
                    .child(crate::a11y_controls::press_on(
                        button("term-find-close", "x", crate::i18n::t("Close")),
                        crate::a11y_controls::Control::button(crate::i18n::t("Find in Terminal"), "close", crate::i18n::t("Close"), |_, _| {}),
                        cx,
                        |this, window, cx| this.close_find(window, cx),
                    )),
            ),
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

    /// (find bar has focus, match count).
    #[cfg(test)]
    pub fn find_state_for_test(&self, window: &Window, cx: &gpui::App) -> (bool, usize) {
        (self.find_has_focus(window, cx), self.find.as_ref().map_or(0, |f| f.total))
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
    // A non-empty literal consumes at least one visible cell, so this is bounded
    // by the viewport. An arbitrary match cap leaves the lower rows unhighlighted.
    RegexIter::new(start, end, Direction::Right, term, regex).collect()
}

#[cfg(test)]
#[path = "terminal_find_tests.rs"]
mod live_tests;

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
