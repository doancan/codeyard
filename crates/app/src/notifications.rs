//! The notification log: a bounded history of everything the app told the user, with severity, source, time, an optional
//! detail text and an optional action. No UI and no clock: the caller passes the time, so every rule is testable.
//!
//! Repeated notifications (same id) collapse into one entry with a counter. Nothing here is persisted.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// Entries kept; the oldest is dropped first, so memory stays bounded however noisy a source is.
pub const CAPACITY: usize = 200;
/// A message longer than this is cut (characters), so one runaway string cannot fill the history.
pub const MAX_MESSAGE_CHARS: usize = 2_000;
/// Same for the detail text (stderr of a failed command, for example).
pub const MAX_DETAIL_CHARS: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// Where a notification comes from; shown beside it so people know which part of the app speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoticeSource {
    App,
    Git,
    Debug,
    Extensions,
    Remote,
    Tasks,
    Language,
    Update,
    Trust,
    Files,
    Terminal,
    Settings,
}

impl NoticeSource {
    pub fn label(self) -> String {
        use crate::i18n::t;
        match self {
            NoticeSource::App => "Codeyard", // i18n-keep: the product name
            NoticeSource::Git => "Git",      // i18n-keep: the product name
            NoticeSource::Debug => t("Debug"),
            NoticeSource::Extensions => t("Extensions"),
            NoticeSource::Remote => t("Remote"),
            NoticeSource::Tasks => t("Tasks"),
            NoticeSource::Language => t("Language"),
            NoticeSource::Update => t("Update"),
            NoticeSource::Trust => t("Workspace Trust"),
            NoticeSource::Files => t("Files"),
            NoticeSource::Terminal => t("Terminal"),
            NoticeSource::Settings => t("Settings"),
        }
        .to_string()
    }
}

/// A palette command a notification offers: it runs the command with this palette label (English; the label shown is translated).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub label: &'static str,
    pub command: &'static str,
}

/// An operation a notification can run again. Plain data: a notification never holds a closure or an entity, so it cannot keep
/// anything alive; the workbench maps each kind to the code that does the work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryKind {
    /// Save the open file again. `host` is the history key of the machine it lives on (empty: this computer), so a file of the
    /// same path on another machine is never saved by mistake.
    SaveFile { host: String, path: PathBuf },
    /// Write this setting (its value as JSON) into this settings file again, unless the file or the setting's value in it is no
    /// longer what it was when the write failed (`before`: that value as JSON, none when the key was not there).
    WriteSetting { path: PathBuf, key: String, json: String, before: Option<String> },
    /// Check for updates again.
    UpdateCheck,
}

/// The button of a notification. Always data (paths and kinds), see [`RetryKind`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    /// A palette command.
    Command(Action),
    /// Show the path in the system's file manager (its nearest existing folder when it is gone).
    Reveal(PathBuf),
    /// Open the file in a tab, at a line (1-based) when given.
    OpenFile(PathBuf, Option<u32>),
    /// Run the operation again.
    Retry(RetryKind),
}

impl From<Action> for NoticeAction {
    fn from(a: Action) -> Self {
        NoticeAction::Command(a)
    }
}

impl NoticeAction {
    /// The button text (English key; translate when drawing).
    pub fn label(&self) -> &'static str {
        match self {
            NoticeAction::Command(a) => a.label,
            NoticeAction::Reveal(_) => crate::i18n::tr("Show in Folder"),
            NoticeAction::OpenFile(..) => crate::i18n::tr("Open File"),
            NoticeAction::Retry(_) => crate::i18n::tr("Retry"),
        }
    }
}

/// What a caller posts.
#[derive(Clone, Debug)]
pub struct Notification {
    pub severity: Severity,
    pub source: NoticeSource,
    pub message: String,
    pub detail: Option<String>,
    pub action: Option<NoticeAction>,
    /// Stable identity for collapsing repeats; by default derived from source, severity and message.
    pub id: Option<String>,
    /// Recorded in the history only: no toast.
    pub quiet: bool,
}

