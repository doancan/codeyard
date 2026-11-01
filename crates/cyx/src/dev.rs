//! `cyx dev`: Codeyard with the extension loaded from its folder (`--extension-dev`). Codeyard reloads data files
//! itself when they change; for code, cyx watches the sources and rebuilds `extension.wasm` on save (cargo, or the
//! TypeScript pipeline), and Codeyard swaps the new build into the running window (crates/app/src/workbench_ext_dev.rs).
//! A failed build prints its errors and leaves the last good build running.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::build;

/// How often the sources are looked at.
const POLL: Duration = Duration::from_millis(150);

/// Files the build writes, which must not start another build.
fn is_output(name: &str) -> bool {
    name == ext_pkg::COMPONENT
        || name == build::PARTIAL
        || name == crate::typescript::SOURCE_MAP
        || name == "Cargo.lock"
        || name == "package-lock.json"
        || name.ends_with(".cyx")
}

/// Names, sizes and modification times of the extension's sources (everything a package would hold, minus the build's
/// own output), sorted.
pub fn sources(dir: &Path) -> Vec<(PathBuf, u64, Option<SystemTime>)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64, Option<SystemTime>)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if ext_pkg::EXCLUDED.contains(&name.as_str()) || is_output(&name) || out.len() > 20_000 {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                walk(&e.path(), out);
            } else {
                out.push((e.path(), m.len(), m.modified().ok()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

/// Build once, start Codeyard, then rebuild on every save until its window closes.
pub fn run(dir: &Path, extra: &[String]) -> Result<(), String> {
    let bin = crate::codeyard_binary()
        .ok_or_else(|| "error: Codeyard was not found\n   = fix: install Codeyard, or set CODEYARD_BIN to the path of its codeyard binary".to_string())?;
    run_with(dir, &bin, extra, None)
}

/// `run` with Codeyard at `bin`, and cargo's output in `target_dir` instead of the crate's own `target/` (tests).
pub fn run_with(dir: &Path, bin: &Path, extra: &[String], target_dir: Option<&Path>) -> Result<(), String> {
    let dir = std::fs::canonicalize(dir).map_err(|e| format!("error: {}: {e}", dir.display()))?;
    // The sources as they are before the first build: a file saved while it builds, while the check runs or while
    // Codeyard starts is a change seen on the first poll, and is built then.
    let mut seen = sources(&dir);
    // Start only something that loads: the window would otherwise open with an error the terminal explains better.
    build::build_in(&dir, target_dir)?;
    let report = crate::check_dev(&dir)?;
    for w in &report.warnings {
        eprintln!("{w}");
    }
    println!("Running {} with {} loaded. Save a file to reload; close the window to stop.", bin.display(), dir.display());
    let mut child = std::process::Command::new(bin)
        .arg("--extension-dev")
        .arg(&dir)
        .args(extra)
        .spawn()
        .map_err(|e| format!("error: cannot start {}: {e}", bin.display()))?;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("error: {e}"))? {
            return if status.success() { Ok(()) } else { Err(format!("Codeyard exited with {status}")) };
        }
        std::thread::sleep(POLL);
        let now = sources(&dir);
        if now == seen {
            continue;
        }
        // An editor may save several files at once: wait until nothing changed for one poll.
        seen = settle(&dir, now);
        rebuild(&dir, target_dir);
    }
}

fn settle(dir: &Path, mut now: Vec<(PathBuf, u64, Option<SystemTime>)>) -> Vec<(PathBuf, u64, Option<SystemTime>)> {
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(50));
        let again = sources(dir);
        if again == now {
            break;
        }
        now = again;
    }
    now
}

/// Rebuild the code; Codeyard notices a new `extension.wasm` and reloads it. Data-only extensions have nothing to build
/// (Codeyard reloads their files itself).
fn rebuild(dir: &Path, target_dir: Option<&Path>) {
    let has_code = build::manifest(dir).is_ok_and(|m| m.code.is_some());
    if !has_code {
        return;
    }
    let before = std::fs::metadata(dir.join(ext_pkg::COMPONENT)).and_then(|m| m.modified()).ok();
    let started = Instant::now();
    match build::build_in(dir, target_dir) {
        Ok(_) => {
            let after = std::fs::metadata(dir.join(ext_pkg::COMPONENT)).and_then(|m| m.modified()).ok();
            let secs = started.elapsed().as_secs_f64();
            if after != before {
                println!("Built in {secs:.1} s; Codeyard reloads it.");
            } else {
                println!("Built in {secs:.1} s; the code did not change.");
            }
        }
        Err(e) => eprintln!("{e}\nThe last good build keeps running; save again to retry."),
    }
}
