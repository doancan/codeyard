//! Read-only files and folders for tests that need a write to fail. The system enforces them for everyone but root (a CI container
//! runs as root): there the probe finds the write still works, and the test returns early instead of failing for the wrong reason.
//! The guard puts the permissions back when it goes out of scope, so a failing assertion never leaves a read-only temp folder.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(super) struct Locked {
    path: PathBuf,
    mode: u32,
}

impl Drop for Locked {
    fn drop(&mut self) {
        set_mode(&self.path, self.mode);
    }
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).map_or(0o755, |m| m.permissions().mode() & 0o7777)
}

fn set_mode(path: &Path, mode: u32) {
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

fn skipped(what: &str) -> Option<Locked> {
    eprintln!("skipped: this process can still write to {what} after chmod (running as root?)");
    None
}

/// Make the folder read-only. `None` when writing there still works.
pub(super) fn lock_dir(path: &Path) -> Option<Locked> {
    let guard = Locked { path: path.to_path_buf(), mode: mode_of(path) };
    set_mode(path, 0o555);
    let probe = path.join(format!(".lock-probe-{}", std::process::id()));
    if std::fs::write(&probe, b"").is_ok() {
        let _ = std::fs::remove_file(&probe);
        drop(guard);
        return skipped(&path.display().to_string());
    }
    Some(guard)
}

/// Make the file read-only. `None` when writing to it still works.
pub(super) fn lock_file(path: &Path) -> Option<Locked> {
    let guard = Locked { path: path.to_path_buf(), mode: mode_of(path) };
    set_mode(path, 0o444);
    if std::fs::OpenOptions::new().append(true).open(path).is_ok() {
        drop(guard);
        return skipped(&path.display().to_string());
    }
    Some(guard)
}
