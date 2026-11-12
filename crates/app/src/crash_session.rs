//! Did the last run end badly? Each process writes `running-<pid>` in the crashes folder when it starts and removes it when it
//! quits normally. A marker whose process is gone is a run that did not finish: killed, crashed, power loss. Only then does the
//! next start say "Codeyard quit unexpectedly last time".
//!
//! A panic that the app survived (a reader thread that died, a worker that was caught) also writes a report, and the report stays
//! in the folder for a bug report, but it announces nothing: the run ended normally, and the report is marked `fatal: false`
//! when the process quits. No clock comparison decides whether to announce, so a wrong system time cannot hide a crash.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::{FATAL_UNKNOWN, MAX_REPORT_BYTES};

pub(super) const RUNNING_PREFIX: &str = "running-";
/// A marker whose process id is alive but that is older than this belongs to a reused process id, not to a running app.
const STALE_AFTER_SECS: u64 = 7 * 24 * 3600;
/// Reports of this process remembered to mark at a normal exit.
const MAX_NOTED: usize = 16;

/// What the last run left behind, for the notification at the next start.
#[derive(Debug, PartialEq, Eq, Default, Clone)]
pub struct PreviousCrash {
    /// Where the newest report of that run says it happened, when there is a report.
    pub location: Option<String>,
    pub message: Option<String>,
}

static NOTED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static PENDING: Mutex<Option<PreviousCrash>> = Mutex::new(None);

/// A report this process wrote, to be marked as survived at a normal exit. Never waits (the panic hook calls it).
pub(super) fn note_report(path: PathBuf) {
    if let Ok(mut noted) = NOTED.try_lock()
        && noted.len() < MAX_NOTED
    {
        noted.push(path);
    }
}

/// Start of a run: find the runs that did not finish (the answer waits for the first window), then mark this one as running.
pub fn begin(dir: &Path) {
    let now = now_secs();
    let previous = begin_with(dir, std::process::id(), now, process_alive);
    if let Ok(mut pending) = PENDING.lock() {
        *pending = previous;
    }
    #[cfg(unix)]
    clean_up_marker_on_signals(&marker_path(dir, std::process::id()));
}

/// The unfinished run, once: the first window takes it.
pub fn take_previous() -> Option<PreviousCrash> {
    PENDING.lock().ok()?.take()
}

/// Normal end of a run: this process's marker goes, and the reports it wrote on the way (panics it survived) say so.
pub fn end(dir: &Path) {
    let noted: Vec<PathBuf> = NOTED.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default();
    end_with(dir, std::process::id(), &noted);
}

pub(super) fn end_with(dir: &Path, pid: u32, reports: &[PathBuf]) {
    std::fs::remove_file(marker_path(dir, pid)).ok();
    reports.iter().for_each(|path| mark_survived(path));
}

fn mark_survived(path: &Path) {
    let Some(text) = read_capped(path) else { return };
    let marked = text.replacen(FATAL_UNKNOWN, "fatal: false (the app kept running and quit normally afterwards)", 1);
    if marked != text {
        crate::fsutil::write_private(path, marked).ok();
    }
}

fn marker_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{RUNNING_PREFIX}{pid}"))
}

/// The contents of a marker: process id, start time (seconds since the epoch), version.
struct Marker {
    pid: Option<u32>,
    started: Option<u64>,
}

fn parse_marker(text: &str) -> Marker {
    let mut lines = text.lines();
    Marker { pid: lines.next().and_then(|l| l.trim().parse().ok()), started: lines.next().and_then(|l| l.trim().parse().ok()) }
}

/// Is this marker the trace of a run that is still going?
fn still_running(marker: &Marker, own_pid: u32, now: u64, alive: &dyn Fn(u32) -> bool) -> bool {
    let Some(pid) = marker.pid else { return false };
    if pid == own_pid {
        return true;
    }
    // A live process id with a marker older than a week is a reused id; a start time in the future is not trusted either way.
    let fresh = marker.started.is_none_or(|started| started > now || now - started < STALE_AFTER_SECS);
    alive(pid) && fresh
}

/// `begin` with the process id, the time and the liveness check given. Returns what to announce, if anything.
pub(super) fn begin_with(dir: &Path, own_pid: u32, now: u64, alive: impl Fn(u32) -> bool) -> Option<PreviousCrash> {
    let mut unfinished: Vec<Marker> = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        if !name.to_str().is_some_and(|n| n.starts_with(RUNNING_PREFIX)) {
            continue;
        }
        let marker = parse_marker(&read_capped(&entry.path()).unwrap_or_default());
        if still_running(&marker, own_pid, now, &alive) {
            continue;
        }
        std::fs::remove_file(entry.path()).ok();
        unfinished.push(marker);
    }
    write_own_marker(dir, own_pid, now);
    if unfinished.is_empty() {
        return None;
    }
    // Details come from the newest report written during the unfinished run, if there is one.
    let started = unfinished.iter().filter_map(|m| m.started).min();
    let report = started.and_then(|started| newest_report_since(dir, started));
    Some(report.map(|text| summarize(&text)).unwrap_or_default())
}

