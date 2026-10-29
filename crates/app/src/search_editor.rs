//! Search Editor: an editor tab that keeps one search (`search_doc::SearchDoc`) with its query, options, context lines and
//! the host and folder it ran in. Results can be navigated with the keyboard, rerun in place, and reviewed as a
//! replacement: the preview is an immutable plan (`replace_plan`) whose changes are selected per match or per file, and
//! nothing is written before Apply. The workbench owns what needs other tabs or the connection (reading open buffers,
//! writing files, undo); this view only shows state and asks through `SearchEditorEvent`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle, MouseButton, SharedString, StyledText, Subscription, Task,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use host::Host;
use proto::Response;

use crate::input::{FieldEvent, TextField};
use crate::replace_apply::Report;
use crate::replace_plan::{Plan, Selection, Tri};
use crate::search_doc::{self, Row, RunState, SearchDoc, SearchSpec, Stale};
use crate::ui::{self, c, icon, layout, palette, text};

gpui::actions!(search_editor, [SelectNextRow, SelectPrevRow, ActivateRow, ToggleRow, RerunSearch]);

/// Context is read from at most this many bytes of files (the rest of the matches show without it).
const CONTEXT_BYTES: usize = 16 * 1024 * 1024;
/// Characters shown around a change in a diff row.
const DIFF_CHARS: usize = 160;

pub enum SearchEditorEvent {
    /// Go to a match: absolute path, 1-based line, 0-based column, length (characters).
    Open { path: String, line: u32, column: u32, len: u32 },
    /// Run the search again (the workbench checks the host and folder first).
    Rerun,
    /// Make a replacement preview from the current results.
    Preview,
    /// Apply the selected changes of the preview.
    Apply,
    /// Stop an apply or undo between files.
    Cancel,
    /// Undo the last replacement.
    Undo,
    /// Something worth saving changed (query, results, replacement).
    Changed,
}

/// Work in progress that blocks starting another.
pub enum Busy {
    Previewing,
    /// The flag stops the remaining files.
    Applying(Arc<AtomicBool>),
    Undoing(Arc<AtomicBool>),
}

