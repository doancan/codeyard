//! Search in files (S05): query, options, globs; results stream in,
//! grouped by file. Search runs on the agent (local or remote).

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle, MouseButton, SharedString, StyledText, Subscription, Task,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use host::Host;
use proto::{FileMatches, Response, SearchQuery};

use crate::input::{FieldEvent, TextField};
use crate::ui::{self, c, icon, layout, palette, text};

gpui::actions!(search, [HistoryPrev, HistoryNext, NextResult, PrevResult]);
// --- structural ---
gpui::actions!(search, [StructuralSearch, StructuralReplace]);

#[path = "search_access.rs"]
mod access;
#[path = "search_structural.rs"]
mod structural_mode;
pub(crate) use access::{AccessField, AccessFrame, AccessKey, AccessView, DISMISS_FILE, REPLACE_IN_FILE, SearchOption};
pub(crate) use structural_mode::describe as describe_pattern_error;

/// Number of remembered queries.
const HISTORY_MAX: usize = 30;

/// Delay before searching once typing stops.
const DEBOUNCE: Duration = Duration::from_millis(250);

pub enum SearchEvent {
    /// Go to a match: absolute path, 1-based line, 0-based column, length (characters).
    Open { path: String, line: u32, column: u32, len: u32 },
    /// "Open Search Editor": keep this query in an editor tab of its own.
    OpenInEditor(crate::search_doc::SearchSpec),
    /// Replace: review the changes in a Search Editor before anything is written. `only`: just this file (absolute path).
    ReviewReplace { spec: crate::search_doc::SearchSpec, only: Option<String> },
    // --- structural ---
    /// A structural search wants the unsaved text of the open files of this language; answered with `run_structural`.
    StructuralBuffers { generation: u32, family: &'static str },
    /// Replace the structural matches in these files (workbench asks for confirmation).
    StructuralReplace { files: Vec<String>, family: &'static str, pattern: String, replacement: String, matches: u32 },
}

enum Status {
    Idle,
    Searching,
    Done { files: u32, matches: u32, truncated: bool },
    Failed(SharedString),
}

struct FileResult {
    path: String,
    matches: Vec<proto::LineMatch>,
    collapsed: bool,
}

#[derive(Clone, Copy)]
enum Row {
    File(usize),
    Match(usize, usize),
}

pub struct SearchPanel {
    host: Arc<dyn Host>,
    root: Option<String>,
    /// Extra workspace folders: searched too, results come as `folder › path`.
    extra_roots: Vec<String>,
    query: Entity<TextField>,
    include: Entity<TextField>,
    exclude: Entity<TextField>,
    replace: Entity<TextField>,
    show_globs: bool,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    /// When replacing, keep the match's letter case.
    preserve_case: bool,
    results: Vec<FileResult>,
    rows: Vec<Row>,
    status: Status,
    /// Running search (id on the agent) and the task feeding it.
    running: Option<(Vec<u32>, Task<()>)>,
    debounce: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    /// Previous queries (newest last) and the position being browsed with ↑/↓.
    history: Vec<String>,
    history_ix: Option<usize>,
    /// Last visited match: (file, match). F4 / ⇧F4 continue from here.
    current: Option<(usize, usize)>,
    // --- structural ---
    st: structural_mode::State,
    /// Where the fields, options, status and rows were laid out in the last frame, for the accessibility tree.
    painted: crate::painted::RowBounds<AccessFrame>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SearchEvent> for SearchPanel {}

impl Focusable for SearchPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SearchPanel {
    pub fn new(host: Arc<dyn Host>, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new(crate::i18n::tr("Search"), cx));
        let include = cx.new(|cx| TextField::new(crate::i18n::tr("Files to include (e.g. *.rs, src/**)"), cx));
        let exclude = cx.new(|cx| TextField::new(crate::i18n::tr("Files to exclude"), cx));
        let replace = cx.new(|cx| TextField::new(crate::i18n::tr("Replace"), cx));
        let mut subs = Vec::new();
        for field in [&query, &include, &exclude] {
            subs.push(cx.subscribe(field, |this, _, ev: &FieldEvent, cx| match ev {
                FieldEvent::Changed => this.schedule(cx),
                FieldEvent::Submit => {
                    this.remember_query(cx);
                    this.start(cx)
                }
                FieldEvent::Escape => this.clear_query(cx),
            }));
        }
        // --- structural ---
        let st = structural_mode::State::new(cx);
        subs.push(cx.subscribe(&st.pattern, |this, _, ev: &FieldEvent, cx| match ev {
            FieldEvent::Changed => this.schedule(cx),
            FieldEvent::Submit => this.start(cx),
            FieldEvent::Escape => this.clear_query(cx),
        }));
        subs.push(cx.subscribe(&replace, |this, _, ev: &FieldEvent, cx| {
            if matches!(ev, FieldEvent::Submit) {
                this.request_replace_all(cx);
            }
        }));
        SearchPanel {
            host,
            root: None,
            query,
            include,
            exclude,
            replace,
            show_globs: false,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            preserve_case: false,
            results: Vec::new(),
            rows: Vec::new(),
            status: Status::Idle,
            running: None,
            extra_roots: Vec::new(),
            debounce: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            history: Vec::new(),
            history_ix: None,
            current: None,
            st,
            painted: Default::default(),
            _subs: subs,
        }
    }

    /// Folder or connection changed.
    pub fn set_context(&mut self, host: Arc<dyn Host>, root: Option<String>, cx: &mut Context<Self>) {
        self.stop();
        self.host = host;
        self.root = root;
        self.extra_roots.clear();
        self.results.clear();
        self.rebuild_rows();
        self.status = Status::Idle;
        self.schedule(cx);
    }

    /// Workspace folders changed: results are stale, search again.
    pub fn set_extra_roots(&mut self, roots: Vec<String>, cx: &mut Context<Self>) {
        if self.extra_roots == roots {
            return;
        }
        self.stop();
        self.extra_roots = roots;
        self.results.clear();
        self.rebuild_rows();
        self.status = Status::Idle;
        self.schedule(cx);
    }

    /// Turn a result path into an absolute path: `folder › path` goes to the extra folder, a plain path to the first root.
    fn abs(&self, rel: &str) -> String {
        if let Some((label, inner)) = rel.split_once(crate::workbench::ROOT_SEP)
            && let Some(extra) = self.extra_roots.iter().find(|e| ui::file_name(e) == label)
        {
            return ui::join(extra, inner);
        }
        ui::join(self.root.as_deref().unwrap_or_default(), rel)
    }

    /// ⇧⌘F: focus the query, fill it with the selected text if any.
    pub fn focus_query(&mut self, prefill: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = prefill.filter(|t| !t.is_empty() && !t.contains('\n')) {
            self.query.update(cx, |q, cx| q.set_text(&t, cx));
        }
        window.focus(&self.query.focus_handle(cx));
    }

    /// "Find TODO Comments": TODO/FIXME/HACK/XXX/BUG (uppercase, as a word), regex on.
    pub fn search_todos(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.regex = true;
        self.case_sensitive = true;
        self.whole_word = false;
        self.query.update(cx, |q, cx| q.set_text(r"\b(TODO|FIXME|HACK|XXX|BUG)\b", cx));
        window.focus(&self.query.focus_handle(cx));
        self.start(cx);
    }

    /// (match count, file count); `None` if the search isn't finished.
    #[cfg(test)]
    pub fn summary_for_test(&self) -> Option<(usize, usize)> {
        match self.status {
            Status::Done { files, matches, .. } => Some((matches as usize, files as usize)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn results_for_test(&self) -> Vec<(String, usize)> {
        self.results.iter().map(|f| (f.path.clone(), f.matches.len())).collect()
    }

    #[cfg(test)]
    pub fn state_for_test(&self, cx: &App) -> (String, bool, bool) {
        (self.query.read(cx).text().to_string(), self.regex, self.case_sensitive)
    }

    /// Add the query run with Enter to the history (duplicates move to the end).
    fn remember_query(&mut self, cx: &mut Context<Self>) {
        let q = self.query.read(cx).text().to_string();
        if q.trim().is_empty() {
            return;
        }
        self.history.retain(|h| *h != q);
        self.history.push(q);
        if self.history.len() > HISTORY_MAX {
            self.history.remove(0);
        }
        self.history_ix = None;
    }

    /// ↑ (`older`): previous query; ↓: next, empty at the end.
    fn step_history(&mut self, older: bool, cx: &mut Context<Self>) {
        if self.history.is_empty() || self.st.on {
            return;
        }
        let next = match (self.history_ix, older) {
            (None, true) => Some(self.history.len() - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            (Some(_), false) | (None, false) => None,
        };
        self.history_ix = next;
        let text = next.map_or(String::new(), |i| self.history[i].clone());
        self.query.update(cx, |q, cx| q.set_text(&text, cx));
    }

    /// Move `delta` steps through the flat order of found matches and go there (F4 / ⇧F4).
    pub fn step_result(&mut self, delta: isize, cx: &mut Context<Self>) {
        let flat: Vec<(usize, usize)> = self.results.iter().enumerate().flat_map(|(fi, f)| (0..f.matches.len()).map(move |mi| (fi, mi))).collect();
        if flat.is_empty() {
            return;
        }
        let at = self.current.and_then(|c| flat.iter().position(|x| *x == c));
        let n = flat.len() as isize;
        let next = match at {
            Some(i) => (i as isize + delta).rem_euclid(n),
            None if delta < 0 => n - 1,
            None => 0,
        } as usize;
        let (fi, mi) = flat[next];
        self.results[fi].collapsed = false;
        self.rebuild_rows();
        self.current = Some((fi, mi));
        if let Some(row) = self.rows.iter().position(|r| matches!(r, Row::Match(a, b) if (*a, *b) == (fi, mi))) {
            self.scroll.scroll_to_item(row, gpui::ScrollStrategy::Center);
        }
        self.open(fi, mi, cx);
        cx.notify();
    }

    pub fn set_all_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        for f in &mut self.results {
            f.collapsed = collapsed;
        }
        self.rebuild_rows();
        cx.notify();
    }

    /// Remove a file from the results (it is also left out of Replace All).
    fn dismiss_file(&mut self, fi: usize, cx: &mut Context<Self>) {
        if fi < self.results.len() {
            self.results.remove(fi);
            self.current = None;
            if let Status::Done { files, matches, .. } = &mut self.status {
                *files = self.results.len() as u32;
                *matches = self.results.iter().map(|f| f.matches.len() as u32).sum();
            }
            self.rebuild_rows();
            cx.notify();
        }
    }

    /// Replace only the matches in this file: reviewed in a Search Editor first.
    fn replace_in_file(&mut self, fi: usize, cx: &mut Context<Self>) {
        if self.st.on {
            return self.request_structural_replace(Some(fi), cx);
        }
        let Some(f) = self.results.get(fi) else { return };
        if self.query.read(cx).text().is_empty() {
            return;
        }
        let only = Some(self.abs(&f.path));
        cx.emit(SearchEvent::ReviewReplace { spec: self.spec(cx), only });
    }

    /// The query, its options and the replacement, as a Search Editor keeps them.
    pub fn spec(&self, cx: &App) -> crate::search_doc::SearchSpec {
        crate::search_doc::SearchSpec {
            pattern: self.query.read(cx).text().to_string(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            preserve_case: self.preserve_case,
            include: if self.show_globs { self.include.read(cx).text().to_string() } else { String::new() },
            exclude: if self.show_globs { self.exclude.read(cx).text().to_string() } else { String::new() },
            context: 1,
            replacement: self.replace.read(cx).text().to_string(),
        }
    }

    /// "Find in Folder…": limit the search to this folder (the file filter opens).
    pub fn limit_to(&mut self, glob: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.show_globs = true;
        self.include.update(cx, |f, cx| f.set_text(glob, cx));
        window.focus(&self.query.focus_handle(cx));
        self.start(cx);
    }

    #[cfg(test)]
    pub fn include_text(&self, cx: &App) -> String {
        self.include.read(cx).text().to_string()
    }

    #[cfg(test)]
    pub fn current_for_test(&self) -> Option<(usize, usize)> {
        self.current
    }

    #[cfg(test)]
    pub fn history_for_test(&self) -> Vec<String> {
        self.history.clone()
    }

    #[cfg(test)]
    pub fn step_history_for_test(&mut self, older: bool, cx: &mut Context<Self>) {
        self.step_history(older, cx);
    }

    #[cfg(test)]
    pub fn remember_for_test(&mut self, cx: &mut Context<Self>) {
        self.remember_query(cx);
    }

    #[cfg(test)]
    pub fn dismiss_for_test(&mut self, fi: usize, cx: &mut Context<Self>) {
        self.dismiss_file(fi, cx);
    }

    #[cfg(test)]
    pub fn collapsed_for_test(&self) -> Vec<bool> {
        self.results.iter().map(|f| f.collapsed).collect()
    }

    fn clear_query(&mut self, cx: &mut Context<Self>) {
        self.active_query().clone().update(cx, |q, cx| q.set_text("", cx));
    }

    fn stop(&mut self) {
        self.cancel_structural();
        if let Some((ids, _task)) = self.running.take() {
            for id in ids {
                self.host.cancel(id);
            }
        }
        self.debounce = None;
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            this.update(cx, |this, cx| this.start(cx)).ok();
        }));
    }

    /// Replace request: the query goes to a Search Editor, which searches again and shows every change before Apply.
    pub fn request_replace_all(&mut self, cx: &mut Context<Self>) {
        if self.st.on {
            return self.request_structural_replace(None, cx);
        }
        if self.root.is_none() || self.query.read(cx).text().is_empty() {
            return;
        }
        cx.emit(SearchEvent::ReviewReplace { spec: self.spec(cx), only: None });
    }

    #[cfg(test)]
    pub fn set_regex(&mut self, on: bool, cx: &mut Context<Self>) {
        self.regex = on;
        self.start(cx);
    }

    #[cfg(test)]
    pub fn set_replacement(&mut self, text: &str, cx: &mut Context<Self>) {
        self.replace.update(cx, |f, cx| f.set_text(text, cx));
    }

    /// Refresh the results after a replace.
    pub fn rerun(&mut self, cx: &mut Context<Self>) {
        self.start(cx);
    }

    fn query(&self, cx: &App) -> Option<SearchQuery> {
        self.query_in(self.root.clone()?, cx)
    }

    fn query_in(&self, root: String, cx: &App) -> Option<SearchQuery> {
        let settings = cx.global::<crate::settings::Settings>();
        let pattern = self.query.read(cx).text().to_string();
        if pattern.is_empty() {
            return None;
        }
        let pattern_has_upper = pattern.chars().any(char::is_uppercase);
        let globs = |f: &Entity<TextField>| -> Vec<String> { f.read(cx).text().split(',').map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect() };
        Some(SearchQuery {
            root,
            pattern,
            regex: self.regex,
            case_sensitive: self.case_sensitive || (settings.search_smart_case && pattern_has_upper),
            whole_word: self.whole_word,
            include: if self.show_globs { globs(&self.include) } else { Vec::new() },
            exclude: {
                let mut ex = if self.show_globs { globs(&self.exclude) } else { Vec::new() };
                ex.extend(settings.files_exclude.iter().cloned());
                ex.extend(settings.search_exclude.iter().cloned());
                ex
            },
            max_results: settings.search_max_results,
        })
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        use futures::{FutureExt as _, StreamExt as _};
        if self.st.on {
            return self.start_structural(cx);
        }
        self.stop();
        self.results.clear();
        self.current = None;
        self.rebuild_rows();
        let Some(query) = self.query(cx) else {
            self.status = Status::Idle;
            cx.notify();
            return;
        };
        // The first root and extra folders are searched concurrently; results merge into one list.
        let mut roots: Vec<(Option<String>, SearchQuery)> = vec![(None, query)];
        for e in self.extra_roots.clone() {
            if let Some(q) = self.query_in(e.clone(), cx) {
                roots.push((Some(ui::file_name(&e).to_string()), q));
            }
        }
        let (mut streams, mut ids) = (Vec::new(), Vec::new());
        for (label, q) in roots {
            match self.host.search(q) {
                Ok(s) => {
                    ids.push(s.id);
                    streams.push(ui::bridge(s.rx).map(move |r| (label.clone(), r)).boxed());
                }
                Err(e) => {
                    for id in ids {
                        self.host.cancel(id);
                    }
                    self.status = Status::Failed(e.to_string().into());
                    cx.notify();
                    return;
                }
            }
        }
        self.status = Status::Searching;
        let pending_ids = ids.clone();
        let task = cx.spawn(async move |this, cx| {
            let mut rx = futures::stream::select_all(streams);
            let (mut pending, mut files_sum, mut matches_sum, mut truncated_any) = (pending_ids.len(), 0u32, 0u32, false);
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Some(Some(more)) = rx.next().now_or_never() {
                    batch.push(more);
                }
                let keep_going = this
                    .update(cx, |this, cx| {
                        let mut more = true;
                        for (label, r) in batch {
                            match r {
                                Response::SearchChunk(files) => {
                                    let mode = cx.global::<crate::settings::Settings>().search_collapse.clone();
                                    for mut f in files {
                                        if let Some(l) = &label {
                                            f.path = format!("{l}{}{}", crate::workbench::ROOT_SEP, f.path);
                                        }
                                        this.add(f, &mode);
                                    }
                                }
                                Response::SearchDone { files, matches, truncated, .. } => {
                                    pending -= 1;
                                    (files_sum, matches_sum, truncated_any) = (files_sum + files, matches_sum + matches, truncated_any | truncated);
                                    if pending == 0 {
                                        this.status = Status::Done { files: files_sum, matches: matches_sum, truncated: truncated_any };
                                        this.running = None;
                                        more = false;
                                    }
                                }
                                Response::Error(e) => {
                                    this.status = Status::Failed(e.to_string().into());
                                    if let Some((ids, _)) = this.running.take() {
                                        for id in ids {
                                            this.host.cancel(id);
                                        }
                                    }
                                    more = false;
                                }
                                _ => {}
                            }
                        }
                        this.rebuild_rows();
                        cx.notify();
                        more
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        });
        self.running = Some((ids, task));
        cx.notify();
    }

    /// Insert sorted by path: the parallel scan's order is random, the list isn't.
    fn add(&mut self, fm: FileMatches, collapse_mode: &str) {
        let at = self.results.partition_point(|r| r.path < fm.path);
        let collapsed = match collapse_mode {
            "alwaysCollapse" => true,
            "alwaysExpand" => false,
            _ => fm.matches.len() >= 10,
        };
        self.results.insert(at, FileResult { path: fm.path, matches: fm.matches, collapsed });
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (fi, f) in self.results.iter().enumerate() {
            self.rows.push(Row::File(fi));
            if !f.collapsed {
                self.rows.extend((0..f.matches.len()).map(|mi| Row::Match(fi, mi)));
            }
        }
    }

    fn open(&mut self, fi: usize, mi: usize, cx: &mut Context<Self>) {
        let Some(f) = self.results.get(fi).filter(|_| self.root.is_some()) else { return };
        let m = &f.matches[mi];
        let len = m.ranges.first().map_or(0, |(a, b)| m.preview[*a as usize..*b as usize].chars().count() as u32);
        cx.emit(SearchEvent::Open { path: self.abs(&f.path), line: m.line, column: m.column, len });
    }

    fn toggle(&self, id: &'static str, label: &'static str, on: bool, tip: &'static str, cx: &mut Context<Self>, set: fn(&mut Self)) -> impl IntoElement {
        let p = *palette(cx);
        ui::option_toggle(id, label, on, tip, &p, p.bg).relative().children(self.option_probe(id)).on_click(cx.listener(move |this, _, _, cx| {
            set(this);
            this.start(cx);
        }))
    }

    /// Keep this search in a Search Editor tab (it runs there again, in the first folder).
    pub fn open_in_editor(&mut self, cx: &mut Context<Self>) {
        cx.emit(SearchEvent::OpenInEditor(self.spec(cx)));
    }

    #[cfg(test)]
    pub fn result_summary(&self) -> Vec<(String, Vec<u32>)> {
        self.results.iter().map(|f| (f.path.clone(), f.matches.iter().map(|m| m.line).collect())).collect()
    }

    #[cfg(test)]
    pub fn is_searching(&self) -> bool {
        self.running.is_some() || self.debounce.is_some() && matches!(self.status, Status::Idle)
    }

    #[cfg(test)]
    pub fn open_first(&mut self, cx: &mut Context<Self>) {
        self.open(0, 0, cx);
    }
}

impl Render for SearchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        self.painted.borrow_mut().clear();
        let status = self.status_line(cx).map(|(s, error)| (s, if error { p.danger } else { p.text_muted }));
        let (q_focus, r_focus, i_focus, e_focus) = (
            self.active_query().focus_handle(cx).is_focused(window),
            self.replace.focus_handle(cx).is_focused(window),
            self.include.focus_handle(cx).is_focused(window),
            self.exclude.focus_handle(cx).is_focused(window),
        );
        let field_box = |id: &'static str, focused: bool, child: gpui::AnyElement| crate::ui::input_box(id, &p, p.bg, focused).relative().child(child);
        let field = |which: AccessField| self.probe(AccessFrame::Field(which));

        div()
            .id("search-panel")
            .key_context("SearchPanel")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &HistoryPrev, _, cx| this.step_history(true, cx)))
            .on_action(cx.listener(|this, _: &HistoryNext, _, cx| this.step_history(false, cx)))
            .on_action(cx.listener(|this, _: &NextResult, _, cx| this.step_result(1, cx)))
            .on_action(cx.listener(|this, _: &PrevResult, _, cx| this.step_result(-1, cx)))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[3]))
            .px(px(theme::SPACE[5]))
            .child(field_box(
                "search-query-box",
                q_focus,
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .child(crate::ui::search_icon(&p))
                    .child(div().flex_1().min_w_0().relative().child(self.active_query().clone()).child(field(AccessField::Query)))
                    // Clear button while there's text (doesn't touch results; only empties the field).
                    .when(!self.active_query().read(cx).text().is_empty(), |d| {
                        d.child(
                            div()
                                .id("s-clear-field")
                                .flex_none()
                                .size(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .child(icon("x", 12., p.text_muted))
                                .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Clear the search field").into())).into())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.clear_query(cx);
                                    this.start(cx);
                                })),
                        )
                    })
                    // --- structural ---
                    .when(self.st.on, |d| d.child(self.language_chip(cx)))
                    .when(!self.st.on, |d| {
                        d.child(self.toggle("s-case", "Aa", self.case_sensitive, crate::i18n::t("Match case"), cx, |s| s.case_sensitive = !s.case_sensitive))
                            .child(self.toggle("s-word", "ab", self.whole_word, crate::i18n::t("Whole word"), cx, |s| s.whole_word = !s.whole_word))
                            .child(self.toggle("s-regex", ".*", self.regex, crate::i18n::t("Regular expression"), cx, |s| s.regex = !s.regex))
                            .child(self.toggle("s-preserve", "AB", self.preserve_case, crate::i18n::t("Preserve case when replacing"), cx, |s| {
                                s.preserve_case = !s.preserve_case
                            }))
                    })
                    .child(self.toggle(
                        "s-structural",
                        "$X",
                        self.st.on,
                        crate::i18n::t("Structural search: the pattern is code; $X matches one node, $$$X several"),
                        cx,
                        |s| s.st.on = !s.st.on,
                    ))
                    .into_any_element(),
            ))
            .when_some(self.st.error.clone().filter(|_| self.st.on), |d, e| {
                d.child(
                    div().id("s-pattern-error").relative().text_size(px(text::XS.0)).text_color(c(p.danger)).child(e).child(self.probe(AccessFrame::Status)),
                )
            })
            .child(
                div()
                    .flex()
                    .gap(px(theme::SPACE[2]))
                    .items_center()
                    .child(field_box("search-replace-box", r_focus, self.replace.clone().into_any_element()).child(field(AccessField::Replace)).flex_1())
                    .child({
                        let ready = matches!(self.status, Status::Done { matches, .. } if matches > 0);
                        div()
                            .id("s-replace-all")
                            .relative()
                            .child(self.probe(AccessFrame::ReplaceAll))
                            .h(px(28.))
                            .px(px(theme::SPACE[4]))
                            .flex()
                            .items_center()
                            .rounded(px(theme::radius::MD))
                            .border_1()
                            .text_size(px(text::XS.0))
                            .when(ready, |d| {
                                d.border_color(c(ui::input_border(&p)))
                                    .text_color(c(p.text))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(c(p.surface_hover)).border_color(c(p.border_input)))
                            })
                            .when(!ready, |d| d.border_color(c(p.border)).text_color(c(p.text_muted)))
                            .child(crate::i18n::t("Replace All"))
                            .tooltip(|_, cx| {
                                cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Review every change before replacing (↵ in the field)").into())).into()
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.request_replace_all(cx)))
                    }),
            )
            .child(
                div()
                    .id("s-globs")
                    .relative()
                    .child(self.probe(AccessFrame::Globs))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .cursor_pointer()
                    .hover(|d| d.text_color(c(p.text)))
                    .child(icon(if self.show_globs { "chevron-down" } else { "chevron-right" }, 12., p.text_muted))
                    .child(crate::i18n::t("Files to include / exclude"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_globs = !this.show_globs;
                        this.start(cx);
                    })),
            )
            .when(self.show_globs, |d| {
                d.child(field_box("search-include-box", i_focus, self.include.clone().into_any_element()).child(field(AccessField::Include)))
                    .child(field_box("search-exclude-box", e_focus, self.exclude.clone().into_any_element()).child(field(AccessField::Exclude)))
            })
            .when_some(status, |d, (s, color)| {
                let any = !self.results.is_empty();
                let all_collapsed = self.results.iter().all(|f| f.collapsed);
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[1]))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .relative()
                                .truncate()
                                .text_size(px(text::XS.0))
                                .text_color(c(color))
                                .child(s)
                                .child(self.probe(AccessFrame::Status)),
                        )
                        .when(any, |d| {
                            d.child(
                                ui::icon_button("s-prev", "arrow-up", crate::i18n::t("Previous result (⇧F4)"), &p)
                                    .on_click(cx.listener(|this, _, _, cx| this.step_result(-1, cx))),
                            )
                            .child(
                                ui::icon_button("s-next", "arrow-down", crate::i18n::t("Next result (F4)"), &p)
                                    .on_click(cx.listener(|this, _, _, cx| this.step_result(1, cx))),
                            )
                            .child(if all_collapsed {
                                ui::icon_button("s-expand", "chevrons-up-down", crate::i18n::t("Expand all results"), &p)
                                    .on_click(cx.listener(|this, _, _, cx| this.set_all_collapsed(false, cx)))
                            } else {
                                ui::icon_button("s-collapse", "chevrons-down-up", crate::i18n::t("Collapse all results"), &p)
                                    .on_click(cx.listener(|this, _, _, cx| this.set_all_collapsed(true, cx)))
                            })
                        })
                        .child(
                            ui::icon_button("s-open-editor", "file-text", crate::i18n::t("Open in a Search Editor tab"), &p)
                                .on_click(cx.listener(|this, _, _, cx| this.open_in_editor(cx))),
                        )
                        .child(
                            ui::icon_button("s-rerun", "refresh-cw", crate::i18n::t("Run the search again"), &p)
                                .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                        )
                        .child(ui::icon_button("s-clear", "x", crate::i18n::t("Clear the search"), &p).on_click(cx.listener(|this, _, _, cx| {
                            this.clear_query(cx);
                            this.results.clear();
                            this.rebuild_rows();
                            this.status = Status::Idle;
                        }))),
                )
            })
            .when_some(self.structural_note(cx).filter(|_| self.st.on), |d, n| {
                d.child(div().id("s-structural-note").text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(n))
            })
            .child(
                div().flex_1().min_h_0().mx(px(-theme::SPACE[5])).child(
                    uniform_list(
                        "search-rows",
                        self.rows.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|ix| match this.rows[ix] {
                                    Row::File(fi) => {
                                        let f = &this.results[fi];
                                        let (dir, name) = f.path.rsplit_once('/').unwrap_or(("", &f.path));
                                        let key = AccessKey::File(f.path.clone());
                                        div()
                                            .id(("s-file", ix))
                                            .relative()
                                            .child(this.probe(AccessFrame::Row(key.clone())))
                                            .group(format!("s-file-{ix}"))
                                            .w_full()
                                            .h(px(layout::ROW_H))
                                            .flex()
                                            .items_center()
                                            .gap(px(theme::SPACE[2]))
                                            .px(px(theme::SPACE[4]))
                                            .cursor_pointer()
                                            .hover(|d| d.bg(c(p.surface_hover)))
                                            .child(icon(if f.collapsed { "chevron-right" } else { "chevron-down" }, layout::ICON, p.text_muted))
                                            .child(crate::icon_theme::file_icon_el(name, layout::ICON, &p))
                                            .child(div().flex_none().text_color(c(p.text)).child(name.to_string()))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_size(px(text::XS.0))
                                                    .text_color(c(p.text_muted))
                                                    .child(dir.to_string()),
                                            )
                                            .child(
                                                div()
                                                    .id(("s-file-replace", fi))
                                                    .relative()
                                                    .child(this.probe(AccessFrame::Button(key.clone(), REPLACE_IN_FILE)))
                                                    .flex_none()
                                                    .invisible()
                                                    .group_hover(format!("s-file-{ix}"), |s| s.visible())
                                                    .size(px(18.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(theme::radius::SM))
                                                    .hover(|d| d.bg(c(p.surface_active)))
                                                    .child(icon("undo-2", 12., p.text_muted))
                                                    .tooltip(|_, cx| {
                                                        cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Replace in this file").into())).into()
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.replace_in_file(fi, cx);
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id(("s-file-dismiss", fi))
                                                    .relative()
                                                    .child(this.probe(AccessFrame::Button(key.clone(), DISMISS_FILE)))
                                                    .flex_none()
                                                    .invisible()
                                                    .group_hover(format!("s-file-{ix}"), |s| s.visible())
                                                    .size(px(18.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(theme::radius::SM))
                                                    .hover(|d| d.bg(c(p.surface_active)))
                                                    .child(icon("x", 12., p.text_muted))
                                                    .tooltip(|_, cx| {
                                                        cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Dismiss this file from the results").into()))
                                                            .into()
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.dismiss_file(fi, cx);
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .px(px(theme::SPACE[2]))
                                                    .rounded(px(8.))
                                                    .bg(c(p.surface_hover))
                                                    .text_size(px(text::XXS.0))
                                                    .text_color(c(p.text_muted))
                                                    .child(f.matches.len().to_string()),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.results[fi].collapsed = !this.results[fi].collapsed;
                                                this.rebuild_rows();
                                                cx.notify();
                                            }))
                                            .into_any_element()
                                    }
                                    Row::Match(fi, mi) => {
                                        let m = &this.results[fi].matches[mi];
                                        // If the match is far along the line, keep it visible: trim the front, add "…".
                                        let (preview, ranges) = trim_preview(&m.preview, &m.ranges);
                                        let highlights = ranges
                                            .into_iter()
                                            .map(|r| (r, HighlightStyle { background_color: Some(ui::ca(p.warning, 0.28).into()), ..Default::default() }));
                                        let key = AccessKey::Match(this.results[fi].path.clone(), m.line, m.column);
                                        div()
                                            .id(("s-match", ix))
                                            .relative()
                                            .child(this.probe(AccessFrame::Row(key)))
                                            .w_full()
                                            .h(px(layout::ROW_H))
                                            .flex()
                                            .items_center()
                                            .gap(px(theme::SPACE[3]))
                                            .pl(px(theme::SPACE[4] + layout::ICON * 2. + theme::SPACE[2]))
                                            .pr(px(theme::SPACE[4]))
                                            .cursor_pointer()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .when(this.current == Some((fi, mi)), |d| d.bg(c(p.surface_active)))
                                            .hover(|d| d.bg(c(p.surface_hover)))
                                            .when(cx.global::<crate::settings::Settings>().search_line_numbers, |d| {
                                                d.child(div().flex_none().text_size(px(text::XS.0)).text_color(c(p.line_number)).child(m.line.to_string()))
                                            })
                                            .child(div().min_w_0().text_color(c(p.text)).child(StyledText::new(preview).with_highlights(highlights)))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.current = Some((fi, mi));
                                                    this.open(fi, mi, cx);
                                                    cx.notify();
                                                }),
                                            )
                                            .into_any_element()
                                    }
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(self.scroll.clone())
                    .size_full(),
                ),
            )
    }
}

