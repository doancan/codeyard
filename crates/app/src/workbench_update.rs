//! Workbench: in-app updates, the part with state and behaviour. `update.rs` and `update_install.rs` know how to find, download,
//! verify and install a release; this file decides when to do it, what the dialog says and what every button does. Drawing is in
//! `workbench_view_update.rs`; the network and the installer sit behind `UpdateBackend` so tests can script them.
//!
//! The rules, in one place:
//! - `update.check` = `ask` and `manual` never contact the network by themselves; only `daily` does: one successful check in
//!   24 hours, and after failed ones (offline) a retry every few hours, at most three failures a day.
//! - A check the user asked for shows its errors; a background check logs them to stderr and stays quiet.
//! - A background check that finds a release shows one notice per version and a status bar item, never a dialog.
//! - Skipping a version silences those two for it; a manual check still offers it.
//! - A list that offers a version lower than the highest ever offered here is a replay: logged, not offered.
//! - Quitting during a download cancels it and removes its files; quitting (or closing a window) while ANY window is swapping
//!   the app in asks first or waits, so the result of the swap always has a window to land in.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt as _;

use super::*;
use crate::i18n::{t, tf};
use crate::markdown;
use crate::update::{ErrorKind, Policy, Progress, Release, UpdateError, higher_version, is_rollback, retry_allowed, should_check_now};
pub(crate) use crate::update_backend::UpdateBackend;
use crate::update_install::{InstallKind, InstallPlan, ManualReason};
use crate::update_restart;

/// Keys in the state database (machine-written values stay out of `settings.jsonc`).
const LAST_CHECK: &str = "update.last_check";
const SKIPPED: &str = "update.skipped";
const INSTALLED: &str = "update.installed";
const NOTIFIED: &str = "update.notified";
const ASK_SNOOZED_UNTIL: &str = "update.ask_snoozed_until";
/// The highest version a verified list has offered here; a lower one later is a replayed old list.
const HIGHEST_SEEN: &str = "update.highest_seen";

/// The first-run question comes a few seconds after the window is up, and waits for a calm moment (no picker, menu or dialog open).
const ASK_AFTER: Duration = Duration::from_secs(5);
const ASK_RETRY: Duration = Duration::from_secs(3);
const ASK_ATTEMPTS: u32 = 40;
/// A daily check starts this long after the question, so it never competes with the window opening.
const CHECK_AFTER_ASK: Duration = Duration::from_secs(10);
/// The question does not appear while keys are being pressed: a letter typed a moment ago would answer it.
const QUIET_BEFORE_ASK: Duration = Duration::from_secs(3);
/// Release notes are drawn up to this many bytes (a note of 256 KiB makes tens of thousands of blocks); the rest is on GitHub.
const NOTES_RENDER_MAX: usize = 64 << 10;
/// What a screen reader is given of them.
const NOTES_SPEECH_MAX: usize = 8 << 10;
/// A session that runs for days looks again every few hours; `should_check_now` keeps it to once in 24 and `retry_allowed`
/// to three failures a day.
const CHECK_EVERY: Duration = Duration::from_secs(3 * 60 * 60);
/// Closing the question with Esc is not an answer: it comes back after this long, not at every start.
const ASK_SNOOZE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Enter and Space do nothing for this long after the question appears on its own, so a key typed into the editor a moment
/// earlier cannot answer it.
const ARM_AFTER: Duration = Duration::from_millis(700);
/// How long quitting waits for the update thread to finish its cleanup.
const QUIT_GRACE: Duration = Duration::from_secs(5);
/// Buttons in the dialog at most (focus handles are made once).
pub(super) const MAX_BUTTONS: usize = 4;

/// What the updater is doing. `Release` is `Clone`, so states are copied around freely.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum UpdateState {
    Idle,
    Checking,
    /// Only after a check the user asked for; a background check that finds nothing stays `Idle`.
    UpToDate,
    Available(Release),
    Downloading {
        release: Release,
        progress: Progress,
    },
    /// Hash is verified; the new app is being put in place. Not cancellable.
    Installing {
        release: Release,
    },
    /// The new version is installed; it runs after a restart. `plan` is how to start it (`None`: unknown, Windows' setup does it).
    Ready {
        version: String,
        plan: Option<InstallPlan>,
    },
    Error(Failure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Check,
    Download,
    Install,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Failure {
    pub(super) kind: ErrorKind,
    /// What the plain-language lead should say beyond the kind.
    pub(super) cause: Cause,
    /// The error's own English text: technical, shown under the plain-language lead.
    pub(super) detail: String,
    /// The release that was being downloaded or installed (`Retry` downloads it again).
    pub(super) release: Option<Release>,
    pub(super) phase: Phase,
}

/// The few errors whose advice differs from the rest of their kind.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Cause {
    Other,
    /// The secure connection failed: a wrong clock, a proxy or security software are the usual reasons.
    Tls,
    /// The release has no signature.
    Unsigned,
    DiskFull,
    /// The swap failed and the previous version could not be put back.
    RollbackFailed {
        app: std::path::PathBuf,
        backup: std::path::PathBuf,
    },
}

impl Cause {
    fn of(e: &UpdateError) -> Self {
        match e {
            UpdateError::Tls(_) => Cause::Tls,
            UpdateError::Unsigned => Cause::Unsigned,
            UpdateError::NoSpace => Cause::DiskFull,
            UpdateError::RollbackFailed { app, backup, .. } => Cause::RollbackFailed { app: app.clone(), backup: backup.clone() },
            _ => Cause::Other,
        }
    }
}

/// What is on screen. The question is separate from the update dialog because it is shown without any update in sight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dialog {
    Closed,
    Ask,
    Update,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UpdateAction {
    CheckDaily,
    OnlyWhenAsked,
    CancelCheck,
    CancelDownload,
    Install,
    Skip,
    ReleasePage,
    Retry,
    RestartNow,
    /// Hides the dialog; what stays depends on the state (a download keeps running).
    Close,
}

/// What the dialog's own elements ask of the workbench. The card is drawn over the editor or in a window of its own; either way
/// it only sends these (see `workbench_view_update.rs`), and the workbench answers in the window the editor lives in.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum UpdateCmd {
    Action(UpdateAction),
    /// Esc.
    Dismiss,
    /// Enter or Space with this button focused (`None`: focus is elsewhere in the dialog, so the primary button answers).
    Confirm(Option<usize>),
    /// A key pressed while the notes have the focus; the answer says whether they scrolled.
    NotesKey(String),
    /// ⌘Q while the dialog's own window has the keyboard.
    Quit,
}

/// What ends the application.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum QuitMode {
    Quit,
    /// Start the new version once this process has exited (`None`: nothing to start, the installer does it).
    Restart(Option<InstallPlan>),
}

/// Messages from the worker thread.
pub(super) enum Msg {
    Checked(Result<Option<Release>, UpdateError>),
    Progress(Progress),
    Installing,
    Done(Result<InstallPlan, UpdateError>),
}

