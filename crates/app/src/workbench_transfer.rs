//! Workbench: file transfer. Upload/download in a remote workspace (files and folders, streamed, with progress and
//! cancel), and copy items dragged from Finder locally. The work runs on a separate thread; the status bar shows
//! progress and a notification appears when it finishes.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::*;

/// Shared state of a running transfer (the worker thread writes, the UI reads).
pub(super) struct TransferRun {
    label: String,
    up: bool,
    bytes: AtomicU64,
    /// 0: unknown (folder download).
    total: AtomicU64,
    cancel: AtomicBool,
    result: Mutex<Option<Result<Done, Fail>>>,
}

/// Result of a finished transfer: the message is built on the UI thread, in the current language.
#[derive(Clone, Debug)]
pub(super) enum Done {
    Uploaded { count: usize, label: String, size: String, dir: String },
    Downloaded { dest: String, size: String },
    Copied { count: usize, label: String, size: String, dir: String },
}

/// Failed transfer: `reason` is the host's own text (shown as is); `partial` is, for multi-item transfers, the stuck
/// item and the number completed (name, completed, total).
#[derive(Clone, Debug)]
pub(super) struct Fail {
    reason: String,
    partial: Option<(String, usize, usize)>,
    cancelled: bool,
}

impl From<host::HostError> for Fail {
    fn from(error: host::HostError) -> Self {
        let cancelled = matches!(error, host::HostError::Op(proto::Error::Cancelled));
        Self { reason: error.to_string().trim_start_matches("disconnected: ").to_string(), partial: None, cancelled }
    }
}

impl TransferRun {
    fn new(label: String, up: bool) -> Arc<Self> {
        Arc::new(TransferRun { label, up, bytes: AtomicU64::new(0), total: AtomicU64::new(0), cancel: AtomicBool::new(false), result: Mutex::new(None) })
    }

    /// Status bar text: "↑ name 42%" (the amount transferred if the total is unknown).
    pub(super) fn status(&self) -> String {
        let (done, total) = (self.bytes.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed));
        let arrow = if self.up { "↑" } else { "↓" };
        match total {
            0 => format!("{arrow} {} · {}", self.label, human_bytes(done)),
            t => format!("{arrow} {} · {}%", self.label, (done * 100 / t.max(1)).min(100)),
        }
    }
}