/// What a preview should select once it is ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Review {
    All,
    Only(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlanRow {
    File(usize),
    /// A change: its line before (with the checkbox) and after.
    Old(usize, usize),
    New(usize, usize),
}

pub struct SearchEditor {
    pub doc: SearchDoc,
    host: Arc<dyn Host>,
    /// Options being edited; `doc.spec` is what the shown results ran with.
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    preserve_case: bool,
    context: u8,
    query: Entity<TextField>,
    include: Entity<TextField>,
    exclude: Entity<TextField>,
    replace: Entity<TextField>,
    rows: Vec<Row>,
    plan: Option<Arc<Plan>>,
    selection: Option<Selection>,
    plan_rows: Vec<PlanRow>,
    /// Keyboard row in the list being shown.
    cursor: Option<usize>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    list_focus: FocusHandle,
    running: Option<(Vec<u32>, Task<()>)>,
    context_task: Option<Task<()>>,
    pub busy: Option<Busy>,
    message: Option<(SharedString, bool)>,
    report: Option<Report>,
    can_undo: bool,
    /// After the next run, make a preview with this selection.
    pub review: Option<Review>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SearchEditorEvent> for SearchEditor {}

impl Focusable for SearchEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SearchEditor {
    pub fn new(doc: SearchDoc, host: Arc<dyn Host>, cx: &mut Context<Self>) -> Self {
        let spec = doc.spec.clone();
        let field = |placeholder: &'static str, value: &str, cx: &mut Context<Self>| {
            let f = cx.new(|cx| TextField::new(crate::i18n::t(placeholder), cx));
            f.update(cx, |f, cx| f.set_text(value, cx));
            f
        };
        let query = field(crate::i18n::tr("Search"), &spec.pattern, cx);
        let include = field(crate::i18n::tr("Files to include (e.g. *.rs, src/**)"), &spec.include, cx);
        let exclude = field(crate::i18n::tr("Files to exclude"), &spec.exclude, cx);
        let replace = field(crate::i18n::tr("Replace"), &spec.replacement, cx);
        let mut subs = Vec::new();
        for f in [&query, &include, &exclude] {
            subs.push(cx.subscribe(f, |_, _, ev: &FieldEvent, cx| {
                if matches!(ev, FieldEvent::Submit) {
                    cx.emit(SearchEditorEvent::Rerun);
                }
            }));
        }
        subs.push(cx.subscribe(&replace, |this, field, ev: &FieldEvent, cx| match ev {
            FieldEvent::Changed => {
                this.doc.spec.replacement = field.read(cx).text().to_string();
                cx.emit(SearchEditorEvent::Changed);
            }
            FieldEvent::Submit => cx.emit(SearchEditorEvent::Preview),
            FieldEvent::Escape => {}
        }));
        let rows = doc.rows();
        SearchEditor {
            case_sensitive: spec.case_sensitive,
            whole_word: spec.whole_word,
            regex: spec.regex,
            preserve_case: spec.preserve_case,
            context: spec.context.min(search_doc::MAX_CONTEXT),
            doc,
            host,
            query,
            include,
            exclude,
            replace,
            rows,
            plan: None,
            selection: None,
            plan_rows: Vec::new(),
            cursor: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            list_focus: cx.focus_handle().tab_stop(true),
            running: None,
            context_task: None,
            busy: None,
            message: None,
            report: None,
            can_undo: false,
            review: None,
            _subs: subs,
        }
    }

    /// Tab title: the query.
    pub fn title(&self) -> String {
        let q: String = self.doc.spec.pattern.chars().take(30).collect();
        crate::i18n::tf("Search: {}", &[&q])
    }

    pub fn host(&self) -> &Arc<dyn Host> {
        &self.host
    }

    /// The spec as edited in the fields and toggles.
    pub fn edited_spec(&self, cx: &App) -> SearchSpec {
        SearchSpec {
            pattern: self.query.read(cx).text().to_string(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            preserve_case: self.preserve_case,
            include: self.include.read(cx).text().to_string(),
            exclude: self.exclude.read(cx).text().to_string(),
            context: self.context,
            replacement: self.replace.read(cx).text().to_string(),
        }
    }

    /// Put a spec into the fields and toggles (a reused tab).
    pub fn load_spec(&mut self, spec: &SearchSpec, cx: &mut Context<Self>) {
        self.case_sensitive = spec.case_sensitive;
        self.whole_word = spec.whole_word;
        self.regex = spec.regex;
        self.preserve_case = spec.preserve_case;
        for (field, text) in [(&self.query, &spec.pattern), (&self.include, &spec.include), (&self.exclude, &spec.exclude), (&self.replace, &spec.replacement)]
        {
            field.update(cx, |f, cx| f.set_text(text, cx));
        }
        cx.notify();
    }

    pub fn focus_query(&self, window: &mut Window, cx: &App) {
        window.focus(&self.query.focus_handle(cx));
    }

    /// The window now shows another host or folder (or the same again): results are marked stale, or cleared of that mark.
    pub fn set_workspace_matches(&mut self, matches: bool, cx: &mut Context<Self>) {
        if !matches {
            self.doc.mark_stale(Stale::Workspace);
        } else if self.doc.stale == Some(Stale::Workspace) {
            self.doc.stale = None;
        }
        cx.notify();
    }

    pub fn set_message(&mut self, text: impl Into<SharedString>, error: bool, cx: &mut Context<Self>) {
        self.message = Some((text.into(), error));
        cx.notify();
    }

    // --- Running ---

    /// Run the edited query on `host` in the search's folder (the workbench has checked that this window shows it).
    pub fn run(&mut self, host: Arc<dyn Host>, cx: &mut Context<Self>) {
        use futures::StreamExt as _;
        self.stop_search();
        self.host = host;
        self.doc.spec = self.edited_spec(cx);
        self.clear_plan();
        self.report = None;
        self.message = None;
        self.cursor = None;
        let generation = self.doc.begin();
        self.rows.clear();
        let settings = cx.global::<crate::settings::Settings>();
        let mut extra = settings.files_exclude.clone();
        extra.extend(settings.search_exclude.iter().cloned());
        let (smart, max) = (settings.search_smart_case, settings.search_max_results);
        let roots: Vec<String> = self.doc.provenance.roots().map(str::to_string).collect();
        let queries: Vec<(String, proto::SearchQuery)> =
            roots.into_iter().filter_map(|root| Some((root.clone(), self.doc.spec.query(&root, smart, &extra, max)?))).collect();
        if queries.is_empty() {
            self.doc.state = RunState::NotRun;
            cx.emit(SearchEditorEvent::Changed);
            cx.notify();
            return;
        }
        // Every folder of the workspace is searched at once; results merge into one list with absolute paths.
        let (mut streams, mut ids) = (Vec::new(), Vec::new());
        for (root, q) in queries {
            match self.host.search(q) {
                Ok(s) => {
                    ids.push(s.id);
                    streams.push(ui::bridge(s.rx).map(move |r| (root.clone(), r)).boxed());
                }
                Err(e) => {
                    for id in ids {
                        self.host.cancel(id);
                    }
                    self.doc.fail(e.to_string());
                    cx.emit(SearchEditorEvent::Changed);
                    cx.notify();
                    return;
                }
            }
        }
        let task = cx.spawn(async move |this, cx| {
            let mut rx = futures::stream::select_all(streams);
            let (mut pending, mut truncated_any, mut cancelled_any) = (rx.len(), false, false);
            while let Some((root, r)) = rx.next().await {
                let more = this
                    .update(cx, |this, cx| {
                        if this.doc.generation != generation {
                            return false;
                        }
                        let more = match r {
                            Response::SearchChunk(files) => {
                                for f in files {
                                    this.doc.add(ui::join(&root, &f.path), f.matches);
                                }
                                true
                            }
                            Response::SearchDone { truncated, cancelled, .. } => {
                                pending -= 1;
                                truncated_any |= truncated;
                                cancelled_any |= cancelled;
                                if pending == 0 {
                                    this.doc.finish(truncated_any, cancelled_any);
                                    this.running = None;
                                    this.after_run(cx);
                                }
                                pending > 0
                            }
                            Response::Error(e) => {
                                this.doc.fail(e.to_string());
                                if let Some((ids, _)) = this.running.take() {
                                    for id in ids {
                                        this.host.cancel(id);
                                    }
                                }
                                cx.emit(SearchEditorEvent::Changed);
                                false
                            }
                            _ => true,
                        };
                        this.rows = this.doc.rows();
                        cx.notify();
                        more
                    })
                    .unwrap_or(false);
                if !more {
                    return;
                }
            }
            // A stream ended without a summary: the connection dropped.
            this.update(cx, |this, cx| {
                if this.doc.generation == generation && this.doc.state == RunState::Running {
                    this.doc.fail(crate::i18n::t("The connection was lost; the results are incomplete.").into());
                    this.running = None;
                    cx.emit(SearchEditorEvent::Changed);
                    cx.notify();
                }
            })
            .ok();
        });
        self.running = Some((ids, task));
        cx.notify();
    }

    fn after_run(&mut self, cx: &mut Context<Self>) {
        self.fetch_context(cx);
        cx.emit(SearchEditorEvent::Changed);
        if self.review.is_some() {
            cx.emit(SearchEditorEvent::Preview);
        }
    }

    /// Stop a running search; what arrived stays, marked cancelled.
    pub fn stop_search(&mut self) {
        if let Some((ids, _task)) = self.running.take() {
            for id in ids {
                self.host.cancel(id);
            }
            self.doc.cancel();
        }
        self.context_task = None;
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Read the context lines around the first `CONTEXT_MATCHES` matches (bounded by `CONTEXT_BYTES`).
    fn fetch_context(&mut self, cx: &mut Context<Self>) {
        let n = self.doc.spec.context as usize;
        if n == 0 || self.doc.stale.is_some() {
            return;
        }
        let targets = self.doc.context_targets();
        let paths: Vec<(usize, String)> = {
            let mut seen = Vec::new();
            for (fi, _, _) in &targets {
                if seen.last().is_none_or(|(f, _)| f != fi) {
                    seen.push((*fi, self.doc.files[*fi].path.clone()));
                }
            }
            seen
        };
        let host = self.host.clone();
        let generation = self.doc.generation;
        let fallback = cx.global::<crate::settings::Settings>().fallback_encoding;
        let read = cx.background_executor().spawn(async move {
            let mut budget = CONTEXT_BYTES;
            let mut out = Vec::new();
            for (fi, path) in paths {
                let Ok((bytes, _)) = host.read(&path) else { continue };
                if bytes.len() > budget {
                    break;
                }
                budget -= bytes.len();
                let Some((text, _)) = crate::encoding::decode(&bytes, fallback) else { continue };
                let lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
                out.push((fi, lines));
            }
            out
        });
        self.context_task = Some(cx.spawn(async move |this, cx| {
            let files = read.await;
            this.update(cx, |this, cx| {
                if this.doc.generation != generation {
                    return;
                }
                for (fi, lines) in files {
                    let count = this.doc.files.get(fi).map_or(0, |f| f.matches.len());
                    for mi in 0..count {
                        let Some(line) = this.doc.files.get(fi).and_then(|f| f.matches.get(mi)).map(|m| m.line as usize) else { continue };
                        let from = line.saturating_sub(1 + n);
                        let before = lines.get(from..line.saturating_sub(1)).map(<[String]>::to_vec).unwrap_or_default();
                        let after = lines.get(line..(line + n).min(lines.len())).map(<[String]>::to_vec).unwrap_or_default();
                        this.doc.set_context(fi, mi, before, after);
                    }
                }
                this.rows = this.doc.rows();
                cx.notify();
            })
            .ok();
        }));
    }

    // --- Preview, apply, undo (driven by the workbench) ---

    pub fn plan(&self) -> Option<(Arc<Plan>, Selection)> {
        Some((self.plan.clone()?, self.selection.clone()?))
    }

    pub fn has_plan(&self) -> bool {
        self.plan.is_some()
    }

    pub fn set_plan(&mut self, plan: Arc<Plan>, cx: &mut Context<Self>) {
        let selection = match self.review.take() {
            Some(Review::Only(path)) => Selection::only(&plan, &path),
            _ => Selection::all(&plan),
        };
        self.busy = None;
        self.message = None;
        self.report = None;
        self.cursor = None;
        self.plan_rows = plan_rows(&plan);
        self.plan = Some(plan);
        self.selection = Some(selection);
        self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    pub fn clear_plan(&mut self) {
        self.plan = None;
        self.selection = None;
        self.plan_rows.clear();
        self.cursor = None;
    }

    pub fn set_busy(&mut self, busy: Option<Busy>, cx: &mut Context<Self>) {
        self.busy = busy;
        cx.notify();
    }

    /// An apply or undo finished: show what happened. After an apply the results no longer describe the files.
    pub fn set_report(&mut self, report: Report, can_undo: bool, cx: &mut Context<Self>) {
        self.busy = None;
        if !report.undo {
            self.clear_plan();
        }
        if !report.changed().is_empty() {
            self.doc.mark_stale(Stale::Replaced);
        }
        self.report = Some(report);
        self.can_undo = can_undo;
        self.message = None;
        cx.emit(SearchEditorEvent::Changed);
        cx.notify();
    }

    pub fn set_can_undo(&mut self, can: bool, cx: &mut Context<Self>) {
        self.can_undo = can;
        cx.notify();
    }

    /// Apply is allowed: a preview with selected changes, made from complete current results of this run.
    pub fn apply_blocker(&self) -> Option<String> {
        let plan = self.plan.as_ref()?;
        if !self.doc.is_current_and_complete() || plan.generation != self.doc.generation {
            return Some(crate::i18n::t("Search results are incomplete; rerun before replacing.").into());
        }
        None
    }

    fn toggle_file(&mut self, f: usize, cx: &mut Context<Self>) {
        if let (Some(plan), Some(sel)) = (&self.plan, &mut self.selection) {
            sel.toggle_file(plan, f);
            cx.notify();
        }
    }

    fn toggle_change(&mut self, f: usize, ch: usize, cx: &mut Context<Self>) {
        if let (Some(plan), Some(sel)) = (&self.plan, &mut self.selection) {
            sel.toggle_change(plan, f, ch);
            cx.notify();
        }
    }

    fn cancel_preview(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_none() {
            self.clear_plan();
            cx.notify();
        }
    }

    // --- Keyboard ---

    fn row_count(&self) -> usize {
        if self.plan.is_some() { self.plan_rows.len() } else { self.rows.len() }
    }

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.row_count();
        if n == 0 {
            return;
        }
        let next = match self.cursor {
            Some(i) => (i as isize + delta).clamp(0, n as isize - 1) as usize,
            None if delta < 0 => n - 1,
            None => 0,
        };
        self.cursor = Some(next);
        self.scroll.scroll_to_item(next, gpui::ScrollStrategy::Center);
        cx.notify();
    }

    fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.cursor = Some(ix);
        if self.plan.is_some() {
            match self.plan_rows.get(ix).copied() {
                Some(PlanRow::File(f)) => self.toggle_file(f, cx),
                Some(PlanRow::Old(f, ch) | PlanRow::New(f, ch)) => self.open_change(f, ch, cx),
                None => {}
            }
            return;
        }
        match self.rows.get(ix).copied() {
            Some(Row::File(f)) => {
                if let Some(file) = self.doc.files.get_mut(f) {
                    file.collapsed = !file.collapsed;
                }
                self.rows = self.doc.rows();
                self.cursor = self.rows.iter().position(|r| *r == Row::File(f));
            }
            Some(Row::Match(f, m)) => self.open_match(f, m, cx),
            Some(Row::Context(f, m, line)) => {
                if let Some(path) = self.doc.files.get(f).map(|x| x.path.clone()) {
                    let _ = m;
                    cx.emit(SearchEditorEvent::Open { path, line, column: 0, len: 0 });
                }
            }
            None => {}
        }
        cx.notify();
    }

    fn toggle_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.plan_rows.get(ix).copied() {
            Some(PlanRow::File(f)) => self.toggle_file(f, cx),
            Some(PlanRow::Old(f, ch) | PlanRow::New(f, ch)) => self.toggle_change(f, ch, cx),
            None => {}
        }
    }

    fn open_match(&mut self, f: usize, m: usize, cx: &mut Context<Self>) {
        let Some((path, dm)) = self.doc.files.get(f).and_then(|x| Some((x.path.clone(), x.matches.get(m)?))) else { return };
        let len = dm.ranges.first().and_then(|(a, b)| dm.preview.get(*a as usize..*b as usize)).map_or(0, |s| s.chars().count() as u32);
        cx.emit(SearchEditorEvent::Open { path, line: dm.line, column: dm.column, len });
    }

    fn open_change(&mut self, f: usize, ch: usize, cx: &mut Context<Self>) {
        let Some(plan) = &self.plan else { return };
        let Some(file) = plan.files.get(f) else { return };
        let Some(change) = file.changes.get(ch) else { return };
        let (old, _, range, _) = file.line_diff(ch, &[]);
        let column = old.get(..range.start).map_or(0, |s| s.chars().count() as u32);
        cx.emit(SearchEditorEvent::Open { path: file.path.clone(), line: change.line, column, len: change.old.chars().count() as u32 });
    }

    /// F4 / ⇧F4: the next or previous match, opened.
    pub fn step_match(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.plan.is_some() {
            return;
        }
        let flat = self.doc.flat_matches();
        if flat.is_empty() {
            return;
        }
        let at = self.cursor.and_then(|i| match self.rows.get(i) {
            Some(Row::Match(f, m)) => flat.iter().position(|x| *x == (*f, *m)),
            _ => None,
        });
        let n = flat.len() as isize;
        let next = match at {
            Some(i) => (i as isize + delta).rem_euclid(n),
            None if delta < 0 => n - 1,
            None => 0,
        } as usize;
        let (f, m) = flat[next];
        if self.doc.files[f].collapsed {
            self.doc.files[f].collapsed = false;
            self.rows = self.doc.rows();
        }
        self.cursor = self.rows.iter().position(|r| *r == Row::Match(f, m));
        if let Some(c) = self.cursor {
            self.scroll.scroll_to_item(c, gpui::ScrollStrategy::Center);
        }
        self.open_match(f, m, cx);
        cx.notify();
    }

    fn set_context_lines(&mut self, n: u8, cx: &mut Context<Self>) {
        let n = n.min(search_doc::MAX_CONTEXT);
        if n == self.context {
            return;
        }
        self.context = n;
        self.doc.spec.context = n;
        for f in &mut self.doc.files {
            for m in &mut f.matches {
                m.before.clear();
                m.after.clear();
            }
        }
        self.rows = self.doc.rows();
        if !self.is_running() {
            self.fetch_context(cx);
        }
        cx.emit(SearchEditorEvent::Changed);
        cx.notify();
    }

    fn copy_text(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(self.doc.as_text()));
    }

    // --- Accessibility and tests ---

    /// What assistive technology sees: the status, the rows around the keyboard row (at most `max`) and the actions.
    pub fn access_view(&self, max: usize, window: &Window, cx: &App) -> AccessView {
        let total = self.row_count();
        let start = self.cursor.unwrap_or(0).saturating_sub(max / 2).min(total.saturating_sub(max));
        let rows = (start..(start + max).min(total)).map(|i| self.row_label(i)).collect();
        AccessView {
            title: self.title(),
            query: self.query.read(cx).text().to_string(),
            status: self.status_lines(cx).into_iter().map(|(s, _)| s).collect::<Vec<_>>().join(" "),
            rows,
            first_row: start,
            rows_total: total,
            selected: self.cursor,
            apply: self.plan.as_ref().map(|_| (self.apply_label(), self.apply_blocker().is_none() && self.busy.is_none() && self.selected_counts().0 > 0)),
            can_undo: self.can_undo && self.busy.is_none(),
            list_focused: self.list_focus.is_focused(window),
            query_focused: self.query.focus_handle(cx).is_focused(window),
        }
    }

    pub fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.list_focus);
        if self.cursor.is_none() && self.row_count() > 0 {
            self.cursor = Some(0);
        }
        cx.notify();
    }

    /// Activate a row by index (keyboard Enter, a click, or an accessibility action).
    pub fn activate_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.row_count() {
            self.activate(ix, cx);
        }
    }

    /// Move the keyboard row (an accessibility focus).
    pub fn select_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.row_count() {
            self.cursor = Some(ix);
            self.scroll.scroll_to_item(ix, gpui::ScrollStrategy::Center);
            cx.notify();
        }
    }

    pub fn row_label(&self, ix: usize) -> (String, Option<String>) {
        if let Some(plan) = &self.plan {
            let sel = self.selection.as_ref();
            return match self.plan_rows.get(ix).copied() {
                Some(PlanRow::File(f)) => {
                    let Some(file) = plan.files.get(f) else { return (String::new(), None) };
                    let state = match sel.map_or(Tri::None, |s| s.state(f)) {
                        Tri::All => crate::i18n::t("selected"),
                        Tri::Some => crate::i18n::t("partly selected"),
                        Tri::None => crate::i18n::t("not selected"),
                    };
                    (self.doc.display_path(&file.path).to_string(), Some(file.problem.as_ref().map_or(state.to_string(), |p| p.message())))
                }
                Some(PlanRow::Old(f, ch)) => {
                    let s = sel.map_or(&[][..], |s| s.file(f));
                    let Some(file) = plan.files.get(f) else { return (String::new(), None) };
                    let (old, new, _, _) = file.line_diff(ch, s);
                    let on = s.get(ch).copied().unwrap_or(false);
                    (
                        crate::i18n::tf("Line {0}: {1} → {2}", &[&file.changes.get(ch).map_or(0, |c| c.line), &old.trim(), &new.trim()]),
                        Some((if on { crate::i18n::t("selected") } else { crate::i18n::t("not selected") }).to_string()),
                    )
                }
                Some(PlanRow::New(f, ch)) => {
                    let line = plan.files.get(f).and_then(|x| x.changes.get(ch)).map_or(0, |c| c.line);
                    (crate::i18n::tf("Line {} after the change", &[&line]), None)
                }
                None => (String::new(), None),
            };
        }
        match self.rows.get(ix).copied() {
            Some(Row::File(f)) => {
                let Some(file) = self.doc.files.get(f) else { return (String::new(), None) };
                (self.doc.display_path(&file.path).to_string(), Some(crate::i18n::tn("{} match", "{} matches", file.matches.len(), &[])))
            }
            Some(Row::Match(f, m)) => {
                let Some(dm) = self.doc.files.get(f).and_then(|x| x.matches.get(m)) else { return (String::new(), None) };
                (crate::i18n::tf("Line {0}: {1}", &[&dm.line, &dm.preview.trim()]), None)
            }
            Some(Row::Context(f, m, line)) => {
                (crate::i18n::tf("Line {0}: {1}", &[&line, &self.doc.context_text(f, m, line).trim()]), Some(crate::i18n::t("context").into()))
            }
            None => (String::new(), None),
        }
    }

    fn selected_counts(&self) -> (usize, usize) {
        self.selection.as_ref().map_or((0, 0), Selection::counts)
    }

    fn apply_label(&self) -> String {
        let (changes, files) = self.selected_counts();
        apply_label(changes, files)
    }

    /// Status lines: (text, is a warning or error).
    fn status_lines(&self, cx: &App) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        let doc = &self.doc;
        let found = || {
            let (m, n) = (doc.match_count(), doc.files.len());
            if n == 1 {
                crate::i18n::tn("{} result in 1 file", "{} results in 1 file", m, &[])
            } else {
                crate::i18n::tn("{0} result in {1} files", "{0} results in {1} files", m, &[&n])
            }
        };
        match &doc.state {
            RunState::NotRun if doc.files.is_empty() => out.push((crate::i18n::t("Type a query and press Enter to search.").into(), false)),
            RunState::NotRun => {}
            RunState::Running => out.push((crate::i18n::t("Searching…").into(), false)),
            RunState::Complete => out.push((found(), false)),
            RunState::Truncated => {
                out.push((crate::i18n::tf("{0}. The search stopped at the result limit, so the results are incomplete.", &[&found()]), true))
            }
            RunState::Cancelled => out.push((crate::i18n::tf("{0}. The search was cancelled, so the results are incomplete.", &[&found()]), true)),
            RunState::Failed(e) if e.is_empty() => out.push((crate::i18n::t("The search failed.").into(), true)),
            RunState::Failed(e) => out.push((e.clone(), true)),
        }
        match doc.stale {
            Some(Stale::Restored) => out.push((
                if doc.partial {
                    crate::i18n::tf(
                        "Restored from the last session, only the first {} results were kept: they may be out of date. Rerun to refresh.",
                        &[&search_doc::SAVED_MAX_MATCHES],
                    )
                } else {
                    crate::i18n::t("Restored from the last session: the results may be out of date. Rerun to refresh.").into()
                },
                true,
            )),
            Some(Stale::Replaced) => out.push((crate::i18n::t("Files were changed by a replacement since this search ran. Rerun to refresh.").into(), true)),
            Some(Stale::Workspace) => out.push((
                crate::i18n::tf(
                    "This search belongs to {0} on {1}, which this window does not show. Results cannot be opened or replaced here.",
                    &[&doc.provenance.root, &doc.provenance.host],
                ),
                true,
            )),
            None => {}
        }
        if self.edited_spec(cx).differs_in_query(&doc.spec) && doc.state != RunState::Running {
            out.push((crate::i18n::t("The query was edited: press Enter or Rerun to search for it.").into(), false));
        }
        out
    }

    #[cfg(test)]
    pub fn rows_for_test(&self) -> Vec<String> {
        (0..self.row_count()).map(|i| self.row_label(i).0).collect()
    }

    #[cfg(test)]
    pub fn status_for_test(&self, cx: &App) -> Vec<String> {
        self.status_lines(cx).into_iter().map(|(s, _)| s).collect()
    }

    #[cfg(test)]
    pub fn apply_label_for_test(&self) -> String {
        self.apply_label()
    }

    #[cfg(test)]
    pub fn selection_for_test(&self) -> Option<Selection> {
        self.selection.clone()
    }

    #[cfg(test)]
    pub fn set_fields_for_test(&mut self, pattern: &str, replacement: &str, regex: bool, cx: &mut Context<Self>) {
        self.query.update(cx, |f, cx| f.set_text(pattern, cx));
        self.replace.update(cx, |f, cx| f.set_text(replacement, cx));
        self.regex = regex;
    }

    #[cfg(test)]
    pub fn set_preserve_case_for_test(&mut self, on: bool) {
        self.preserve_case = on;
    }

    #[cfg(test)]
    pub fn set_context_for_test(&mut self, n: u8, cx: &mut Context<Self>) {
        self.set_context_lines(n, cx);
    }

    #[cfg(test)]
    pub fn report_lines_for_test(&self) -> Vec<String> {
        let root = self.doc.provenance.root.clone();
        self.report
            .as_ref()
            .map(|r| {
                r.lines(|p| p.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).unwrap_or(p).to_string())
                    .into_iter()
                    .map(|(l, _)| l)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .chain(self.message.as_ref().map(|(m, _)| m.to_string()))
            .collect()
    }

    #[cfg(test)]
    pub fn cursor_for_test(&self) -> Option<usize> {
        self.cursor
    }

    #[cfg(test)]
    pub fn list_focus_for_test(&self) -> FocusHandle {
        self.list_focus.clone()
    }
}