/// Start the preview about 12 characters before the first match (if remote); highlight
/// ranges (bytes) shift too.
pub(crate) fn trim_preview(preview: &str, ranges: &[(u32, u32)]) -> (String, Vec<std::ops::Range<usize>>) {
    const KEEP_BEFORE: usize = 12;
    const MAX_LEAD: usize = 40;
    let first = ranges.first().map_or(0, |r| r.0 as usize).min(preview.len());
    if first <= MAX_LEAD {
        return (preview.to_string(), ranges.iter().map(|&(a, b)| a as usize..b as usize).collect());
    }
    // On a character boundary, KEEP_BEFORE characters before the match.
    let start = preview[..first].char_indices().rev().nth(KEEP_BEFORE - 1).map_or(0, |(i, _)| i);
    let shift = start as isize - "…".len() as isize;
    let text = format!("…{}", &preview[start..]);
    let ranges = ranges.iter().filter(|r| r.0 as usize >= start).map(|&(a, b)| (a as isize - shift) as usize..(b as isize - shift) as usize).collect();
    (text, ranges)
}

#[cfg(test)]
mod preview_tests {
    #[test]
    fn far_match_is_brought_into_view() {
        let line = format!("{}omega = 1", "x".repeat(60));
        let (text, ranges) = super::trim_preview(&line, &[(60, 65)]);
        assert!(text.starts_with('…'));
        assert_eq!(&text[ranges[0].clone()], "omega");
        let (same, r) = super::trim_preview("a omega", &[(2, 7)]);
        assert_eq!((same.as_str(), r), ("a omega", vec![2..7]));
    }
}
