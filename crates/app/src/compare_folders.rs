//! Compare Folders: a tab listing what differs between two folders (only on one side, changed, identical).
//! Both folders are scanned by their agents, the lists stream in and are matched by `compare_logic`; this file owns the scan
//! and the list. Copying, deleting and opening a diff are asked of the workbench through `CompareEvent`.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::{FutureExt, StreamExt};
use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, SharedString, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list};
use host::Host;
use proto::{Kind, Response};

use crate::compare_logic::{self as logic, Counts, Direction, Row, Side, Status, Tree};
use crate::settings::Settings;
use crate::ui::{self, c, ca, icon, palette, text};

/// While entries stream in, the list is rebuilt at most this often (sorting tens of thousands of paths is not free).
const REBUILD_EVERY: Duration = Duration::from_millis(120);
/// Changed files read for the whitespace and line-ending toggles; beyond this the rest stays "changed".
const REFINE_MAX: usize = 2000;
const ROW_H: f32 = 24.;

/// One side of the comparison.
#[derive(Clone)]
pub struct End {
    pub host: Arc<dyn Host>,
    pub path: String,
    /// Shown in the title: "This computer" or the SSH host.
    pub label: String,
    /// Which machine the path is on, to tell when both sides are one folder.
    pub machine: String,
    /// The folder is on this computer (for the trust check: a remote window has no say over it).
    pub local: bool,
}

pub enum CompareEvent {
    OpenDiff(String),
    Copy(String, Direction),
    Delete(String, Side),
    /// Something to tell the user (report copied, export written).
    Message(String),
}

struct Running {
    /// Streams to cancel on the agents when the scan is stopped.
    ids: Vec<(Arc<dyn Host>, u32)>,
    _task: Task<()>,
}

pub struct CompareView {
    ends: [End; 2],
    trees: [Tree; 2],
    done: [bool; 2],
    truncated: [bool; 2],
    /// The connection ended before the side's list was complete: what is shown is partial.
    dropped: [bool; 2],
    failed: [Option<String>; 2],
    same_folder: bool,
    /// Content hashes read so far and wanted in all (both sides together).
    hashing: Option<(usize, usize)>,
    refine: HashMap<String, bool>,
    rows: Vec<Row>,
    shown: Vec<usize>,
    counts: Counts,
    toggled: HashSet<String>,
    last_rebuild: Instant,
    pub show_identical: bool,
    pub ignore_ws: bool,
    pub ignore_eol: bool,
    pub gitignore: bool,
    /// Sides the user allows deleting from.
    pub writable: [bool; 2],
    /// What the workbench is doing to the folders right now ("Copying…"), and the flag that stops it.
    op: Option<(SharedString, Arc<AtomicBool>)>,
    running: Option<Running>,
    refining: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
}

impl EventEmitter<CompareEvent> for CompareView {}

impl Focusable for CompareView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CompareView {
    pub fn new(left: End, right: End, cx: &mut Context<Self>) -> Self {
        let mut view = CompareView {
            ends: [left, right],
            trees: Default::default(),
            done: [false; 2],
            truncated: [false; 2],
            dropped: [false; 2],
            failed: Default::default(),
            same_folder: false,
            hashing: None,
            refine: HashMap::new(),
            rows: Vec::new(),
            shown: Vec::new(),
            counts: Counts::default(),
            toggled: HashSet::new(),
            last_rebuild: Instant::now(),
            show_identical: false,
            ignore_ws: false,
            ignore_eol: false,
            gitignore: true,
            writable: [false; 2],
            op: None,
            running: None,
            refining: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        };
        view.rescan(cx);
        view
    }

    pub fn title(&self) -> String {
        crate::i18n::tf("Compare: {0} ↔ {1}", &[&ui::file_name(&self.ends[0].path), &ui::file_name(&self.ends[1].path)])
    }

    pub fn end(&self, side: Side) -> &End {
        &self.ends[side as usize]
    }

    /// Path of a row's entry on the side's machine.
    pub fn full_path(&self, side: Side, rel: &str) -> String {
        ui::join(&self.ends[side as usize].path, rel)
    }

