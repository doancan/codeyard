//! Workbench: Compare Folders. Picks the two folders, opens the comparison tab, and carries out what its rows ask for:
//! opening a diff, copying between the sides and deleting from one. Every write is checked against what the scan saw.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gpui::{Entity, PathPromptOptions};
use host::{HashEvent, Host, HostError};
use proto::Kind;

use super::*;
use crate::compare_folders::{CompareEvent, CompareView, End};
use crate::compare_logic::{CopyJob, DeletePlan, Direction, Expect, Side};

/// A folder on one of the two machines a window can reach: this computer, or the host the window is connected to.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Spot {
    pub local: bool,
    pub path: String,
}

pub(super) enum CompareChoice {
    Folder(Spot),
    /// The system folder dialog, for a folder on this computer.
    PickLocal,
    /// A typed path on the window's host.
    TypeOnHost,
}

impl Workbench {
    fn compare_end(&self, spot: &Spot) -> End {
        let host = if spot.local { self.remote.local_host.clone() } else { self.remote.host.clone() };
        let remote = host.is_remote();
        let label = if remote { host.label() } else { crate::i18n::t("This computer").to_string() };
        End { machine: if remote { label.clone() } else { "local".into() }, local: spot.local || !remote, label, path: spot.path.clone(), host }
    }

    /// "Compare: Folders…": left folder, then right folder. The window's folder is the first choice.
    pub(super) fn compare_folders(&mut self, _: &super::CompareFolders, window: &mut Window, cx: &mut Context<Self>) {
        self.show_compare_picker(None, window, cx);
    }

