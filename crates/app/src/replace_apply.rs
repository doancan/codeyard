//! Writing a replacement plan to disk and undoing it, one file at a time. This is not a multi-file transaction: each file
//! is published atomically by the agent (temporary file and rename) and only if it still has the hash the preview read,
//! but files are written one after another. So every file gets its own recorded outcome, and after an error the file is
//! read back to tell "not changed" from "changed" and from "unknown" (a lost connection). Nothing is retried.
//!
//! Undo writes the previous bytes back only if the file still has the hash the replacement produced: a file edited
//! since is reported as an undo conflict and left as it is. The agent refuses a write against a version when the file is
//! gone, so a file deleted since the preview (or since the replacement) is a conflict too, never recreated.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use host::{Host, HostError};

/// The file operations apply and undo need (the host in the app, a fault-injecting wrapper in tests).
pub trait Store: Send + Sync {
    fn read(&self, path: &str) -> Result<(Vec<u8>, u64), HostError>;
    /// Write `data` if the file's current hash is `expect`; returns the new hash. A file missing at that moment is a
    /// conflict (the agent refuses to recreate it).
    fn write(&self, path: &str, data: Vec<u8>, expect: Option<u64>) -> Result<u64, HostError>;
}

impl Store for Arc<dyn Host> {
    fn read(&self, path: &str) -> Result<(Vec<u8>, u64), HostError> {
        self.as_ref().read(path)
    }
    fn write(&self, path: &str, data: Vec<u8>, expect: Option<u64>) -> Result<u64, HostError> {
        self.as_ref().write(path, data, expect)
    }
}

/// What happened to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Written (or undone); for apply, the number of changes.
    Changed(usize),
    /// The file differs from the version the preview (or the replacement) saw; left as it is.
    Conflict,
    /// Refused or failed and confirmed unchanged.
    Failed(String),
    /// The result could not be confirmed (connection lost, or the file changed in a way that is neither version).
    Uncertain(String),
    /// Not attempted: cancelled before this file.
    NotAttempted,
    /// Not attempted: the preview rejected it (binary, encoding) or nothing was selected.
    Skipped(String),
}

impl Outcome {
    pub fn changed(&self) -> bool {
        matches!(self, Outcome::Changed(_))
    }
}

/// One file to write.
pub struct Job {
    pub path: String,
    pub bytes: Vec<u8>,
    /// The hash the preview read; the write is refused if the file has changed since.
    pub expect: u64,
    /// The bytes the preview read, kept for undo.
    pub original: Arc<Vec<u8>>,
    pub changes: usize,
}

/// What undo needs for a written file.
#[derive(Clone, Debug)]
pub struct UndoDisk {
    pub path: String,
    pub original: Arc<Vec<u8>>,
    /// The hash the replacement left; undo is refused if the file has changed since.
    pub hash_after: u64,
}

/// Write each job in order; stop starting new ones once `cancel` is set. Returns each file's outcome and, for the files
/// written, what undo needs.
/// Jobs are taken one at a time, so a caller can build each file's bytes only when it is its turn.
pub fn write_all(store: &dyn Store, jobs: impl IntoIterator<Item = Job>, cancel: &AtomicBool) -> (Vec<(String, Outcome)>, Vec<UndoDisk>) {
    let mut outcomes = Vec::new();
    let mut undo = Vec::new();
    for job in jobs {
        if cancel.load(Ordering::SeqCst) {
            outcomes.push((job.path, Outcome::NotAttempted));
            continue;
        }
        let new_hash = proto::content_hash(&job.bytes);
        let outcome = match store.write(&job.path, job.bytes, Some(job.expect)) {
            Ok(hash) => {
                undo.push(UndoDisk { path: job.path.clone(), original: job.original.clone(), hash_after: hash });
                Outcome::Changed(job.changes)
            }
            Err(e) => match settle(store, &job.path, job.expect, new_hash, e) {
                Outcome::Changed(_) => {
                    undo.push(UndoDisk { path: job.path.clone(), original: job.original.clone(), hash_after: new_hash });
                    Outcome::Changed(job.changes)
                }
                other => other,
            },
        };
        outcomes.push((job.path, outcome));
    }
    (outcomes, undo)
}

