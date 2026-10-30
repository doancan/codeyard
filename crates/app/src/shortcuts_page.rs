//! "Keyboard Shortcuts Reference": every command and its key, searchable; clicking a row
//! runs the command. (To assign keys use "Preferences: Open Keyboard Shortcuts".)

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Window, div, prelude::*, px};

use crate::input::TextField;
use crate::ui::{self, c, palette, text};

pub enum ShortcutsEvent {
    /// The command's label: the workbench runs it.
    Run(String),
}

pub struct ShortcutsPage {
    rows: Vec<(String, Option<String>)>,
    query: Entity<TextField>,
    /// Where each shown row (by its place in the list) was painted in the last frame (accessibility).
    bounds: crate::painted::RowBounds<usize>,
    focus_handle: FocusHandle,
    _sub: Subscription,
}

impl EventEmitter<ShortcutsEvent> for ShortcutsPage {}

impl Focusable for ShortcutsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Whether the command name or key matches the search (case-insensitive).
pub fn matches(label: &str, keys: Option<&str>, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || label.to_lowercase().contains(&q) || keys.is_some_and(|k| k.to_lowercase().contains(&q))
}

impl ShortcutsPage {
    /// `rows`: (command name, key); those with a key first, sorted by name within each group.
    pub fn new(mut rows: Vec<(String, Option<String>)>, cx: &mut Context<Self>) -> Self {
        rows.sort_by(|a, b| b.1.is_some().cmp(&a.1.is_some()).then_with(|| a.0.cmp(&b.0)));
        let query = cx.new(|cx| TextField::new(crate::i18n::tr("Search commands or keys (e.g. “comment”, “⌘D”)"), cx));
        let sub = cx.observe(&query, |_, _, cx| cx.notify());
        ShortcutsPage { rows, query, bounds: Default::default(), focus_handle: cx.focus_handle(), _sub: sub }
    }

    pub fn focus_search(&self, window: &mut Window, cx: &App) {
        window.focus(&self.query.focus_handle(cx));
    }

    /// For assistive technology: the search text, the first `max` visible rows (command, key), how many are visible,
    /// and whether the search field has focus.
    pub fn access_rows(&self, max: usize, window: &Window, cx: &App) -> (String, Vec<(String, Option<String>)>, usize, bool) {
        let q = self.query.read(cx).text().to_string();
        let visible: Vec<_> = self.rows.iter().filter(|(l, k)| matches(l, k.as_deref(), &q)).collect();
        let rows = visible.iter().take(max).map(|r| (*r).clone()).collect();
        (q, rows, visible.len(), self.query.focus_handle(cx).is_focused(window))
    }

    /// The painted, visible bounds of the first `count` shown rows (physical pixels; `None` while scrolled out of view).
    pub fn access_row_bounds(&self, count: usize, window: &Window) -> Vec<Option<accesskit::Rect>> {
        (0..count).map(|i| crate::painted::physical(&self.bounds, &i, window)).collect()
    }

    /// Filters the list as typing `q` into the search field does.
    pub fn set_query(&mut self, q: &str, cx: &mut Context<Self>) {
        self.query.update(cx, |f, cx| f.set_text(q, cx));
    }

    #[cfg(test)]
    pub fn visible_for_test(&self, cx: &App) -> Vec<String> {
        let q = self.query.read(cx).text().to_string();
        self.rows.iter().filter(|(l, k)| matches(l, k.as_deref(), &q)).map(|(l, _)| l.clone()).collect()
    }

    #[cfg(test)]
    pub fn rows_for_test(&self) -> Vec<(String, Option<String>)> {
        self.rows.clone()
    }
}

impl Render for ShortcutsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let q = self.query.read(cx).text().to_string();
        let rows: Vec<_> = self.rows.iter().filter(|(l, k)| matches(l, k.as_deref(), &q)).cloned().collect();
        let n = rows.len();
        // Rows record where they are painted in this frame.
        self.bounds.borrow_mut().clear();
        let bounds = self.bounds.clone();
        div()
            .id("shortcuts-page")
            .key_context("ShortcutsPage")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[5]))
                    .px(px(theme::SPACE[9]))
                    .py(px(theme::SPACE[5]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .child(ui::icon("keyboard", 20., p.accent))
                    .child(div().text_size(px(text::LG.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(crate::i18n::t("Keyboard Shortcuts")))
                    .child(
                        crate::ui::input_box("shortcuts-search-box", &p, p.surface, self.query.focus_handle(cx).is_focused(window))
                            .flex_1()
                            .max_w(px(520.))
                            .child(crate::ui::search_icon(&p))
                            .child(div().flex_1().min_w_0().child(self.query.clone())),
                    )
                    .child(div().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(crate::i18n::tn("{} command", "{} commands", n, &[]))),
            )
            .child(div().id("shortcut-list").flex_1().min_h_0().overflow_y_scroll().px(px(theme::SPACE[9])).py(px(theme::SPACE[4])).children(
                rows.into_iter().enumerate().map(|(i, (label, keys))| {
                    let run = label.clone();
                    div()
                        .id(("shortcut", i))
                        .relative()
                        .child(crate::painted::probe(&bounds, i))
                        .h(px(30.))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[5]))
                        .px(px(theme::SPACE[4]))
                        .rounded(px(theme::radius::SM))
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(label))
                        .when_some(keys, |d, k| d.child(crate::workbench::kbd(k, &p)))
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(ShortcutsEvent::Run(run.clone()))))
                }),
            ))
    }
}