pub(super) fn human_bytes(n: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024. && i < U.len() - 1 {
        v /= 1024.;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", U[i]) }
}

#[derive(Clone, Debug)]
pub(super) enum Job {
    /// Upload local items to a folder on the server.
    Upload { alias: String, items: Vec<PathBuf>, remote_dir: String },
    /// Download an item from the server into `local_dir` on this computer.
    Download { alias: String, remote: String, is_dir: bool, local_dir: PathBuf },
    /// Locally: copy items dragged from outside into a folder in the workspace.
    CopyIn { items: Vec<PathBuf>, dest_dir: String },
}

impl Job {
    fn label(&self) -> String {
        let name = |p: &str| ui::file_name(p).to_string();
        match self {
            Job::Upload { items, .. } | Job::CopyIn { items, .. } if items.len() == 1 => name(&items[0].to_string_lossy()),
            Job::Upload { items, .. } | Job::CopyIn { items, .. } => crate::i18n::tn("{} item", "{} items", items.len(), &[]),
            Job::Download { remote, .. } => name(remote),
        }
    }
}

fn probe_local_destination(path: &std::path::Path) -> Result<Option<bool>, String> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some(meta.is_dir())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

impl Workbench {
    fn remote_alias(&self) -> Option<String> {
        match &self.remote.conn {
            Conn::Connected { alias } if self.remote.host.is_remote() => Some(alias.clone()),
            _ => None,
        }
    }

    /// Message saying what happened and what to do when not connected (`what`: 0 download, 1 upload, 2 copy here).
    fn not_connected_hint(&self, what: u8) -> String {
        use crate::i18n::{t, tf};
        match (&self.remote.conn, what) {
            (Conn::Local, 0) => t("Downloading works with a connected SSH host: connect first (⌃R).").into(),
            (Conn::Local, 1) => t("Uploading works with a connected SSH host: connect first (⌃R).").into(),
            (Conn::Local, _) => t("Copying files here works with a connected SSH host: connect first (⌃R).").into(),
            (Conn::Connecting { .. }, 0) => t("Downloading has to wait: the connection is still being set up.").into(),
            (Conn::Connecting { .. }, 1) => t("Uploading has to wait: the connection is still being set up.").into(),
            (Conn::Connecting { .. }, _) => t("Copying files here has to wait: the connection is still being set up.").into(),
            (Conn::Reconnecting { .. }, 0) => t("Downloading has to wait: the connection dropped and is reconnecting.").into(),
            (Conn::Reconnecting { .. }, 1) => t("Uploading has to wait: the connection dropped and is reconnecting.").into(),
            (Conn::Reconnecting { .. }, _) => t("Copying files here has to wait: the connection dropped and is reconnecting.").into(),
            (Conn::Failed { alias, .. }, 0) => tf("Downloading needs the connection to {}, which failed: use Reconnect first.", &[alias]),
            (Conn::Failed { alias, .. }, 1) => tf("Uploading needs the connection to {}, which failed: use Reconnect first.", &[alias]),
            (Conn::Failed { alias, .. }, _) => tf("Copying files here needs the connection to {}, which failed: use Reconnect first.", &[alias]),
            (Conn::Connected { .. }, 0) => t("Downloading is not possible in this workspace.").into(),
            (Conn::Connected { .. }, 1) => t("Uploading is not possible in this workspace.").into(),
            (Conn::Connected { .. }, _) => t("Copying files here is not possible in this workspace.").into(),
        }
    }

    pub(super) fn transfer_active(&self) -> Option<&Arc<TransferRun>> {
        self.remote.transfer.as_ref()
    }

    /// Context menu: "Download…" (file or folder) → pick a local folder.
    pub(super) fn start_download(&mut self, remote: String, is_dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(alias) = self.remote_alias() else {
            self.notice = Some(Notice::Info(self.not_connected_hint(0).into()));
            cx.notify();
            return;
        };
        let source_host = self.remote.host.clone();
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(crate::i18n::t("Download Here").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(local_dir) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| {
                    if Arc::ptr_eq(&this.remote.host, &source_host) {
                        this.begin(Job::Download { alias, remote, is_dir, local_dir }, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// "Upload Files Here…" and the palette: pick local files/folders, upload to `remote_dir` on the server.
    pub(super) fn start_upload(&mut self, remote_dir: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(alias) = self.remote_alias() else {
            self.notice = Some(Notice::Info(self.not_connected_hint(1).into()));
            cx.notify();
            return;
        };
        let source_host = self.remote.host.clone();
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: true, multiple: true, prompt: Some(crate::i18n::t("Upload").into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(items))) = rx.await
                && !items.is_empty()
            {
                this.update_in(cx, |this, window, cx| {
                    if Arc::ptr_eq(&this.remote.host, &source_host) {
                        this.begin(Job::Upload { alias, items, remote_dir }, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Dropped from Finder onto the tree: upload on the server, copy locally.
    pub(super) fn drop_external(&mut self, items: Vec<PathBuf>, to_dir: String, window: &mut Window, cx: &mut Context<Self>) {
        if items.is_empty() {
            return;
        }
        let job = match self.remote_alias() {
            Some(alias) => Job::Upload { alias, items, remote_dir: to_dir },
            None if !self.remote.host.is_remote() => Job::CopyIn { items, dest_dir: to_dir },
            None => {
                self.notice = Some(Notice::Error(self.not_connected_hint(2).into()));
                cx.notify();
                return;
            }
        };
        self.begin(job, window, cx);
    }

    /// Palette: "Remote: Upload Files to This Folder…" (folder selected in the explorer, or the root).
    pub(super) fn upload_here(&mut self, _: &UploadHere, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.explorer_target_dir(cx) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a folder first.").into()));
            cx.notify();
            return;
        };
        self.start_upload(dir, window, cx);
    }

    /// Palette: "Remote: Download Active File…".
    pub(super) fn download_active(&mut self, _: &DownloadActive, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_editor().map(|e| e.read(cx).path.clone()).filter(|p| !p.is_empty()) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a file first.").into()));
            cx.notify();
            return;
        };
        self.start_download(path, false, window, cx);
    }

    fn explorer_target_dir(&self, cx: &App) -> Option<String> {
        self.roots.primary.read(cx).selected_dir().or_else(|| self.root(cx))
    }

    /// Check for conflicts (in the background), ask if there are any, then start.
    pub(super) fn begin(&mut self, job: Job, window: &mut Window, cx: &mut Context<Self>) {
        if self.remote.transfer.is_some() {
            self.notice =
                Some(Notice::Info(crate::i18n::t("A transfer is already running. Wait for it to finish or click it in the status bar to cancel.").into()));
            cx.notify();
            return;
        }
        let host = self.remote.host.clone();
        let source_host = host.clone();
        let probe = job.clone();
        // Entries at the target with the same name, and their kinds: for the conflict prompt and to flag file/folder mismatches up front.
        let task = cx.background_executor().spawn(async move {
            let items: Vec<(String, bool)> = match &probe {
                Job::Upload { items, .. } | Job::CopyIn { items, .. } => {
                    items.iter().filter_map(|p| Some((p.file_name()?.to_string_lossy().into_owned(), p.is_dir()))).collect()
                }
                Job::Download { remote, is_dir, .. } => vec![(ui::file_name(remote).to_string(), *is_dir)],
            };
            let mut existing = Vec::new();
            let mut mismatch = None;
            for (name, is_dir) in items {
                let there_is_dir = match &probe {
                    Job::Upload { remote_dir, .. } => match host.stat(&ui::join(remote_dir, &name)) {
                        Ok(meta) => Ok(Some(meta.kind == proto::Kind::Dir)),
                        Err(host::HostError::Op(proto::Error::NotFound(_))) => Ok(None),
                        Err(error) => Err(error.to_string()),
                    },
                    Job::Download { local_dir, .. } => probe_local_destination(&local_dir.join(&name)),
                    Job::CopyIn { dest_dir, .. } => probe_local_destination(&PathBuf::from(ui::join(dest_dir, &name))),
                }?;
                match there_is_dir {
                    Some(d) if d != is_dir => {
                        mismatch.get_or_insert((name, d));
                    }
                    Some(_) => existing.push(name),
                    None => {}
                }
            }
            Ok::<_, String>((existing, mismatch))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if !Arc::ptr_eq(&this.remote.host, &source_host) {
                    return;
                }
                let (existing, mismatch) = match result {
                    Ok(probe) => probe,
                    Err(error) => {
                        this.notice = Some(Notice::Error(crate::i18n::tf("Cannot inspect the transfer destination: {}", &[&error]).into()));
                        cx.notify();
                        return;
                    }
                };
                if let Some((name, there_is_dir)) = mismatch {
                    let m = if there_is_dir {
                        crate::i18n::tf("A folder named “{}” is already there; a file cannot replace it. Rename or remove it first.", &[&name])
                    } else {
                        crate::i18n::tf("A file named “{}” is already there; a folder cannot replace it. Rename or remove it first.", &[&name])
                    };
                    this.notice = Some(Notice::Error(m.into()));
                    cx.notify();
                    return;
                }
                if existing.is_empty() {
                    this.run_job(job, &source_host, cx);
                    return;
                }
                let title = match existing.as_slice() {
                    [one] => crate::i18n::tf("“{}” already exists. Replace it?", &[one]),
                    many => crate::i18n::tn("{} item already exists. Replace it?", "{} items already exist. Replace them?", many.len(), &[]),
                };
                let answer = window.prompt(
                    gpui::PromptLevel::Warning,
                    &title,
                    Some(crate::i18n::t("Files with the same name are overwritten; other files in the folder are kept.")),
                    &[crate::i18n::t("Replace"), crate::i18n::t("Cancel")],
                    cx,
                );
                cx.spawn(async move |this, cx| {
                    if answer.await == Ok(0) {
                        this.update(cx, |this, cx| this.run_job(job, &source_host, cx)).ok();
                    }
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }

    fn run_job(&mut self, job: Job, source_host: &Arc<dyn Host>, cx: &mut Context<Self>) {
        // Conflict checks and prompts yield to other UI work. Revalidate both
        // ownership and exclusivity immediately before starting the mutation.
        if !Arc::ptr_eq(&self.remote.host, source_host) {
            return;
        }
        if self.remote.transfer.is_some() {
            self.notice =
                Some(Notice::Info(crate::i18n::t("A transfer is already running. Wait for it to finish or click it in the status bar to cancel.").into()));
            cx.notify();
            return;
        }
        let up = !matches!(job, Job::Download { .. });
        let run = TransferRun::new(job.label(), up);
        self.remote.transfer = Some(run.clone());
        let host = self.remote.host.clone();
        let monitor_host = host.clone();
        let thread_run = run.clone();
        let thread_job = job.clone();
        std::thread::spawn(move || {
            let result = execute(&thread_job, host, &thread_run);
            *thread_run.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        });
        // Refresh the status bar; report the result when done.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let finished = run.result.lock().unwrap_or_else(|e| e.into_inner()).take();
                let alive = this
                    .update(cx, |this, cx| {
                        let owns_run = this.remote.transfer.as_ref().is_some_and(|active| Arc::ptr_eq(active, &run));
                        if !owns_run || !Arc::ptr_eq(&this.remote.host, &monitor_host) {
                            run.cancel.store(true, Ordering::Relaxed);
                            if owns_run {
                                this.remote.transfer = None;
                                cx.notify();
                            }
                            return false;
                        }
                        if let Some(result) = finished.clone() {
                            this.remote.transfer = None;
                            this.finish_transfer(&job, result, cx);
                        }
                        cx.notify();
                        finished.is_none()
                    })
                    .unwrap_or(false);
                if !alive {
                    if finished.is_none() {
                        run.cancel.store(true, Ordering::Relaxed);
                    }
                    break;
                }
            }
        })
        .detach();
    }

    fn finish_transfer(&mut self, job: &Job, result: Result<Done, Fail>, cx: &mut Context<Self>) {
        match result {
            Ok(done) => {
                let msg = match done {
                    Done::Uploaded { count: 1, label, size, dir } => crate::i18n::tf("Uploaded “{}” ({}) to {}", &[&label, &size, &dir]),
                    Done::Uploaded { count, size, dir, .. } => {
                        crate::i18n::tn("Uploaded {} item ({}) to {}", "Uploaded {} items ({}) to {}", count, &[&size, &dir])
                    }
                    Done::Downloaded { dest, size } => crate::i18n::tf("Downloaded to {} ({})", &[&dest, &size]),
                    Done::Copied { count: 1, label, size, dir } => crate::i18n::tf("Copied “{}” ({}) to {}", &[&label, &size, &dir]),
                    Done::Copied { count, size, dir, .. } => crate::i18n::tn("Copied {} item ({}) to {}", "Copied {} items ({}) to {}", count, &[&size, &dir]),
                };
                // Make the upload/copy target folder visible in the tree.
                if let Job::Upload { remote_dir: dir, .. } | Job::CopyIn { dest_dir: dir, .. } = job {
                    let dir = dir.clone();
                    self.roots.primary.update(cx, |t, cx| t.reload(&dir, cx));
                    self.roots.file_index = None;
                }
                self.notice = Some(Notice::Info(msg.into()));
            }
            Err(e) if e.cancelled => self.notice = Some(Notice::Info(crate::i18n::t("Transfer cancelled.").into())),
            Err(e) => {
                let reason = match e.partial {
                    Some((name, done, total)) => {
                        crate::i18n::tf("{} (“{}” failed; {} of {} items were transferred before it)", &[&e.reason, &name, &done, &total])
                    }
                    None => e.reason,
                };
                self.notice = Some(Notice::Error(crate::i18n::tf("Transfer failed: {}", &[&reason]).into()));
            }
        }
    }

    /// Click on the transfer in the status bar: ask whether to cancel.
    pub(super) fn ask_cancel_transfer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.remote.transfer.clone() else { return };
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &crate::i18n::tf("Cancel “{}”?", &[&run.label]),
            Some(crate::i18n::t("Temporary transfer data is removed. Changes already made at the destination may remain.")),
            &[crate::i18n::t("Cancel Transfer"), crate::i18n::t("Keep Going")],
            cx,
        );
        cx.spawn(async move |_, _| {
            if answer.await == Ok(0) {
                run.cancel.store(true, Ordering::Relaxed);
            }
        })
        .detach();
    }

    #[cfg(test)]
    pub fn transfer_status(&self) -> Option<String> {
        self.remote.transfer.as_ref().map(|r| r.status())
    }
}

/// On the worker thread: do the work, write progress to `run`, return the result message to show the user.
fn execute(job: &Job, host: Arc<dyn Host>, run: &TransferRun) -> Result<Done, Fail> {
    let step = |n: u64| {
        run.bytes.fetch_add(n, Ordering::Relaxed);
    };
    match job {
        Job::Upload { alias, items, remote_dir } => {
            run.total.store(items.iter().map(|p| host::transfer::local_size(p)).sum(), Ordering::Relaxed);
            let shell = host::transfer::shell_for(alias);
            for (done, item) in items.iter().enumerate() {
                host::transfer::upload(&shell, item, remote_dir, &run.cancel, step).map_err(|e| partial(done, items.len(), item, e.into()))?;
            }
            Ok(Done::Uploaded { count: items.len(), label: run.label.clone(), size: human_bytes(run.bytes.load(Ordering::Relaxed)), dir: remote_dir.clone() })
        }
        Job::Download { alias, remote, is_dir, local_dir } => {
            if !*is_dir && let Ok(meta) = host.stat(remote) {
                run.total.store(meta.size, Ordering::Relaxed);
            }
            let shell = host::transfer::shell_for(alias);
            let dest = host::transfer::download(&shell, remote, *is_dir, local_dir, &run.cancel, step).map_err(Fail::from)?;
            Ok(Done::Downloaded { dest: dest.display().to_string(), size: human_bytes(run.bytes.load(Ordering::Relaxed)) })
        }
        Job::CopyIn { items, dest_dir } => {
            run.total.store(items.iter().map(|p| host::transfer::local_size(p)).sum(), Ordering::Relaxed);
            for (done, item) in items.iter().enumerate() {
                host::transfer::copy_local(item, std::path::Path::new(dest_dir), &run.cancel, step).map_err(|e| partial(done, items.len(), item, e.into()))?;
            }
            Ok(Done::Copied { count: items.len(), label: run.label.clone(), size: human_bytes(run.bytes.load(Ordering::Relaxed)), dir: dest_dir.clone() })
        }
    }
}

/// If one of several items fails, say how many completed and which one got stuck (a cancel message stays as is).
fn partial(done: usize, total: usize, item: &std::path::Path, mut error: Fail) -> Fail {
    if error.cancelled || total == 1 {
        return error;
    }
    let name = item.file_name().map_or_else(|| item.display().to_string(), |n| n.to_string_lossy().into_owned());
    error.partial = Some((name, done, total));
    error
}

#[cfg(test)]
#[path = "workbench_transfer_tests.rs"]
mod tests;