impl SearchSpec {
    /// The query or its options differ (the replacement and context do not change which results a run finds).
    pub fn differs_in_query(&self, other: &SearchSpec) -> bool {
        (&self.pattern, self.regex, self.case_sensitive, self.whole_word, &self.include, &self.exclude)
            != (&other.pattern, other.regex, other.case_sensitive, other.whole_word, &other.include, &other.exclude)
    }
}

/// "Apply 3 changes in 2 files", with singular forms for one change or one file.
pub fn apply_label(changes: usize, files: usize) -> String {
    let changes = crate::i18n::tn("{} change", "{} changes", changes, &[]);
    let files = crate::i18n::tn("in {} file", "in {} files", files, &[]);
    crate::i18n::tf("Apply {0} {1}", &[&changes, &files])
}

/// Rows of a preview: each file, then each change as an old line and a new line.
fn plan_rows(plan: &Plan) -> Vec<PlanRow> {
    let mut rows = Vec::new();
    for (f, file) in plan.files.iter().enumerate() {
        rows.push(PlanRow::File(f));
        for ch in 0..file.changes.len() {
            rows.push(PlanRow::Old(f, ch));
            rows.push(PlanRow::New(f, ch));
        }
    }
    rows
}

/// What the accessibility tree shows of a Search Editor.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessView {
    pub title: String,
    pub query: String,
    pub status: String,
    /// (label, description) of the rows `first_row..`, a window around the keyboard row.
    pub rows: Vec<(String, Option<String>)>,
    pub first_row: usize,
    pub rows_total: usize,
    pub selected: Option<usize>,
    /// The Apply button's label and whether it can be pressed (while a preview is shown).
    pub apply: Option<(String, bool)>,
    pub can_undo: bool,
    pub list_focused: bool,
    pub query_focused: bool,
}

