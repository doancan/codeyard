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

fn report_text(info: &std::panic::PanicHookInfo<'_>, backtrace: &std::backtrace::Backtrace) -> String {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into());
    let at = info.location().map_or("(unknown)".into(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
    format!(
        "Codeyard {} ({} {})\nthread: {}\nat: {at}\n\n{message}\n\n{backtrace}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::current().name().unwrap_or("(unnamed)"),
    )
}

/// Install the panic hook: the previous hook (stderr output) keeps running, and it stays silent if the report can't be written.
pub fn install(dir: Option<PathBuf>) {
    let Some(dir) = dir else { return };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        let backtrace = std::backtrace::Backtrace::force_capture();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        match write_report(&dir, secs, &report_text(info, &backtrace)) {
            Ok(path) => eprintln!("crash report written to {}", path.display()),
            Err(e) => eprintln!("could not write a crash report: {e}"),
        }
    }));
}

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
        let _ = std::panic::take_hook();
        std::fs::remove_dir_all(&dir).ok();
    }
}