    pub fn is_busy(&self) -> bool {
        self.op.is_some() || self.scanning()
    }

    pub fn scanning(&self) -> bool {
        self.running.is_some() || self.refining.is_some()
    }

    /// A copy or delete starts: the list locks and "Stop" cancels it through the returned flag.
    pub fn begin_op(&mut self, what: String, cx: &mut Context<Self>) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.op = Some((what.into(), flag.clone()));
        cx.notify();
        flag
    }

    pub fn end_op(&mut self, cx: &mut Context<Self>) {
        self.op = None;
        cx.notify();
    }

    pub fn copy_plan(&self, path: &str, dir: Direction) -> logic::CopyPlan {
        logic::plan_copy(&self.rows, path, dir)
    }

    pub fn delete_plan(&self, path: &str, side: Side) -> Option<logic::DeletePlan> {
        logic::plan_delete(&self.rows, path, side)
    }

    pub fn is_partial(&self) -> bool {
        self.dropped.iter().any(|d| *d)
    }

    pub fn report_text(&self) -> String {
        logic::report(&self.rows, &self.ends[0].label_with_path(), &self.ends[1].label_with_path(), self.show_identical)
    }

    /// Cancel the scan on both agents and forget what was running.
    pub fn stop(&mut self) {
        if let Some((_, flag)) = &self.op {
            flag.store(true, Ordering::Relaxed);
        }
        if let Some(run) = self.running.take() {
            for (host, id) in run.ids {
                host.cancel(id);
            }
        }
        self.refining = None;
        self.hashing = None;
    }

    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        self.stop();
        self.trees = Default::default();
        self.done = [false; 2];
        self.truncated = [false; 2];
        self.dropped = [false; 2];
        self.failed = Default::default();
        self.refine.clear();
        let (a, b) = (&self.ends[0], &self.ends[1]);
        self.same_folder = logic::same_place(&a.machine, &a.path, &b.machine, &b.path);
        if self.same_folder {
            self.done = [true; 2];
            self.rebuild();
            cx.notify();
            return;
        }
        let (ignore, limit) = {
            let s = cx.global::<Settings>();
            (s.compare_ignore.clone(), s.compare_max_files)
        };
        let (mut ids, mut streams) = (Vec::new(), Vec::new());
        for side in 0..2 {
            let end = &self.ends[side];
            match end.host.hash_tree(&end.path, ignore.clone(), self.gitignore, limit, None) {
                Ok(stream) => {
                    ids.push((end.host.clone(), stream.id));
                    streams.push(ui::bridge(stream.rx).map(move |r| (side, r)).boxed());
                }
                Err(e) => {
                    self.failed[side] = Some(e.to_string());
                    self.done[side] = true;
                }
            }
        }
        let task = cx.spawn(async move |this, cx| {
            let mut rx = futures::stream::select_all(streams);
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Some(Some(more)) = rx.next().now_or_never() {
                    batch.push(more);
                }
                if this.update(cx, |this, cx| this.on_listing(batch, cx)).is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| this.listing_ended(cx)).ok();
        });
        self.running = Some(Running { ids, _task: task });
        self.rebuild();
        cx.notify();
    }

    fn on_listing(&mut self, batch: Vec<(usize, Response)>, cx: &mut Context<Self>) {
        for (side, resp) in batch {
            match resp {
                Response::HashChunk(records) => {
                    for rec in records.into_iter().filter_map(logic::sanitize) {
                        self.trees[side].insert(rec.path.clone(), rec);
                    }
                }
                Response::HashDone { truncated, .. } => {
                    self.done[side] = true;
                    self.truncated[side] = truncated;
                }
                Response::Error(e) => {
                    self.failed[side] = Some(e.to_string());
                    self.done[side] = true;
                }
                _ => {}
            }
        }
        if self.last_rebuild.elapsed() >= REBUILD_EVERY {
            self.rebuild();
        }
        cx.notify();
    }

    /// Both listing streams are over: finished, failed, or cut off by a lost connection.
    fn listing_ended(&mut self, cx: &mut Context<Self>) {
        for side in 0..2 {
            if !self.done[side] {
                self.dropped[side] = true;
                self.done[side] = true;
            }
        }
        self.running = None;
        self.rebuild();
        let healthy = self.failed.iter().all(Option::is_none) && !self.is_partial();
        let wanted = if healthy { logic::hash_candidates(&self.trees[0], &self.trees[1]) } else { Vec::new() };
        if wanted.is_empty() {
            self.finished(cx);
        } else {
            self.start_hashing(wanted, cx);
        }
        cx.notify();
    }

    /// Second pass: read the hashes of the files that exist on both sides with the same size.
    fn start_hashing(&mut self, wanted: Vec<String>, cx: &mut Context<Self>) {
        let (ignore, limit) = {
            let s = cx.global::<Settings>();
            (s.compare_ignore.clone(), s.compare_max_files)
        };
        let (mut ids, mut streams) = (Vec::new(), Vec::new());
        for side in 0..2 {
            let end = &self.ends[side];
            match end.host.hash_tree(&end.path, ignore.clone(), self.gitignore, limit, Some(wanted.clone())) {
                Ok(stream) => {
                    ids.push((end.host.clone(), stream.id));
                    streams.push(ui::bridge(stream.rx).map(move |r| (side, r)).boxed());
                }
                Err(e) => self.failed[side] = Some(e.to_string()),
            }
        }
        self.hashing = Some((0, wanted.len() * 2));
        let task = cx.spawn(async move |this, cx| {
            let mut rx = futures::stream::select_all(streams);
            let mut ended = [false; 2];
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Some(Some(more)) = rx.next().now_or_never() {
                    batch.push(more);
                }
                for (side, r) in &batch {
                    ended[*side] |= matches!(r, Response::HashDone { .. } | Response::Error(_));
                }
                if this.update(cx, |this, cx| this.on_hashes(batch, cx)).is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                for (side, ended) in ended.into_iter().enumerate() {
                    this.dropped[side] |= !ended && this.failed[side].is_none();
                }
                this.running = None;
                this.hashing = None;
                this.rebuild();
                this.finished(cx);
                cx.notify();
            })
            .ok();
        });
        self.running = Some(Running { ids, _task: task });
    }

    fn on_hashes(&mut self, batch: Vec<(usize, Response)>, cx: &mut Context<Self>) {
        for (side, resp) in batch {
            match resp {
                Response::HashChunk(records) => {
                    if let Some((done, _)) = &mut self.hashing {
                        *done += records.len();
                    }
                    for rec in records {
                        logic::apply_hash(&mut self.trees[side], rec);
                    }
                }
                Response::Error(e) => self.failed[side] = Some(e.to_string()),
                _ => {}
            }
        }
        if self.last_rebuild.elapsed() >= REBUILD_EVERY {
            self.rebuild();
        }
        cx.notify();
    }

    fn finished(&mut self, cx: &mut Context<Self>) {
        self.refine_changed(cx);
    }

    /// With "ignore whitespace" or "ignore line endings" on, read the changed text files and see whether anything is left.
    fn refine_changed(&mut self, cx: &mut Context<Self>) {
        self.refining = None;
        if !(self.ignore_ws || self.ignore_eol) {
            return;
        }
        let todo: Vec<String> =
            self.rows.iter().filter(|r| r.diffable() && !self.refine.contains_key(&r.path)).take(REFINE_MAX).map(|r| r.path.clone()).collect();
        if todo.is_empty() {
            return;
        }
        let hosts = [self.ends[0].host.clone(), self.ends[1].host.clone()];
        let paths: Vec<(String, [String; 2])> =
            todo.into_iter().map(|rel| (rel.clone(), [self.full_path(Side::Left, &rel), self.full_path(Side::Right, &rel)])).collect();
        let (ws, eol) = (self.ignore_ws, self.ignore_eol);
        let work = cx.background_executor().spawn(async move {
            paths
                .into_iter()
                .filter_map(|(rel, full)| {
                    let a = hosts[0].read(&full[0]).ok()?;
                    let b = hosts[1].read(&full[1]).ok()?;
                    Some((rel, logic::equal_ignoring(&a.0, &b.0, ws, eol)))
                })
                .collect::<Vec<_>>()
        });
        self.refining = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.refine.extend(result);
                this.refining = None;
                this.rebuild();
                cx.notify();
            })
            .ok();
        }));
    }

    fn rebuild(&mut self) {
        let none = HashMap::new();
        let refine = if self.ignore_ws || self.ignore_eol { &self.refine } else { &none };
        self.rows = logic::classify(&self.trees[0], &self.trees[1], self.done, refine);
        self.counts = logic::counts(&self.rows);
        self.shown = logic::visible(&self.rows, self.show_identical, &self.toggled);
        self.last_rebuild = Instant::now();
    }

    fn toggle_fold(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.toggled.remove(path) {
            self.toggled.insert(path.to_string());
        }
        self.shown = logic::visible(&self.rows, self.show_identical, &self.toggled);
        cx.notify();
    }

    fn set_option(&mut self, set: fn(&mut Self), rescan: bool, cx: &mut Context<Self>) {
        set(self);
        if rescan {
            self.rescan(cx);
            return;
        }
        // The text refinement depends on what is ignored: start over with the new choice.
        self.refine.clear();
        self.rebuild();
        self.refine_changed(cx);
        cx.notify();
    }

    fn open_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.shown.get(ix).and_then(|&i| self.rows.get(i)) else { return };
        let path = row.path.clone();
        if row.is_dir() {
            self.toggle_fold(&path, cx);
        } else if row.diffable() {
            cx.emit(CompareEvent::OpenDiff(path));
        }
    }

    fn copy_report(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(self.report_text()));
        cx.emit(CompareEvent::Message(crate::i18n::t("Report copied").to_string()));
    }

    fn export_report(&mut self, cx: &mut Context<Self>) {
        let text = self.report_text();
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir);
        let rx = cx.prompt_for_new_path(&dir, Some("folder-comparison.txt"));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let message = match std::fs::write(&path, text) {
                Ok(()) => crate::i18n::tf("Report saved to {}", &[&path.display()]),
                Err(e) => crate::i18n::tf("Could not save the report: {}", &[&e]),
            };
            this.update(cx, |_, cx| cx.emit(CompareEvent::Message(message))).ok();
        })
        .detach();
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    #[cfg(test)]
    pub fn row(&self, path: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.path == path)
    }

    #[cfg(test)]
    pub fn counts(&self) -> Counts {
        self.counts
    }

    #[cfg(test)]
    pub fn is_same_folder(&self) -> bool {
        self.same_folder
    }

    #[cfg(test)]
    pub fn status_of(&self, path: &str) -> Option<Status> {
        self.row(path).map(|r| r.status)
    }

    #[cfg(test)]
    pub fn shown_paths(&self) -> Vec<String> {
        self.shown.iter().map(|&i| self.rows[i].path.clone()).collect()
    }

    #[cfg(test)]
    pub fn set_options_for_test(&mut self, show_identical: bool, ws: bool, eol: bool, cx: &mut Context<Self>) {
        (self.show_identical, self.ignore_ws, self.ignore_eol) = (show_identical, ws, eol);
        self.set_option(|_| {}, false, cx);
    }

    #[cfg(test)]
    pub fn open_for_test(&mut self, path: &str, cx: &mut Context<Self>) {
        cx.emit(CompareEvent::OpenDiff(path.to_string()));
    }

    #[cfg(test)]
    pub fn request_for_test(&mut self, path: &str, dir: Direction, cx: &mut Context<Self>) {
        cx.emit(CompareEvent::Copy(path.to_string(), dir));
    }
}