/// Keep `range` of `line` visible within about `DIFF_CHARS` characters: trims around it and adds "…".
fn window_around(line: &str, range: std::ops::Range<usize>) -> (String, std::ops::Range<usize>) {
    let range = range.start.min(line.len())..range.end.min(line.len());
    let lead: usize = line[..range.start].chars().count();
    let (start, prefix) = if lead > 40 {
        let skip = line[..range.start].char_indices().rev().nth(30).map_or(0, |(i, _)| i);
        (skip, "…")
    } else {
        (0, "")
    };
    let end = line[range.end..].char_indices().nth(DIFF_CHARS).map_or(line.len(), |(i, _)| range.end + i);
    let suffix = if end < line.len() { "…" } else { "" };
    let text = format!("{prefix}{}{suffix}", &line[start..end]);
    let shift = prefix.len() as isize - start as isize;
    let r = (range.start as isize + shift) as usize..(range.end as isize + shift) as usize;
    (text, r)
}

impl Render for SearchEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let busy = self.busy.is_some();
        let has_plan = self.plan.is_some();
        let status = self.status_lines(cx);
        let field_box = |id: &'static str, focused: bool, child: gpui::AnyElement| ui::input_box(id, &p, p.bg, focused).child(child);
        let focused = |f: &Entity<TextField>| f.focus_handle(cx).is_focused(window);
        let (q_focus, i_focus, e_focus, r_focus) = (focused(&self.query), focused(&self.include), focused(&self.exclude), focused(&self.replace));
        let button = |id: &'static str, label: String, enabled: bool, primary: bool| {
            let (bg, fg) = match (enabled, primary) {
                (true, true) => (p.accent, p.on_accent),
                (true, false) => (p.surface_hover, p.text),
                (false, _) => (p.surface, p.text_muted),
            };
            div()
                .id(id)
                .flex_none()
                .h(px(24.))
                .px(px(theme::SPACE[4]))
                .flex()
                .items_center()
                .rounded(px(theme::radius::SM))
                .bg(c(bg))
                .text_color(c(fg))
                .text_size(px(text::XS.0))
                .when(enabled, |d| d.cursor_pointer())
                .when(!enabled, |d| d.border_1().border_color(c(p.border)))
                .child(label)
        };
        let toggle = |id: &'static str, label: &'static str, on: bool, tip: &'static str, cx: &mut Context<Self>, set: fn(&mut Self)| {
            ui::option_toggle(id, label, on, tip, &p, p.bg).on_click(cx.listener(move |this, _, _, cx| {
                set(this);
                cx.emit(SearchEditorEvent::Rerun);
                cx.notify();
            }))
        };
        let provenance = format!("{} · {}", self.doc.provenance.host, self.doc.provenance.root);
        let (n_changes, n_files) = self.selected_counts();
        let blocker = self.apply_blocker();
        let can_apply = has_plan && !busy && blocker.is_none() && n_changes > 0;
        let running = self.is_running();

        div()
            .id("search-editor")
            .key_context("SearchEditor")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &crate::search::NextResult, _, cx| this.step_match(1, cx)))
            .on_action(cx.listener(|this, _: &crate::search::PrevResult, _, cx| this.step_match(-1, cx)))
            .on_action(cx.listener(|_, _: &RerunSearch, _, cx| cx.emit(SearchEditorEvent::Rerun)))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .text_size(px(text::SM.0))
            .text_color(c(p.text))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[3]))
                    .px(px(theme::SPACE[6]))
                    .py(px(theme::SPACE[4]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[3]))
                            .child(
                                field_box(
                                    "se-query-box",
                                    q_focus,
                                    div()
                                        .flex()
                                        .flex_1()
                                        .min_w_0()
                                        .items_center()
                                        .gap(px(theme::SPACE[2]))
                                        .child(ui::search_icon(&p))
                                        .child(div().flex_1().min_w_0().child(self.query.clone()))
                                        .child(toggle("se-case", "Aa", self.case_sensitive, crate::i18n::t("Match case"), cx, |s| {
                                            s.case_sensitive = !s.case_sensitive
                                        }))
                                        .child(toggle("se-word", "ab", self.whole_word, crate::i18n::t("Whole word"), cx, |s| s.whole_word = !s.whole_word))
                                        .child(toggle("se-regex", ".*", self.regex, crate::i18n::t("Regular expression"), cx, |s| s.regex = !s.regex))
                                        .into_any_element(),
                                )
                                .flex_1(),
                            )
                            .child(if running {
                                button("se-stop", crate::i18n::t("Stop").into(), true, false)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.stop_search();
                                        cx.emit(SearchEditorEvent::Changed);
                                        cx.notify();
                                    }))
                                    .into_any_element()
                            } else {
                                button("se-rerun", crate::i18n::t("Rerun").into(), !busy, false)
                                    .tooltip(|_, cx| {
                                        cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Run this search again in the same folder").into())).into()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.busy.is_none() {
                                            cx.emit(SearchEditorEvent::Rerun);
                                        }
                                    }))
                                    .into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(theme::SPACE[3]))
                            .child(field_box("se-include-box", i_focus, self.include.clone().into_any_element()).flex_1().min_w(px(160.)))
                            .child(field_box("se-exclude-box", e_focus, self.exclude.clone().into_any_element()).flex_1().min_w(px(160.))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(theme::SPACE[3]))
                            .child(field_box("se-replace-box", r_focus, self.replace.clone().into_any_element()).flex_1().min_w(px(160.)))
                            .child(toggle("se-preserve", "AB", self.preserve_case, crate::i18n::t("Preserve case when replacing"), cx, |s| {
                                s.preserve_case = !s.preserve_case
                            }))
                            .child(
                                button("se-preview", crate::i18n::t("Preview Replace").into(), !busy && !running && !self.doc.files.is_empty(), false)
                                    .tooltip(|_, cx| {
                                        cx.new(|_| {
                                            crate::workbench::Tooltip(crate::i18n::t("Show every change before anything is written (↵ in the field)").into())
                                        })
                                        .into()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.busy.is_none() && !this.is_running() {
                                            cx.emit(SearchEditorEvent::Preview);
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[3]))
                            .text_size(px(text::XS.0))
                            .text_color(c(p.text_muted))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(provenance))
                            .child(crate::i18n::tn("Context: {} line", "Context: {} lines", self.context as usize, &[]))
                            .child(ui::icon_button("se-context-less", "minus", crate::i18n::t("Fewer context lines"), &p).on_click(cx.listener(
                                |this, _, _, cx| {
                                    let n = this.context.saturating_sub(1);
                                    this.set_context_lines(n, cx)
                                },
                            )))
                            .child(ui::icon_button("se-context-more", "plus", crate::i18n::t("More context lines"), &p).on_click(cx.listener(
                                |this, _, _, cx| {
                                    let n = this.context + 1;
                                    this.set_context_lines(n, cx)
                                },
                            )))
                            .child(
                                ui::icon_button("se-copy", "copy", crate::i18n::t("Copy the results as text"), &p)
                                    .on_click(cx.listener(|this, _, _, cx| this.copy_text(cx))),
                            ),
                    ),
            )
            .children(status.into_iter().enumerate().map(|(i, (s, warn))| {
                div()
                    .id(("se-status", i))
                    .flex_none()
                    .px(px(theme::SPACE[6]))
                    .py(px(theme::SPACE[2]))
                    .text_size(px(text::XS.0))
                    .text_color(c(if warn { p.warning } else { p.text_muted }))
                    .child(s)
            }))
            .when_some(self.message.clone(), |d, (m, err)| {
                d.child(
                    div()
                        .id("se-message")
                        .flex_none()
                        .px(px(theme::SPACE[6]))
                        .py(px(theme::SPACE[2]))
                        .text_size(px(text::XS.0))
                        .text_color(c(if err { p.danger } else { p.text_muted }))
                        .child(m),
                )
            })
            .when_some(self.report.as_ref(), |d, report| {
                let root = self.doc.provenance.root.clone();
                let name = move |path: &str| path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).unwrap_or(path).to_string();
                d.child(
                    div()
                        .id("se-report")
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE[1]))
                        .px(px(theme::SPACE[6]))
                        .py(px(theme::SPACE[3]))
                        .border_b_1()
                        .border_color(c(p.border))
                        .text_size(px(text::XS.0))
                        .children(report.lines(name).into_iter().take(50).map(|(l, bad)| div().text_color(c(if bad { p.warning } else { p.success })).child(l)))
                        .child(
                            div()
                                .text_color(c(p.text_muted))
                                .child(crate::i18n::t("Each file is written on its own; files that were not changed keep their contents.")),
                        )
                        .when(self.can_undo, |d| {
                            d.child(div().flex().child(button("se-undo", crate::i18n::t("Undo Replace").into(), !busy, false).on_click(cx.listener(
                                |this, _, _, cx| {
                                    if this.busy.is_none() {
                                        cx.emit(SearchEditorEvent::Undo);
                                    }
                                },
                            ))))
                        }),
                )
            })
            .when(has_plan, |d| {
                d.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(theme::SPACE[3]))
                        .px(px(theme::SPACE[6]))
                        .py(px(theme::SPACE[3]))
                        .border_b_1()
                        .border_color(c(p.border))
                        .child(button("se-apply", self.apply_label(), can_apply, true).on_click(cx.listener(|this, _, _, cx| {
                            if this.busy.is_none() && this.apply_blocker().is_none() && this.selected_counts().0 > 0 {
                                cx.emit(SearchEditorEvent::Apply);
                            }
                        })))
                        .child(if busy {
                            button("se-cancel-apply", crate::i18n::t("Stop").into(), true, false)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEditorEvent::Cancel)))
                                .into_any_element()
                        } else {
                            button("se-close-preview", crate::i18n::t("Close Preview").into(), true, false)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_preview(cx)))
                                .into_any_element()
                        })
                        .child(div().text_size(px(text::XS.0)).text_color(c(if blocker.is_some() { p.warning } else { p.text_muted })).child(
                            blocker.unwrap_or_else(|| {
                                crate::i18n::tf(
                                    "{0} of {1} changes in {2} files selected. Nothing is written before Apply.",
                                    &[&n_changes, &self.plan.as_ref().map_or(0, |pl| pl.files.iter().map(|f| f.changes.len()).sum::<usize>()), &n_files],
                                )
                            }),
                        )),
                )
            })
            .child(
                div()
                    .id("se-list")
                    .key_context("SearchEditorList")
                    .track_focus(&self.list_focus)
                    .on_action(cx.listener(|this, _: &SelectNextRow, _, cx| this.move_cursor(1, cx)))
                    .on_action(cx.listener(|this, _: &SelectPrevRow, _, cx| this.move_cursor(-1, cx)))
                    .on_action(cx.listener(|this, _: &ActivateRow, _, cx| {
                        if let Some(i) = this.cursor {
                            this.activate(i, cx)
                        }
                    }))
                    .on_action(cx.listener(|this, _: &ToggleRow, _, cx| {
                        if let Some(i) = this.cursor {
                            this.toggle_row(i, cx)
                        }
                    }))
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "se-rows",
                            self.row_count(),
                            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                                let list_focused = this.list_focus.is_focused(window);
                                range.map(|ix| this.render_row(ix, list_focused, &p, cx)).collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(self.scroll.clone())
                        .size_full(),
                    ),
            )
    }
}

