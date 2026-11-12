//! Workbench: the symbols of the front file, for the Explorer's OUTLINE section and for the breadcrumbs. They come from
//! the language server's `textDocument/documentSymbol` when one is running for the file, and from line patterns
//! (`outline.rs`) until it answers and wherever there is no server. The section filters as you type, sorts by position,
//! name or kind, and follows the cursor; the breadcrumbs show the chain of symbols around the cursor, and each one opens a
//! picker of its siblings.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{UniformListScrollHandle, div};

use super::*;
use crate::a11y_controls::{Control, press_on};
use crate::input::{FieldEvent, TextField};
use crate::outline::model::{self, Row, Sort};
use crate::outline::{self, Kind, Symbol};

/// Longest a server is waited for; the line patterns are shown meanwhile.
const SERVER_WAIT: Duration = Duration::from_secs(5);

/// Where the symbols on show come from.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Source {
    #[default]
    Patterns,
    Server,
}

/// How the section is drawn (kept per workspace).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct View {
    sort: Sort,
    follow: bool,
}

impl Default for View {
    fn default() -> Self {
        Self { sort: Sort::Position, follow: true }
    }
}

impl View {
    fn encode(self) -> String {
        format!("{} {}", self.sort.key(), if self.follow { "follow" } else { "stay" })
    }

    fn decode(saved: &str) -> Self {
        let mut words = saved.split_whitespace();
        Self { sort: Sort::from_key(words.next().unwrap_or_default()), follow: words.next() != Some("stay") }
    }
}

/// Explorer OUTLINE section: symbols of the active file (refreshed at most every 400 ms while typing).
#[derive(Default)]
pub(in crate::workbench) struct OutlineState {
    key: Option<(String, u64)>,
    pub(in crate::workbench) symbols: Vec<Symbol>,
    supported: bool,
    too_large: bool,
    computed_at: Option<std::time::Instant>,
    refresh_pending: bool,
    pub(in crate::workbench) source: Source,
    /// The request to the language server for `key`, dropped (cancelled) when the file or its text moves on.
    server: Option<gpui::Task<()>>,
    /// The server was asked (or has no symbols to offer) for `key`: a server that starts later is still asked once.
    asked: bool,
    view: View,
    /// The workspace `view` was read for (`Some(None)`: none).
    view_for: Option<Option<i64>>,
    filter: Option<Entity<TextField>>,
    scroll: UniformListScrollHandle,
    /// The symbol the list last scrolled to for the cursor.
    followed: Cell<Option<usize>>,
    /// Counts the changes of `symbols`; what is derived from them is kept for the generation it was made for.
    generation: u64,
    memo: RefCell<Memo>,
    /// The server's last answer is on show while the file's new text waits for a new one.
    pub(in crate::workbench) stale: bool,
    /// What the line patterns found for the new text, used if the server does not answer.
    fallback: Option<(Vec<Symbol>, bool)>,
}

/// What is worked out from the symbols (and the filter and sort), so a frame does not do it again.
#[derive(Default)]
struct Memo {
    parents: Option<(u64, Rc<Vec<Option<usize>>>)>,
    rows: Option<(u64, String, Sort, Rc<Vec<Row>>)>,
    path: Option<(u64, usize, Rc<Vec<usize>>)>,
}

impl OutlineState {
    fn set_symbols(&mut self, symbols: Vec<Symbol>) {
        self.symbols = symbols;
        self.generation += 1;
    }

    fn parents(&self) -> Rc<Vec<Option<usize>>> {
        let mut memo = self.memo.borrow_mut();
        match &memo.parents {
            Some((g, p)) if *g == self.generation => p.clone(),
            _ => {
                let p = Rc::new(model::parents(&self.symbols));
                memo.parents = Some((self.generation, p.clone()));
                p
            }
        }
    }