/// Put the previous bytes back, each file only if it still has the hash the replacement left.
pub fn undo_all(store: &dyn Store, entries: &[UndoDisk], cancel: &AtomicBool) -> Vec<(String, Outcome)> {
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        if cancel.load(Ordering::SeqCst) {
            out.push((e.path.clone(), Outcome::NotAttempted));
            continue;
        }
        let restored = proto::content_hash(&e.original);
        let outcome = match store.write(&e.path, e.original.to_vec(), Some(e.hash_after)) {
            Ok(_) => Outcome::Changed(0),
            Err(err) => settle(store, &e.path, e.hash_after, restored, err),
        };
        out.push((e.path.clone(), outcome));
    }
    out
}

/// After a failed write: read the file back. Still `before` → confirmed unchanged; `after` → the write landed; anything
/// else, or no answer, cannot be confirmed either way.
fn settle(store: &dyn Store, path: &str, before: u64, after: u64, err: HostError) -> Outcome {
    if let HostError::Op(proto::Error::Conflict { .. }) = err {
        return Outcome::Conflict;
    }
    match store.read(path) {
        Ok((_, h)) if h == after => Outcome::Changed(0),
        Ok((_, h)) if h == before => Outcome::Failed(err.to_string()),
        Ok(_) => Outcome::Uncertain(crate::i18n::tf("The file changed during the write ({})", &[&err])),
        Err(read) => Outcome::Uncertain(format!("{err}; {read}")),
    }
}

/// A finished apply or undo, for the report: every file's outcome.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub undo: bool,
    pub files: Vec<(String, Outcome)>,
}

impl Report {
    pub fn changed(&self) -> Vec<&str> {
        self.files.iter().filter(|(_, o)| o.changed()).map(|(p, _)| p.as_str()).collect()
    }

    pub fn changes(&self) -> usize {
        self.files.iter().map(|(_, o)| if let Outcome::Changed(n) = o { *n } else { 0 }).sum()
    }

    /// Report lines for the user: what changed, what did not, what is unknown. `name` shortens a path for display.
    pub fn lines(&self, name: impl Fn(&str) -> String) -> Vec<(String, bool)> {
        let list = |paths: Vec<&str>| paths.into_iter().map(&name).collect::<Vec<_>>().join(", ");
        let mut out = Vec::new();
        let changed = self.changed();
        if !changed.is_empty() {
            out.push((
                if self.undo {
                    crate::i18n::tn("Restored {} file: {}", "Restored {} files: {}", changed.len(), &[&list(changed)])
                } else {
                    crate::i18n::tf("Changed {0} in {1}", &[&crate::i18n::tn("{} change", "{} changes", self.changes(), &[]), &list(changed)])
                },
                false,
            ));
        }
        for (path, o) in &self.files {
            let line = match o {
                Outcome::Changed(_) => continue,
                Outcome::Conflict if self.undo => crate::i18n::tf("Undo conflict: {} changed after the replacement and was left as it is.", &[&name(path)]),
                Outcome::Conflict => crate::i18n::tf("File changed since preview: {}", &[&name(path)]),
                Outcome::Failed(e) => crate::i18n::tf("Not changed: {0} ({1})", &[&name(path), e]),
                Outcome::Uncertain(e) => crate::i18n::tf("Unknown outcome, check before retrying: {0} ({1})", &[&name(path), e]),
                Outcome::NotAttempted => crate::i18n::tf("Not changed (cancelled): {}", &[&name(path)]),
                Outcome::Skipped(why) => crate::i18n::tf("Not changed: {0} ({1})", &[&name(path), why]),
            };
            out.push((line, true));
        }
        out
    }
}

#[cfg(test)]
#[path = "replace_apply_tests.rs"]
mod tests;