impl SearchEditor {
    fn render_row(&self, ix: usize, list_focused: bool, p: &theme::Palette, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = *p;
        let current = self.cursor == Some(ix);
        let base = div()
            .id(("se-row", ix))
            .debug_selector(move || format!("se-row-{ix}"))
            .w_full()
            .h(px(layout::ROW_H))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[3]))
            .px(px(theme::SPACE[6]))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .when(current, |d| d.bg(c(if list_focused { ui::focused_selection(&p) } else { p.surface_active })))
            .when(!current, |d| d.hover(|d| d.bg(c(p.surface_hover))))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.list_focus);
                    this.activate(ix, cx);
                }),
            );
        let line_no = |n: u32| div().flex_none().w(px(44.)).text_size(px(text::XS.0)).text_color(c(p.line_number)).child(n.to_string());
        if let Some(plan) = &self.plan {
            let sel = self.selection.as_ref();
            return match self.plan_rows.get(ix).copied() {
                Some(PlanRow::File(f)) => {
                    let Some(file) = plan.files.get(f) else { return base.into_any_element() };
                    let state = sel.map_or(Tri::None, |s| s.state(f));
                    let name = self.doc.display_path(&file.path).to_string();
                    base.child(checkbox(("se-file-check", f), state, file.problem.is_none(), &p).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_file(f, cx)
                    })))
                    .child(crate::icon_theme::file_icon_el(ui::file_name(&file.path), layout::ICON, &p))
                    .child(div().flex_none().child(name))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(text::XS.0))
                            .text_color(c(if file.problem.is_some() { p.warning } else { p.text_muted }))
                            .child(match &file.problem {
                                Some(problem) => problem.message(),
                                None => {
                                    crate::i18n::tf("{0} of {1} changes", &[&sel.map_or(0, |s| s.file(f).iter().filter(|x| **x).count()), &file.changes.len()])
                                }
                            }),
                    )
                    .into_any_element()
                }
                Some(PlanRow::Old(f, ch)) => {
                    let Some(file) = plan.files.get(f) else { return base.into_any_element() };
                    let s = sel.map_or(&[][..], |s| s.file(f));
                    let on = s.get(ch).copied().unwrap_or(false);
                    let (old, _, range, _) = file.line_diff(ch, s);
                    let (text, r) = window_around(&old, range);
                    let hl = HighlightStyle {
                        background_color: Some(c(p.diff_removed).into()),
                        strikethrough: on.then(|| gpui::StrikethroughStyle { thickness: px(1.), color: Some(c(p.text).into()) }),
                        ..Default::default()
                    };
                    base.child(div().w(px(16.)))
                        .child(checkbox(("se-change-check", ix), if on { Tri::All } else { Tri::None }, file.problem.is_none(), &p).on_click(cx.listener(
                            move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_change(f, ch, cx)
                            },
                        )))
                        .child(line_no(file.changes.get(ch).map_or(0, |c| c.line)))
                        .child(div().flex_none().text_color(c(p.danger)).child("−"))
                        .child(div().min_w_0().child(StyledText::new(text).with_highlights([(r, hl)])))
                        .into_any_element()
                }
                Some(PlanRow::New(f, ch)) => {
                    let Some(file) = plan.files.get(f) else { return base.into_any_element() };
                    let s = sel.map_or(&[][..], |s| s.file(f));
                    let on = s.get(ch).copied().unwrap_or(false);
                    let (_, new, _, range) = file.line_diff(ch, s);
                    let (text, r) = window_around(&new, range);
                    let hl = HighlightStyle { background_color: Some(c(if on { p.diff_added } else { p.surface_hover }).into()), ..Default::default() };
                    base.child(div().w(px(16. + 16. + 44. + theme::SPACE[3] * 2.)))
                        .child(div().flex_none().text_color(c(if on { p.success } else { p.text_muted })).child(if on { "+" } else { "=" }))
                        .child(div().min_w_0().text_color(c(if on { p.text } else { p.text_muted })).child(StyledText::new(text).with_highlights([(r, hl)])))
                        .into_any_element()
                }
                None => base.into_any_element(),
            };
        }
        match self.rows.get(ix).copied() {
            Some(Row::File(f)) => {
                let Some(file) = self.doc.files.get(f) else { return base.into_any_element() };
                let shown = self.doc.display_path(&file.path);
                let (dir, name) = shown.rsplit_once('/').unwrap_or(("", &shown));
                base.child(icon(if file.collapsed { "chevron-right" } else { "chevron-down" }, layout::ICON, p.text_muted))
                    .child(crate::icon_theme::file_icon_el(name, layout::ICON, &p))
                    .child(div().flex_none().child(name.to_string()))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(dir.to_string()))
                    .child(
                        div()
                            .flex_none()
                            .px(px(theme::SPACE[2]))
                            .rounded(px(8.))
                            .bg(c(p.surface_hover))
                            .text_size(px(text::XXS.0))
                            .text_color(c(p.text_muted))
                            .child(file.matches.len().to_string()),
                    )
                    .into_any_element()
            }
            Some(Row::Match(f, m)) => {
                let Some(dm) = self.doc.files.get(f).and_then(|x| x.matches.get(m)) else { return base.into_any_element() };
                let (preview, ranges) = crate::search::trim_preview(&dm.preview, &dm.ranges);
                let highlights =
                    ranges.into_iter().map(|r| (r, HighlightStyle { background_color: Some(ui::ca(p.warning, 0.28).into()), ..Default::default() }));
                base.pl(px(theme::SPACE[6] + layout::ICON))
                    .child(line_no(dm.line))
                    .child(div().min_w_0().text_color(c(p.text)).child(StyledText::new(preview).with_highlights(highlights)))
                    .into_any_element()
            }
            Some(Row::Context(f, m, line)) => base
                .pl(px(theme::SPACE[6] + layout::ICON))
                .child(line_no(line))
                .child(div().min_w_0().text_color(c(p.text_muted)).child(self.doc.context_text(f, m, line).to_string()))
                .into_any_element(),
            None => base.into_any_element(),
        }
    }
}