/// The worker thread of one operation. Dropping it cancels the transfer and stops listening to it.
pub(super) struct Run {
    id: u64,
    cancel: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    _task: gpui::Task<()>,
}

impl Drop for Run {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

// --- What the dialog shows (state in, text out; the view only draws it) ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Accent,
    Success,
    Danger,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Body {
    None,
    Paragraphs(Vec<SharedString>),
    /// Release notes, with an optional line above them. `text` is the notes as plain text, for the accessibility tree.
    Notes {
        note: Option<SharedString>,
        text: SharedString,
    },
    /// `fraction`: how far along (`None`: not known yet).
    Progress {
        fraction: Option<f32>,
        caption: SharedString,
    },
    Working(SharedString),
    Failure {
        lead: SharedString,
        detail: SharedString,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct DialogButton {
    pub(super) action: UpdateAction,
    pub(super) label: SharedString,
    pub(super) primary: bool,
    /// Drawn in the group on the left, away from the answer buttons.
    pub(super) start: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct DialogModel {
    /// Icon name; `loader` is drawn as a spinner.
    pub(super) icon: &'static str,
    pub(super) tone: Tone,
    pub(super) title: SharedString,
    pub(super) subtitle: Option<SharedString>,
    pub(super) body: Body,
    /// In tab order.
    pub(super) buttons: Vec<DialogButton>,
    /// What Esc does.
    pub(super) escape: UpdateAction,
}

impl DialogModel {
    /// Title and text in reading order, for the accessibility tree.
    pub(super) fn text(&self) -> String {
        let mut parts: Vec<String> = vec![self.title.to_string()];
        parts.extend(self.subtitle.iter().map(|s| s.to_string()));
        match &self.body {
            Body::None => {}
            Body::Notes { note, text } => parts.extend(note.iter().chain((!text.is_empty()).then_some(text)).map(|t| t.to_string())),
            Body::Paragraphs(lines) => parts.extend(lines.iter().map(|l| l.to_string())),
            Body::Progress { caption, .. } => parts.push(caption.to_string()),
            Body::Working(text) => parts.push(text.to_string()),
            Body::Failure { lead, detail } => parts.extend([lead.to_string(), detail.to_string()]),
        }
        parts.join("\n")
    }
}

/// The status bar item.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StatusItem {
    pub(super) icon: &'static str,
    pub(super) tone: Tone,
    pub(super) label: SharedString,
    pub(super) tip: SharedString,
}

/// What the Ready dialog may say about the restart; each flag is something that will really happen, given the settings.
#[derive(Clone, Copy)]
struct Promise {
    /// A window is on an SSH host: it reconnects, which may ask for a password.
    remote: bool,
    /// The tabs come back (`window.restoreSession`).
    files: bool,
    /// Unsaved edits come back too (`window.restoreSession` and `files.hotExit`).
    unsaved: bool,
}

impl Promise {
    fn paragraphs(self) -> Vec<String> {
        let mut text = t("Codeyard will reopen your windows.").to_string();
        if self.unsaved {
            text = format!("{text} {}", t("Your open files and unsaved changes come back after the restart."));
        } else if self.files {
            text = format!("{text} {}", t("Your open files come back after the restart."));
        }
        let mut paragraphs = vec![text];
        if self.remote {
            paragraphs.push(t("Windows on a server reconnect, which can ask for a password.").to_string());
        }
        paragraphs
    }
}

fn button(action: UpdateAction, label: &str, primary: bool) -> DialogButton {
    DialogButton { action, label: label.to_string().into(), primary, start: false }
}

fn start_button(action: UpdateAction, label: &str) -> DialogButton {
    DialogButton { action, label: label.to_string().into(), primary: false, start: true }
}

/// "12.3" for a size in megabytes (1 MB = 2^20 bytes, as file managers show it).
fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1u64 << 20) as f64)
}

/// The Releases page of GitHub for `version`, or the list when no version is known.
pub(super) fn release_url(version: Option<&str>) -> String {
    let repository = env!("CARGO_PKG_REPOSITORY");
    match version {
        Some(v) => format!("{repository}/releases/tag/v{v}"),
        None => format!("{repository}/releases/latest"),
    }
}

impl Failure {
    pub(super) fn title(&self) -> String {
        match (self.kind, self.phase) {
            (ErrorKind::Network, Phase::Check) | (ErrorKind::NoRelease, _) => t("Couldn't check for updates"),
            (ErrorKind::Network, _) => t("Couldn't download the update"),
            (ErrorKind::Untrusted, _) => t("The update couldn't be verified"),
            _ => t("The update couldn't be installed"),
        }
        .to_string()
    }

    fn lead(&self) -> String {
        match (&self.cause, self.kind) {
            (Cause::Unsigned, _) => t("Nothing was installed. This release isn't signed, so Codeyard can't verify it."),
            (Cause::Tls, _) => t("Codeyard couldn't set up a secure connection to the update server. Check that your date and time are right and that no proxy or security software is blocking it, then try again."),
            (Cause::DiskFull, _) => t("There isn't enough free disk space for the download. Free some space and try again."),
            (Cause::RollbackFailed { app, backup }, _) => {
                return tf(
                    "The update failed part-way and Codeyard couldn't put the previous version back. It may be at {0}. If Codeyard doesn't start, move it back to {1}.",
                    &[&backup.display(), &app.display()],
                );
            }
            (Cause::Other, ErrorKind::Network) => t("Codeyard couldn't reach the update server. Check your internet connection and try again."),
            (Cause::Other, ErrorKind::NoRelease) => t("GitHub has no release with a checksum list yet, so there is nothing to compare this version with. You can look at the release page."),
            (Cause::Other, ErrorKind::Untrusted) => t("Nothing was installed. What the update server sent didn't match the signed checksums, so Codeyard discarded it."),
            _ => t("The new version couldn't be installed from here. You can download it from the release page instead."),
        }
        .to_string()
    }

    /// "Try Again" makes sense: the trouble can go away by itself or by the user's doing.
    fn can_retry(&self) -> bool {
        self.kind == ErrorKind::Network || self.cause == Cause::DiskFull
    }

    fn from_error(e: &UpdateError, release: Option<Release>, phase: Phase) -> Self {
        Failure { kind: e.kind(), cause: Cause::of(e), detail: e.to_string(), release, phase }
    }
}

/// Why this copy cannot replace itself, in the words of the dialog.
fn manual_note(reason: &ManualReason) -> String {
    match reason {
        ManualReason::DevelopmentBuild => t("This is a development build and can't update itself. Download the new version yourself."),
        ManualReason::NotInstalled => t("This copy isn't installed as an app, so download the new version yourself."),
        ManualReason::ReadOnlyFolder(dir) => {
            return tf(
                "Codeyard can't replace itself in {}: your account can't write there. Download the new version and drag it in yourself.",
                &[&dir.display()],
            );
        }
        ManualReason::Portable => t("This is the portable version, which can't update itself. Download the new version and replace it yourself."),
        ManualReason::OtherSystem => t("Codeyard can't update itself on this system. Download the new version yourself."),
    }
    .to_string()
}