    /// The rows for `filter` and `sort`.
    fn rows(&self, filter: &str, sort: Sort) -> Rc<Vec<Row>> {
        if let Some((g, f, s, rows)) = &self.memo.borrow().rows
            && *g == self.generation
            && f == filter
            && *s == sort
        {
            return rows.clone();
        }
        let rows = Rc::new(model::rows(&self.symbols, &self.parents(), filter, sort));
        self.memo.borrow_mut().rows = Some((self.generation, filter.to_string(), sort, rows.clone()));
        rows
    }

    /// The symbols around `line`, outermost first.
    fn path_at(&self, line: usize) -> Rc<Vec<usize>> {
        if let Some((g, l, path)) = &self.memo.borrow().path
            && *g == self.generation
            && *l == line
        {
            return path.clone();
        }
        let path = Rc::new(model::path_at(&self.symbols, line));
        self.memo.borrow_mut().path = Some((self.generation, line, path.clone()));
        path
    }
}

/// The color of a symbol's icon.
fn kind_color(kind: Kind, p: &Palette) -> u32 {
    match kind {
        Kind::Module => p.syntax_keyword,
        Kind::Class | Kind::Type | Kind::Interface | Kind::Enum => p.syntax_type,
        Kind::Function | Kind::Method => p.syntax_function,
        Kind::Property | Kind::Field | Kind::Variable => p.syntax_tag,
        Kind::Constant => p.syntax_number,
        Kind::Heading | Kind::Section => p.info,
        Kind::Other => p.text_muted,
    }
}

/// A symbol's icon: its mark on a tinted square.
fn kind_icon(kind: Kind, p: &Palette) -> gpui::Div {
    let color = kind_color(kind, p);
    div()
        .flex_none()
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::radius::SM))
        .bg(ui::ca(color, 0.16))
        .text_color(c(color))
        .text_size(px(text::XS.0))
        .child(kind.glyph())
}

impl Workbench {
    fn outline_view_key(&self) -> Option<String> {
        self.persist.workspace_id.map(|ws| format!("outline.view:{ws}"))
    }

    /// The saved sort order and follow switch of the workspace come back (once per workspace).
    fn load_outline_view(&mut self) {
        if self.outline.view_for == Some(self.persist.workspace_id) {
            return;
        }
        self.outline.view_for = Some(self.persist.workspace_id);
        let saved = self.outline_view_key().and_then(|key| self.persist.state.as_ref()?.get(&key));
        self.outline.view = saved.map(|s| View::decode(&s)).unwrap_or_default();
    }

    fn change_outline_view(&mut self, change: impl FnOnce(&mut View), cx: &mut Context<Self>) {
        change(&mut self.outline.view);
        self.outline.followed.set(None);
        if let (Some(key), Some(state)) = (self.outline_view_key(), &self.persist.state) {
            let _ = state.set(&key, &self.outline.view.encode());
        }
        cx.notify();
    }

    fn ensure_outline_filter(&mut self, cx: &mut Context<Self>) {
        if self.outline.filter.is_some() {
            return;
        }
        let field = cx.new(|cx| TextField::new(crate::i18n::tr("Filter symbols"), cx));
        self._subs.push(cx.subscribe(&field, |this, _, ev: &FieldEvent, cx| {
            if matches!(ev, FieldEvent::Changed) {
                this.outline.followed.set(None);
                cx.notify();
            }
        }));
        self.outline.filter = Some(field);
    }

    fn outline_filter_text(&self, cx: &App) -> String {
        self.outline.filter.as_ref().map(|f| f.read(cx).text().to_string()).unwrap_or_default()
    }

    /// The symbols are wanted: by the OUTLINE section when it is on screen, by the breadcrumbs when they are on.
    fn outline_wanted(&self, cx: &App) -> bool {
        (self.view_shown(Panel::Explorer) && self.sections.is_open("outline", true)) || cx.global::<Settings>().breadcrumbs
    }

