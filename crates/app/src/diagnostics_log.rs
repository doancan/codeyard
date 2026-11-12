//! The log file: `logs/codeyard.log` in the state folder, for the failures a window cannot show (a daemon that exited, an update that
//! could not be installed). Nothing is sent anywhere.
//!
//! * A line is `time level area: message`, one line per entry, with the home folder and credentials removed before anything is kept.
//! * Callers never wait: a line goes into a bounded queue and a background thread writes it; when the queue is full the line is
//!   dropped (and counted) instead of blocking the interface.
//! * The file rotates at `MAX_BYTES`: `codeyard.log`, `codeyard.log.1`, `codeyard.log.2` are kept, so the folder stays under 3 MB.
//! * The last `RING_LINES` lines are also kept in memory for the diagnostics report and the crash report.
//! * Logging never panics and a failing disk only costs the lines.

use super::redact::redact_and_cut;
use std::collections::VecDeque;
use std::fmt::{Arguments, Write as _};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

pub const FILE_NAME: &str = "codeyard.log";
/// The terminal daemon is a second process of the same app: its own file and rotation set, so the two never rotate each other's files.
pub const DAEMON_FILE_NAME: &str = "codeyard-daemon.log";
/// A file is closed and the next one started past this size.
const MAX_BYTES: u64 = 1_000_000;
/// Files kept: the current one and the older ones (`.1`, `.2`).
const FILES: usize = 3;
/// Lines waiting for the writer thread; more than this are dropped.
const QUEUE_LINES: usize = 1_024;
/// A longer line is cut (characters).
const MAX_LINE_CHARS: usize = 2_000;
const MAX_AREA_CHARS: usize = 32;
/// Lines kept in memory (cut shorter) for reports.
const RING_LINES: usize = 200;
const RING_LINE_CHARS: usize = 500;
/// Notifications remembered for crash reports.
const NOTICE_LINES: usize = 20;
const FLUSH_WAIT: Duration = Duration::from_millis(1_000);
const HOOK_FLUSH_WAIT: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    pub const NAMES: [&'static str; 4] = ["error", "warn", "info", "debug"];

    pub fn parse(name: &str) -> Option<Level> {
        match name {
            "error" => Some(Level::Error),
            "warn" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
        }
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
static WRITER: OnceLock<Writer> = OnceLock::new();
static RING: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static NOTICES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static HOME: OnceLock<Option<String>> = OnceLock::new();

pub fn set_level(level: Level) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}

pub fn enabled(level: Level) -> bool {
    level as u8 <= LEVEL.load(Ordering::Relaxed)
}

pub fn error(area: &str, message: Arguments<'_>) {
    log(Level::Error, area, message);
}
pub fn warn(area: &str, message: Arguments<'_>) {
    log(Level::Warn, area, message);
}
pub fn info(area: &str, message: Arguments<'_>) {
    log(Level::Info, area, message);
}
pub fn debug(area: &str, message: Arguments<'_>) {
    log(Level::Debug, area, message);
}

/// Where the log lives: `<state folder>/logs`.
pub fn log_dir() -> Option<PathBuf> {
    crate::workbench::state_path()?.parent().map(|d| d.join("logs"))
}

/// Start writing `file_name` in `dir`. Called once, early; a second call or a folder that cannot be created leaves the log in
/// memory only.
pub fn init(dir: Option<PathBuf>, file_name: &str) {
    home();
    let Some(dir) = dir else { return };
    if let Some(writer) = Writer::spawn(Files::new(dir, file_name, MAX_BYTES)) {
        // What was logged before the file was ready (the legacy folder move runs first) is written first.
        for line in tail(RING_LINES) {
            writer.send(line);
        }
        WRITER.set(writer).ok();
    }
}

pub fn log(level: Level, area: &str, message: Arguments<'_>) {
    if !enabled(level) {
        return;
    }
    let line = format_line(now_millis(), level, area, &message.to_string(), home());
    remember(&line);
    if level <= Level::Warn && !cfg!(test) {
        // Developers started from a terminal still see failures there; a closed stderr is not an error.
        writeln!(std::io::stderr(), "{line}").ok();
    }
    if let Some(writer) = WRITER.get() {
        writer.send(line);
    }
}

/// `error` for the panic hook, where a panic would abort the process and a wait would hang it: no blocking lock (a contended ring
/// is skipped), no formatting beyond the capped line, the queue is only tried, and nothing can fail loudly.
pub fn error_from_hook(area: &str, message: Arguments<'_>) {
    if !enabled(Level::Error) {
        return;
    }
    let line = format_line(now_millis(), Level::Error, area, &message.to_string(), home_if_ready());
    if let Ok(mut ring) = RING.try_lock() {
        if ring.len() == RING_LINES {
            ring.pop_front();
        }
        ring.push_back(cut(&line, RING_LINE_CHARS));
    }
    if let Some(writer) = WRITER.get() {
        writer.send(line);
    }
}

/// `flush` for the panic hook: a short wait at most.
pub fn flush_from_hook() {
    if let Some(writer) = WRITER.get() {
        writer.flush_within(HOOK_FLUSH_WAIT);
    }
}

/// The newest lines of this session, oldest first (already redacted).
pub fn tail(count: usize) -> Vec<String> {
    let ring = lock(&RING);
    ring.iter().skip(ring.len().saturating_sub(count)).cloned().collect()
}

/// Like `tail` for the panic hook: gives up instead of waiting when another thread holds the lock.
pub fn try_tail(count: usize) -> Vec<String> {
    match RING.try_lock() {
        Ok(ring) => ring.iter().skip(ring.len().saturating_sub(count)).cloned().collect(),
        Err(_) => Vec::new(),
    }
}

/// Remember a notification shown to the user (severity, source, message) for the next crash report, and log it: errors and
/// warnings at their own level, information as debug.
pub fn notice(level: Level, source: &str, message: &str) {
    let line = format_line(now_millis(), level, source, message, home());
    let mut notices = lock(&NOTICES);
    if notices.len() == NOTICE_LINES {
        notices.pop_front();
    }
    notices.push_back(cut(&line, RING_LINE_CHARS));
    drop(notices);
    log(if level == Level::Info { Level::Debug } else { level }, "notice", format_args!("{source}: {message}"));
}

/// The last notifications, oldest first; gives up instead of waiting (for the panic hook).
pub fn try_notices() -> Vec<String> {
    NOTICES.try_lock().map(|n| n.iter().cloned().collect()).unwrap_or_default()
}

/// The current user's home folder, as redaction needs it (looked up once).
pub fn home() -> Option<&'static str> {
    HOME.get_or_init(|| dirs::home_dir().map(|h| h.to_string_lossy().into_owned())).as_deref()
}