impl UpdateState {
    /// Something is running on a worker thread right now.
    pub(super) fn is_working(&self) -> bool {
        matches!(self, UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Installing { .. })
    }

    fn version(&self) -> Option<&str> {
        match self {
            UpdateState::Available(r) | UpdateState::Downloading { release: r, .. } | UpdateState::Installing { release: r } => Some(&r.version),
            UpdateState::Ready { version, .. } => Some(version),
            UpdateState::Error(f) => f.release.as_ref().map(|r| r.version.as_str()),
            _ => None,
        }
    }
}

/// The notes as blocks to draw and as text to speak. Past `NOTES_RENDER_MAX` bytes (cut at a line) the rest is left to GitHub; the
/// spoken text is shorter still and names the release page.
fn render_notes(notes: &str, release_page: &str) -> (Vec<markdown::Block>, String) {
    let longer = notes.len() > NOTES_RENDER_MAX;
    let shown = if longer { cut_at_line(notes, NOTES_RENDER_MAX) } else { notes };
    let mut blocks = markdown::parse(shown);
    if longer {
        blocks.push(markdown::Block::Paragraph(vec![markdown::Span {
            text: t("The release notes are longer: read the rest on GitHub.").into(),
            style: Default::default(),
        }]));
    }
    let mut speech = markdown::plain_text(&blocks);
    if speech.len() > NOTES_SPEECH_MAX {
        speech.truncate(cut_at_line(&speech, NOTES_SPEECH_MAX).len());
        speech.push('\n');
        speech.push_str(&tf("The release notes are longer: read the rest at {}", &[&release_page]));
    }
    (blocks, speech)
}

/// The longest prefix of `text` of at most `max` bytes that ends at a line end (or, for one huge line, at a character).
fn cut_at_line(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    head.rfind('\n').map_or(head, |n| &head[..n])
}

/// Where the notes scroll to (vertical offset: 0 at the top, `-max` at the end) for a key; `None` for any other key.
fn notes_scroll_target(key: &str, offset: f32, page: f32, max: f32) -> Option<f32> {
    const LINE: f32 = 20.;
    let target = match key {
        "down" => offset - LINE,
        "up" => offset + LINE,
        // A page less one line, so the line at the edge stays in view.
        "pagedown" => offset - (page - LINE).max(LINE),
        "pageup" => offset + (page - LINE).max(LINE),
        "home" => 0.,
        "end" => -max,
        _ => return None,
    };
    Some(target.clamp(-max.max(0.), 0.))
}

/// Whether a window handle still names an open window.
pub(super) fn window_is_open<T: 'static>(handle: gpui::WindowHandle<T>, cx: &App) -> bool {
    cx.windows().iter().any(|w| w.window_id() == handle.window_id())
}

fn progress_fraction(p: &Progress) -> Option<f32> {
    p.total.filter(|t| *t > 0).map(|t| (p.done as f64 / t as f64).clamp(0., 1.) as f32)
}

impl Workbench {
    /// What the dialog shows for the current state, or `None` while it is closed.
    pub(super) fn dialog_model(&self, cx: &App) -> Option<DialogModel> {
        use UpdateAction as A;
        let version = self.update.backend.version().to_string();
        match (self.update.dialog, &self.update.state) {
            (Dialog::Closed, _) => None,
            (Dialog::Ask, _) => Some(DialogModel {
                icon: "refresh-cw",
                tone: Tone::Accent,
                title: t("Check for updates automatically?").into(),
                subtitle: None,
                body: Body::Paragraphs(vec![
                    t("Once a day, Codeyard downloads two small files from github.com to look for a newer version. GitHub sees your IP address, nothing about your files.").into(),
                    t("Change this in Settings (update.check). Check for Updates… always works.").into(),
                ]),
                buttons: vec![button(A::OnlyWhenAsked, t("Only When I Ask"), false), button(A::CheckDaily, t("Check Daily"), true)],
                escape: A::Close,
            }),
            (Dialog::Update, UpdateState::Idle) => None,
            (Dialog::Update, UpdateState::Checking) => Some(DialogModel {
                icon: "loader",
                tone: Tone::Accent,
                title: t("Checking for updates…").into(),
                subtitle: None,
                body: Body::None,
                buttons: vec![button(A::CancelCheck, t("Cancel"), true)],
                escape: A::CancelCheck,
            }),
            (Dialog::Update, UpdateState::UpToDate) => Some(DialogModel {
                icon: "check",
                tone: Tone::Success,
                title: t("Codeyard is up to date").into(),
                subtitle: Some(tf("You have {}, the latest version.", &[&version]).into()),
                body: Body::None,
                buttons: vec![button(A::Close, t("Close"), true)],
                escape: A::Close,
            }),
            (Dialog::Update, UpdateState::Available(release)) => {
                let can_install = !matches!(self.update.kind, InstallKind::Manual(_)) && release.asset.is_some();
                let note = match (&self.update.kind, release.asset.is_some()) {
                    (InstallKind::Manual(reason), _) => Some(manual_note(reason).into()),
                    (_, false) => Some(t("This release has no installer for this system. Download it from the release page.").into()),
                    _ => None,
                };
                let mut buttons = vec![start_button(A::Skip, t("Skip This Version"))];
                if can_install {
                    buttons.push(start_button(A::ReleasePage, t("Release Page")));
                }
                buttons.push(button(A::Close, t("Later"), false));
                buttons.push(if can_install {
                    button(A::Install, t("Install and Restart"), true)
                } else {
                    button(A::ReleasePage, t("Download Page"), true)
                });
                Some(DialogModel {
                    icon: "arrow-down",
                    tone: Tone::Accent,
                    title: tf("Codeyard {} is available", &[&release.version]).into(),
                    subtitle: Some(tf("You have {}", &[&version]).into()),
                    body: Body::Notes { note, text: self.update.notes_text.clone().into() },
                    buttons,
                    escape: A::Close,
                })
            }
            (Dialog::Update, UpdateState::Downloading { release, progress }) => {
                let cancelling = self.update.run.as_ref().is_some_and(|r| r.cancel.load(Ordering::Relaxed));
                let caption = match (cancelling, progress.total) {
                    (true, _) => t("Cancelling…").to_string(),
                    (_, Some(total)) => tf("{0} of {1} MB", &[&megabytes(progress.done), &megabytes(total)]),
                    (_, None) if progress.done == 0 => t("Starting the download…").to_string(),
                    (_, None) => format!("{} MB", megabytes(progress.done)),
                };
                Some(DialogModel {
                    icon: "arrow-down",
                    tone: Tone::Accent,
                    title: tf("Downloading Codeyard {}", &[&release.version]).into(),
                    subtitle: None,
                    body: Body::Progress { fraction: progress_fraction(progress), caption: caption.into() },
                    buttons: vec![button(A::CancelDownload, t("Cancel"), false), button(A::Close, t("Hide"), true)],
                    escape: A::Close,
                })
            }
            (Dialog::Update, UpdateState::Installing { release }) => Some(DialogModel {
                icon: "loader",
                tone: Tone::Accent,
                title: tf("Installing Codeyard {}", &[&release.version]).into(),
                subtitle: None,
                body: Body::Working(
                    if matches!(self.update.kind, InstallKind::WindowsInstalled) { t("Installing — Codeyard will restart.") } else { t("Verifying and installing…") }.into(),
                ),
                buttons: vec![button(A::Close, t("Hide"), true)],
                escape: A::Close,
            }),
            (Dialog::Update, UpdateState::Ready { .. }) => {
                let mut paragraphs: Vec<SharedString> = vec![t("Restart Codeyard to finish.").into()];
                let s = cx.global::<Settings>();
                let (folders, _) = self.reopenable_windows(cx);
                let promised = (self.persist.data_dir.is_some() && !folders.is_empty()).then_some(Promise {
                    remote: folders.iter().any(|f| f.host.is_some()),
                    files: s.restore_session,
                    unsaved: s.restore_session && s.hot_exit,
                });
                paragraphs.extend(promised.into_iter().flat_map(Promise::paragraphs).map(SharedString::from));
                Some(DialogModel {
                    icon: "check",
                    tone: Tone::Success,
                    title: t("Update installed").into(),
                    subtitle: None,
                    body: Body::Paragraphs(paragraphs),
                    buttons: vec![button(A::Close, t("Later"), false), button(A::RestartNow, t("Restart Now"), true)],
                    escape: A::Close,
                })
            }
            (Dialog::Update, UpdateState::Error(f)) => {
                let mut buttons = Vec::new();
                if f.can_retry() {
                    buttons.push(start_button(A::ReleasePage, t("Release Page")));
                    buttons.push(button(A::Close, t("Close"), false));
                    buttons.push(button(A::Retry, t("Try Again"), true));
                } else {
                    buttons.push(button(A::Close, t("Close"), false));
                    buttons.push(button(A::ReleasePage, t("Release Page"), true));
                }
                Some(DialogModel {
                    icon: "alert",
                    tone: Tone::Danger,
                    title: f.title().into(),
                    subtitle: None,
                    body: Body::Failure { lead: f.lead().into(), detail: f.detail.clone().into() },
                    buttons,
                    escape: A::Close,
                })
            }
        }
    }

