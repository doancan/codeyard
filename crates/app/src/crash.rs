//! Crash trace: on panic, the version, location, message and backtrace are written under `crashes/` in the state folder.
//! Nothing is sent over the network; the user shares the file themselves.

use std::io::Write;
use std::path::{Path, PathBuf};

/// At most this many reports are kept in the folder (oldest are deleted).
const KEEP: usize = 10;

/// Write the report as `dir/crash-<unix seconds>[-n].txt` and clean up old reports.
pub(crate) fn write_report(dir: &Path, secs: u64, text: &str) -> std::io::Result<PathBuf> {
    // Backtraces carry paths and sometimes message text: not for other users of the machine.
    crate::fsutil::create_private_dir(dir)?;
    // A second crash within the same second must not overwrite the first.
    let mut path = dir.join(format!("crash-{secs}.txt"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("crash-{secs}-{n}.txt"));
        n += 1;
    }
    crate::fsutil::create_private(&path)?.write_all(text.as_bytes())?;
    prune(dir);
    Ok(path)
}

/// Of the `crash-*.txt` files the newest `KEEP` remain (the name carries a timestamp, so they sort by name).
fn prune(dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(u64, PathBuf)> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let secs = name.strip_prefix("crash-")?.strip_suffix(".txt")?.split('-').next()?.parse().ok()?;
            Some((secs, e.path()))
        })
        .collect();
    files.sort();
    let extra = files.len().saturating_sub(KEEP);
    for (_, path) in files.into_iter().take(extra) {
        std::fs::remove_file(path).ok();
    }
}

/// A report is cut here (bytes): a backtrace of a runaway recursion must not fill the disk or the memory.
const MAX_REPORT_BYTES: usize = 256 * 1024;
/// Log lines and notifications that go into a report.
const LOG_LINES: usize = 20;

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    info.payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

fn panic_location(info: &std::panic::PanicHookInfo<'_>) -> String {
    info.location().map_or("(unknown)".into(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
}

/// The report as written: the home folder and credentials are removed and the size is bounded. `logged` and `notices` are the
/// last lines of the log and the last notifications (oldest first).
fn compose(header: &str, message: &str, logged: &[String], notices: &[String], backtrace: &str, home: Option<&str>) -> String {
    let mut text = String::with_capacity(8 * 1024);
    text.push_str(header);
    text.push_str(&format!("\n{message}\n"));
    text.push_str("\nlast notifications:\n");
    notices.iter().for_each(|l| text.push_str(&format!("{l}\n")));
    text.push_str("\nlast log lines:\n");
    logged.iter().for_each(|l| text.push_str(&format!("{l}\n")));
    text.push_str(&format!("\n{backtrace}\n"));
    let mut text = crate::diagnostics::redact::redact(&text, home);
    if text.len() > MAX_REPORT_BYTES {
        let mut end = MAX_REPORT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n(cut)\n");
    }
    text
}

fn report_text(info: &std::panic::PanicHookInfo<'_>, backtrace: &std::backtrace::Backtrace) -> String {
    use crate::diagnostics::log;
    let header = format!(
        "Codeyard {} ({} {})\nthread: {}\nat: {}\n{FATAL_UNKNOWN}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::current().name().unwrap_or("(unnamed)"),
        panic_location(info),
    );
    let logged = log::try_tail(LOG_LINES);
    compose(&header, &panic_message(info), &logged, &log::try_notices(), &backtrace.to_string(), log::home_if_ready())
}

/// What a report says about whether the process survived: the hook cannot know. A normal exit rewrites it as `fatal: false`.
pub(crate) const FATAL_UNKNOWN: &str = "fatal: unknown";

/// Install the panic hook. The previous hook (stderr output) keeps running.
///
/// A panic inside a panic hook aborts the process, and `catch_unwind` does not contain it, so nothing here may panic or wait:
/// no `unwrap`, `expect` or indexing; the log and notification buffers are read with `try_lock` and skipped when another
/// thread holds them; every size is capped; the patterns the redaction needs are compiled in advance; and every I/O error is
/// ignored.
pub fn install(dir: Option<PathBuf>) {
    let Some(dir) = dir else { return };
    crate::diagnostics::log::home();
    crate::diagnostics::redact::warm_up();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        crate::diagnostics::log::error_from_hook("crash", format_args!("panic at {}: {}", panic_location(info), panic_message(info)));
        let backtrace = std::backtrace::Backtrace::force_capture();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        if let Ok(path) = write_report(&dir, secs, &report_text(info, &backtrace)) {
            // Stderr may be closed (a GUI app): a failed write is not worth a second panic.
            let _ = writeln!(std::io::stderr(), "crash report written to {}", path.display());
            session::note_report(path);
        }
        crate::diagnostics::log::flush_from_hook();
    }));
}

#[path = "crash_session.rs"]
pub mod session;
pub use session::PreviousCrash;

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-crash-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        d
    }

    #[cfg(unix)]
    #[test]
    fn reports_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("private");
        let path = write_report(&dir, 1, "x").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir), mode(&path)), (0o700, 0o600));
    }

    #[test]
    fn reports_never_overwrite_and_old_ones_are_pruned() {
        let dir = tmp("prune");
        let a = write_report(&dir, 100, "first").unwrap();
        let b = write_report(&dir, 100, "second").unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "second");
        for secs in 101..130 {
            write_report(&dir, secs, "x").unwrap();
        }
        let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names.len(), KEEP);
        assert_eq!(names.last().unwrap(), "crash-129.txt");
        assert!(!names.contains(&"crash-100.txt".to_string()), "{names:?}");
        // Other files are left alone.
        std::fs::write(dir.join("notes.txt"), "keep").unwrap();
        write_report(&dir, 200, "x").unwrap();
        assert!(dir.join("notes.txt").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hook_writes_a_report_with_message_and_location() {
        let dir = tmp("hook");
        // The hook is global: installed only in this test, the panic is caught in a separate thread.
        install(Some(dir.clone()));
        let r = std::thread::Builder::new().name("boom".into()).spawn(|| panic!("kaboom {}", 42)).unwrap().join();
        assert!(r.is_err());
        // The hook is process-wide: another test running in parallel may write a report on panic too, so look for ours.
        let text = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .find(|t| t.contains("kaboom 42"))
            .expect("a report for our panic");
        assert!(text.contains("thread: boom"), "{text}");
        assert!(text.contains("crash.rs"), "{text}");
        assert!(text.starts_with("Codeyard "), "{text}");
        assert!(text.contains("last log lines:") && text.contains("last notifications:"), "{text}");
        if let Some(home) = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).filter(|h| h.len() > 3) {
            assert!(!text.contains(&home), "the home folder must not be in the report");
        }
        let _ = std::panic::take_hook();
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
#[path = "crash_tests.rs"]
mod follow_up_tests;
