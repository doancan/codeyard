//! The hand-over between the old and the new process of an update restart: which windows to reopen.
//!
//! The old version writes `update-restart.json` into the state folder right before it quits (after hot exit has saved every
//! window); the relaunch helper starts the new version with `--after-update`, which reads the file once and deletes it. A file
//! instead of folder arguments on the command line, because the folders of several windows would have to pass through `open
//! --args` and the Windows installer's `[Run]` parameters, and a path with quotes or spaces is exactly what those mangle. The
//! file is read only from the state folder (never from a project), only with the flag, only for the version the updater installed
//! and only for ten minutes; whatever is wrong with it, the new version starts like it always did.
//!
//! What comes back inside a window (tabs, cursors, unsaved edits) is not here: it is the folder's own session (`window.restoreSession`,
//! `files.hotExit`), restored when the folder is opened.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, WindowHandle};
use serde_json::{Value, json};

use crate::workbench::Workbench;

/// Command line flag that tells the new version it was started by an update restart.
pub const FLAG: &str = "--after-update";
const FILE: &str = "update-restart.json";
const SCHEMA: u64 = 1;
/// A restart takes seconds; a file this old belongs to a restart that did not happen.
const MAX_AGE: Duration = Duration::from_secs(10 * 60);
const MAX_WINDOWS: usize = 32;

/// One window's folder: on this computer (`host` is `None`) or on an SSH host (its alias).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub host: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handover {
    /// The version the update installed: the only version that may read the file.
    pub to: String,
    pub windows: Vec<Folder>,
    /// Index into `windows` of the window that was in front.
    pub focused: usize,
}

#[derive(Debug)]
pub enum HandoverError {
    Io(io::Error),
    Corrupt(&'static str),
    /// Written for another version: a leftover of an update that did not finish.
    WrongVersion {
        found: String,
        running: String,
    },
    /// Older than ten minutes, or from the future.
    Stale,
}

impl std::fmt::Display for HandoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Corrupt(why) => write!(f, "malformed: {why}"),
            Self::WrongVersion { found, running } => write!(f, "written for version {found}, this is {running}"),
            Self::Stale => write!(f, "too old"),
        }
    }
}

impl std::error::Error for HandoverError {}

impl From<io::Error> for HandoverError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

fn file(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Writes the file atomically: a half-written one is never seen by the new process.
pub fn write(dir: &Path, h: &Handover, now: SystemTime) -> Result<(), HandoverError> {
    let ms = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let windows: Vec<Value> = h.windows.iter().map(|w| json!({ "host": w.host, "path": w.path })).collect();
    let doc = json!({ "schema": SCHEMA, "to": h.to, "written_ms": ms, "focused": h.focused, "windows": windows });
    crate::fsutil::write_private(&file(dir), doc.to_string())?;
    Ok(())
}

/// Removes the file (a restart that did not happen). A missing file is fine.
pub fn discard(dir: &Path) {
    if let Err(e) = std::fs::remove_file(file(dir))
        && e.kind() != io::ErrorKind::NotFound
    {
        eprintln!("could not remove {}: {e}", file(dir).display());
    }
}

/// Reads and deletes the file: it is used once, whatever it holds. `Ok(None)`: there is none.
pub fn take(dir: &Path, running: &str, now: SystemTime) -> Result<Option<Handover>, HandoverError> {
    let bytes = match std::fs::read(file(dir)) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    discard(dir);
    parse(&bytes, running, now).map(Some)
}

fn parse(bytes: &[u8], running: &str, now: SystemTime) -> Result<Handover, HandoverError> {
    use HandoverError::{Corrupt, Stale, WrongVersion};
    let v: Value = serde_json::from_slice(bytes).map_err(|_| Corrupt("not JSON"))?;
    if v["schema"].as_u64() != Some(SCHEMA) {
        return Err(Corrupt("unknown schema"));
    }
    let to = v["to"].as_str().ok_or(Corrupt("no version"))?;
    if to != running {
        return Err(WrongVersion { found: to.into(), running: running.into() });
    }
    let written = v["written_ms"].as_u64().ok_or(Corrupt("no time"))?;
    match now.duration_since(UNIX_EPOCH + Duration::from_millis(written)) {
        Ok(age) if age <= MAX_AGE => {}
        _ => return Err(Stale),
    }
    let mut windows = Vec::new();
    for w in v["windows"].as_array().ok_or(Corrupt("no windows"))?.iter().take(MAX_WINDOWS) {
        let path = w["path"].as_str().filter(|p| !p.is_empty() && !p.contains('\0')).ok_or(Corrupt("bad path"))?;
        let host = match &w["host"] {
            Value::Null => None,
            // An alias is handed to ssh: one that looks like an option would be one.
            Value::String(h) if !h.is_empty() && !h.starts_with('-') && !h.contains('\0') => Some(h.clone()),
            _ => return Err(Corrupt("bad host")),
        };
        windows.push(Folder { host, path: path.into() });
    }
    if windows.is_empty() {
        return Err(Corrupt("no windows"));
    }
    let focused = v["focused"].as_u64().and_then(|i| usize::try_from(i).ok()).filter(|i| *i < windows.len()).unwrap_or(0);
    Ok(Handover { to: to.into(), windows, focused })
}

/// Opens the folders of `h`: the first in `first` (the window the process started with), every other in a window of its own made by
/// `new_window`; then the one that was in front is brought to the front.
pub fn reopen_windows(h: &Handover, first: WindowHandle<Workbench>, new_window: &mut dyn FnMut(&mut App) -> Option<WindowHandle<Workbench>>, cx: &mut App) {
    let mut opened = Vec::new();
    for (i, folder) in h.windows.iter().enumerate() {
        let Some(window) = (if i == 0 { Some(first) } else { new_window(cx) }) else { continue };
        let folder = folder.clone();
        let done = window.update(cx, |wb, window, cx| {
            wb.reopen(folder.host, folder.path, window, cx);
            window.focus(&gpui::Focusable::focus_handle(wb, cx));
        });
        if done.is_ok() {
            opened.push((i, window));
        }
    }
    if let Some((_, window)) = opened.iter().find(|(i, _)| *i == h.focused) {
        window.update(cx, |_, window, _| window.activate_window()).ok();
    }
}

#[cfg(test)]
#[path = "update_restart_tests.rs"]
mod tests;