    /// The status bar item: shown while an update is on offer, on its way or waiting for a restart.
    pub(super) fn update_status_item(&self) -> Option<StatusItem> {
        let version = self.update.state.version()?.to_string();
        let (icon, tone, label, tip) = match &self.update.state {
            UpdateState::Available(_) => {
                ("arrow-down", Tone::Accent, tf("Update {}", &[&version]), tf("Codeyard {} is available. Click for details.", &[&version]))
            }
            UpdateState::Downloading { progress, .. } => {
                let label = match progress_fraction(progress) {
                    Some(f) => tf("Updating… {}%", &[&((f * 100.).floor() as u32)]),
                    None => t("Updating…").to_string(),
                };
                ("loader", Tone::Accent, label, tf("Downloading Codeyard {}. Click for details.", &[&version]))
            }
            UpdateState::Installing { .. } => {
                ("loader", Tone::Accent, t("Installing…").to_string(), tf("Installing Codeyard {}. Click for details.", &[&version]))
            }
            UpdateState::Ready { .. } => {
                ("refresh-cw", Tone::Success, t("Restart to update").to_string(), tf("Codeyard {} is installed. Click for details.", &[&version]))
            }
            UpdateState::Error(_) => ("alert", Tone::Danger, t("Update failed").to_string(), t("The update failed. Click for details.").to_string()),
            _ => return None,
        };
        Some(StatusItem { icon, tone, label: label.into(), tip: tip.into() })
    }

    // --- Commands and buttons ---

    /// "Check for Updates…": works whatever `update.check` says.
    pub(super) fn check_for_updates(&mut self, _: &super::CheckForUpdates, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_check(true, window, cx);
    }