    fn show_compare_picker(&mut self, first: Option<Spot>, window: &mut Window, cx: &mut Context<Self>) {
        let host_label = if self.remote.host.is_remote() { self.remote.host.label() } else { crate::i18n::t("This computer").to_string() };
        let (mut items, mut choices) = (Vec::new(), Vec::new());
        for root in self.all_roots(cx) {
            items.push(PickItem { label: ui::file_name(&root).to_string().into(), detail: Some(format!("{host_label} · {root}").into()), keys: None });
            choices.push(CompareChoice::Folder(Spot { local: false, path: root }));
        }
        items.push(PickItem { label: crate::i18n::t("Choose a folder on this computer…").into(), detail: None, keys: None });
        choices.push(CompareChoice::PickLocal);
        if self.remote.host.is_remote() {
            items.push(PickItem { label: crate::i18n::tf("Enter a folder path on {}…", &[&host_label]).into(), detail: None, keys: None });
            choices.push(CompareChoice::TypeOnHost);
        }
        let title = match &first {
            None => crate::i18n::t("Compare Folders: pick the left folder").to_string(),
            Some(a) => crate::i18n::tf("Compare Folders: pick the folder to compare with {}", &[&ui::file_name(&a.path)]),
        };
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], &title, "", None, cx));
        self.show_overlay(picker, OverlayKind::CompareFolder { first, choices }, window, cx);
    }

    /// A row of the folder picker was chosen.
    pub(super) fn on_compare_pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay { kind: OverlayKind::CompareFolder { first, mut choices }, .. }) = self.close_overlay(window, cx) else { return };
        if index >= choices.len() {
            return;
        }
        match choices.swap_remove(index) {
            CompareChoice::Folder(spot) => self.compare_continue(first, spot, window, cx),
            CompareChoice::PickLocal => {
                let rx = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some(crate::i18n::t("Choose").to_string().into()),
                });
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = rx.await
                        && let Some(path) = paths.into_iter().next()
                    {
                        let spot = Spot { local: true, path: ui::local_path(&path) };
                        this.update_in(cx, |this, window, cx| this.compare_continue(first, spot, window, cx)).ok();
                    }
                })
                .detach();
            }
            CompareChoice::TypeOnHost => {
                let start = self.root(cx).unwrap_or_else(|| self.remote.host.info().home.clone());
                let start = format!("{}/", start.trim_end_matches('/'));
                self.show_input(
                    InputPurpose::CompareFolder { first },
                    crate::i18n::t("Compare"),
                    crate::i18n::t("Path of the folder (~ is your home folder)"),
                    &start,
                    window,
                    cx,
                );
            }
        }
    }

    /// A folder path typed for the window's host.
    pub(super) fn finish_compare_input(&mut self, first: Option<Spot>, typed: String, window: &mut Window, cx: &mut Context<Self>) {
        let path = super::folder_glue::normalize(&typed, &self.remote.host.info().home);
        self.compare_continue(first, Spot { local: false, path }, window, cx);
    }

    fn compare_continue(&mut self, first: Option<Spot>, picked: Spot, window: &mut Window, cx: &mut Context<Self>) {
        match first {
            None => self.show_compare_picker(Some(picked), window, cx),
            Some(left) => self.open_folder_compare(left, picked, window, cx),
        }
    }

    /// Tree menu: "Select Folder for Compare".
    pub(super) fn select_folder_for_compare(&mut self, path: String, cx: &mut Context<Self>) {
        self.notice = Some(Notice::Info(
            crate::i18n::tf("Selected {} for compare. Right-click another folder and choose Compare Folder with Selected.", &[&ui::file_name(&path)]).into(),
        ));
        self.roots.compare_folder_a = Some(Spot { local: false, path });
        cx.notify();
    }

    pub(super) fn compare_folder_with_selected(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(left) = self.roots.compare_folder_a.take() {
            self.open_folder_compare(left, Spot { local: false, path }, window, cx);
        }
    }

    pub(super) fn open_folder_compare(&mut self, left: Spot, right: Spot, window: &mut Window, cx: &mut Context<Self>) {
        let (l, r) = (self.compare_end(&left), self.compare_end(&right));
        let view = cx.new(|cx| CompareView::new(l, r, cx));
        self.tabs._page_sub =
            Some(cx.subscribe_in(&view, window, |this, view, ev: &CompareEvent, window, cx| this.on_compare_event(view.clone(), ev, window, cx)));
        window.focus(&view.focus_handle(cx));
        self.tabs.page = Some(Page::Compare(view));
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        cx.notify();
    }

    fn on_compare_event(&mut self, view: Entity<CompareView>, event: &CompareEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            CompareEvent::OpenDiff(path) => self.compare_open_diff(view, path.clone(), window, cx),
            CompareEvent::Copy(path, dir) => self.compare_copy(view, path.clone(), *dir, window, cx),
            CompareEvent::Delete(path, side) => self.compare_delete(view, path.clone(), *side, window, cx),
            CompareEvent::Message(text) => {
                self.notice = Some(Notice::Info(text.clone().into()));
                cx.notify();
            }
        }
    }

    fn notify_compare(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.notice = Some(notice);
        cx.notify();
    }

    /// Open the existing diff view for a changed file, labelled "Left ↔ Right".
    fn compare_open_diff(&mut self, view: Entity<CompareView>, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let (hosts, full, eol) = {
            let v = view.read(cx);
            ([v.end(Side::Left).host.clone(), v.end(Side::Right).host.clone()], [v.full_path(Side::Left, &path), v.full_path(Side::Right, &path)], v.ignore_eol)
        };
        let task = cx.background_executor().spawn(async move {
            let read = |side: usize| hosts[side].read(&full[side]).map(|(d, _)| d).map_err(|e| format!("{}: {e}", full[side]));
            Ok::<_, String>((read(0)?, read(1)?))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((a, b)) if a.iter().chain(&b).take(8192).any(|&x| x == 0) => {
                    this.notify_compare(Notice::Info(crate::i18n::tf("{} is a binary file: there is no text diff.", &[&path]).into()), cx);
                }
                Ok((a, b)) => {
                    let text = |d: &[u8]| {
                        let s = String::from_utf8_lossy(d);
                        if eol { s.replace("\r\n", "\n") } else { s.into_owned() }
                    };
                    let diff =
                        cx.new(|cx| crate::diff::DiffView::new(path.clone(), false, &text(&a), &text(&b), cx).with_label(crate::i18n::t("Left ↔ Right")));
                    window.focus(&diff.focus_handle(cx));
                    this.tabs.diff = Some(diff);
                    this.tabs.diff_active = true;
                    this.tabs.page_active = false;
                    this.tabs.large_active = false;
                    cx.notify();
                }
                Err(e) => this.notify_compare(Notice::Error(crate::i18n::tf("Could not read {}", &[&e]).into()), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Writes are refused in a folder that is not trusted (the same record "Trust This Folder" writes).
    fn compare_end_trusted(&self, end: &End, cx: &App) -> bool {
        if !cx.global::<Settings>().workspace_trust {
            return true;
        }
        // A folder on this computer seen from a remote window is not covered by that window's trust records.
        if end.local && self.remote.host.is_remote() {
            return self.persist.state.as_ref().is_some_and(|s| s.get(&format!("trust::{}", end.path)).is_some());
        }
        let root = self.root_of(&end.path, cx).unwrap_or_else(|| end.path.clone());
        self.is_trusted_root(&root, cx)
    }

    fn alias_for_transfers(&self) -> Option<String> {
        match &self.remote.conn {
            Conn::Connected { alias } if self.remote.host.is_remote() => Some(alias.clone()),
            _ => None,
        }
    }

    fn compare_copy(&mut self, view: Entity<CompareView>, path: String, dir: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let (from_side, to_side) = match dir {
            Direction::LeftToRight => (Side::Left, Side::Right),
            Direction::RightToLeft => (Side::Right, Side::Left),
        };
        let (from, to, plan) = {
            let v = view.read(cx);
            (v.end(from_side).clone(), v.end(to_side).clone(), v.copy_plan(&path, dir))
        };
        if !self.compare_end_trusted(&to, cx) {
            self.notify_compare(
                Notice::Error(
                    crate::i18n::tf("{} is not a trusted folder, so nothing is written to it. Open it as a folder and trust it first.", &[&to.path]).into(),
                ),
                cx,
            );
            return;
        }
        if plan.jobs.is_empty() {
            let why = plan.skipped.first().map_or_else(|| crate::i18n::t("There is nothing to copy.").to_string(), |(p, why)| format!("{p}: {why}"));
            self.notify_compare(Notice::Info(why.into()), cx);
            return;
        }
        let (files, folders) = (plan.jobs.iter().filter(|j| !j.is_dir).count(), plan.jobs.iter().filter(|j| j.is_dir).count());
        let mut detail = crate::i18n::tf(
            "{0} files and {1} folders go from {2} ({3}) to {4} ({5}). Files that exist there are replaced; anything that changed since the scan is left alone.",
            &[&files, &folders, &from.label, &from.path, &to.label, &to.path],
        );
        if !plan.skipped.is_empty() {
            detail.push(' ');
            detail.push_str(&crate::i18n::tn("{} entry is left out.", "{} entries are left out.", plan.skipped.len(), &[]));
        }
        let answer = crate::questions::compare_copy(ui::file_name(&path), &to.label, &detail).ask(window, cx);
        let alias = self.alias_for_transfers();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Some(0) {
                return;
            }
            let Ok(cancel) = view.update(cx, |v, cx| v.begin_op(crate::i18n::t("Copying…").to_string(), cx)) else { return };
            let flag = cancel.clone();
            let work = cx.background_executor().spawn(async move { copy_jobs(&from, &to, alias.as_deref(), &plan.jobs, &flag) });
            let outcome = work.await;
            this.update(cx, |this, cx| {
                view.update(cx, |v, cx| {
                    v.end_op(cx);
                    v.rescan(cx);
                });
                this.notify_compare(Notice::Info(outcome.summary().into()), cx);
            })
            .ok();
        })
        .detach();
    }

    fn compare_delete(&mut self, view: Entity<CompareView>, path: String, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let (end, plan, writable) = {
            let v = view.read(cx);
            (v.end(side).clone(), v.delete_plan(&path, side), v.writable[side as usize])
        };
        let Some(plan) = plan.filter(|_| writable) else { return };
        if !self.compare_end_trusted(&end, cx) {
            self.notify_compare(
                Notice::Error(
                    crate::i18n::tf("{} is not a trusted folder, so nothing is deleted from it. Open it as a folder and trust it first.", &[&end.path]).into(),
                ),
                cx,
            );
            return;
        }
        let full = view.read(cx).full_path(side, &path);
        let host = end.host.clone();
        let (root, plan_for_check) = (end.path.clone(), plan.snapshot.clone());
        let check_plan = DeletePlan { path: plan.path.clone(), is_dir: plan.is_dir, snapshot: plan_for_check };
        let (check_host, check_full, check_root) = (host.clone(), full.clone(), root.clone());
        // First look: has it changed since the scan, and what else lives in the folder (ignored files are deleted too)?
        let first = cx.background_executor().spawn(async move { verify_delete(check_host.as_ref(), &check_full, &check_plan, &check_root) });
        cx.spawn_in(window, async move |this, cx| {
            let hidden = match first.await {
                Ok(n) => n,
                Err(why) => {
                    this.update(cx, |this, cx| this.notify_compare(Notice::Error(crate::i18n::tf("Not deleted: {}", &[&why]).into()), cx)).ok();
                    return;
                }
            };
            let answer =
                this.update_in(cx, |_, window, cx| crate::questions::compare_delete(ui::file_name(&path), &full, &end.label, hidden).ask(window, cx)).ok();
            let Some(answer) = answer else { return };
            if answer.await != Some(0) {
                return;
            }
            let _ = view.update(cx, |v, cx| v.begin_op(crate::i18n::t("Deleting…").to_string(), cx));
            let work_plan = plan;
            let work = cx.background_executor().spawn(async move {
                // Second look right before the delete: the prompt can sit open for a while.
                verify_delete(host.as_ref(), &full, &work_plan, &root)?;
                host.delete(&full).map_err(|e| e.to_string())
            });
            let result = work.await;
            this.update(cx, |this, cx| {
                view.update(cx, |v, cx| {
                    v.end_op(cx);
                    v.rescan(cx);
                });
                let notice = match result {
                    Ok(()) => Notice::Info(crate::i18n::tf("Deleted “{0}” from {1}", &[&ui::file_name(&path), &end.label]).into()),
                    Err(why) => Notice::Error(crate::i18n::tf("Not deleted: {}", &[&why]).into()),
                };
                this.notify_compare(notice, cx);
            })
            .ok();
        })
        .detach();
    }

    #[cfg(test)]
    pub fn delete_request_for_test(&mut self, view: &Entity<CompareView>, path: &str, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        self.compare_delete(view.clone(), path.to_string(), side, window, cx);
    }

    #[cfg(test)]
    pub fn compare_view_for_test(&self) -> Option<Entity<CompareView>> {
        match &self.tabs.page {
            Some(Page::Compare(v)) => Some(v.clone()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn open_folder_compare_for_test(&mut self, left: (bool, &str), right: (bool, &str), window: &mut Window, cx: &mut Context<Self>) {
        self.open_folder_compare(Spot { local: left.0, path: left.1.into() }, Spot { local: right.0, path: right.1.into() }, window, cx);
    }
}

/// What a copy did.
#[derive(Debug, Default, PartialEq)]
pub(super) struct CopyOutcome {
    pub copied: usize,
    /// Entries left alone, each with the reason.
    pub refused: Vec<(String, String)>,
    pub cancelled: bool,
}

impl CopyOutcome {
    pub fn summary(&self) -> String {
        let mut text = crate::i18n::tn("Copied {} item.", "Copied {} items.", self.copied, &[]);
        if !self.refused.is_empty() {
            let (path, why) = &self.refused[0];
            text.push(' ');
            text.push_str(&crate::i18n::tn(
                "{0} was left alone: {1}",
                "{0} were left alone, the first one {1}",
                self.refused.len(),
                &[&format!("{path}: {why}")],
            ));
        }
        if self.cancelled {
            text.push(' ');
            text.push_str(crate::i18n::t("Stopped."));
        }
        text
    }
}

static STAGE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn changed() -> String {
    crate::i18n::t("it changed after the scan").to_string()
}

fn op_err(e: HostError) -> String {
    e.to_string()
}

/// The content hash the agent computes for one file (`None` if it does not exist).
fn hash_of(host: &dyn Host, root: &str, rel: &str) -> Result<Option<u64>, String> {
    let stream = host.hash_tree(root, Vec::new(), false, 1, Some(vec![rel.to_string()])).map_err(op_err)?;
    let mut hash = None;
    loop {
        match stream.next() {
            Some(HashEvent::Records(records)) => {
                for rec in records {
                    if let Some(e) = rec.error {
                        return Err(e);
                    }
                    hash = rec.hash;
                }
            }
            Some(HashEvent::Done { .. }) => return Ok(hash),
            Some(HashEvent::Failed(e)) => return Err(e.to_string()),
            None => return Err(crate::i18n::t("the connection was lost").to_string()),
        }
    }
}

/// Is the entry still what the scan saw?
fn check(host: &dyn Host, root: &str, rel: &str, expect: &Expect) -> Result<(), String> {
    let full = ui::join(root, rel);
    match expect {
        Expect::Absent => match host.stat(&full) {
            Err(HostError::Op(proto::Error::NotFound(_))) => Ok(()),
            Ok(_) => Err(crate::i18n::t("it appeared after the scan").to_string()),
            Err(e) => Err(op_err(e)),
        },
        Expect::Hash(h) => match hash_of(host, root, rel)? {
            Some(now) if now == *h => Ok(()),
            _ => Err(changed()),
        },
        Expect::Stat { size, mtime_ms } => {
            let m = host.stat(&full).map_err(op_err)?;
            if m.size == *size && m.mtime_ms == *mtime_ms { Ok(()) } else { Err(changed()) }
        }
    }
}

fn stage_dir() -> PathBuf {
    std::env::temp_dir().join(format!("codeyard-compare-{}-{}", std::process::id(), STAGE_COUNTER.fetch_add(1, Ordering::Relaxed)))
}

/// Copy the files and folders of `jobs` from one side to the other. A file that is not what the scan saw, at either end, is
/// left alone and named; the rest carries on.
pub(super) fn copy_jobs(from: &End, to: &End, alias: Option<&str>, jobs: &[CopyJob], cancel: &AtomicBool) -> CopyOutcome {
    let mut outcome = CopyOutcome::default();
    for job in jobs {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            break;
        }
        let result = if job.is_dir {
            match to.host.create_dir(&ui::join(&to.path, &job.path)) {
                Ok(()) | Err(HostError::Op(proto::Error::AlreadyExists(_))) => Ok(()),
                Err(e) => Err(op_err(e)),
            }
        } else {
            copy_file(from, to, alias, job, cancel)
        };
        match result {
            Ok(()) => outcome.copied += 1,
            Err(_) if cancel.load(Ordering::Relaxed) => outcome.cancelled = true,
            Err(why) => outcome.refused.push((job.path.clone(), why)),
        }
    }
    outcome
}

fn copy_file(from: &End, to: &End, alias: Option<&str>, job: &CopyJob, cancel: &AtomicBool) -> Result<(), String> {
    let (src, dst) = (ui::join(&from.path, &job.path), ui::join(&to.path, &job.path));
    check(from.host.as_ref(), &from.path, &job.path, &job.source).map_err(|why| format!("{} ({})", crate::i18n::t("source"), why))?;
    check(to.host.as_ref(), &to.path, &job.path, &job.dest).map_err(|why| format!("{} ({})", crate::i18n::t("destination"), why))?;
    let size = from.host.stat(&src).map_err(op_err)?.size;
    if size > proto::MAX_FILE {
        return copy_big_file(from, to, alias, job, cancel);
    }
    let (data, hash) = from.host.read(&src).map_err(op_err)?;
    if let Expect::Hash(h) = job.source
        && hash != h
    {
        return Err(format!("{} ({})", crate::i18n::t("source"), changed()));
    }
    let conflict = |e: HostError| match e {
        HostError::Op(proto::Error::Conflict { .. }) => format!("{} ({})", crate::i18n::t("destination"), changed()),
        other => op_err(other),
    };
    match &job.dest {
        // Created empty first: `create_file` never overwrites, and the write below then only succeeds on that empty file.
        Expect::Absent => {
            to.host.create_file(&dst).map_err(|e| match e {
                HostError::Op(proto::Error::AlreadyExists(_)) => {
                    format!("{} ({})", crate::i18n::t("destination"), crate::i18n::t("it appeared after the scan"))
                }
                other => op_err(other),
            })?;
            to.host.write(&dst, data, Some(proto::content_hash(&[]))).map(|_| ()).map_err(|e| {
                let _ = to.host.delete(&dst);
                conflict(e)
            })
        }
        Expect::Hash(h) => to.host.write(&dst, data, Some(*h)).map(|_| ()).map_err(conflict),
        Expect::Stat { .. } => {
            let (_, now) = to.host.read(&dst).map_err(op_err)?;
            to.host.write(&dst, data, Some(now)).map(|_| ()).map_err(conflict)
        }
    }
}

/// Files over the agent's size limit go through a temporary copy with the transfer code (streamed, no memory spike).
fn copy_big_file(from: &End, to: &End, alias: Option<&str>, job: &CopyJob, cancel: &AtomicBool) -> Result<(), String> {
    let (src, dst) = (ui::join(&from.path, &job.path), ui::join(&to.path, &job.path));
    let stage = stage_dir();
    let result = (|| {
        let shell = alias.map(host::transfer::shell_for);
        let need_shell = || shell.as_ref().ok_or_else(|| crate::i18n::t("the connection was lost").to_string());
        let tmp_dir = stage.join("in");
        let staged = if from.host.is_remote() {
            host::transfer::download(need_shell()?, &src, false, &tmp_dir, cancel, |_| {}).map_err(|e| e.to_string())?
        } else {
            host::transfer::copy_local(Path::new(&src), &tmp_dir, cancel, |_| {}).map_err(|e| e.to_string())?
        };
        if let Expect::Hash(h) = job.source {
            let now = std::fs::File::open(&staged).and_then(|mut f| proto::content_hash_reader(&mut f)).map_err(|e| e.to_string())?;
            if now != h {
                return Err(format!("{} ({})", crate::i18n::t("source"), changed()));
            }
        }
        let parent = dst.rsplit_once('/').map_or("/", |(p, _)| if p.is_empty() { "/" } else { p }).to_string();
        if to.host.is_remote() {
            host::transfer::upload(need_shell()?, &staged, &parent, cancel, |_| {}).map(|_| ()).map_err(|e| e.to_string())
        } else {
            host::transfer::copy_local(&staged, Path::new(&parent), cancel, |_| {}).map(|_| ()).map_err(|e| e.to_string())
        }
    })();
    let _ = std::fs::remove_dir_all(&stage);
    result
}

/// Look at what is about to be deleted: it must still be what the scan saw. Returns how many entries inside the folder
/// the comparison did not show (ignored ones), which the delete would take along.
pub(super) fn verify_delete(host: &dyn Host, full: &str, plan: &DeletePlan, _root: &str) -> Result<usize, String> {
    if !plan.is_dir {
        let m = host.stat(full).map_err(op_err)?;
        return match plan.snapshot.first() {
            Some((_, _, size, mtime)) if m.size == *size && m.mtime_ms == *mtime => Ok(0),
            _ => Err(changed()),
        };
    }
    let stream = host.hash_tree(full, Vec::new(), false, 5_000_000, None).map_err(op_err)?;
    let prefix = format!("{}/", plan.path);
    let mut want: HashMap<&str, (Kind, u64, i64)> =
        plan.snapshot.iter().filter_map(|(p, kind, size, mtime)| p.strip_prefix(&prefix).map(|rel| (rel, (*kind, *size, *mtime)))).collect();
    let mut extra = 0;
    loop {
        match stream.next() {
            Some(HashEvent::Records(records)) => {
                for rec in records {
                    match want.remove(rec.path.as_str()) {
                        Some((kind, size, mtime)) if kind == rec.kind && (kind != Kind::File || (size == rec.size && mtime == rec.mtime_ms)) => {}
                        Some(_) => return Err(changed()),
                        None => extra += 1,
                    }
                }
            }
            Some(HashEvent::Done { truncated, .. }) => {
                return if want.is_empty() && !truncated { Ok(extra) } else { Err(changed()) };
            }
            Some(HashEvent::Failed(e)) => return Err(e.to_string()),
            None => return Err(crate::i18n::t("the connection was lost").to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare_logic::{self as logic, Row};

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("codeyard-cmpops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::ui::canonical_for_test(&dir)
    }

    fn end(path: &Path) -> End {
        End {
            host: std::sync::Arc::new(host::local()),
            path: path.to_string_lossy().into_owned(),
            label: "This computer".into(),
            machine: "local".into(),
            local: true,
        }
    }

    /// Classify two real folders through the agent, the way the view does (listing, then hashes of equal-sized files).
    fn rows_of(left: &Path, right: &Path) -> Vec<Row> {
        let scan = |root: &Path, only: Option<Vec<String>>| {
            let h = host::local();
            let s = h.hash_tree(&root.to_string_lossy(), vec![], false, 1000, only).unwrap();
            let mut out = Vec::new();
            while let Some(HashEvent::Records(r)) = s.next() {
                out.extend(r);
            }
            out
        };
        let mut trees: [logic::Tree; 2] = [left, right].map(|root| scan(root, None).into_iter().map(|r| (r.path.clone(), r)).collect());
        let wanted = logic::hash_candidates(&trees[0], &trees[1]);
        for (i, root) in [left, right].into_iter().enumerate() {
            for rec in scan(root, Some(wanted.clone())) {
                logic::apply_hash(&mut trees[i], rec);
            }
        }
        logic::classify(&trees[0], &trees[1], [true, true], &HashMap::new())
    }

    #[test]
    fn copies_new_and_changed_files_and_folders() {
        let (l, r) = (tmp("copy-l"), tmp("copy-r"));
        std::fs::create_dir_all(l.join("d/sub")).unwrap();
        std::fs::write(l.join("d/new.txt"), "yeni").unwrap();
        std::fs::write(l.join("d/sub/deep.txt"), "derin").unwrap();
        std::fs::write(l.join("same.txt"), "x").unwrap();
        std::fs::write(r.join("same.txt"), "x").unwrap();
        std::fs::write(l.join("edit.txt"), "sol").unwrap();
        std::fs::write(r.join("edit.txt"), "sağ!").unwrap();
        let rows = rows_of(&l, &r);
        let plan = logic::plan_copy(&rows, "d", Direction::LeftToRight);
        let plan2 = logic::plan_copy(&rows, "edit.txt", Direction::LeftToRight);
        let jobs: Vec<CopyJob> = plan.jobs.into_iter().chain(plan2.jobs).collect();
        let out = copy_jobs(&end(&l), &end(&r), None, &jobs, &AtomicBool::new(false));
        assert_eq!((out.copied, out.refused.len(), out.cancelled), (5, 0, false), "{out:?}");
        assert_eq!(std::fs::read_to_string(r.join("d/new.txt")).unwrap(), "yeni");
        assert_eq!(std::fs::read_to_string(r.join("d/sub/deep.txt")).unwrap(), "derin");
        assert_eq!(std::fs::read_to_string(r.join("edit.txt")).unwrap(), "sol");
        let after = logic::counts(&rows_of(&l, &r));
        assert_eq!((after.only_left, after.changed), (0, 0), "nothing differs any more");
    }

    #[test]
    fn a_destination_that_changed_since_the_scan_is_refused_and_left_untouched() {
        let (l, r) = (tmp("conflict-l"), tmp("conflict-r"));
        std::fs::write(l.join("a.txt"), "left").unwrap();
        std::fs::write(r.join("a.txt"), "right!").unwrap();
        std::fs::write(l.join("b.txt"), "new on the left").unwrap();
        let rows = rows_of(&l, &r);
        let mut jobs = logic::plan_copy(&rows, "a.txt", Direction::LeftToRight).jobs;
        jobs.extend(logic::plan_copy(&rows, "b.txt", Direction::LeftToRight).jobs);
        // Somebody edits a.txt on the right and creates b.txt there after the scan.
        std::fs::write(r.join("a.txt"), "edited since!").unwrap();
        std::fs::write(r.join("b.txt"), "created since").unwrap();
        let out = copy_jobs(&end(&l), &end(&r), None, &jobs, &AtomicBool::new(false));
        assert_eq!(out.copied, 0);
        assert_eq!(out.refused.len(), 2, "{out:?}");
        assert!(out.refused[0].1.contains("destination") && out.refused[0].1.contains("changed"), "{out:?}");
        assert!(out.refused[1].1.contains("appeared"), "{out:?}");
        assert_eq!(std::fs::read_to_string(r.join("a.txt")).unwrap(), "edited since!");
        assert_eq!(std::fs::read_to_string(r.join("b.txt")).unwrap(), "created since");
    }

    #[test]
    fn a_source_that_changed_since_the_scan_is_refused() {
        let (l, r) = (tmp("srcchg-l"), tmp("srcchg-r"));
        std::fs::write(l.join("a.txt"), "before").unwrap();
        std::fs::write(r.join("a.txt"), "other!").unwrap();
        let rows = rows_of(&l, &r);
        let jobs = logic::plan_copy(&rows, "a.txt", Direction::LeftToRight).jobs;
        std::fs::write(l.join("a.txt"), "after!").unwrap();
        let out = copy_jobs(&end(&l), &end(&r), None, &jobs, &AtomicBool::new(false));
        assert_eq!(out.copied, 0);
        assert!(out.refused[0].1.starts_with("source"), "{out:?}");
        assert_eq!(std::fs::read_to_string(r.join("a.txt")).unwrap(), "other!");
    }

    #[test]
    fn a_cancelled_copy_writes_nothing() {
        let (l, r) = (tmp("cancel-l"), tmp("cancel-r"));
        std::fs::write(l.join("a.txt"), "a").unwrap();
        let rows = rows_of(&l, &r);
        let jobs = logic::plan_copy(&rows, "a.txt", Direction::LeftToRight).jobs;
        let out = copy_jobs(&end(&l), &end(&r), None, &jobs, &AtomicBool::new(true));
        assert!(out.cancelled && out.copied == 0);
        assert!(!r.join("a.txt").exists());
        assert!(out.summary().contains("Stopped"));
    }

    #[test]
    fn delete_checks_the_snapshot_and_counts_hidden_entries() {
        let l = tmp("delete-l");
        std::fs::create_dir_all(l.join("gone/node_modules")).unwrap();
        std::fs::write(l.join("gone/a.txt"), "1").unwrap();
        std::fs::write(l.join("gone/node_modules/x.js"), "2").unwrap();
        let r = tmp("delete-r");
        let scan = |ignore: Vec<String>| {
            let h = host::local();
            let s = h.hash_tree(&l.to_string_lossy(), ignore, false, 100, None).unwrap();
            let mut out = Vec::new();
            while let Some(HashEvent::Records(rs)) = s.next() {
                out.extend(rs);
            }
            out
        };
        let tree: logic::Tree = scan(vec!["node_modules".into()]).into_iter().map(|r| (r.path.clone(), r)).collect();
        let rows = logic::classify(&tree, &logic::Tree::new(), [true, true], &HashMap::new());
        let plan = logic::plan_delete(&rows, "gone", Side::Left).unwrap();
        let h = host::local();
        let full = l.join("gone").to_string_lossy().into_owned();
        assert_eq!(verify_delete(&h, &full, &plan, &l.to_string_lossy()), Ok(2), "node_modules and its file are not in the snapshot");
        std::fs::write(l.join("gone/a.txt"), "changed").unwrap();
        assert!(verify_delete(&h, &full, &plan, "").is_err(), "a file changed since the scan");
        let tree: logic::Tree = scan(vec!["node_modules".into()]).into_iter().map(|r| (r.path.clone(), r)).collect();
        let rows = logic::classify(&tree, &logic::Tree::new(), [true, true], &HashMap::new());
        let plan = logic::plan_delete(&rows, "gone", Side::Left).unwrap();
        std::fs::write(l.join("gone/late.txt"), "late").unwrap();
        assert_eq!(verify_delete(&h, &full, &plan, ""), Ok(3), "a new file counts as extra, it is deleted along");
        drop(r);
    }
}