/// Status colors nudged until they can be read on the page background.
fn legible(p: &theme::Palette, color: u32) -> u32 {
    ui::ensure_contrast(color, p.text, &[p.bg], 4.5)
}

impl End {
    fn label_with_path(&self) -> String {
        format!("{}: {}", self.label, self.path)
    }
}

fn status_text(row: &Row, scanning: bool) -> String {
    match row.status {
        Status::OnlyLeft => crate::i18n::t("Only left").into(),
        Status::OnlyRight => crate::i18n::t("Only right").into(),
        Status::Changed(why) => format!("{}: {}", crate::i18n::t("Changed"), logic::why_text(why)),
        Status::Identical => crate::i18n::t("Identical").into(),
        Status::Equivalent => crate::i18n::t("Same apart from whitespace").into(),
        Status::Pending if scanning => crate::i18n::t("Checking…").into(),
        Status::Pending => crate::i18n::t("Not compared").into(),
        Status::Nested => String::new(),
        Status::Error => row.error.clone().unwrap_or_else(|| crate::i18n::t("Error").into()),
    }
}

fn side_text(row: &Row, side: Side) -> String {
    let Some(info) = (match side {
        Side::Left => &row.left,
        Side::Right => &row.right,
    }) else {
        return String::new();
    };
    match row.kind {
        Kind::Dir => String::new(),
        Kind::Symlink => format!("→ {}", info.link.as_deref().unwrap_or("?")),
        _ => format!("{} · {}", logic::human_size(info.size), logic::utc_text(info.mtime_ms)),
    }
}