    pub(super) fn update_action(&mut self, action: UpdateAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            UpdateAction::CheckDaily => self.choose_policy(Policy::Daily, window, cx),
            UpdateAction::OnlyWhenAsked => self.choose_policy(Policy::Manual, window, cx),
            UpdateAction::CancelCheck => self.cancel_check(window, cx),
            UpdateAction::CancelDownload => self.cancel_download(cx),
            UpdateAction::Install => self.start_update_download(window, cx),
            UpdateAction::Skip => self.skip_version(window, cx),
            UpdateAction::ReleasePage => {
                let version = self.update.state.version().map(str::to_string);
                cx.open_url(&release_url(version.as_deref()));
            }
            UpdateAction::Retry => self.retry(window, cx),
            UpdateAction::RestartNow => self.restart_now(window, cx),
            UpdateAction::Close => self.close_update_dialog(window, cx),
        }
    }

    /// Runs what a dialog element asked for. `window` is the one with the editor in it. `true`: the key was used.
    pub(super) fn update_cmd(&mut self, cmd: UpdateCmd, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match cmd {
            UpdateCmd::Action(action) => self.update_action(action, window, cx),
            UpdateCmd::Dismiss => self.dismiss_update_dialog(window, cx),
            UpdateCmd::Confirm(focused) => self.confirm_update_button(focused, window, cx),
            UpdateCmd::NotesKey(key) => return self.scroll_notes(&key, cx),
            UpdateCmd::Quit => {
                window.activate_window();
                self.quit(&super::Quit, window, cx);
            }
        }
        true
    }

    /// Esc.
    pub(super) fn dismiss_update_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(model) = self.dialog_model(cx) {
            self.update_action(model.escape, window, cx);
        }
    }

    /// Enter and Space: the focused button, or the primary one when focus is elsewhere in the dialog.
    fn confirm_update_button(&mut self, focused: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.update.dialog == Dialog::Ask && !self.update.armed {
            return;
        }
        let Some(model) = self.dialog_model(cx) else { return };
        let focused = focused.filter(|i| *i < model.buttons.len());
        let chosen = focused.map(|i| &model.buttons[i]).or_else(|| model.buttons.iter().find(|b| b.primary));
        if let Some(b) = chosen {
            self.update_action(b.action, window, cx);
        }
    }

    pub(super) fn open_update_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.update.dialog == Dialog::Closed {
            self.update.return_focus = window.focused(cx);
        }
        self.update.dialog = Dialog::Update;
        self.focus_update_primary(window, cx);
        cx.notify();
    }

    fn close_update_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.update.dialog {
            Dialog::Closed => return,
            // Not answering is not an answer: stay undecided, and ask again later rather than at every start.
            Dialog::Ask => {
                let until = self.update.backend.now() + ASK_SNOOZE;
                self.remember(ASK_SNOOZED_UNTIL, &unix_ms(until).to_string());
            }
            Dialog::Update => {}
        }
        self.update.dialog = Dialog::Closed;
        // What was only worth showing in the dialog is dropped with it. So is an error the user has read: its status item goes
        // away, and a background check is no longer held back by it (the next one finds the release again).
        if matches!(self.update.state, UpdateState::UpToDate | UpdateState::Error(_)) {
            self.update.state = UpdateState::Idle;
        }
        match self.update.return_focus.take() {
            Some(handle) => window.focus(&handle),
            None => window.focus(&self.focus_handle),
        }
        cx.notify();
    }

    /// Focus goes to the primary button, so Enter does what the dialog recommends.
    pub(super) fn focus_update_primary(&mut self, window: &mut Window, cx: &mut App) {
        let Some(model) = self.dialog_model(cx) else { return };
        let ix = model.buttons.iter().position(|b| b.primary).unwrap_or(0);
        let Some(handle) = self.update.focus.get(ix).cloned() else { return };
        match self.update.window.filter(|w| window_is_open(*w, cx)) {
            // The buttons are in the dialog's window. This may run while a click in that window is still being handled (the
            // window is busy then), so the focus moves right after.
            Some(own) => cx.defer(move |cx| {
                own.update(cx, |_, window, _| window.focus(&handle)).ok();
            }),
            None => window.focus(&handle),
        }
    }

    // --- Reading the notes with the keyboard ---

    /// Scrolls the notes for a key pressed while they have the focus. `false`: not a scrolling key.
    pub(super) fn scroll_notes(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let handle = &self.update.notes_scroll;
        let (offset, page, max) = (f32::from(handle.offset().y), f32::from(handle.bounds().size.height), f32::from(handle.max_offset().height));
        let Some(target) = notes_scroll_target(key, offset, page, max) else { return false };
        handle.set_offset(gpui::point(handle.offset().x, px(target)));
        cx.notify();
        true
    }

    // --- The question ---

    /// Shows the one-time question when `update.check` is undecided, it was not snoozed and nothing else is in the way (a picker or
    /// menu is open, the window is in the background or a key was pressed a moment ago: a dialog must not appear under someone
    /// else's hands). `false`: not now, try again soon.
    pub(super) fn ask_update_policy(&mut self, window_active: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let snoozed = self.stored(ASK_SNOOZED_UNTIL).and_then(|v| v.parse::<u64>().ok()).is_some_and(|until| unix_ms(self.update.backend.now()) < until);
        if cx.global::<Settings>().update_check != Policy::Ask || snoozed {
            return true;
        }
        let now = self.update.backend.now();
        let typing = self.update.last_key.is_some_and(|t| now.duration_since(t).map_or(true, |age| age < QUIET_BEFORE_ASK));
        let calm = self.overlay.is_none() && self.menu.is_none() && self.update.dialog == Dialog::Closed && window_active && !typing;
        if !calm {
            return false;
        }
        self.update.return_focus = window.focused(cx);
        self.update.dialog = Dialog::Ask;
        self.update.armed = false;
        self.focus_update_primary(window, cx);
        self.update.arm = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ARM_AFTER).await;
            this.update(cx, |this, cx| {
                this.update.armed = true;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
        true
    }

    fn choose_policy(&mut self, policy: Policy, window: &mut Window, cx: &mut Context<Self>) {
        self.update.arm = None;
        // The dialog closes first so the notice that `write_setting` leaves ("update.check: daily") is the last word.
        self.update.dialog = Dialog::Closed;
        match self.update.return_focus.take() {
            Some(handle) => window.focus(&handle),
            None => window.focus(&self.focus_handle),
        }
        self.write_setting("update.check", serde_json::json!(policy.as_str()), window, cx);
        // Saying yes to daily checks is also the first one: nobody wants to wait a day to see it work.
        if policy == Policy::Daily && cx.global::<Settings>().update_check == Policy::Daily {
            self.begin_check(false, window, cx);
        }
    }

    // --- Scheduling ---

    /// First window only, from `main`: the question (if undecided) and the daily check (if chosen). Never called in debug builds,
    /// in tests or with `--do`/`--panel`.
    pub(crate) fn schedule_update_checks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update.schedule = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(ASK_AFTER).await;
            for _ in 0..ASK_ATTEMPTS {
                match this.update_in(cx, |this, window, cx| this.ask_update_policy(window.is_window_active(), window, cx)) {
                    Ok(true) => break,
                    Ok(false) => cx.background_executor().timer(ASK_RETRY).await,
                    Err(_) => return,
                }
            }
            cx.background_executor().timer(CHECK_AFTER_ASK).await;
            loop {
                if this.update_in(cx, |this, window, cx| this.check_if_due(window, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(CHECK_EVERY).await;
            }
        }));
    }

    /// The background check of policy `daily`: when 24 hours have passed since the last good one, and after failures only while
    /// fewer than three happened today.
    pub(super) fn check_if_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let policy = cx.global::<Settings>().update_check;
        let now = self.update.backend.now();
        if should_check_now(self.last_check(), now, policy) && retry_allowed(&self.update.failures, now) {
            self.begin_check(false, window, cx);
        }
    }

    // --- Checking ---

    /// `manual`: the user asked, so the dialog opens and errors are shown. Otherwise nothing is shown unless a release is found.
    pub(super) fn begin_check(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        if manual && let Some(other) = self.window_updating(window, cx) {
            // One update at a time: the window that is already doing it shows its dialog.
            other
                .update(cx, |wb, window, cx| {
                    window.activate_window();
                    wb.open_update_dialog(window, cx);
                })
                .ok();
            return;
        }
        match &self.update.state {
            UpdateState::Downloading { .. } | UpdateState::Installing { .. } | UpdateState::Ready { .. } => {
                if manual {
                    self.open_update_dialog(window, cx);
                }
                return;
            }
            UpdateState::Checking => {
                // Adopt the check that is already running.
                if manual {
                    self.update.manual = true;
                    self.open_update_dialog(window, cx);
                }
                return;
            }
            // A background check only looks when nothing is on offer or on screen.
            UpdateState::Available(_) | UpdateState::UpToDate | UpdateState::Error(_) if !manual => return,
            _ => {}
        }
        self.update.state = UpdateState::Checking;
        self.update.manual = manual;
        if manual {
            self.open_update_dialog(window, cx);
        }
        let started = self.start_worker(
            |backend, tx, _| {
                // A closed channel means the window is gone or the check was cancelled: nobody wants the answer.
                let _ = tx.send(Msg::Checked(backend.check()));
            },
            window,
            cx,
        );
        if let Err(e) = started {
            self.fail(&e, None, Phase::Check, window, cx);
        }
        cx.notify();
    }

    /// Another window that is checking, downloading or installing right now.
    fn window_updating(&self, window: &Window, cx: &App) -> Option<gpui::WindowHandle<Workbench>> {
        let me = window.window_handle();
        cx.windows()
            .into_iter()
            .filter(|w| *w != me)
            .filter_map(|w| w.downcast::<Workbench>())
            .find(|w| w.read(cx).is_ok_and(|wb| wb.update.state.is_working()))
    }

    fn cancel_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The request itself cannot be interrupted; its answer is dropped with the run.
        self.update.run = None;
        self.update.manual = false;
        self.update.state = UpdateState::Idle;
        self.close_update_dialog(window, cx);
    }

    fn on_checked(&mut self, result: Result<Option<Release>, UpdateError>, window: &mut Window, cx: &mut Context<Self>) {
        self.update.run = None;
        let manual = std::mem::take(&mut self.update.manual);
        match result {
            Ok(None) => {
                self.record_check();
                self.update.state = if manual { UpdateState::UpToDate } else { UpdateState::Idle };
            }
            Ok(Some(release)) => {
                self.record_check();
                let seen = self.stored(HIGHEST_SEEN);
                if seen.as_deref().is_some_and(|seen| is_rollback(&release.version, seen)) {
                    // Genuinely signed, but older than what this installation was offered before: an old list played back.
                    eprintln!("update: ignoring {}: an earlier check already saw {}", release.version, seen.unwrap_or_default());
                    self.update.state = if manual { UpdateState::UpToDate } else { UpdateState::Idle };
                } else {
                    self.remember(HIGHEST_SEEN, &higher_version(&release.version, seen.as_deref()));
                    self.offer(release, manual, cx);
                }
            }
            Err(e) => self.fail(&e, None, Phase::Check, window, cx),
        }
        if manual {
            self.focus_update_primary(window, cx);
        }
        cx.notify();
    }

    /// A newer release exists. Quiet paths (background check): an installed one waits for a restart, a skipped one is not mentioned.
    fn offer(&mut self, release: Release, manual: bool, cx: &mut Context<Self>) {
        let version = release.version.clone();
        if self.stored(INSTALLED).as_deref() == Some(version.as_str()) {
            let plan = self.plan_for_restart();
            self.update.state = UpdateState::Ready { version, plan };
            return;
        }
        if !manual && self.stored(SKIPPED).as_deref() == Some(version.as_str()) {
            self.update.state = UpdateState::Idle;
            return;
        }
        self.update.kind = self.update.backend.install_kind();
        (self.update.notes, self.update.notes_text) = render_notes(&release.notes, &release_url(Some(version.as_str())));
        self.update.notes_scroll.set_offset(gpui::point(px(0.), px(0.)));
        self.update.state = UpdateState::Available(release);
        if !manual && self.stored(NOTIFIED).as_deref() != Some(version.as_str()) {
            self.remember(NOTIFIED, &version);
            self.notice = Some(Notice::Info(tf("Codeyard {} is available.", &[&version]).into()));
        }
        cx.notify();
    }

    fn skip_version(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let UpdateState::Available(release) = &self.update.state {
            let version = release.version.clone();
            self.remember(SKIPPED, &version);
            self.update.state = UpdateState::Idle;
        }
        self.close_update_dialog(window, cx);
    }

    // --- Downloading and installing ---

    fn start_update_download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let release = match &self.update.state {
            UpdateState::Available(r) => r.clone(),
            UpdateState::Error(Failure { release: Some(r), .. }) => r.clone(),
            _ => return,
        };
        if matches!(self.update.kind, InstallKind::Manual(_)) || release.asset.is_none() {
            return;
        }
        let kind = self.update.kind.clone();
        self.update.state = UpdateState::Downloading { release: release.clone(), progress: Progress { done: 0, total: None } };
        let started = self.start_worker(
            move |backend, tx, cancel| {
                // Same as for the check: a closed channel only means nobody is listening any more.
                let result = (|| {
                    let dl = backend.download(&release, &mut |p| drop(tx.send(Msg::Progress(p))), cancel)?;
                    if cancel.load(Ordering::Relaxed) {
                        backend.discard(dl);
                        return Err(UpdateError::Cancelled);
                    }
                    let _ = tx.send(Msg::Installing);
                    let installed = backend.stage_and_install(&dl, &kind);
                    // The Windows setup program runs from this file; everything else is done with it.
                    if !matches!(installed, Ok(InstallPlan::WindowsInstaller)) {
                        backend.discard(dl);
                    }
                    installed
                })();
                let _ = tx.send(Msg::Done(result));
            },
            window,
            cx,
        );
        if let Err(e) = started {
            let release = self.update.state.release().cloned();
            self.fail(&e, release, Phase::Download, window, cx);
        }
        self.focus_update_primary(window, cx);
        cx.notify();
    }

    fn cancel_download(&mut self, cx: &mut Context<Self>) {
        // The worker notices within a moment, removes the partial file and answers `Cancelled`.
        if let Some(run) = &self.update.run
            && matches!(self.update.state, UpdateState::Downloading { .. })
        {
            run.cancel.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let UpdateState::Error(failure) = &self.update.state else { return };
        match (failure.phase, failure.release.clone()) {
            (Phase::Download, Some(release)) => {
                self.update.state = UpdateState::Available(release);
                self.start_update_download(window, cx);
            }
            _ => {
                self.update.state = UpdateState::Idle;
                self.begin_check(true, window, cx);
            }
        }
    }

    fn on_installed(&mut self, result: Result<InstallPlan, UpdateError>, window: &mut Window, cx: &mut Context<Self>) {
        self.update.run = None;
        let release = self.update.state.release().cloned();
        let phase = if matches!(self.update.state, UpdateState::Installing { .. }) { Phase::Install } else { Phase::Download };
        match (result, release) {
            (Ok(plan), Some(release)) => {
                self.remember(INSTALLED, &release.version);
                self.update.state = UpdateState::Ready { version: release.version, plan: Some(plan.clone()) };
                if plan == InstallPlan::WindowsInstaller {
                    // The setup program is running and closes this app itself; leave through the normal quit path first.
                    self.restart_now(window, cx);
                } else if self.update.dialog != Dialog::Update {
                    self.notice = Some(Notice::Info(t("Update installed. Restart Codeyard to finish.").into()));
                }
            }
            (Err(e), Some(release)) if e.kind() == ErrorKind::Cancelled => self.update.state = UpdateState::Available(release),
            (Err(e), release) => self.fail(&e, release, phase, window, cx),
            (Ok(_), None) => {}
        }
        self.focus_update_primary(window, cx);
        cx.notify();
    }

    /// Turns an error into what the user sees: the dialog when it is open, otherwise (a background step) the log, and for a
    /// failed download or install also a notice, because someone was waiting for it.
    fn fail(&mut self, error: &UpdateError, release: Option<Release>, phase: Phase, window: &mut Window, cx: &mut Context<Self>) {
        self.update.run = None;
        let failure = Failure::from_error(error, release, phase);
        eprintln!("update: {}: {}", failure.title(), failure.detail);
        if phase == Phase::Check {
            let now = self.update.backend.now();
            self.update.failures.push(now);
        }
        if self.update.dialog == Dialog::Update {
            self.update.state = UpdateState::Error(failure);
        } else if phase == Phase::Check {
            self.update.state = UpdateState::Idle;
        } else {
            self.notice = Some(Notice::Error(failure.title().into()));
            self.update.state = UpdateState::Error(failure);
        }
        self.focus_update_primary(window, cx);
        cx.notify();
    }

    // --- Worker threads ---

    /// Runs `job` on its own thread (a download can take half an hour, too long for the shared pool) and feeds its messages to
    /// `on_update_msg`. At most one run exists; starting another drops (and cancels) the previous one.
    fn start_worker(
        &mut self,
        job: impl FnOnce(&dyn UpdateBackend, &std::sync::mpsc::Sender<Msg>, &AtomicBool) + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), UpdateError> {
        self.update.next_run += 1;
        let id = self.update.next_run;
        let (cancel, exited) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (tx, rx) = std::sync::mpsc::channel();
        let backend = self.update.backend.clone();
        let (thread_cancel, thread_exited) = (cancel.clone(), exited.clone());
        std::thread::Builder::new()
            .name("update".into())
            .spawn(move || {
                job(&*backend, &tx, &thread_cancel);
                drop(tx);
                thread_exited.store(true, Ordering::Release);
            })
            .map_err(|source| UpdateError::Io { what: "start the update", source })?;
        let mut messages = ui::bridge(rx);
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(msg) = messages.next().await {
                if this.update_in(cx, |this, window, cx| this.on_update_msg(id, msg, window, cx)).is_err() {
                    break;
                }
            }
        });
        self.update.run = Some(Run { id, cancel, exited, _task: task });
        Ok(())
    }

    fn on_update_msg(&mut self, id: u64, msg: Msg, window: &mut Window, cx: &mut Context<Self>) {
        // A message of a run that was cancelled or replaced.
        if self.update.run.as_ref().map(|r| r.id) != Some(id) {
            return;
        }
        match msg {
            Msg::Checked(result) => self.on_checked(result, window, cx),
            Msg::Progress(progress) => {
                if let UpdateState::Downloading { progress: shown, .. } = &mut self.update.state {
                    *shown = progress;
                    cx.notify();
                }
            }
            Msg::Installing => {
                if let UpdateState::Downloading { release, .. } = &self.update.state {
                    self.update.state = UpdateState::Installing { release: release.clone() };
                    // The button that had the focus (Cancel) is gone.
                    self.focus_update_primary(window, cx);
                    cx.notify();
                }
            }
            Msg::Done(result) => self.on_installed(result, window, cx),
        }
    }

    // --- Restart and quit ---

    pub(super) fn restart_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let plan = match &self.update.state {
            UpdateState::Ready { plan: Some(plan), .. } => Some(plan.clone()),
            UpdateState::Ready { plan: None, .. } => self.plan_for_restart(),
            _ => return,
        };
        self.confirm_quit_with(QuitMode::Restart(plan), window, cx);
    }

    /// How to start the installed version when this window did not do the installing itself.
    fn plan_for_restart(&self) -> Option<InstallPlan> {
        match self.update.backend.install_kind() {
            InstallKind::MacBundle { app } => Some(InstallPlan::MacApp { app }),
            InstallKind::WindowsInstalled | InstallKind::Manual(_) => None,
        }
    }

    /// The last step of quitting or restarting, after hot exit has saved everything. The new version is started BEFORE this
    /// process ends (its helper waits for us to exit); if that fails nothing is quit, so the user is not left without an editor.
    pub(super) fn finish_quit(&mut self, mode: QuitMode, cx: &mut Context<Self>) {
        let handed_over = matches!(mode, QuitMode::Restart(_)) && self.write_handover(cx);
        if let QuitMode::Restart(Some(plan)) = &mode
            && let Err(e) = self.update.backend.relaunch(plan)
        {
            if handed_over && let Some(dir) = &self.persist.data_dir {
                update_restart::discard(dir);
            }
            self.notice =
                Some(Notice::Error(tf("Codeyard could not start the new version: {}. The update is installed; open Codeyard again to use it.", &[&e]).into()));
            cx.notify();
            return;
        }
        let backend = self.update.backend.clone();
        backend.quit(cx);
    }

    /// The folders of all windows, in window order, and the index of the one in front (this window when none is): what the new
    /// version reopens. Windows without a folder have nothing to come back to.
    pub(super) fn reopenable_windows(&self, cx: &App) -> (Vec<update_restart::Folder>, usize) {
        let (me, active) = (self.window_handle, cx.active_window());
        let (mut folders, mut focused, mut mine) = (Vec::new(), None, None);
        for w in cx.windows() {
            let key = if w == me { self.workspace_key(cx) } else { w.downcast::<Workbench>().and_then(|h| h.read(cx).ok()?.workspace_key(cx)) };
            let Some((host, path)) = key else { continue };
            if w == me {
                mine = Some(folders.len());
            }
            if Some(w) == active {
                focused = Some(folders.len());
            }
            folders.push(update_restart::Folder { host, path });
        }
        (folders, focused.or(mine).unwrap_or(0))
    }

    /// Leaves the folders of all windows for the new version (`update_restart.rs`). `false`: nothing was written.
    fn write_handover(&self, cx: &App) -> bool {
        let (UpdateState::Ready { version, .. }, Some(dir)) = (&self.update.state, &self.persist.data_dir) else { return false };
        let (windows, focused) = self.reopenable_windows(cx);
        if windows.is_empty() {
            return false;
        }
        match update_restart::write(dir, &update_restart::Handover { to: version.clone(), windows, focused }, self.update.backend.now()) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("could not leave the open windows for the new version: {e}");
                false
            }
        }
    }

    /// Is the app being swapped in, by this window or any other?
    fn installing_anywhere(&self, window: &Window, cx: &App) -> bool {
        let me = window.window_handle();
        matches!(self.update.state, UpdateState::Installing { .. })
            || cx
                .windows()
                .into_iter()
                .filter(|w| *w != me)
                .filter_map(|w| w.downcast::<Workbench>())
                .any(|w| w.read(cx).is_ok_and(|wb| matches!(wb.update.state, UpdateState::Installing { .. })))
    }

    /// While the new app is being swapped in (by any window), ending the process can leave a half-installed app: ask first.
    /// `true`: the question is on screen and the caller must stop.
    pub(super) fn hold_quit_for_install(&mut self, mode: QuitMode, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.installing_anywhere(window, cx) {
            return false;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            t("Codeyard is still installing an update"),
            Some(t("Quitting now can leave the installation unfinished. It takes a few seconds.")),
            &[t("Quit Anyway"), t("Keep Waiting")],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| this.finish_quit(mode, cx)).ok();
            }
        })
        .detach();
        true
    }

    /// A window is being closed. The last one ends the app, so it asks like quitting does. Another one that is doing the install
    /// stays open for the few seconds it takes: closing it would drop the answer (the version that is now installed, the
    /// restart item), and the other windows would never learn that a restart is due.
    pub(crate) fn hold_close_for_install(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if cx.windows().len() == 1 {
            return self.hold_quit_for_install(QuitMode::Quit, window, cx);
        }
        if !matches!(self.update.state, UpdateState::Installing { .. }) {
            return false;
        }
        self.notice = Some(Notice::Info(t("This window is installing an update. You can close it in a few seconds.").into()));
        cx.notify();
        true
    }

    /// The application is quitting: a download is cancelled (its worker removes the partial file) and the returned flag says when
    /// the worker is done. A check is simply abandoned.
    pub(super) fn cancel_update_for_quit(&mut self) -> Option<Arc<AtomicBool>> {
        let run = self.update.run.take()?;
        run.cancel.store(true, Ordering::Relaxed);
        Some(run.exited.clone())
    }

    /// Waits (briefly) for `exited`, so the temporary files are gone before the process ends.
    pub(super) fn wait_for_update_thread(exited: Arc<AtomicBool>, cx: &App) -> impl std::future::Future<Output = ()> + use<> {
        let executor = cx.background_executor().clone();
        async move {
            let start = std::time::Instant::now();
            while !exited.load(Ordering::Acquire) && start.elapsed() < QUIT_GRACE {
                executor.timer(Duration::from_millis(50)).await;
            }
        }
    }

    // --- Stored values ---

    fn stored(&self, key: &str) -> Option<String> {
        self.persist.state.as_ref().and_then(|s| s.get(key))
    }

    /// A failed write only costs a repeated notice or check, so it is logged, not shown.
    fn remember(&self, key: &str, value: &str) {
        if let Some(Err(e)) = self.persist.state.as_ref().map(|s| s.set(key, value)) {
            eprintln!("could not remember {key}: {e}");
        }
    }

    fn last_check(&self) -> Option<SystemTime> {
        self.update.last_check.or_else(|| self.stored(LAST_CHECK)?.parse::<u64>().ok().map(|ms| UNIX_EPOCH + Duration::from_millis(ms)))
    }

    /// A check that got an answer (a release or "up to date") counts; a network failure does not, so the next start tries again.
    fn record_check(&mut self) {
        let now = self.update.backend.now();
        self.update.last_check = Some(now);
        self.update.failures.clear();
        self.remember(LAST_CHECK, &unix_ms(now).to_string());
    }
}