/// `home` for the panic hook: never looks anything up. `init` and `crash::install` have done it by then.
pub fn home_if_ready() -> Option<&'static str> {
    HOME.get().and_then(|h| h.as_deref())
}

/// Wait (briefly) until the lines logged so far are on disk, so a file opened right after shows them.
pub fn flush() {
    if let Some(writer) = WRITER.get() {
        writer.flush_within(FLUSH_WAIT);
    }
}

fn remember(line: &str) {
    let mut ring = lock(&RING);
    if ring.len() == RING_LINES {
        ring.pop_front();
    }
    ring.push_back(cut(line, RING_LINE_CHARS));
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { text.chars().take(max).collect::<String>() + "…" }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// `2026-11-12T10:00:00.123Z WARN  update: message`: one line, redacted, controls replaced.
pub(crate) fn format_line(millis: u128, level: Level, area: &str, message: &str, home: Option<&str>) -> String {
    let secs = (millis / 1_000) as u64;
    let stamp = crate::texttools::iso_datetime(secs);
    let mut line = String::new();
    // The area is a caller's word, not a message: letters, digits, `-`, `_` and `.` only, so it cannot start a second line or field.
    let area: String = area.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).take(MAX_AREA_CHARS).collect();
    write!(line, "{}.{:03}Z {} {area}: ", stamp.trim_end_matches('Z'), millis % 1_000, level.label()).ok();
    let one_line: String = message.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    // Redacted first, cut second: a secret cut in two by the limit would leave its first half in the file.
    line.push_str(&redact_and_cut(&one_line, home, MAX_LINE_CHARS));
    line
}

enum Message {
    Line(String),
    Flush(SyncSender<()>),
}

