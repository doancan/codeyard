//! A small record of a transaction in progress, kept on this computer (also for an SSH folder) while its steps run. When
//! Codeyard dies in the middle (a crash, a kill, power), the record is what is left: on the next start it is found and the user
//! is told which files may be half-changed. It lists the planned steps and the paths only: it holds no copies of files, so
//! nothing is restored from it; the user checks the named files (with their version control, where there is one).
//!
//! A record is removed when the transaction ends with everything done or everything taken back; one whose rollback failed
//! stays, so the warning is not lost. Reading and writing it is blocking file work: callers run it on a background thread.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::apply::Step;

/// A record older than this, when the process that made it cannot be asked about, is taken as left over.
const STALE_AFTER_SECS: u64 = 10 * 60;

pub struct Journal {
    path: PathBuf,
}

/// What an interrupted transaction left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interrupted {
    pub host: String,
    /// The folder is on an SSH host, not on this computer.
    pub remote: bool,
    pub root: String,
    /// The paths of the steps, in order, each with what it was to do.
    pub steps: Vec<(String, String)>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn step_json(step: &Step) -> Value {
    match step {
        Step::Create { path } => json!({ "op": "create", "path": path }),
        Step::Rename { from, to } => json!({ "op": "rename", "path": from, "to": to }),
        Step::Delete { path } => json!({ "op": "delete", "path": path }),
        Step::Write { path, expect, bytes, .. } => json!({ "op": "write", "path": path, "was": expect, "becomes": proto::content_hash(bytes) }),
    }
}

impl Journal {
    /// Write the record before the first step. `None` when it cannot be written: the transaction still runs (it is as safe as
    /// before), only without a record.
    pub fn begin(dir: &Path, host: &str, remote: bool, root: &str, steps: &[Step]) -> Option<Journal> {
        std::fs::create_dir_all(dir).ok()?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = dir.join(format!("refactor-journal-{}-{}.json", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let body = json!({ "pid": std::process::id(), "started": now(), "host": host, "remote": remote, "root": root, "steps": steps.iter().map(step_json).collect::<Vec<_>>() });
        std::fs::write(&path, body.to_string()).ok()?;
        Some(Journal { path })
    }

    /// The transaction ended; the record is no longer needed unless `keep` (a step could not be taken back).
    pub fn finish(self, keep: bool) {
        if !keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn process_alive(pid: u32) -> Option<bool> {
    #[cfg(unix)]
    {
        // Signal 0 only asks whether the process exists; "not permitted" means it does.
        let gone = unsafe { libc::kill(pid as i32, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        Some(!gone)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// Records of transactions that did not end: made by a process that is gone (or, where that cannot be asked, long ago). Each
/// is removed as it is read, so it is reported once.
pub fn take_interrupted(dir: &Path) -> Vec<Interrupted> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("refactor-journal-") && name.ends_with(".json")) {
            continue;
        }
        let Some(body) = std::fs::read_to_string(entry.path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else {
            continue;
        };
        let pid = body["pid"].as_u64().unwrap_or(0) as u32;
        let age = now().saturating_sub(body["started"].as_u64().unwrap_or(0));
        let stale = pid != std::process::id() && process_alive(pid).map_or(age > STALE_AFTER_SECS, |alive| !alive);
        if !stale {
            continue;
        }
        let steps = body["steps"]
            .as_array()
            .map(|a| a.iter().map(|s| (s["path"].as_str().unwrap_or_default().to_string(), s["op"].as_str().unwrap_or_default().to_string())).collect())
            .unwrap_or_default();
        out.push(Interrupted {
            host: body["host"].as_str().unwrap_or_default().into(),
            remote: body["remote"].as_bool().unwrap_or(false),
            root: body["root"].as_str().unwrap_or_default().into(),
            steps,
        });
        let _ = std::fs::remove_file(entry.path());
    }
    out
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
