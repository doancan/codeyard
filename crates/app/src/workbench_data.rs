//! Workbench side of the data lens (`datalens.rs` has the logic): "Data: Convert To…", the live query bar,
//! "Data: Copy Path at Cursor" with its status bar item, and "Data: Decode Selection…".

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{AnyElement, AppContext as _, ClipboardItem, Context, Entity, Focusable as _, Subscription, Window, div, prelude::*, px};
use serde_json::Value;

use super::{DataConvert, DataCopyPath, DataDecode, DataQuery, Notice, Overlay, OverlayKind, Workbench};
use crate::datalens::{self, Format, MAX_QUERY_BYTES, Options, QueryError, QueryOutput};
use crate::editor::DataSource;
use crate::i18n::{t, tf, tn};
use crate::input::{FieldEvent, TextField};
use crate::picker::{Pick, PickItem, Picker, PickerEvent, Source as PickSource};
use crate::ui::{self, Palette, c, icon, text};

/// The status bar path is worked out while rendering, so only for files small enough to parse on every cursor move.
const PATH_LIMIT_BYTES: usize = 512 << 10;
/// Wait after the last key before a query runs.
const QUERY_DEBOUNCE_MS: u64 = 100;

#[derive(Default)]
pub(super) struct DataState {
    query: Option<QueryBar>,
    /// (file, text version, cursor) -> label, so a repaint does not parse the file again.
    path_cache: RefCell<Option<((String, u64, usize), Option<String>)>>,
}

struct QueryBar {
    field: Entity<TextField>,
    _sub: Subscription,
    /// Path of the editor that is queried.
    source: String,
    /// Parsed document and the editor version it was parsed from.
    doc: Option<Arc<Value>>,
    version: Option<u64>,
    status: Status,
    /// Counts query runs; a run whose number is no longer `latest` stops itself.
    seq: u64,
    latest: Arc<AtomicU64>,
    task: Option<gpui::Task<()>>,
    /// The `query.json` tab that shows the results.
    results_path: Option<String>,
}

enum Status {
    Reading,
    Idle,
    Done {
        results: usize,
        truncated: bool,
    },
    /// The file itself does not parse (or is too large).
    Broken(String),
    /// The query does not parse or failed while running; `column` is 1-based for syntax errors.
    Failed {
        column: Option<usize>,
        message: String,
    },
}

/// One row of the "Convert To…" list.
#[derive(Clone, Copy)]
pub(super) struct Target {
    format: Format,
    replace: bool,
    /// .env only: nested values become `A__B=…`.
    flatten_env: bool,
}

pub(super) struct ConvertJob {
    targets: Vec<Target>,
    path: String,
    source: DataSource,
}

/// What a query run needs, gathered on the UI thread.
struct QueryJob {
    query: String,
    opts: Options,
    doc: Option<Arc<Value>>,
    /// The file changed since `doc` was parsed: its text, format and editor version.
    fresh: Option<(String, Option<Format>, u64)>,
}

struct Outcome {
    parsed: Option<(u64, Result<Arc<Value>, String>)>,
    query: Option<Result<QueryOutput, QueryError>>,
}

impl QueryJob {
    fn run(self, latest: &AtomicU64, seq: u64) -> Outcome {
        let mut doc = self.doc;
        let mut parsed = None;
        if let Some((text, hint, version)) = self.fresh {
            match datalens::parse_any(&text, hint) {
                Ok(loaded) => {
                    doc = Some(Arc::new(loaded.value));
                    parsed = Some((version, Ok(doc.clone().expect("just set"))));
                }
                Err(e) => {
                    doc = None;
                    parsed = Some((version, Err(e.to_string())));
                }
            }
        }
        let cancelled = || latest.load(Ordering::SeqCst) != seq;
        let query = match &doc {
            Some(d) if !self.query.trim().is_empty() => Some(datalens::run_query(d, &self.query, &self.opts, &cancelled)),
            _ => None,
        };
        Outcome { parsed, query }
    }
}

fn options(cx: &gpui::App) -> Options {
    let s = cx.global::<crate::settings::Settings>();
    Options { indent: s.data_indent, sort_keys: s.data_sort_keys, flatten_env: false }
}

impl Workbench {
    // --- Convert ---