    /// Re-extract the active file's outline (if needed).
    pub(in crate::workbench) fn sync_outline(&mut self, cx: &mut Context<Self>) {
        if !self.outline_wanted(cx) {
            return;
        }
        self.load_outline_view();
        self.ensure_outline_filter(cx);
        let Some(ed) = self
            .tabs
            .active
            .filter(|_| !self.tabs.diff_active && !self.tabs.large_active && !self.tabs.page_active)
            .and_then(|i| self.tabs.editors.get(i))
            .cloned()
        else {
            self.outline.key = None;
            self.outline.set_symbols(Vec::new());
            self.outline.stale = false;
            self.outline.server = None;
            return;
        };
        let (path, version, lines) = {
            let e = ed.read(cx);
            (e.path.clone(), e.version(), e.line_count())
        };
        let key = Some((path.clone(), version));
        if self.outline.key == key {
            if !self.outline.asked && ed.read(cx).lsp_client().is_some() && !self.outline.too_large {
                self.ask_server_for_symbols(ed, key, cx);
            }
            return;
        }
        // Defer re-extraction until 400 ms after the last one, not on every keystroke.
        let wait = std::time::Duration::from_millis(400);
        let same_file = self.outline.key.as_ref().is_some_and(|(p, _)| *p == path);
        if let (true, Some(at)) = (same_file, self.outline.computed_at)
            && at.elapsed() < wait
        {
            if !self.outline.refresh_pending {
                self.outline.refresh_pending = true;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    this.update(cx, |this, cx| {
                        this.outline.refresh_pending = false;
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            return;
        }
        self.outline.key = key.clone();
        self.outline.computed_at = Some(std::time::Instant::now());
        self.outline.server = None;
        self.outline.asked = false;
        self.outline.followed.set(None);
        self.outline.too_large = lines > 30_000;
        if self.outline.too_large {
            self.outline.set_symbols(Vec::new());
            self.outline.source = Source::Patterns;
            self.outline.stale = false;
            self.outline.fallback = None;
            self.outline.supported = true;
            return;
        }
        let text = ed.read(cx).text_lines();
        let (found, supported) = match outline::extract(&path, &text) {
            Some(s) => (s, true),
            None => (Vec::new(), false),
        };
        // The server's last answer for this file stays on show (marked stale) until its answer for the new text arrives:
        // the symbols do not flicker to the line patterns and back after each edit.
        if same_file && self.outline.source == Source::Server && ed.read(cx).lsp_client().is_some() {
            self.outline.fallback = Some((found, supported));
            self.outline.stale = true;
        } else {
            self.outline.set_symbols(found);
            self.outline.supported = supported;
            self.outline.source = Source::Patterns;
            self.outline.stale = false;
            self.outline.fallback = None;
        }
        self.ask_server_for_symbols(ed, key, cx);
    }

    /// The server will not answer for the text on screen: the line patterns' symbols replace the stale ones.
    fn use_pattern_symbols(&mut self) {
        self.outline.stale = false;
        if let Some((found, supported)) = self.outline.fallback.take() {
            self.outline.set_symbols(found);
            self.outline.supported = supported;
            self.outline.source = Source::Patterns;
        }
    }

    /// Asks the language server of the file (if one runs and offers symbols) in the background; its answer replaces the
    /// line patterns' while the file is still the same text, and a failure or an empty answer leaves them.
    fn ask_server_for_symbols(&mut self, ed: Entity<EditorView>, key: Option<(String, u64)>, cx: &mut Context<Self>) {
        let Some((path, _)) = key.clone() else { return };
        let Some((client, ..)) = ed.update(cx, |e, _| e.lsp_context()) else {
            self.use_pattern_symbols();
            return;
        };
        self.outline.asked = true;
        if !offers_symbols(&client) {
            self.use_pattern_symbols();
            return;
        }
        let params = serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&path) } });
        let request = cx.background_executor().spawn(async move { client.request("textDocument/documentSymbol", params, SERVER_WAIT) });
        self.outline.server = Some(cx.spawn(async move |this, cx| {
            let answer = request.await;
            this.update(cx, |this, cx| {
                if this.outline.key != key {
                    return;
                }
                this.outline.server = None;
                let from_server = answer.map(|value| model::from_lsp(&crate::lsp::parse_symbols(&value))).unwrap_or_default();
                if from_server.is_empty() {
                    this.use_pattern_symbols();
                    cx.notify();
                    return;
                }
                this.outline.set_symbols(from_server);
                this.outline.supported = true;
                this.outline.source = Source::Server;
                this.outline.stale = false;
                this.outline.fallback = None;
                this.outline.followed.set(None);
                cx.notify();
            })
            .ok();
        }));
    }

    /// The sort button's words.
    fn outline_sort_label(sort: Sort) -> &'static str {
        match sort {
            Sort::Position => crate::i18n::tr("Sort: Position"),
            Sort::Name => crate::i18n::tr("Sort: Name"),
            Sort::Kind => crate::i18n::tr("Sort: Kind"),
        }
    }

    /// OUTLINE body: a filter box with the sort and follow buttons, then the symbols, the one at the cursor marked.
    pub(in crate::workbench) fn outline_rows(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let note = |t: &str| {
            div()
                .relative()
                .child(crate::a11y_controls::control(Control::text(crate::i18n::t("Explorer"), format!("note:{t}"), accesskit::Role::Label, t)))
                .px(px(theme::SPACE[6]))
                .py(px(theme::SPACE[2]))
                .text_color(c(p.text_muted))
                .text_size(px(text::XS.0))
                .child(t.to_string())
                .into_any_element()
        };
        let Some(ed) =
            self.tabs.active.and_then(|i| self.tabs.editors.get(i)).filter(|_| !self.tabs.diff_active && !self.tabs.large_active && !self.tabs.page_active)
        else {
            return note(crate::i18n::t("Open a file to see its structure."));
        };
        if self.outline.too_large {
            return note(crate::i18n::t("The file is too large for an outline."));
        }
        if !self.outline.supported {
            return note(crate::i18n::t("No outline for this file type."));
        }
        if self.outline.symbols.is_empty() {
            return note(crate::i18n::t("No symbols found."));
        }
        let p = *p;
        let view = self.outline.view;
        let filter = self.outline_filter_text(cx);
        let rows = self.outline.rows(&filter, view.sort);
        let cursor_line = ed.read(cx).cursor_position().0.saturating_sub(1);
        let here = self.outline.path_at(cursor_line).last().copied().or_else(|| outline::current(&self.outline.symbols, cursor_line));
        let path = ed.read(cx).path.clone();
        if view.follow
            && self.outline.followed.get() != here
            && let Some(row) = here.and_then(|h| rows.iter().position(|r| r.ix == h))
        {
            self.outline.followed.set(here);
            self.outline.scroll.scroll_to_item(row, gpui::ScrollStrategy::Center);
        }
        let toolbar = self.outline_toolbar(&filter, view, &p, cx);
        let list: AnyElement = if rows.is_empty() {
            note(crate::i18n::t("No symbols match the filter."))
        } else {
            // Virtualized list: in a file with tens of thousands of symbols only the visible rows are drawn.
            gpui::uniform_list(
                "outline-rows",
                rows.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    range.filter_map(|at| this.outline_row(at, rows.get(at)?, here, view.sort, &path, &p, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(self.outline.scroll.clone())
            .size_full()
            .into_any_element()
        };
        // The server's answer for the text before the last edit is dimmed until the new one comes.
        let stale = self.outline.stale;
        div().size_full().flex().flex_col().child(toolbar).child(div().flex_1().min_h_0().when(stale, |d| d.opacity(0.6)).child(list)).into_any_element()
    }

    fn outline_toolbar(&self, filter: &str, view: View, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let surface = crate::i18n::t("Outline");
        let field = self.outline.filter.clone().map(|field| {
            let handle = field.read(cx).focus_handle(cx);
            let access = crate::a11y_controls::control(
                Control::new(surface, "outline-filter", accesskit::Role::TextInput, crate::i18n::t("Filter symbols"))
                    .value(filter.to_string())
                    .focus(handle.clone())
                    .on_entity(cx.entity().downgrade(), move |_: &mut Self, w, _| w.focus(&handle)),
            );
            div()
                .relative()
                .flex_1()
                .min_w(px(60.))
                .px(px(theme::SPACE[2]))
                .rounded(px(theme::radius::SM))
                .border_1()
                .border_color(c(p.border_input))
                .bg(c(p.bg))
                .child(access)
                .child(field)
        });
        let sort_label = Self::outline_sort_label(view.sort);
        let sort = press_on(
            ui::icon_button("outline-sort", "list-filter", sort_label, p),
            Control::new(surface, "outline-sort", accesskit::Role::Button, sort_label).description(crate::i18n::t("Changes the order of the symbols")),
            cx,
            |this: &mut Self, _, cx| this.change_outline_view(|v| v.sort = v.sort.next(), cx),
        );
        let follow_label = crate::i18n::t("Follow Cursor");
        let follow = press_on(
            ui::toggle_button("outline-follow", "pin", follow_label, view.follow, p),
            Control::new(surface, "outline-follow", accesskit::Role::Button, follow_label).toggled(view.follow),
            cx,
            |this: &mut Self, _, cx| this.change_outline_view(|v| v.follow = !v.follow, cx),
        );
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[1]))
            .px(px(theme::SPACE[3]))
            .pb(px(theme::SPACE[1]))
            .children(field)
            .child(sort)
            .child(follow)
            .into_any_element()
    }

    /// One row of the list: icon, name, and (in a flat order) where it lives, then its line.
    fn outline_row(&self, at: usize, row: &Row, here: Option<usize>, sort: Sort, path: &str, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.outline.symbols.get(row.ix)?;
        let (line, col, ix) = (s.line as u32, s.col as u32, row.ix);
        let open_path = path.to_string();
        let parent =
            (sort != Sort::Position).then(|| self.outline.parents().get(ix).copied().flatten().map(|j| self.outline.symbols[j].name.clone())).flatten();
        let selected = here == Some(ix);
        let (kind, name) = (s.kind, s.name.clone());
        let open = {
            let path = open_path.clone();
            move |this: &mut Self, w: &mut Window, cx: &mut Context<Self>| this.open_location(path.clone(), line, col, w, cx)
        };
        let click = open.clone();
        Some(
            div()
                .id(("outline", at))
                .relative()
                .child(crate::a11y_controls::control(
                    Control::new(crate::i18n::t("Outline"), format!("{ix}:{name}"), accesskit::Role::TreeItem, name.clone())
                        .description(crate::i18n::tf("{0}, line {1}", &[&kind.label(), &(s.line + 1)]))
                        .selected(selected)
                        .on_entity(cx.entity().downgrade(), move |this, w, cx| open(this, w, cx)),
                ))
                .w_full()
                .h(px(22.))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .pl(px(theme::SPACE[6] + row.depth as f32 * 12.))
                .pr(px(theme::SPACE[3]))
                .cursor_pointer()
                .when(selected, |d| d.bg(c(p.surface_active)))
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(kind_icon(kind, p))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(text::SM.0))
                        .when(row.context, |d| d.text_color(c(p.text_muted)))
                        .child(name),
                )
                .children(parent.map(|n| div().flex_none().max_w(px(80.)).truncate().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(n)))
                .child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.line_number)).child((s.line + 1).to_string()))
                .on_click(cx.listener(move |this, _, window, cx| click(this, window, cx)))
                .into_any_element(),
        )
    }

    /// The breadcrumbs' symbol segments: the chain of symbols around the cursor, each a button that opens its siblings.
    pub(in crate::workbench) fn symbol_crumbs(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(ed) = self.tab_editor() else { return Vec::new() };
        if self.outline.too_large || !self.outline.supported {
            return Vec::new();
        }
        let line = ed.read(cx).cursor_position().0.saturating_sub(1);
        let chain = self.outline.path_at(line);
        let mut items = Vec::new();
        for (level, &ix) in chain.iter().enumerate() {
            let Some(s) = self.outline.symbols.get(ix) else { continue };
            let (kind, name) = (s.kind, s.name.clone());
            items.push(icon("chevron-right", 12., p.text_muted).into_any_element());
            let el = div()
                .id(SharedString::from(format!("symbol-crumb-{level}")))
                .flex()
                .items_center()
                .flex_none()
                .gap(px(theme::SPACE[1]))
                .px(px(theme::SPACE[1]))
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .text_color(c(p.text))
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(kind_icon(kind, p))
                .child(name.clone());
            let control = Control::new(crate::i18n::t("Breadcrumbs"), format!("symbol:{level}:{name}"), accesskit::Role::Button, name.clone())
                .description(crate::i18n::tf("{0}. Choose another symbol at this level.", &[&kind.label()]));
            items.push(press_on(el, control, cx, move |this: &mut Self, w, cx| this.pick_sibling_symbol(Some(ix), w, cx)).into_any_element());
        }
        items
    }

    /// The picker of the symbols that share a parent with symbol `of` (the top level when `None`); choosing one goes there.
    pub(in crate::workbench) fn pick_sibling_symbol(&mut self, of: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let symbols = &self.outline.symbols;
        let ixs = match of {
            Some(ix) => model::siblings(symbols, ix),
            None => self.outline.parents().iter().enumerate().filter(|(_, parent)| parent.is_none()).map(|(i, _)| i).collect(),
        };
        if ixs.is_empty() {
            self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("No symbols in this file").into()));
            cx.notify();
            return;
        }
        let items = ixs
            .iter()
            .map(|&i| PickItem {
                label: symbols[i].name.clone().into(),
                detail: Some(crate::i18n::tf("{0} · line {1}", &[&symbols[i].kind.label(), &(symbols[i].line + 1)]).into()),
                keys: None,
            })
            .collect();
        let targets = ixs.iter().map(|&i| (symbols[i].line as u32, symbols[i].col as u32)).collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Go to symbol at this level"), "", None, cx));
        self.show_overlay(picker, OverlayKind::Symbols(targets), window, cx);
    }

    /// *Go: Pick Symbol at the Cursor's Level*: the innermost symbol around the cursor offers its siblings.
    pub(in crate::workbench) fn breadcrumb_symbols(&mut self, _: &BreadcrumbSymbols, window: &mut Window, cx: &mut Context<Self>) {
        let line = self.tab_editor().map(|e| e.read(cx).cursor_position().0.saturating_sub(1)).unwrap_or_default();
        let innermost = self.outline.path_at(line).last().copied();
        self.pick_sibling_symbol(innermost, window, cx);
    }

    #[cfg(test)]
    pub fn outline_state_for_test(&self) -> (Source, Vec<(String, usize)>) {
        (self.outline.source, self.outline.symbols.iter().map(|s| (s.name.clone(), s.depth)).collect())
    }

    #[cfg(test)]
    pub fn outline_rows_for_test(&self, cx: &App) -> Vec<(String, usize)> {
        let rows = self.outline.rows(&self.outline_filter_text(cx), self.outline.view.sort);
        rows.iter().map(|r| (self.outline.symbols[r.ix].name.clone(), r.depth)).collect()
    }
}

gpui::actions!(outline, [BreadcrumbSymbols]);

/// Whether the server answers `textDocument/documentSymbol`.
fn offers_symbols(client: &Arc<crate::lsp::Client>) -> bool {
    let provider = &client.capabilities["documentSymbolProvider"];
    provider.as_bool().unwrap_or(provider.is_object())
}

#[cfg(test)]
#[path = "workbench_outline_tests.rs"]
mod tests;