/// The background thread that owns the files.
struct Writer {
    queue: SyncSender<Message>,
    /// Lines dropped because the queue was full (this writer's own count).
    dropped: AtomicUsize,
}

impl Writer {
    fn spawn(files: Files) -> Option<Writer> {
        let (queue, inbox) = sync_channel(QUEUE_LINES);
        std::thread::Builder::new().name("codeyard-log".into()).spawn(move || run(files, inbox)).ok()?;
        Some(Writer { queue, dropped: AtomicUsize::new(0) })
    }

    /// Never waits: a full queue drops the line.
    fn send(&self, line: String) {
        match self.queue.try_send(Message::Line(line)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    #[cfg(test)]
    fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    fn flush_within(&self, wait_at_most: Duration) {
        let (done, wait) = sync_channel(1);
        let deadline = std::time::Instant::now() + wait_at_most;
        let mut request = Message::Flush(done);
        // A full queue is only a reason to wait a little: the request goes in behind the lines.
        loop {
            match self.queue.try_send(request) {
                Ok(()) => break,
                Err(TrySendError::Full(back)) if std::time::Instant::now() < deadline => {
                    request = back;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return,
            }
        }
        wait.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()).max(Duration::from_millis(50))).ok();
    }
}

fn run(mut files: Files, inbox: Receiver<Message>) {
    while let Ok(first) = inbox.recv() {
        let mut waiting = Vec::new();
        for message in std::iter::once(first).chain(inbox.try_iter().take(QUEUE_LINES)) {
            match message {
                Message::Line(line) => files.append(&line),
                Message::Flush(done) => waiting.push(done),
            }
        }
        files.flush();
        for done in waiting {
            done.try_send(()).ok();
        }
    }
}

/// The current file and its rotated predecessors.
struct Files {
    dir: PathBuf,
    name: String,
    max_bytes: u64,
    out: Option<std::io::BufWriter<std::fs::File>>,
    size: u64,
}

impl Files {
    fn new(dir: PathBuf, name: &str, max_bytes: u64) -> Files {
        Files { dir, name: name.to_string(), max_bytes, out: None, size: 0 }
    }

    fn path(&self, n: usize) -> PathBuf {
        if n == 0 { self.dir.join(&self.name) } else { self.dir.join(format!("{}.{n}", self.name)) }
    }

    /// Write one line (a newline is added). Errors are dropped: a log must not be the reason for another failure.
    fn append(&mut self, line: &str) {
        let needed = line.len() as u64 + 1;
        if self.out.is_some() && self.size + needed > self.max_bytes {
            self.rotate();
        }
        if self.out.is_none() && self.open().is_err() {
            return;
        }
        if let Some(out) = self.out.as_mut() {
            if writeln!(out, "{line}").is_ok() {
                self.size += needed;
            } else {
                self.out = None;
            }
        }
    }

    fn flush(&mut self) {
        if let Some(out) = self.out.as_mut() {
            out.flush().ok();
        }
    }

    fn open(&mut self) -> std::io::Result<()> {
        crate::fsutil::create_private_dir(&self.dir)?;
        let path = self.path(0);
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        self.size = file.metadata().map_or(0, |m| m.len());
        self.out = Some(std::io::BufWriter::new(file));
        if self.size > self.max_bytes {
            self.rotate();
        }
        Ok(())
    }

    /// `.1` moves to `.2`, the current file to `.1`, and a new file starts. A rename onto an existing file fails on Windows,
    /// so the target is removed first.
    fn rotate(&mut self) {
        self.flush();
        self.out = None;
        for n in (1..FILES).rev() {
            let to = self.path(n);
            std::fs::remove_file(&to).ok();
            std::fs::rename(self.path(n - 1), &to).ok();
        }
        self.size = 0;
        self.open().ok();
    }
}

/// Files of the log in `dir`, newest first, for a reader (diagnostics report, tests).
#[cfg(test)]
fn read_all(dir: &std::path::Path, name: &str) -> String {
    (0..FILES).rev().filter_map(|n| std::fs::read_to_string(Files::new(dir.to_path_buf(), name, 0).path(n)).ok()).collect()
}

#[cfg(test)]
#[path = "diagnostics_log_tests.rs"]
mod tests;