impl Notification {
    pub fn new(severity: Severity, source: NoticeSource, message: impl Into<String>) -> Self {
        Self { severity, source, message: message.into(), detail: None, action: None, id: None, quiet: false }
    }
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    pub fn action(mut self, action: impl Into<NoticeAction>) -> Self {
        self.action = Some(action.into());
        self
    }
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
    /// Keep it in the history without showing a toast (something that is expected, such as being offline).
    pub fn quiet(mut self) -> Self {
        self.quiet = true;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Grows with every post (also with a repeat), so it names "this showing" of the entry.
    pub serial: u64,
    pub id: String,
    pub severity: Severity,
    pub source: NoticeSource,
    pub message: String,
    pub detail: Option<String>,
    pub action: Option<NoticeAction>,
    /// Seconds since the Unix epoch of the last time it was posted.
    pub at: u64,
    pub count: u32,
    pub read: bool,
}

/// What `Log::post` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Posted {
    pub serial: u64,
    /// The entry already existed: its counter went up and it moved to the top.
    pub repeated: bool,
}

#[derive(Default)]
pub struct Log {
    /// Oldest first.
    entries: VecDeque<Entry>,
    next_serial: u64,
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { text.chars().take(max).collect::<String>() + "…" }
}

fn derived_id(n: &Notification) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (n.source, n.severity, &n.message).hash(&mut h);
    format!("auto:{:016x}", h.finish())
}

impl Log {
    /// Record `n` at time `now`. Info is stored as read (it is a confirmation, not something to come back to);
    /// warnings and errors are unread until the list is opened or they are dismissed.
    pub fn post(&mut self, n: Notification, now: u64) -> Posted {
        let id = n.id.clone().unwrap_or_else(|| derived_id(&n));
        self.next_serial += 1;
        let serial = self.next_serial;
        // A quiet entry is for the history only: it never raises the bell's badge, whatever its severity.
        let read = n.severity == Severity::Info || n.quiet;
        if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            let mut e = self.entries.remove(i).expect("position is in range");
            e.serial = serial;
            e.at = now;
            e.count = e.count.saturating_add(1);
            e.read = read;
            e.severity = n.severity;
            e.message = cut(&n.message, MAX_MESSAGE_CHARS);
            e.detail = n.detail.map(|d| cut(&d, MAX_DETAIL_CHARS));
            e.action = n.action;
            self.entries.push_back(e);
            return Posted { serial, repeated: true };
        }
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            serial,
            id,
            severity: n.severity,
            source: n.source,
            message: cut(&n.message, MAX_MESSAGE_CHARS),
            detail: n.detail.map(|d| cut(&d, MAX_DETAIL_CHARS)),
            action: n.action,
            at: now,
            count: 1,
            read,
        });
        Posted { serial, repeated: false }
    }

    /// Newest first.
    pub fn newest_first(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().rev()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn unread(&self) -> usize {
        self.entries.iter().filter(|e| !e.read).count()
    }

    pub fn unread_errors(&self) -> usize {
        self.entries.iter().filter(|e| !e.read && e.severity == Severity::Error).count()
    }

    pub fn mark_all_read(&mut self) {
        self.entries.iter_mut().for_each(|e| e.read = true);
    }

    pub fn dismiss(&mut self, serial: u64) -> bool {
        match self.entries.iter().position(|e| e.serial == serial) {
            Some(i) => {
                self.entries.remove(i);
                true
            }
            None => false,
        }
    }

    /// Remove the entry with this id (a failure that has been put right). `false` when there is none.
    pub fn dismiss_id(&mut self, id: &str) -> bool {
        match self.entries.iter().position(|e| e.id == id) {
            Some(i) => {
                self.entries.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn get(&self, serial: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.serial == serial)
    }
}

/// "just now", "5 minutes ago", "2 hours ago", "3 days ago".
pub fn age_label(now: u64, at: u64) -> String {
    use crate::i18n::{t, tn};
    let secs = now.saturating_sub(at);
    match secs {
        0..=59 => t("just now").to_string(),
        60..=3_599 => tn("{} minute ago", "{} minutes ago", (secs / 60) as usize, &[]),
        3_600..=86_399 => tn("{} hour ago", "{} hours ago", (secs / 3_600) as usize, &[]),
        _ => tn("{} day ago", "{} days ago", (secs / 86_400) as usize, &[]),
    }
}

/// The text for the clipboard: message, then the detail when there is one.
pub fn copy_text(e: &Entry) -> String {
    match &e.detail {
        Some(d) => format!("{}\n\n{d}", e.message),
        None => e.message.clone(),
    }
}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;