    /// "Data: Convert To…": pick the format (and new tab or replace), then convert the selection or the whole file.
    pub(super) fn data_convert(&mut self, _: &DataConvert, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else {
            return self.data_notice(Notice::Info(t("Open a file first.").into()), cx);
        };
        if self.overlay.is_some() {
            return;
        }
        let (path, source) = {
            let e = ed.read(cx);
            (e.path.clone(), e.data_source())
        };
        let mut targets = Vec::new();
        for replace in [false, true] {
            for format in Format::ALL {
                targets.push(Target { format, replace, flatten_env: false });
            }
            targets.push(Target { format: Format::Env, replace, flatten_env: true });
        }
        let items = targets
            .iter()
            .map(|target| {
                let name = match (target.format, target.flatten_env) {
                    (Format::Env, true) => t(".env, nested keys joined with __").to_string(),
                    (f, _) => f.label().to_string(),
                };
                PickItem {
                    label: if target.replace { tf("Replace the text with {}", &[&name]) } else { tf("{} in a new tab", &[&name]) }.into(),
                    detail: None,
                    keys: None,
                }
            })
            .collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], t("Convert to…"), "", None, cx));
        self.show_overlay(picker, OverlayKind::DataConvert(Box::new(ConvertJob { targets, path, source })), window, cx);
    }

    fn run_convert(&mut self, job: ConvertJob, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&target) = job.targets.get(index) else { return };
        let opts = Options { flatten_env: target.flatten_env, ..options(cx) };
        // A selection can be anything (a snippet inside a Markdown file), so only a whole file goes by its name.
        let hint = if job.source.range.is_none() { Format::from_path(&job.path) } else { None };
        let converted = datalens::parse_any(&job.source.text, hint).and_then(|loaded| {
            let out = datalens::emit(&loaded.value, target.format, &opts)?;
            let mut notes = loaded.notes;
            for n in out.notes {
                if !notes.contains(&n) {
                    notes.push(n);
                }
            }
            Ok((out.text, notes))
        });
        match converted {
            Ok((out, notes)) => {
                if target.replace {
                    let editor = self.all_editors().into_iter().find(|e| e.read(cx).path == job.path);
                    if let Some(ed) = editor {
                        ed.update(cx, |e, cx| e.replace_with_generated(job.source.range.clone(), &out, cx));
                    }
                } else {
                    let stem = ui::file_name(&job.path).rsplit_once('.').map_or("", |(s, _)| s);
                    let name = format!("{}.{}", if stem.is_empty() { "converted" } else { stem }, target.format.extension());
                    self.create_scratch_with(name, &out, window, cx);
                }
                let note = if notes.is_empty() {
                    tf("Converted to {}", &[&target.format.label()])
                } else {
                    notes.iter().map(|n| n.text()).collect::<Vec<_>>().join(" ")
                };
                self.data_notice(Notice::Info(note.into()), cx);
            }
            Err(e) => {
                // Take the user to the line when the whole file was converted.
                if let (Some((line, _)), None) = (e.at, &job.source.range)
                    && let Some(ed) = self.all_editors().into_iter().find(|ed| ed.read(cx).path == job.path)
                {
                    ed.update(cx, |ed, cx| ed.go_to_line(line, window, cx));
                }
                self.data_notice(Notice::Error(e.to_string().into()), cx);
            }
        }
    }

    fn data_notice(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.notice = Some(notice);
        cx.notify();
    }

    // --- Query ---

    /// "Data: Query…": a bar above the editor; results go to a `query.json` tab as you type.
    pub(super) fn data_query(&mut self, _: &DataQuery, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bar) = &self.data.query {
            window.focus(&bar.field.focus_handle(cx));
            return;
        }
        let Some(ed) = self.active_editor().cloned() else {
            return self.data_notice(Notice::Info(t("Open a file first.").into()), cx);
        };
        let (path, bytes) = {
            let e = ed.read(cx);
            (e.path.clone(), e.data_len_bytes())
        };
        if bytes > MAX_QUERY_BYTES {
            let message = tf("This file is {0} MB; Data: Query handles files up to {1} MB.", &[&(bytes >> 20), &(MAX_QUERY_BYTES >> 20)]);
            return self.data_notice(Notice::Error(message.into()), cx);
        }
        let field = cx.new(|cx| TextField::new(crate::i18n::tr("Query, e.g. .items[].name"), cx));
        let sub = cx.subscribe_in(&field, window, |this, _, event: &FieldEvent, window, cx| match event {
            FieldEvent::Changed => this.schedule_query(QUERY_DEBOUNCE_MS, window, cx),
            FieldEvent::Submit => this.close_query(true, window, cx),
            FieldEvent::Escape => this.close_query(false, window, cx),
        });
        window.focus(&field.focus_handle(cx));
        self.data.query = Some(QueryBar {
            field,
            _sub: sub,
            source: path,
            doc: None,
            version: None,
            status: Status::Reading,
            seq: 0,
            latest: Arc::new(AtomicU64::new(0)),
            task: None,
            results_path: None,
        });
        // Read the file now so it is parsed by the time the first character is typed.
        self.schedule_query(0, window, cx);
        cx.notify();
    }

    /// Close the bar; the keyboard goes back to the editor (the results tab when `to_results`).
    fn close_query(&mut self, to_results: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.data.query.take() else { return };
        let target = bar.results_path.as_ref().filter(|_| to_results).and_then(|p| self.all_editors().into_iter().find(|e| &e.read(cx).path == p));
        match target.or_else(|| self.active_editor().cloned()) {
            Some(ed) => window.focus(&ed.focus_handle(cx)),
            None => window.focus(&self.focus_handle),
        }
        cx.notify();
    }

    fn schedule_query(&mut self, delay_ms: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.data.query.as_mut() else { return };
        bar.seq += 1;
        let seq = bar.seq;
        let latest = bar.latest.clone();
        latest.store(seq, Ordering::SeqCst);
        bar.task = Some(cx.spawn_in(window, async move |this, cx| {
            if delay_ms > 0 {
                cx.background_executor().timer(Duration::from_millis(delay_ms)).await;
            }
            let Ok(Some(job)) = this.update(cx, |this, cx| this.take_query_job(seq, cx)) else { return };
            let flag = latest.clone();
            let outcome = cx.background_executor().spawn(async move { job.run(&flag, seq) }).await;
            this.update_in(cx, |this, window, cx| this.finish_query(seq, outcome, window, cx)).ok();
        }));
    }

    /// On the UI thread: the query text and, when the file changed since it was last parsed, its text.
    fn take_query_job(&mut self, seq: u64, cx: &mut Context<Self>) -> Option<QueryJob> {
        let (source, current) = self.data.query.as_ref().filter(|b| b.seq == seq).map(|b| (b.source.clone(), b.version))?;
        let ed = self.all_editors().into_iter().find(|e| e.read(cx).path == source);
        let bar = self.data.query.as_mut()?;
        let Some(ed) = ed else {
            bar.status = Status::Broken(t("The file is not open any more.").to_string());
            cx.notify();
            return None;
        };
        let (version, bytes) = {
            let e = ed.read(cx);
            (e.version(), e.data_len_bytes())
        };
        let fresh = if current == Some(version) {
            None
        } else if bytes > MAX_QUERY_BYTES {
            bar.status = Status::Broken(tf("This file is {0} MB; Data: Query handles files up to {1} MB.", &[&(bytes >> 20), &(MAX_QUERY_BYTES >> 20)]));
            cx.notify();
            return None;
        } else {
            let hint = Format::from_path(&source);
            Some((ed.read(cx).data_source().text, hint, version))
        };
        if bar.doc.is_none() || fresh.is_some() {
            bar.status = Status::Reading;
        }
        let query = bar.field.read(cx).text().to_string();
        let job = QueryJob { query, opts: options(cx), doc: self.data.query.as_ref().and_then(|b| b.doc.clone()), fresh };
        cx.notify();
        Some(job)
    }

    fn finish_query(&mut self, seq: u64, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.data.query.as_mut() else { return };
        let mut broken = None;
        if let Some((version, parsed)) = outcome.parsed {
            bar.version = Some(version);
            match parsed {
                Ok(doc) => bar.doc = Some(doc),
                Err(message) => {
                    bar.doc = None;
                    broken = Some(message);
                }
            }
        }
        // A newer run is already on its way; only the file's own state is taken from this one.
        if bar.seq != seq {
            return;
        }
        match (broken, outcome.query) {
            (Some(message), _) => bar.status = Status::Broken(message),
            (None, None) => bar.status = if bar.doc.is_some() { Status::Idle } else { Status::Broken(t("The file could not be read.").to_string()) },
            (None, Some(Ok(out))) => {
                bar.status = Status::Done { results: out.results, truncated: out.truncated };
                self.show_query_results(out.text, window, cx);
            }
            // The run was stopped because a newer one started.
            (None, Some(Err(e))) if e.message.is_empty() => {}
            (None, Some(Err(e))) => {
                let column = e.offset.map(|o| bar.field.read(cx).text().get(..o).map_or(o, |s| s.chars().count()) + 1);
                bar.status = Status::Failed { column, message: e.message };
            }
        }
        cx.notify();
    }

    /// Put the results into the `query.json` tab, opening it the first time.
    fn show_query_results(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let existing =
            self.data.query.as_ref().and_then(|b| b.results_path.as_ref()).and_then(|p| self.all_editors().into_iter().find(|e| &e.read(cx).path == p));
        if let Some(ed) = existing {
            ed.update(cx, |e, cx| {
                e.replace_with_generated(None, &text, cx);
                // The tab is a scratch file: keep it saved so closing it does not ask about unsaved changes.
                e.save_now(cx);
            });
            return;
        }
        let Some(path) = self.create_scratch_with("query.json".into(), &text, window, cx) else { return };
        let path = ui::local_path(std::path::Path::new(&path));
        let Some(bar) = self.data.query.as_mut() else { return };
        bar.results_path = Some(path);
        // Opening the tab takes the keyboard; give it back to the query field.
        let field = bar.field.clone();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            this.update_in(cx, |this, window, cx| {
                if this.data.query.is_some() {
                    window.focus(&field.focus_handle(cx));
                }
            })
            .ok();
        })
        .detach();
    }

    /// The bar above the editor, while a query is open.
    pub(super) fn data_query_bar(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.data.query.as_ref()?;
        let (message, color) = match &bar.status {
            Status::Reading => (t("Reading the file…").to_string(), p.text_muted),
            Status::Idle => (t("Type a query. Esc closes the bar.").to_string(), p.text_muted),
            Status::Done { results, truncated } => {
                let n = tn("{} result", "{} results", *results, &[]);
                (if *truncated { tf("{} (cut at 5 MB)", &[&n]) } else { n }, p.text_muted)
            }
            Status::Broken(m) => (m.clone(), p.danger),
            Status::Failed { column: Some(col), message } => (tf("Column {0}: {1}", &[col, message]), p.danger),
            Status::Failed { column: None, message } => (message.clone(), p.danger),
        };
        Some(
            div()
                .id("data-query-bar")
                .flex_none()
                .h(px(34.))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .px(px(theme::SPACE[4]))
                .bg(c(p.surface))
                .border_b_1()
                .border_color(c(p.border))
                .text_size(px(text::XS.0))
                .child(icon("code", 14., p.text_muted))
                .child(div().flex_none().text_color(c(p.text_muted)).child(t("Query")))
                .child({
                    let handle = bar.field.focus_handle(cx);
                    let press = handle.clone();
                    div().flex_1().min_w_0().flex().child(bar.field.clone()).relative().child(crate::a11y_controls::control(
                        crate::a11y_controls::Control::new(t("Query"), "field", accesskit::Role::TextInput, t("Query"))
                            .value(bar.field.read(cx).text().to_string())
                            .focus(handle)
                            .on_press(move |window, _| window.focus(&press)),
                    ))
                })
                .child(
                    div()
                        .flex_none()
                        .max_w(px(460.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(c(color))
                        .relative()
                        .child(crate::a11y_controls::said(t("Query"), "status", accesskit::Role::Status, message.clone()))
                        .child(message),
                )
                .child(
                    div()
                        .id("data-query-close")
                        .relative()
                        .child(self.ctl(t("Query"), "close", t("Close"), cx, |this, w, cx| this.close_query(false, w, cx)))
                        .flex_none()
                        .cursor_pointer()
                        .child(icon("x", 14., p.text_muted))
                        .on_click(cx.listener(|this, _, window, cx| this.close_query(false, window, cx))),
                )
                .into_any_element(),
        )
    }

    // --- Path at the cursor ---

    /// "Data: Copy Path at Cursor": `.servers[2].host` for the JSON, YAML or TOML value under the cursor.
    pub(super) fn data_copy_path(&mut self, _: &DataCopyPath, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else {
            return self.data_notice(Notice::Info(t("Open a file first.").into()), cx);
        };
        let (file, (text, offset)) = {
            let e = ed.read(cx);
            (e.path.clone(), e.data_text_and_cursor())
        };
        let Some(format) = Format::from_path(&file).filter(|f| *f != Format::Env) else {
            return self.data_notice(Notice::Info(t("Paths are shown for JSON, YAML and TOML files").into()), cx);
        };
        match datalens::path_at(&text, format, offset) {
            Ok(path) => {
                cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                self.data_notice(Notice::Info(tf("Copied {}", &[&path]).into()), cx);
            }
            Err(e) => self.data_notice(Notice::Error(e.to_string().into()), cx),
        }
    }

    /// Text of the status bar item: the path under the cursor of a JSON, YAML or TOML file, if it can be told.
    pub(crate) fn data_path_label(&self, cx: &gpui::App) -> Option<String> {
        let e = self.active_editor()?.read(cx);
        let format = Format::from_path(&e.path).filter(|f| *f != Format::Env)?;
        if e.data_len_bytes() > PATH_LIMIT_BYTES {
            return None;
        }
        let (text, offset) = e.data_text_and_cursor();
        let key = (e.path.clone(), e.version(), offset);
        if let Some((k, label)) = &*self.data.path_cache.borrow()
            && *k == key
        {
            return label.clone();
        }
        let label = datalens::path_at(&text, format, offset).ok().map(|p| shorten(&p));
        *self.data.path_cache.borrow_mut() = Some((key, label.clone()));
        label
    }

    // --- Decode ---

    /// "Data: Decode Selection…": a JWT or Unix timestamps in a read-only list.
    pub(super) fn data_decode(&mut self, _: &DataDecode, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.is_some() {
            return;
        }
        let selected = self.active_editor().and_then(|e| e.read(cx).selected_text());
        let Some(selected) = selected else {
            return self.data_notice(Notice::Info(t("Select a JWT or a Unix timestamp first").into()), cx);
        };
        match crate::datalens_decode::decode(&selected) {
            Ok(lines) => {
                let items = lines.into_iter().map(|l| PickItem { label: if l.is_empty() { " ".into() } else { l.into() }, detail: None, keys: None }).collect();
                let picker =
                    cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], t("Decoded here only: nothing is sent or logged"), "", None, cx));
                self.show_overlay(picker, OverlayKind::DataDecode, window, cx);
            }
            Err(e) => self.data_notice(Notice::Error(e.to_string().into()), cx),
        }
    }

    /// Picker events of the data overlays; false when the overlay is not one of them.
    pub(super) fn on_data_overlay_event(&mut self, event: &PickerEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(Overlay { kind, .. }) = &self.overlay else { return false };
        match (kind, event) {
            (OverlayKind::DataConvert(_), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let index = *index;
                if let Some(Overlay { kind: OverlayKind::DataConvert(job), .. }) = self.close_overlay(window, cx) {
                    self.run_convert(*job, index, window, cx);
                }
                true
            }
            (OverlayKind::DataDecode, PickerEvent::Confirm(_)) => {
                self.close_overlay(window, cx);
                true
            }
            _ => false,
        }
    }

    #[cfg(test)]
    pub fn query_text_for_test(&mut self, query: &str, cx: &mut Context<Self>) {
        if let Some(bar) = &self.data.query {
            bar.field.update(cx, |f, cx| f.set_text(query, cx));
        }
    }

    /// The bar's message, or `None` when it is closed.
    #[cfg(test)]
    pub fn query_status_for_test(&self) -> Option<String> {
        Some(match &self.data.query.as_ref()?.status {
            Status::Reading => "reading".into(),
            Status::Idle => "idle".into(),
            Status::Done { results, truncated } => format!("{results} results{}", if *truncated { " cut" } else { "" }),
            Status::Broken(m) => format!("broken: {m}"),
            Status::Failed { column, message } => format!("failed {column:?}: {message}"),
        })
    }

    #[cfg(test)]
    pub fn convert_for_test(&mut self, format: Format, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let (path, source) = {
            let e = ed.read(cx);
            (e.path.clone(), e.data_source())
        };
        let targets = vec![Target { format, replace, flatten_env: false }];
        self.run_convert(ConvertJob { targets, path, source }, 0, window, cx);
    }
}

/// Long paths keep their end, which is the part that tells values apart.
fn shorten(path: &str) -> String {
    const MAX: usize = 48;
    let n = path.chars().count();
    if n <= MAX { path.to_string() } else { format!("…{}", path.chars().skip(n - MAX + 1).collect::<String>()) }
}