impl UpdateState {
    fn release(&self) -> Option<&Release> {
        match self {
            UpdateState::Available(r) | UpdateState::Downloading { release: r, .. } | UpdateState::Installing { release: r } => Some(r),
            UpdateState::Error(f) => f.release.as_ref(),
            _ => None,
        }
    }
}

/// Milliseconds since the Unix epoch (0 for a clock set before it).
fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_restart_promise_has_one_sentence_per_thing_that_will_happen() {
        let said = |remote, files, unsaved| Promise { remote, files, unsaved }.paragraphs();
        assert_eq!(said(false, false, false), ["Codeyard will reopen your windows."]);
        assert_eq!(said(false, true, false), ["Codeyard will reopen your windows. Your open files come back after the restart."]);
        assert_eq!(said(false, true, true), ["Codeyard will reopen your windows. Your open files and unsaved changes come back after the restart."]);
        assert_eq!(said(true, false, false), ["Codeyard will reopen your windows.", "Windows on a server reconnect, which can ask for a password."]);
        let _tr = crate::i18n::LangGuard::set("tr");
        let tr = said(true, true, true);
        assert!(tr.iter().all(|p| !p.contains("Codeyard will") && !p.contains("Windows on") && !p.contains("come back")), "{tr:?}");
    }

    #[test]
    fn sizes_are_shown_in_megabytes_with_one_decimal() {
        assert_eq!(megabytes(0), "0.0");
        assert_eq!(megabytes(12_897_484), "12.3");
        assert_eq!(megabytes(20_700_000), "19.7");
    }

    #[test]
    fn the_release_page_is_the_tag_of_the_version_or_the_latest_release() {
        let repository = env!("CARGO_PKG_REPOSITORY");
        assert_eq!(release_url(Some("0.2.0")), format!("{repository}/releases/tag/v0.2.0"));
        assert_eq!(release_url(None), format!("{repository}/releases/latest"));
    }

    #[test]
    fn progress_is_a_fraction_only_when_the_total_is_known() {
        assert_eq!(progress_fraction(&Progress { done: 5, total: Some(10) }), Some(0.5));
        assert_eq!(progress_fraction(&Progress { done: 5, total: None }), None);
        assert_eq!(progress_fraction(&Progress { done: 5, total: Some(0) }), None, "no division by zero");
        assert_eq!(progress_fraction(&Progress { done: 12, total: Some(10) }), Some(1.), "a server that sends more than it said");
    }

    #[test]
    fn every_error_kind_has_a_plain_title_and_only_network_errors_are_retried() {
        let failure = |kind, phase| Failure { kind, cause: Cause::Other, detail: String::new(), release: None, phase };
        assert_eq!(failure(ErrorKind::Network, Phase::Check).title(), "Couldn't check for updates");
        assert_eq!(failure(ErrorKind::Network, Phase::Download).title(), "Couldn't download the update");
        assert_eq!(failure(ErrorKind::Untrusted, Phase::Download).title(), "The update couldn't be verified");
        assert_eq!(failure(ErrorKind::Install, Phase::Install).title(), "The update couldn't be installed");
        assert!(failure(ErrorKind::Untrusted, Phase::Check).lead().starts_with("Nothing was installed."));
    }

    #[test]
    fn the_scroll_keys_stay_inside_the_notes() {
        let target = notes_scroll_target;
        assert_eq!(target("down", 0., 260., 500.), Some(-20.));
        assert_eq!(target("up", 0., 260., 500.), Some(0.), "not above the top");
        assert_eq!(target("pagedown", 0., 260., 500.), Some(-240.), "a page less a line");
        assert_eq!(target("pagedown", -400., 260., 500.), Some(-500.), "not below the end");
        assert_eq!(target("end", 0., 260., 500.), Some(-500.));
        assert_eq!(target("home", -300., 260., 500.), Some(0.));
        assert_eq!(target("down", 0., 260., 0.), Some(0.), "notes that fit do not move");
        assert_eq!(target("a", 0., 260., 500.), None);
    }
}