impl Render for CompareView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let scanning = self.scanning();
        let stoppable = self.is_busy();
        let chip = |id: &'static str, label: &'static str, on: bool, cx: &mut Context<Self>, set: fn(&mut Self), rescan: bool| {
            let (bg, fg) = if on { (ui::mix(p.bg, p.accent, 0.26), legible(&p, p.accent)) } else { (p.surface_hover, p.text_muted) };
            div()
                .id(id)
                .h(px(22.))
                .px(px(theme::SPACE[3]))
                .flex()
                .items_center()
                .rounded(px(theme::radius::SM))
                .bg(c(bg))
                .text_color(c(fg))
                .text_size(px(text::XS.0))
                .cursor_pointer()
                .child(crate::i18n::t(label).to_string())
                .on_click(cx.listener(move |this, _, _, cx| this.set_option(set, rescan, cx)))
        };
        let action = |id: &'static str, label: &'static str, cx: &mut Context<Self>, run: fn(&mut Self, &mut Context<Self>)| {
            div()
                .id(id)
                .h(px(22.))
                .px(px(theme::SPACE[3]))
                .flex()
                .items_center()
                .rounded(px(theme::radius::SM))
                .bg(c(p.surface_hover))
                .text_color(c(p.text))
                .text_size(px(text::XS.0))
                .cursor_pointer()
                .child(crate::i18n::t(label).to_string())
                .on_click(cx.listener(move |this, _, _, cx| run(this, cx)))
        };

        let mut notes: Vec<(String, u32)> = Vec::new();
        if self.same_folder {
            notes.push((crate::i18n::t("Both sides are the same folder.").into(), p.info));
        }
        for side in 0..2 {
            let name = &self.ends[side].label;
            if let Some(e) = &self.failed[side] {
                notes.push((crate::i18n::tf("Could not read {0}: {1}", &[&self.ends[side].label_with_path(), e]), p.danger));
            } else if self.dropped[side] {
                notes.push((
                    crate::i18n::tf("The connection to {} was lost during the scan, so the result is partial. Scan again once it is back.", &[name]),
                    p.warning,
                ));
            }
            if self.truncated[side] {
                notes.push((crate::i18n::tf("{} has more entries than compare.folders.maxFiles allows, so the result is partial.", &[name]), p.warning));
            }
        }
        let status_line: String = if let Some((what, _)) = &self.op {
            what.to_string()
        } else if let Some((done, total)) = self.hashing {
            crate::i18n::tf("Comparing contents… {0} of {1}", &[&done, &total])
        } else if self.running.is_some() {
            crate::i18n::tf("Reading folders… {0} and {1} entries", &[&self.trees[0].len(), &self.trees[1].len()])
        } else if self.refining.is_some() {
            crate::i18n::t("Checking whitespace…").into()
        } else {
            crate::i18n::t("Done").into()
        };
        let empty_note = (!scanning && self.shown.is_empty() && notes.is_empty() && !self.same_folder).then(|| crate::i18n::t("No differences."));
        let busy = self.is_busy();
        let writable = self.writable;

        div()
            .key_context("CompareView")
            .track_focus(&self.focus_handle)
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
                            .gap(px(theme::SPACE[4]))
                            .child(icon("git-compare", 14., p.info))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(self.ends[0].label_with_path()))
                            .child(div().text_color(c(p.text_muted)).child("↔"))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(self.ends[1].label_with_path()))
                            .child(if stoppable {
                                action("compare-stop", crate::i18n::tr("Stop"), cx, |this, cx| {
                                    this.stop();
                                    cx.notify();
                                })
                                .into_any_element()
                            } else {
                                action("compare-rescan", crate::i18n::tr("Scan Again"), cx, |this, cx| this.rescan(cx)).into_any_element()
                            })
                            .child(action("compare-copy-report", crate::i18n::tr("Copy Report"), cx, |this, cx| this.copy_report(cx)))
                            .child(action("compare-export", crate::i18n::tr("Export as Text…"), cx, |this, cx| this.export_report(cx))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(theme::SPACE[3]))
                            .child(chip(
                                "compare-identical",
                                crate::i18n::tr("Show identical"),
                                self.show_identical,
                                cx,
                                |v| v.show_identical = !v.show_identical,
                                false,
                            ))
                            .child(chip("compare-ws", crate::i18n::tr("Ignore whitespace"), self.ignore_ws, cx, |v| v.ignore_ws = !v.ignore_ws, false))
                            .child(chip("compare-eol", crate::i18n::tr("Ignore line endings"), self.ignore_eol, cx, |v| v.ignore_eol = !v.ignore_eol, false))
                            .child(chip("compare-gitignore", crate::i18n::tr("Respect .gitignore"), self.gitignore, cx, |v| v.gitignore = !v.gitignore, true))
                            .child(div().w(px(theme::SPACE[4])))
                            .child(chip(
                                "compare-left-writable",
                                crate::i18n::tr("Left is writable"),
                                writable[0],
                                cx,
                                |v| v.writable[0] = !v.writable[0],
                                false,
                            ))
                            .child(chip(
                                "compare-right-writable",
                                crate::i18n::tr("Right is writable"),
                                writable[1],
                                cx,
                                |v| v.writable[1] = !v.writable[1],
                                false,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(theme::SPACE[5]))
                            .text_size(px(text::XS.0))
                            .text_color(c(p.text_muted))
                            .child(logic::summary(&self.counts))
                            .child(div().flex_1())
                            .child(status_line),
                    ),
            )
            .children(notes.into_iter().map(|(note, color)| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .px(px(theme::SPACE[6]))
                    .py(px(theme::SPACE[2]))
                    .bg(ca(color, 0.14))
                    .text_size(px(text::XS.0))
                    .child(icon("alert", 12., legible(&p, color)))
                    .child(note)
            }))
            .when_some(empty_note, |d, note| d.child(div().p(px(theme::SPACE[7])).text_color(c(p.text_muted)).child(note)))
            .child(
                div().flex_1().min_h_0().child(
                    uniform_list(
                        "compare-rows",
                        self.shown.len(),
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            range
                                .filter_map(|ix| {
                                    let row = this.rows.get(*this.shown.get(ix)?)?;
                                    let folded = row.is_dir() && logic::is_folded(row, &this.toggled);
                                    let color = match row.status {
                                        Status::OnlyLeft | Status::Error => p.danger,
                                        Status::OnlyRight => p.success,
                                        Status::Changed(_) => p.warning,
                                        _ => p.text_muted,
                                    };
                                    let can = |side: Side| match side {
                                        Side::Left => row.left.is_some(),
                                        Side::Right => row.right.is_some(),
                                    };
                                    let differs = matches!(row.status, Status::OnlyLeft | Status::OnlyRight | Status::Changed(_))
                                        || (row.is_dir() && row.status == Status::Nested);
                                    let (copy_right, copy_left) = (differs && can(Side::Left) && !busy, differs && can(Side::Right) && !busy);
                                    let (del_left, del_right) = (writable[0] && can(Side::Left) && !busy, writable[1] && can(Side::Right) && !busy);
                                    let path = row.path.clone();
                                    let small = |id: String, label: &'static str, tip: &'static str, on: bool, cx: &mut Context<Self>, ev: RowAction| {
                                        let path = path.clone();
                                        div()
                                            .id(SharedString::from(id))
                                            .w(px(22.))
                                            .h(px(20.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(theme::radius::SM))
                                            .text_color(c(if on { p.text } else { ui::mix(p.bg, p.text_muted, 0.4) }))
                                            .when(on, |d| {
                                                d.cursor_pointer().hover(|s| s.bg(c(p.surface_hover))).on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.stop_propagation();
                                                    cx.emit(match ev {
                                                        RowAction::Copy(dir) => CompareEvent::Copy(path.clone(), dir),
                                                        RowAction::Delete(side) => CompareEvent::Delete(path.clone(), side),
                                                    })
                                                }))
                                            })
                                            .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t(tip).into())).into())
                                            .child(label)
                                    };
                                    Some(
                                        div()
                                            .id(("compare-row", ix))
                                            .w_full()
                                            .h(px(ROW_H))
                                            .px(px(theme::SPACE[6]))
                                            .flex()
                                            .items_center()
                                            .gap(px(theme::SPACE[3]))
                                            .when(row.is_dir() || row.diffable(), |d| d.cursor_pointer().hover(|s| s.bg(c(p.surface_hover))))
                                            .on_click(cx.listener(move |this, _, _, cx| this.open_row(ix, cx)))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(theme::SPACE[2]))
                                                    .pl(px(row.depth as f32 * 14.))
                                                    .child(div().w(px(12.)).flex_none().when(row.is_dir(), |d| {
                                                        d.child(icon(if folded { "chevron-right" } else { "chevron-down" }, 12., p.text_muted))
                                                    }))
                                                    .child(if row.is_dir() {
                                                        icon("folder", 14., p.text_muted).into_any_element()
                                                    } else {
                                                        crate::icon_theme::file_icon_el(row.name(), 14., &p).into_any_element()
                                                    })
                                                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().child(row.name().to_string()))
                                                    .when(row.case_clash, |d| {
                                                        d.child(
                                                            div()
                                                                .flex_none()
                                                                .text_size(px(text::XS.0))
                                                                .text_color(c(legible(&p, p.warning)))
                                                                .child(crate::i18n::t("differs only in letter case").to_string()),
                                                        )
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .w(px(150.))
                                                    .flex_none()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_size(px(text::XS.0))
                                                    .text_color(c(p.text_muted))
                                                    .child(side_text(row, Side::Left)),
                                            )
                                            .child(
                                                div()
                                                    .w(px(190.))
                                                    .flex_none()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_size(px(text::XS.0))
                                                    .text_color(c(legible(&p, color)))
                                                    .child(status_text(row, scanning)),
                                            )
                                            .child(
                                                div()
                                                    .w(px(150.))
                                                    .flex_none()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_size(px(text::XS.0))
                                                    .text_color(c(p.text_muted))
                                                    .child(side_text(row, Side::Right)),
                                            )
                                            .child(
                                                div()
                                                    .w(px(96.))
                                                    .flex_none()
                                                    .flex()
                                                    .child(small(
                                                        format!("cr-{ix}"),
                                                        "→",
                                                        crate::i18n::tr("Copy to the right"),
                                                        copy_right,
                                                        cx,
                                                        RowAction::Copy(Direction::LeftToRight),
                                                    ))
                                                    .child(small(
                                                        format!("cl-{ix}"),
                                                        "←",
                                                        crate::i18n::tr("Copy to the left"),
                                                        copy_left,
                                                        cx,
                                                        RowAction::Copy(Direction::RightToLeft),
                                                    ))
                                                    .child(small(
                                                        format!("dl-{ix}"),
                                                        "✕",
                                                        crate::i18n::tr("Delete from the left"),
                                                        del_left,
                                                        cx,
                                                        RowAction::Delete(Side::Left),
                                                    ))
                                                    .child(small(
                                                        format!("dr-{ix}"),
                                                        "✕",
                                                        crate::i18n::tr("Delete from the right"),
                                                        del_right,
                                                        cx,
                                                        RowAction::Delete(Side::Right),
                                                    )),
                                            ),
                                    )
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

/// What a row's small button asks for (the event itself needs the row's path, filled in on click).
#[derive(Clone, Copy)]
enum RowAction {
    Copy(Direction),
    Delete(Side),
}