/// A 14 px checkbox: checked, partly checked or empty; dimmed when it cannot be changed.
fn checkbox(id: impl Into<gpui::ElementId>, state: Tri, enabled: bool, p: &theme::Palette) -> gpui::Stateful<gpui::Div> {
    let glyph = match state {
        Tri::All => "check",
        Tri::Some => "minus",
        Tri::None => "",
    };
    let filled = state != Tri::None && enabled;
    div()
        .id(id)
        .flex_none()
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(c(if enabled { p.border_input } else { p.border }))
        .when(filled, |d| d.bg(c(p.accent)).border_color(c(p.accent)))
        .when(!glyph.is_empty(), |d| d.child(icon(glyph, 11., if filled { p.on_accent } else { p.text_muted })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn apply_label_has_singular_and_plural_forms() {
        assert_eq!(super::apply_label(1, 1), "Apply 1 change in 1 file");
        assert_eq!(super::apply_label(2, 1), "Apply 2 changes in 1 file");
        assert_eq!(super::apply_label(3, 2), "Apply 3 changes in 2 files");
        assert_eq!(super::apply_label(0, 0), "Apply 0 changes in 0 files");
    }

    #[test]
    fn diff_window_keeps_the_change_visible() {
        let line = format!("{}needle{}", "x".repeat(100), "y".repeat(400));
        let (text, r) = super::window_around(&line, 100..106);
        assert!(text.starts_with('…') && text.ends_with('…'));
        assert_eq!(&text[r], "needle");
        let (short, r) = super::window_around("a needle b", 2..8);
        assert_eq!((short.as_str(), &"a needle b"[2..8]), ("a needle b", "needle"));
        assert_eq!(r, 2..8);
        // A range past the end is clamped instead of panicking.
        let (_, r) = super::window_around("ab", 5..9);
        assert_eq!(r, 2..2);
    }
}