fn write_own_marker(dir: &Path, pid: u32, now: u64) {
    if crate::fsutil::create_private_dir(dir).is_err() {
        return;
    }
    let text = format!("{pid}\n{now}\n{}\n", env!("CARGO_PKG_VERSION"));
    if let Err(e) = crate::fsutil::write_private(&marker_path(dir, pid), text) {
        crate::diagnostics::log::warn("crash", format_args!("could not write the running marker: {e}"));
    }
}

/// `crash-<secs>[-<n>].txt` as a sortable key.
pub(super) fn crash_key(name: &str) -> Option<(u64, u32)> {
    let rest = name.strip_prefix("crash-")?.strip_suffix(".txt")?;
    let (secs, n) = rest.split_once('-').unwrap_or((rest, "0"));
    Some((secs.parse().ok()?, n.parse().ok()?))
}

fn newest_report_since(dir: &Path, started: u64) -> Option<String> {
    let newest = std::fs::read_dir(dir).ok()?.flatten().filter_map(|e| Some((crash_key(e.file_name().to_str()?)?, e.path()))).max()?;
    (newest.0.0 >= started).then(|| read_capped(&newest.1)).flatten()
}

/// The `at:` line and the message of a report this module wrote; anything else gives what can be found.
pub(super) fn summarize(text: &str) -> PreviousCrash {
    let location = text.lines().find_map(|l| l.strip_prefix("at: ")).map(str::to_string);
    let message = text.split("\n\n").nth(1).map(|m| m.trim().chars().take(300).collect::<String>()).filter(|m| !m.is_empty());
    PreviousCrash { location, message }
}

/// The start of a file, at most `MAX_REPORT_BYTES`.
fn read_capped(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(MAX_REPORT_BYTES as u64).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Is a process with this id running? Being refused the question (`EPERM`, access denied) still means it exists.
fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, STILL_ACTIVE};
        use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: the handle is used only here and closed; the exit code is a plain out value.
        unsafe {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(handle) => {
                    let mut code = 0u32;
                    let running = GetExitCodeProcess(handle, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
                    let _ = CloseHandle(handle);
                    running
                }
                // Refused: the process exists but belongs to someone else. Any other error (no such process) means it is gone.
                Err(e) => e.code() == windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0),
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(unix)]
static MARKER_C_PATH: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();

/// Removes the running marker. Only async-signal-safe calls: it runs inside a signal handler.
#[cfg(unix)]
fn unlink_marker_signal_safe() {
    if let Some(path) = MARKER_C_PATH.get() {
        // SAFETY: `unlink` is async-signal-safe and the path is a valid C string that is never freed.
        unsafe { libc::unlink(path.as_ptr()) };
    }
}

#[cfg(unix)]
extern "C" fn on_terminating_signal(sig: libc::c_int) {
    unlink_marker_signal_safe();
    // SAFETY: restoring the default action and raising the signal again ends the process the way it would have ended anyway.
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

/// SIGTERM, SIGHUP and SIGINT end the run by the user's or the system's request, not by a crash: the marker goes, then the
/// signal does its usual work. Handlers someone else installed are left alone. Only SIGKILL and real crashes keep the marker.
#[cfg(unix)]
fn clean_up_marker_on_signals(marker: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(marker.as_os_str().as_bytes()) else { return };
    if MARKER_C_PATH.set(path).is_err() {
        return;
    }
    for sig in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
        // SAFETY: installing a handler that only calls async-signal-safe functions.
        unsafe {
            let previous = libc::signal(sig, on_terminating_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
            if previous != libc::SIG_DFL && previous != libc::SIG_ERR {
                libc::signal(sig, previous);
            }
        }
    }
}

#[cfg(all(test, unix))]
pub(super) fn unlink_marker_for_test(path: &Path) -> bool {
    // The static holds one path for the life of the process; a test borrows it only when it is the first to set it.
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    if MARKER_C_PATH.set(c).is_err() {
        return false;
    }
    unlink_marker_signal_safe();
    true
}

#[cfg(all(test, unix))]
pub(super) fn process_alive_for_test(pid: u32) -> bool {
    process_alive(pid)
}
