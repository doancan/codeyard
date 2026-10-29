//! Workbench: Compare Folders. Picks the two folders, opens the comparison tab, and carries out what its rows ask for:
//! opening a diff, copying between the sides and deleting from one. Every write is checked against what the scan saw.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gpui::{Entity, PathPromptOptions, PromptLevel};
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
        let title = crate::i18n::tf("Copy “{0}” to {1}?", &[&ui::file_name(&path), &to.label]);
        let answer = window.prompt(PromptLevel::Warning, &title, Some(&detail), &[crate::i18n::t("Copy"), crate::i18n::t("Cancel")], cx);
        let alias = self.alias_for_transfers();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
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
                let message = outcome.summary().into();
                this.notify_compare(if outcome.refused.is_empty() { Notice::Info(message) } else { Notice::Error(message) }, cx);
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
            let answer = this
                .update_in(cx, |_, window, cx| {
                    let title = crate::i18n::tf("Delete “{0}” from {1}?", &[&ui::file_name(&path), &end.label]);
                    let mut detail =
                        crate::i18n::tf("{0} on {1} is deleted for good, with everything in it. It is not moved to the Trash.", &[&full, &end.label]);
                    if hidden > 0 {
                        detail.push(' ');
                        detail.push_str(&crate::i18n::tn(
                            "{} more entry that the comparison hides (ignored by .gitignore or the compare settings) goes too.",
                            "{} more entries that the comparison hides (ignored by .gitignore or the compare settings) go too.",
                            hidden,
                            &[],
                        ));
                    }
                    window.prompt(PromptLevel::Critical, &title, Some(&detail), &[crate::i18n::t("Delete"), crate::i18n::t("Cancel")], cx)
                })
                .ok();
            let Some(answer) = answer else { return };
            if answer.await != Ok(0) {
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
    /// Entries whose copy was not confirmed, each with the reason. The destination may have changed.
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
                "Copying {0} item was not confirmed: {1}",
                "Copying {0} items was not confirmed; first: {1}",
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

#[derive(Debug)]
enum CopyFailure {
    Cancelled,
    Failed(String),
}

impl CopyFailure {
    fn at(self, side: &str) -> Self {
        match self {
            Self::Cancelled => Self::Cancelled,
            Self::Failed(message) => Self::Failed(format!("{side} ({message})")),
        }
    }
}

impl From<String> for CopyFailure {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<HostError> for CopyFailure {
    fn from(error: HostError) -> Self {
        match error {
            HostError::Op(proto::Error::Cancelled) => Self::Cancelled,
            other => Self::Failed(other.to_string()),
        }
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
fn hash_of(host: &dyn Host, root: &str, rel: &str) -> Result<Option<u64>, CopyFailure> {
    let stream = host.hash_tree(root, Vec::new(), false, 1, Some(vec![rel.to_string()])).map_err(CopyFailure::from)?;
    let mut hash = None;
    loop {
        match stream.next() {
            Some(HashEvent::Records(records)) => {
                for rec in records {
                    if let Some(e) = rec.error {
                        return Err(e.into());
                    }
                    hash = rec.hash;
                }
            }
            Some(HashEvent::Done { .. }) => return Ok(hash),
            Some(HashEvent::Failed(e)) => return Err(HostError::Op(e).into()),
            None => return Err(crate::i18n::t("the connection was lost").to_string().into()),
        }
    }
}

/// Is the entry still what the scan saw?
fn check(host: &dyn Host, root: &str, rel: &str, expect: &Expect) -> Result<(), CopyFailure> {
    let full = ui::join(root, rel);
    match expect {
        Expect::Absent => match host.stat(&full) {
            Err(HostError::Op(proto::Error::NotFound(_))) => Ok(()),
            Ok(_) => Err(crate::i18n::t("it appeared after the scan").to_string().into()),
            Err(e) => Err(e.into()),
        },
        Expect::Hash(h) => match hash_of(host, root, rel)? {
            Some(now) if now == *h => Ok(()),
            _ => Err(changed().into()),
        },
        Expect::Stat { size, mtime_ms } => {
            let m = host.stat(&full).map_err(CopyFailure::from)?;
            if m.size == *size && m.mtime_ms == *mtime_ms { Ok(()) } else { Err(changed().into()) }
        }
    }
}

fn stage_dir_at(parent: &Path, counter: &AtomicU64) -> std::io::Result<PathBuf> {
    for _ in 0..100 {
        let path = parent.join(format!("codeyard-compare-{}-{}", std::process::id(), counter.fetch_add(1, Ordering::Relaxed)));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, crate::i18n::t("Could not reserve a temporary file for comparison.")))
}

fn stage_dir() -> std::io::Result<PathBuf> {
    stage_dir_at(&std::env::temp_dir(), &STAGE_COUNTER)
}

/// Copy the files and folders of `jobs` from one side to the other. A file that is not what the scan saw, at either end, is
/// left alone and named; the rest carries on.
pub(super) fn copy_jobs(from: &End, to: &End, alias: Option<&str>, jobs: &[CopyJob], cancel: &AtomicBool) -> CopyOutcome {
    let mut outcome = CopyOutcome::default();
    let mut blocked_folders: Vec<(String, String)> = Vec::new();
    for job in jobs {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            break;
        }
        if let Some((folder, why)) = blocked_folders.iter().find(|(folder, _)| job.path.strip_prefix(folder).is_some_and(|suffix| suffix.starts_with('/'))) {
            outcome.refused.push((job.path.clone(), format!("{folder}: {why}")));
            continue;
        }
        let result = (|| {
            check_copy_path(from, job, false, cancel).map_err(|error| error.at(crate::i18n::t("source")))?;
            check_copy_path(to, job, true, cancel).map_err(|error| error.at(crate::i18n::t("destination")))?;
            if job.is_dir { copy_directory(to, job, cancel) } else { copy_file(from, to, alias, job, cancel) }
        })();
        match result {
            Ok(()) => outcome.copied += 1,
            Err(CopyFailure::Cancelled) => {
                outcome.cancelled = true;
                break;
            }
            Err(CopyFailure::Failed(why)) => {
                if job.is_dir {
                    blocked_folders.push((job.path.clone(), why.clone()));
                }
                outcome.refused.push((job.path.clone(), why));
                if cancel.load(Ordering::Relaxed) {
                    outcome.cancelled = true;
                    break;
                }
            }
        }
    }
    outcome
}

fn check_copy_path(end: &End, job: &CopyJob, allow_missing: bool, cancel: &AtomicBool) -> Result<(), CopyFailure> {
    let mut path = end.path.clone();
    let mut components = job.path.split('/').peekable();
    while let Some(component) = components.next() {
        if cancel.load(Ordering::Relaxed) {
            return Err(CopyFailure::Cancelled);
        }
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(changed().into());
        }
        path = ui::join(&path, component);
        let expected = if components.peek().is_some() || job.is_dir { Kind::Dir } else { Kind::File };
        match end.host.lstat(&path) {
            Ok(meta) if meta.kind == expected => {}
            Ok(_) => return Err(changed().into()),
            Err(HostError::Op(proto::Error::NotFound(_))) if allow_missing => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn copy_directory(to: &End, job: &CopyJob, cancel: &AtomicBool) -> Result<(), CopyFailure> {
    if cancel.load(Ordering::Relaxed) {
        return Err(CopyFailure::Cancelled);
    }
    to.host.create_dir(&ui::join(&to.path, &job.path)).map_err(|error| match error {
        HostError::Op(proto::Error::AlreadyExists(_)) => {
            CopyFailure::Failed(format!("{} ({})", crate::i18n::t("destination"), crate::i18n::t("it appeared after the scan")))
        }
        other => CopyFailure::from(other).at(crate::i18n::t("destination")),
    })
}

fn copy_file(from: &End, to: &End, alias: Option<&str>, job: &CopyJob, cancel: &AtomicBool) -> Result<(), CopyFailure> {
    let (src, dst) = (ui::join(&from.path, &job.path), ui::join(&to.path, &job.path));
    check(from.host.as_ref(), &from.path, &job.path, &job.source).map_err(|error| error.at(crate::i18n::t("source")))?;
    check(to.host.as_ref(), &to.path, &job.path, &job.dest).map_err(|error| error.at(crate::i18n::t("destination")))?;
    let size = from.host.stat(&src).map_err(CopyFailure::from)?.size;
    if size > proto::MAX_FILE {
        return copy_big_file(from, to, alias, job, cancel);
    }
    let (data, hash) = from.host.read(&src).map_err(CopyFailure::from)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(CopyFailure::Cancelled);
    }
    if let Expect::Hash(h) = job.source
        && hash != h
    {
        return Err(format!("{} ({})", crate::i18n::t("source"), changed()).into());
    }
    let conflict = |e: HostError| match e {
        HostError::Op(proto::Error::Conflict { .. }) => CopyFailure::Failed(format!("{} ({})", crate::i18n::t("destination"), changed())),
        HostError::Op(proto::Error::AlreadyExists(_)) => {
            CopyFailure::Failed(format!("{} ({})", crate::i18n::t("destination"), crate::i18n::t("it appeared after the scan")))
        }
        other => CopyFailure::from(other),
    };
    match &job.dest {
        Expect::Absent => copy_new_file(to.host.as_ref(), &dst, data, cancel).map_err(conflict),
        Expect::Hash(h) => to.host.write(&dst, data, Some(*h)).map(|_| ()).map_err(conflict),
        Expect::Stat { .. } => {
            let (_, now) = to.host.read(&dst).map_err(CopyFailure::from)?;
            if cancel.load(Ordering::Relaxed) {
                return Err(CopyFailure::Cancelled);
            }
            to.host.write(&dst, data, Some(now)).map(|_| ()).map_err(conflict)
        }
    }
}

/// Keep the requested destination absent until publication. A failed write or
/// exclusive rename may remove only the staging entry we reserved ourselves.
fn copy_new_file(host: &dyn Host, destination: &str, data: Vec<u8>, cancel: &AtomicBool) -> Result<(), HostError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(HostError::Op(proto::Error::Cancelled));
    }
    let parent = destination.rsplit_once('/').map_or(".", |(parent, _)| if parent.is_empty() { "/" } else { parent });
    for _ in 0..100 {
        let stage = ui::join(parent, &format!(".codeyard-compare-new-{}-{}", std::process::id(), STAGE_COUNTER.fetch_add(1, Ordering::Relaxed)));
        if ui::file_name(&stage).eq_ignore_ascii_case(ui::file_name(destination)) {
            continue;
        }
        match host.create_file(&stage) {
            Err(HostError::Op(proto::Error::AlreadyExists(_))) => continue,
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = host.delete(&stage);
            return Err(HostError::Op(proto::Error::Cancelled));
        }
        let result = host.write(&stage, data, Some(proto::content_hash(&[]))).and_then(|_| {
            if cancel.load(Ordering::Relaxed) {
                return Err(HostError::Op(proto::Error::Cancelled));
            }
            host.rename(&stage, destination)
        });
        if result.is_err() {
            let _ = host.delete(&stage);
        }
        return result;
    }
    Err(HostError::Op(proto::Error::Other(crate::i18n::t("Could not reserve a temporary file for comparison.").into())))
}

/// Files over the agent's size limit go through a temporary copy with the transfer code (streamed, no memory spike).
fn copy_big_file(from: &End, to: &End, alias: Option<&str>, job: &CopyJob, cancel: &AtomicBool) -> Result<(), CopyFailure> {
    let (src, dst) = (ui::join(&from.path, &job.path), ui::join(&to.path, &job.path));
    let stage = stage_dir().map_err(|error| error.to_string())?;
    let result = (|| {
        let shell = alias.map(host::transfer::shell_for);
        let need_shell = || shell.as_ref().ok_or_else(|| crate::i18n::t("the connection was lost").to_string());
        let tmp_dir = stage.join("in");
        let staged = if from.host.is_remote() {
            host::transfer::download(need_shell()?, &src, false, &tmp_dir, cancel, |_| {}).map_err(CopyFailure::from)?
        } else {
            host::transfer::copy_local(Path::new(&src), &tmp_dir, cancel, |_| {}).map_err(CopyFailure::from)?
        };
        if let Expect::Hash(h) = job.source {
            let now = std::fs::File::open(&staged).and_then(|mut f| proto::content_hash_reader(&mut f)).map_err(|e| e.to_string())?;
            if now != h {
                return Err(format!("{} ({})", crate::i18n::t("source"), changed()).into());
            }
        }
        let parent = dst.rsplit_once('/').map_or("/", |(p, _)| if p.is_empty() { "/" } else { p }).to_string();
        if to.host.is_remote() {
            host::transfer::upload(need_shell()?, &staged, &parent, cancel, |_| {}).map(|_| ()).map_err(CopyFailure::from)
        } else {
            host::transfer::copy_local(&staged, Path::new(&parent), cancel, |_| {}).map(|_| ()).map_err(CopyFailure::from)
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
        std::fs::canonicalize(&dir).unwrap()
    }

    fn end(path: &Path) -> End {
        End { host: std::sync::Arc::new(host::local()), path: ui::local_path(path), label: "This computer".into(), machine: "local".into(), local: true }
    }

    fn hooked_host(mut hook: impl FnMut(&proto::Request) -> Option<proto::Response> + Send + 'static) -> std::sync::Arc<dyn Host> {
        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, mut resp_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || {
            let mut requests = std::io::BufReader::new(req_r);
            while let Ok(Some(proto::Frame { id, body })) = proto::read_frame::<proto::Request>(&mut requests) {
                if matches!(body, proto::Request::Bye) {
                    break;
                }
                let response = hook(&body).unwrap_or_else(|| agent::handle(body));
                if proto::write_frame(&mut resp_w, &proto::Frame { id, body: response }).is_err() {
                    break;
                }
            }
        });
        std::sync::Arc::new(host::RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "copy fixture").unwrap())
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
    fn comparison_staging_is_reserved_before_any_contents_are_written() {
        let root = tmp("stage-reservation");
        let stage = stage_dir_at(&root, &AtomicU64::new(0)).unwrap();
        assert!(stage.is_dir(), "a stage must be exclusively reserved before callers can clean it up");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&stage).unwrap().permissions().mode() & 0o777, 0o700);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn comparison_staging_skips_preexisting_files_and_directories() {
        let root = tmp("stage-preexisting");
        let counter = AtomicU64::new(0);
        let name = |index| root.join(format!("codeyard-compare-{}-{index}", std::process::id()));
        std::fs::write(name(0), "keep file").unwrap();
        std::fs::create_dir(name(1)).unwrap();
        std::fs::write(name(1).join("precious.txt"), "keep contents").unwrap();
        let stage = stage_dir_at(&root, &counter).unwrap();
        assert_ne!(stage, name(0), "a preexisting file cannot become our stage");
        assert_ne!(stage, name(1), "a preexisting directory cannot become our stage");
        assert!(stage.is_dir());
        std::fs::write(stage.join("ours.txt"), "new contents").unwrap();
        std::fs::remove_dir_all(stage).unwrap();
        assert_eq!(std::fs::read_to_string(name(0)).unwrap(), "keep file");
        assert_eq!(std::fs::read_to_string(name(1).join("precious.txt")).unwrap(), "keep contents");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn comparison_staging_never_reuses_directory_links() {
        for dangling in [false, true] {
            let root = tmp(if dangling { "stage-dangling-link" } else { "stage-directory-link" });
            let target = root.join("target");
            if !dangling {
                std::fs::create_dir(&target).unwrap();
                std::fs::write(target.join("precious.txt"), "keep contents").unwrap();
            }
            let link = root.join(format!("codeyard-compare-{}-0", std::process::id()));
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &link).unwrap();
            #[cfg(windows)]
            {
                let output = std::process::Command::new("cmd.exe").args(["/D", "/C", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap();
                assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            }
            let stage = stage_dir_at(&root, &AtomicU64::new(0)).unwrap();
            assert_ne!(stage, link);
            std::fs::write(stage.join("ours.txt"), "new contents").unwrap();
            std::fs::remove_dir_all(stage).unwrap();
            assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
            if !dangling {
                assert_eq!(std::fs::read_to_string(target.join("precious.txt")).unwrap(), "keep contents");
                assert_eq!(std::fs::read_dir(&target).unwrap().count(), 1);
            } else {
                assert!(!target.exists());
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn comparison_staging_reports_creation_errors_without_touching_existing_data() {
        let root = tmp("stage-create-error");
        let parent = root.join("file.txt");
        std::fs::write(&parent, "keep file").unwrap();
        assert!(stage_dir_at(&parent, &AtomicU64::new(0)).is_err());
        assert_eq!(std::fs::read_to_string(parent).unwrap(), "keep file");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn copying_a_nested_file_refuses_source_and_destination_parent_links() {
        for source_link in [false, true] {
            let (left, right, outside) = (
                tmp(&format!("parent-link-{source_link}-left")),
                tmp(&format!("parent-link-{source_link}-right")),
                tmp(&format!("parent-link-{source_link}-outside")),
            );
            std::fs::create_dir_all(left.join("pack/inner")).unwrap();
            std::fs::create_dir_all(right.join("pack/inner")).unwrap();
            std::fs::write(left.join("pack/inner/file.txt"), "source content").unwrap();
            std::fs::write(right.join("pack/inner/file.txt"), "original").unwrap();
            let jobs = logic::plan_copy(&rows_of(&left, &right), "pack/inner/file.txt", Direction::LeftToRight).jobs;
            let link = if source_link { left.join("pack") } else { right.join("pack") };
            let target = outside.join("moved-pack");
            std::fs::rename(&link, &target).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &link).unwrap();
            #[cfg(windows)]
            {
                let output = std::process::Command::new("cmd.exe").args(["/D", "/C", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap();
                assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            }
            let outcome = copy_jobs(&end(&left), &end(&right), None, &jobs, &AtomicBool::new(false));
            assert_eq!(outcome.copied, 0, "source link: {source_link}; {outcome:?}");
            assert_eq!(outcome.refused.len(), 1, "{outcome:?}");
            assert_eq!(std::fs::read_to_string(target.join("inner/file.txt")).unwrap(), if source_link { "source content" } else { "original" });
            assert_eq!(std::fs::read_to_string(right.join("pack/inner/file.txt")).unwrap(), "original");
        }
    }

    #[test]
    fn copying_one_nested_file_still_creates_missing_destination_parents() {
        let (left, right) = (tmp("missing-copy-parents-left"), tmp("missing-copy-parents-right"));
        std::fs::create_dir_all(left.join("pack/inner")).unwrap();
        std::fs::write(left.join("pack/inner/file.txt"), "source content").unwrap();
        let jobs = logic::plan_copy(&rows_of(&left, &right), "pack/inner/file.txt", Direction::LeftToRight).jobs;
        let outcome = copy_jobs(&end(&left), &end(&right), None, &jobs, &AtomicBool::new(false));
        assert_eq!(outcome, CopyOutcome { copied: 1, ..Default::default() });
        assert_eq!(std::fs::read_to_string(right.join("pack/inner/file.txt")).unwrap(), "source content");
    }

    #[test]
    fn new_folder_collisions_are_errors_and_block_their_descendants() {
        for is_file in [false, true] {
            let (left, right) = (tmp(&format!("folder-clash-{is_file}-left")), tmp(&format!("folder-clash-{is_file}-right")));
            std::fs::create_dir(left.join("pack")).unwrap();
            std::fs::write(left.join("pack/new.txt"), "source content").unwrap();
            std::fs::write(left.join("package.txt"), "unrelated sibling").unwrap();
            let rows = rows_of(&left, &right);
            let mut jobs = logic::plan_copy(&rows, "pack", Direction::LeftToRight).jobs;
            jobs.extend(logic::plan_copy(&rows, "package.txt", Direction::LeftToRight).jobs);
            if is_file {
                std::fs::write(right.join("pack"), "keep file").unwrap();
            } else {
                std::fs::create_dir(right.join("pack")).unwrap();
                std::fs::write(right.join("pack/precious.txt"), "keep contents").unwrap();
            }
            let outcome = copy_jobs(&end(&left), &end(&right), None, &jobs, &AtomicBool::new(false));
            assert_eq!(outcome.copied, 1, "{outcome:?}");
            assert_eq!(outcome.refused.len(), jobs.len() - 1, "the conflicting folder and its subtree must be refused: {outcome:?}");
            assert_eq!(std::fs::read_to_string(right.join("package.txt")).unwrap(), "unrelated sibling", "a matching basename prefix is not a descendant");
            if is_file {
                assert_eq!(std::fs::read_to_string(right.join("pack")).unwrap(), "keep file");
            } else {
                assert_eq!(std::fs::read_to_string(right.join("pack/precious.txt")).unwrap(), "keep contents");
                assert!(!right.join("pack/new.txt").exists());
            }
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn a_source_folder_removed_or_replaced_since_the_scan_cannot_create_a_destination() {
        for replacement in ["missing", "file", "link"] {
            let (left, right) = (tmp(&format!("changed-folder-{replacement}-left")), tmp(&format!("changed-folder-{replacement}-right")));
            std::fs::create_dir(left.join("empty")).unwrap();
            let jobs = logic::plan_copy(&rows_of(&left, &right), "empty", Direction::LeftToRight).jobs;
            std::fs::remove_dir(left.join("empty")).unwrap();
            if replacement == "file" {
                std::fs::write(left.join("empty"), "now a file").unwrap();
            } else if replacement == "link" {
                let target = left.join("other");
                std::fs::create_dir(&target).unwrap();
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, left.join("empty")).unwrap();
                #[cfg(windows)]
                {
                    let output = std::process::Command::new("cmd.exe").args(["/D", "/C", "mklink", "/J"]).arg(left.join("empty")).arg(target).output().unwrap();
                    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
                }
            }
            let outcome = copy_jobs(&end(&left), &end(&right), None, &jobs, &AtomicBool::new(false));
            assert_eq!(outcome.copied, 0, "{outcome:?}");
            assert_eq!(outcome.refused.len(), 1, "{outcome:?}");
            assert!(!right.join("empty").exists());
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn a_folder_link_appearing_during_creation_cannot_receive_child_copies() {
        let (left, right, outside) = (tmp("folder-link-left"), tmp("folder-link-right"), tmp("folder-link-outside"));
        std::fs::create_dir(left.join("pack")).unwrap();
        std::fs::write(left.join("pack/new.txt"), "source content").unwrap();
        std::fs::write(outside.join("precious.txt"), "keep contents").unwrap();
        let jobs = logic::plan_copy(&rows_of(&left, &right), "pack", Direction::LeftToRight).jobs;
        let target = outside.clone();
        let linked = std::sync::Arc::new(AtomicBool::new(false));
        let observed = linked.clone();
        let mut destination = end(&right);
        destination.host = hooked_host(move |request| {
            if let proto::Request::CreateDir { path } = request {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, path).unwrap();
                #[cfg(windows)]
                {
                    let output =
                        std::process::Command::new("cmd.exe").args(["/D", "/C", "mklink", "/J"]).arg(path.replace('/', "\\")).arg(&target).output().unwrap();
                    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
                }
                observed.store(true, Ordering::Relaxed);
            }
            None
        });
        let outcome = copy_jobs(&end(&left), &destination, None, &jobs, &AtomicBool::new(false));
        assert!(linked.load(Ordering::Relaxed), "the test must create the concurrent directory link");
        assert_eq!(outcome.copied, 0, "{outcome:?}");
        assert_eq!(outcome.refused.len(), jobs.len(), "{outcome:?}");
        assert!(!outside.join("new.txt").exists(), "copying cannot escape through a newly appeared link");
        assert_eq!(std::fs::read_to_string(outside.join("precious.txt")).unwrap(), "keep contents");
    }

    #[test]
    fn a_typed_cancellation_stops_copies_without_becoming_an_error() {
        for phase in ["stat", "read", "write"] {
            let (left, right) = (tmp(&format!("typed-cancel-{phase}-left")), tmp(&format!("typed-cancel-{phase}-right")));
            std::fs::write(left.join("file.txt"), "source content").unwrap();
            std::fs::write(left.join("next.txt"), "next file").unwrap();
            std::fs::write(right.join("file.txt"), "original").unwrap();
            let rows = rows_of(&left, &right);
            let mut jobs = logic::plan_copy(&rows, "file.txt", Direction::LeftToRight).jobs;
            jobs.extend(logic::plan_copy(&rows, "next.txt", Direction::LeftToRight).jobs);
            let peer = hooked_host(move |request| {
                let cancel = matches!(
                    (phase, request),
                    ("stat", proto::Request::Stat { .. }) | ("read", proto::Request::Read { .. }) | ("write", proto::Request::Write { .. })
                );
                cancel.then_some(proto::Response::Error(proto::Error::Cancelled))
            });
            let (mut from, mut to) = (end(&left), end(&right));
            if phase == "write" {
                to.host = peer;
            } else {
                from.host = peer;
            }
            let outcome = copy_jobs(&from, &to, None, &jobs, &AtomicBool::new(false));
            assert_eq!(outcome, CopyOutcome { cancelled: true, ..Default::default() }, "phase: {phase}");
            assert_eq!(std::fs::read_to_string(right.join("file.txt")).unwrap(), "original");
            assert!(!right.join("next.txt").exists(), "no later job runs after a typed cancellation");
        }
    }

    #[test]
    fn a_concurrent_cancel_request_cannot_hide_a_copy_error() {
        let (left, right) = (tmp("cancel-error-left"), tmp("cancel-error-right"));
        std::fs::write(left.join("file.txt"), "source content").unwrap();
        std::fs::write(right.join("file.txt"), "original").unwrap();
        let jobs = logic::plan_copy(&rows_of(&left, &right), "file.txt", Direction::LeftToRight).jobs;
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let on_write = cancel.clone();
        let mut destination = end(&right);
        destination.host = hooked_host(move |request| {
            if let proto::Request::Write { path, .. } = request {
                on_write.store(true, Ordering::Relaxed);
                return Some(proto::Response::Error(proto::Error::PermissionDenied(path.clone())));
            }
            None
        });
        let outcome = copy_jobs(&end(&left), &destination, None, &jobs, &cancel);
        assert!(outcome.cancelled);
        assert_eq!(outcome.refused.len(), 1, "the operation error must remain visible: {outcome:?}");
        assert!(outcome.summary().contains("permission denied"), "{}", outcome.summary());
        assert_eq!(std::fs::read_to_string(right.join("file.txt")).unwrap(), "original");
    }

    #[test]
    fn a_lost_write_reply_is_reported_even_when_cancellation_is_requested() {
        let (left, right) = (tmp("lost-write-left"), tmp("lost-write-right"));
        std::fs::write(left.join("file.txt"), "source content").unwrap();
        std::fs::write(right.join("file.txt"), "original").unwrap();
        let jobs = logic::plan_copy(&rows_of(&left, &right), "file.txt", Direction::LeftToRight).jobs;
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let on_write = cancel.clone();
        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, mut resp_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || {
            let mut requests = std::io::BufReader::new(req_r);
            while let Ok(Some(proto::Frame { id, body })) = proto::read_frame::<proto::Request>(&mut requests) {
                if matches!(body, proto::Request::Bye) {
                    break;
                }
                let is_write = matches!(body, proto::Request::Write { .. });
                let response = agent::handle(body);
                if is_write {
                    assert!(matches!(response, proto::Response::Written { .. }));
                    on_write.store(true, Ordering::Relaxed);
                    break; // The write completed, but its acknowledgement never reaches the client.
                }
                if proto::write_frame(&mut resp_w, &proto::Frame { id, body: response }).is_err() {
                    break;
                }
            }
        });
        let mut destination = end(&right);
        destination.host = std::sync::Arc::new(host::RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "lost write").unwrap());
        let outcome = copy_jobs(&end(&left), &destination, None, &jobs, &cancel);
        assert_eq!(std::fs::read_to_string(right.join("file.txt")).unwrap(), "source content", "the server really wrote the destination");
        assert!(outcome.cancelled);
        assert_eq!(outcome.refused.len(), 1, "a lost reply must not become a clean cancellation: {outcome:?}");
        assert!(!outcome.summary().contains("left alone"), "the destination was changed: {}", outcome.summary());
    }

    #[test]
    fn a_large_comparison_copy_streams_the_complete_file() {
        let (left, right) = (tmp("large-copy-left"), tmp("large-copy-right"));
        let source = left.join("large.bin");
        std::fs::File::create(&source).unwrap().set_len(proto::MAX_FILE + 1).unwrap();
        let from = end(&left);
        let to = end(&right);
        let metadata = from.host.stat(&ui::join(&from.path, "large.bin")).unwrap();
        let job = CopyJob {
            path: "large.bin".into(),
            is_dir: false,
            source: Expect::Stat { size: metadata.size, mtime_ms: metadata.mtime_ms },
            dest: Expect::Absent,
        };
        let outcome = copy_jobs(&from, &to, None, &[job], &AtomicBool::new(false));
        assert_eq!(outcome, CopyOutcome { copied: 1, ..Default::default() });
        let destination = right.join("large.bin");
        assert_eq!(std::fs::metadata(&destination).unwrap().len(), proto::MAX_FILE + 1);
        let hash = |path: &Path| proto::content_hash_reader(&mut std::fs::File::open(path).unwrap()).unwrap();
        assert_eq!(hash(&source), hash(&destination));
        std::fs::remove_dir_all(left).unwrap();
        std::fs::remove_dir_all(right).unwrap();
    }

    #[test]
    fn a_destination_edited_during_a_new_copy_is_never_deleted_on_conflict() {
        let (left, right) = (tmp("copy-race-left"), tmp("copy-race-right"));
        std::fs::write(left.join("file.txt"), "source").unwrap();
        let job = logic::plan_copy(&rows_of(&left, &right), "file.txt", Direction::LeftToRight).jobs;
        let target = right.join("file.txt");
        let concurrent = target.clone();
        let edited = std::sync::Arc::new(AtomicBool::new(false));
        let observed = edited.clone();
        let mut destination = end(&right);
        destination.host = hooked_host(move |request| {
            if matches!(request, proto::Request::Write { .. }) {
                std::fs::write(&concurrent, "concurrent edit").unwrap();
                observed.store(true, Ordering::Relaxed);
            }
            None
        });
        let outcome = copy_jobs(&end(&left), &destination, None, &job, &AtomicBool::new(false));
        assert!(edited.load(Ordering::Relaxed), "the test must reach the concurrent edit");
        assert_eq!(outcome.copied, 0, "{outcome:?}");
        assert_eq!(outcome.refused.len(), 1, "{outcome:?}");
        assert_eq!(std::fs::read_to_string(target).unwrap(), "concurrent edit");
        assert_eq!(std::fs::read_dir(&right).unwrap().count(), 1, "only the destination remains");
    }

    #[test]
    fn new_copy_staging_collisions_and_write_failures_preserve_other_files() {
        for reject_write in [false, true] {
            let root = tmp(if reject_write { "stage-failure" } else { "stage-collision" });
            let planted = std::sync::Arc::new(std::sync::Mutex::new(None::<PathBuf>));
            let observed = planted.clone();
            let peer = hooked_host(move |request| {
                if let proto::Request::CreateFile { path } = request {
                    let mut planted = observed.lock().unwrap();
                    if planted.is_none() {
                        std::fs::write(path, "unrelated file").unwrap();
                        *planted = Some(PathBuf::from(path));
                    }
                }
                if reject_write && let proto::Request::Write { path, .. } = request {
                    return Some(proto::Response::Error(proto::Error::PermissionDenied(path.clone())));
                }
                None
            });
            let target = ui::join(&ui::local_path(&root), "file.txt");
            let result = copy_new_file(peer.as_ref(), &target, b"new contents".to_vec(), &AtomicBool::new(false));
            if reject_write {
                assert!(matches!(result, Err(HostError::Op(proto::Error::PermissionDenied(_)))), "{result:?}");
                assert!(!root.join("file.txt").exists(), "failed writes leave no destination placeholder");
            } else {
                result.unwrap();
                assert_eq!(std::fs::read_to_string(root.join("file.txt")).unwrap(), "new contents");
            }
            assert_eq!(std::fs::read_to_string(planted.lock().unwrap().as_ref().unwrap()).unwrap(), "unrelated file");
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), if reject_write { 1 } else { 2 });
        }
    }

    #[test]
    fn cancellation_after_staging_a_new_file_removes_only_the_stage() {
        let root = tmp("stage-cancelled");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let on_write = cancel.clone();
        let peer = hooked_host(move |request| {
            if matches!(request, proto::Request::Write { .. }) {
                on_write.store(true, Ordering::Relaxed);
            }
            None
        });
        let result = copy_new_file(peer.as_ref(), &ui::join(&ui::local_path(&root), "file.txt"), b"contents".to_vec(), &cancel);
        assert!(matches!(result, Err(HostError::Op(proto::Error::Cancelled))), "{result:?}");
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
    }

    #[test]
    fn cancellation_during_source_or_destination_read_cannot_overwrite_the_destination() {
        for source_side in [true, false] {
            let (left, right) =
                if source_side { (tmp("read-cancel-left"), tmp("read-cancel-right")) } else { (tmp("dest-read-cancel-left"), tmp("dest-read-cancel-right")) };
            std::fs::write(left.join("file.txt"), "source content").unwrap();
            std::fs::write(right.join("file.txt"), "original").unwrap();
            let jobs = logic::plan_copy(&rows_of(&left, &right), "file.txt", Direction::LeftToRight).jobs;
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            let on_read = cancel.clone();
            let mut source = end(&left);
            let peer = hooked_host(move |request| {
                if matches!(request, proto::Request::Read { .. }) {
                    on_read.store(true, Ordering::Relaxed);
                }
                None
            });
            let mut destination = end(&right);
            if source_side {
                source.host = peer;
            } else {
                destination.host = peer;
            }
            let outcome = copy_jobs(&source, &destination, None, &jobs, &cancel);
            assert!(cancel.load(Ordering::Relaxed), "the test must reach the read");
            assert_eq!(outcome.copied, 0, "{outcome:?}");
            assert!(outcome.cancelled, "{outcome:?}");
            assert_eq!(std::fs::read_to_string(right.join("file.txt")).unwrap(), "original");
        }
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
