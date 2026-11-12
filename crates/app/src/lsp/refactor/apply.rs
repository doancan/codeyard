//! Writing a refactoring to the host as one transaction, as far as a file system allows one.
//!
//! The steps run in the order the language server gave them. Each step records how to take it back; when one fails (or the
//! user stops it) the recorded steps are taken back in reverse, so the folder is left as it was found:
//!
//! - a created file is removed again,
//! - a rename is renamed back (the agent never overwrites, so a name taken meanwhile fails the step instead of losing a file),
//! - a delete only moves the file or folder to a hidden name next to it; the move is what can be taken back, and the real
//!   delete happens after every other step has worked, so nothing is lost because a later step failed,
//! - a text write is written back with the bytes read for the preview, but only if the file still has the hash the write
//!   left (a file somebody else changed in between is reported, not overwritten).
//!
//! What can still go wrong is reported file by file: a rollback step that fails leaves its file named in the report with
//! where it stands. Open editors are not part of this (the workbench applies them first, all or nothing, and takes them back
//! when this transaction fails).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use host::{Host, HostError};

use super::plan::{Chosen, Item, RefactorPlan, Resource};
use crate::replace_apply::{Outcome, Store, UndoDisk};
use crate::replace_plan::Version;

/// The file operations a refactoring needs: `Store`'s reads and hash-checked writes plus create, rename and delete.
pub trait Fs: Store {
    /// A new empty file (and missing folders above it); `AlreadyExists` if it is there. Never overwrites.
    fn create(&self, path: &str) -> Result<(), HostError>;
    /// `AlreadyExists` if `to` is taken. Never overwrites.
    fn rename(&self, from: &str, to: &str) -> Result<(), HostError>;
    fn delete(&self, path: &str) -> Result<(), HostError>;
    fn is_dir(&self, path: &str) -> bool;
    fn dir_is_empty(&self, path: &str) -> bool;
    /// `path` with `..` and symbolic links resolved, as the plan checked it. The default is the path itself (a fake without
    /// links).
    fn resolve(&self, path: &str) -> String {
        path.to_string()
    }
}

impl Fs for Arc<dyn Host> {
    fn create(&self, path: &str) -> Result<(), HostError> {
        self.as_ref().create_file(path)
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), HostError> {
        self.as_ref().rename(from, to)
    }
    fn delete(&self, path: &str) -> Result<(), HostError> {
        self.as_ref().delete(path)
    }
    fn is_dir(&self, path: &str) -> bool {
        self.as_ref().stat(path).is_ok_and(|m| m.kind == proto::Kind::Dir)
    }
    fn dir_is_empty(&self, path: &str) -> bool {
        self.as_ref().read_dir(path).is_ok_and(|entries| entries.is_empty())
    }
    fn resolve(&self, path: &str) -> String {
        super::plan::Env::resolve(&super::env::HostEnv::new(self.clone(), Default::default()), path)
    }
}

/// One thing to do on the host.
#[derive(Clone, Debug)]
pub enum Step {
    Create { path: String },
    Rename { from: String, to: String },
    Delete { path: String },
    Write { path: String, bytes: Vec<u8>, expect: u64, original: Arc<Vec<u8>>, changes: usize },
}

impl Step {
    pub fn path(&self) -> &str {
        match self {
            Step::Create { path } | Step::Delete { path } | Step::Write { path, .. } => path,
            Step::Rename { from, .. } => from,
        }
    }
}

/// What happened to one step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    Done,
    /// Refused or failed and confirmed unchanged.
    Failed(String),
    /// A file the preview read has changed since.
    Conflict,
    /// Worked, then taken back because a later step failed or the user stopped.
    RolledBack,
    /// Worked, but could not be taken back: the path stands as the step left it.
    RollbackFailed(String),
    /// Not started: an earlier step failed or the user stopped.
    NotRun,
}

#[derive(Debug, Default)]
pub struct Transaction {
    pub steps: Vec<(Step, StepOutcome)>,
    /// The text writes that stand, for undo (empty when the transaction was taken back).
    pub undo: Vec<UndoDisk>,
    /// Hidden names of deleted entries that could not be removed after all (the delete itself is done: they are no longer
    /// where the server said).
    pub leftovers: Vec<String>,
}

impl Transaction {
    pub fn ok(&self) -> bool {
        self.steps.iter().all(|(_, o)| *o == StepOutcome::Done)
    }

    pub fn has_resource_steps(&self) -> bool {
        self.steps.iter().any(|(s, _)| !matches!(s, Step::Write { .. }))
    }
}

/// How to take a finished step back.
enum Back {
    /// A created file and the folders that did not exist above it (deepest first), removed only while empty.
    RemoveCreated {
        path: String,
        folders: Vec<String>,
    },
    Rename {
        from: String,
        to: String,
    },
    Write {
        path: String,
        original: Arc<Vec<u8>>,
        hash_after: u64,
    },
}

static TOMBSTONE: AtomicU64 = AtomicU64::new(0);

/// The hidden name a deleted entry waits under until the transaction is complete.
fn tombstone(path: &str) -> String {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let n = TOMBSTONE.fetch_add(1, Ordering::Relaxed);
    let name = name.chars().take(80).collect::<String>();
    format!("{dir}/.codeyard-removed-{}-{n}-{name}", std::process::id())
}

/// Run the steps in order. A failing step, or `cancel`, takes back everything done so far. Before each step the paths it
/// touches are resolved again and must still be inside `roots` (the resolved open folders; none: not checked): a symbolic
/// link moved between the review and now cannot lead a write out of the folder.
pub fn run(fs: &dyn Fs, steps: Vec<Step>, cancel: &AtomicBool, roots: &[String]) -> Transaction {
    let mut journal: Vec<(usize, Back)> = Vec::new();
    let mut tombstones: Vec<String> = Vec::new();
    let mut outcomes: Vec<StepOutcome> = vec![StepOutcome::NotRun; steps.len()];
    let mut failed = false;
    let mut undo = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            failed = true;
            outcomes[i] = StepOutcome::Failed(crate::i18n::t("Stopped.").into());
            break;
        }
        if let Some(real) = escapes(fs, step, roots) {
            outcomes[i] = StepOutcome::Failed(crate::i18n::tf("Outside the open folders: {}", &[&real]));
            failed = true;
            break;
        }
        let result: Result<Back, (StepOutcome, Option<Back>)> = match step {
            Step::Create { path } => {
                let folders = missing_folders(fs, path);
                fs.create(path).map(|_| Back::RemoveCreated { path: path.clone(), folders }).map_err(|e| (StepOutcome::Failed(e.to_string()), None))
            }
            Step::Rename { from, to } => {
                fs.rename(from, to).map(|_| Back::Rename { from: to.clone(), to: from.clone() }).map_err(|e| (StepOutcome::Failed(e.to_string()), None))
            }
            Step::Delete { path } => {
                let tomb = tombstone(path);
                match fs.rename(path, &tomb) {
                    Ok(()) => {
                        tombstones.push(tomb.clone());
                        Ok(Back::Rename { from: tomb, to: path.clone() })
                    }
                    Err(e) => Err((StepOutcome::Failed(e.to_string()), None)),
                }
            }
            Step::Write { path, bytes, expect, original, .. } => write(fs, path, bytes, *expect, original),
        };
        match result {
            Ok(back) => {
                if let Back::Write { path, original, hash_after } = &back {
                    undo.push(UndoDisk { path: path.clone(), original: original.clone(), hash_after: *hash_after });
                }
                journal.push((i, back));
                outcomes[i] = StepOutcome::Done;
            }
            Err((outcome, maybe)) => {
                outcomes[i] = outcome;
                journal.extend(maybe.map(|back| (i, back)));
                failed = true;
                break;
            }
        }
    }
    if failed {
        undo.clear();
        for (i, back) in journal.into_iter().rev() {
            outcomes[i] = match take_back(fs, back) {
                Ok(()) => StepOutcome::RolledBack,
                Err(why) => StepOutcome::RollbackFailed(why),
            };
        }
        return Transaction { steps: steps.into_iter().zip(outcomes).collect(), undo: Vec::new(), leftovers: Vec::new() };
    }
    // Everything worked: now the deletes become real.
    let leftovers = tombstones.into_iter().filter(|t| fs.delete(t).is_err()).collect();
    Transaction { steps: steps.into_iter().zip(outcomes).collect(), undo, leftovers }
}

/// The real location of a path a step touches that is not inside the open folders, if there is one.
fn escapes(fs: &dyn Fs, step: &Step, roots: &[String]) -> Option<String> {
    if roots.is_empty() {
        return None;
    }
    let paths: Vec<&str> = match step {
        Step::Create { path } | Step::Delete { path } | Step::Write { path, .. } => vec![path],
        Step::Rename { from, to } => vec![from, to],
    };
    paths.into_iter().map(|p| fs.resolve(p)).find(|real| !roots.iter().any(|r| super::plan::inside(real, r)))
}

/// The folders above `path` that the create will have to make, deepest first.
fn missing_folders(fs: &dyn Fs, path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = path;
    while let Some((parent, _)) = at.rsplit_once('/') {
        if parent.is_empty() || parent.ends_with(':') || fs.is_dir(parent) {
            break;
        }
        out.push(parent.to_string());
        at = parent;
    }
    out
}

fn write(fs: &dyn Fs, path: &str, bytes: &[u8], expect: u64, original: &Arc<Vec<u8>>) -> Result<Back, (StepOutcome, Option<Back>)> {
    let new_hash = proto::content_hash(bytes);
    let back = |hash_after| Back::Write { path: path.to_string(), original: original.clone(), hash_after };
    match fs.write(path, bytes.to_vec(), Some(expect)) {
        Ok(hash) => Ok(back(hash)),
        Err(e) => match crate::replace_apply::settle(fs, path, expect, new_hash, e) {
            Outcome::Changed(_) => Ok(back(new_hash)),
            Outcome::Conflict => Err((StepOutcome::Conflict, None)),
            Outcome::Failed(why) => Err((StepOutcome::Failed(why), None)),
            // Unknown whether it landed: the step fails, and the write-back is queued in case it did.
            Outcome::Uncertain(why) => Err((StepOutcome::Failed(why), Some(back(new_hash)))),
            Outcome::NotAttempted | Outcome::Skipped(_) => Err((StepOutcome::Failed(String::new()), None)),
        },
    }
}

fn take_back(fs: &dyn Fs, back: Back) -> Result<(), String> {
    match back {
        Back::RemoveCreated { path, folders } => {
            fs.delete(&path).map_err(|e| e.to_string())?;
            for folder in folders {
                // Only while empty: the agent's delete takes a folder with everything in it.
                if fs.dir_is_empty(&folder) {
                    fs.delete(&folder).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
        Back::Rename { from, to } => fs.rename(&from, &to).map_err(|e| e.to_string()),
        Back::Write { path, original, hash_after } => match fs.write(&path, original.to_vec(), Some(hash_after)) {
            Ok(_) => Ok(()),
            // Not what the write left: fine if it is what it was before (the write never landed).
            Err(e) => match fs.read(&path) {
                Ok((_, hash)) if hash == proto::content_hash(&original) => Ok(()),
                _ => Err(e.to_string()),
            },
        },
    }
}

/// Take back the text writes of an applied refactoring (each file only if it is still what the refactoring wrote).
pub fn undo(fs: &dyn Fs, entries: &[UndoDisk], cancel: &AtomicBool) -> Vec<(String, Outcome)> {
    crate::replace_apply::undo_all(fs, entries, cancel)
}

// --- From plan to steps ---

/// An open editor to change: the character ranges and replacements, and the state of the document the preview read.
pub struct BufferJob {
    pub path: String,
    pub edits: Vec<(std::ops::Range<usize>, String)>,
    pub epoch: u64,
    pub version: u64,
    pub hash: u64,
    pub changes: usize,
}

/// What the selection means: edits for open documents, steps for the host, and files that could not be prepared.
pub struct Prepared {
    pub buffers: Vec<BufferJob>,
    pub steps: Vec<Step>,
    pub skipped: Vec<(String, String)>,
}

pub fn prepare(plan: &RefactorPlan, chosen: &Chosen) -> Prepared {
    let mut out = Prepared { buffers: Vec::new(), steps: Vec::new(), skipped: Vec::new() };
    for (i, item) in plan.items.iter().enumerate() {
        let selected = chosen.item(i);
        let n = selected.iter().filter(|x| **x).count();
        if n == 0 {
            continue;
        }
        match item {
            Item::File(f) => {
                let file = &f.file;
                if let Some(p) = &file.problem {
                    out.skipped.push((file.path.clone(), p.message()));
                    continue;
                }
                match file.version {
                    Version::Buffer { epoch, version, hash } => {
                        out.buffers.push(BufferJob { path: file.path.clone(), edits: file.buffer_edits(selected), epoch, version, hash, changes: n });
                    }
                    Version::Disk { hash } => match file.output_bytes(selected) {
                        Ok(bytes) => out.steps.push(Step::Write { path: file.path.clone(), bytes, expect: hash, original: file.original(), changes: n }),
                        Err(p) => out.skipped.push((file.path.clone(), p.message())),
                    },
                }
            }
            Item::Resource(r) if r.noop => {}
            Item::Resource(r) => out.steps.push(match &r.op {
                Resource::Create { path } => Step::Create { path: path.clone() },
                Resource::Rename { from, to } => Step::Rename { from: from.clone(), to: to.clone() },
                Resource::Delete { path, .. } => Step::Delete { path: path.clone() },
            }),
        }
    }
    out
}

// --- Report ---

/// What to tell the user, line by line: (text, is a problem). `name` shortens a path for display.
pub fn report(tx: &Transaction, buffers: &[(String, usize)], name: impl Fn(&str) -> String) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let rolled_back = tx.steps.iter().any(|(_, o)| matches!(o, StepOutcome::RolledBack | StepOutcome::RollbackFailed(_)));
    let failed = tx.steps.iter().any(|(_, o)| matches!(o, StepOutcome::Failed(_) | StepOutcome::Conflict));
    if tx.ok() {
        let changes: usize = buffers.iter().map(|(_, n)| n).sum::<usize>()
            + tx.steps.iter().map(|(s, _)| if let Step::Write { changes, .. } = s { *changes } else { 0 }).sum::<usize>();
        let files = buffers.len() + tx.steps.iter().filter(|(s, _)| matches!(s, Step::Write { .. })).count();
        if changes > 0 {
            out.push((
                crate::i18n::tf(
                    "Changed {0} in {1}",
                    &[&crate::i18n::tn("{} change", "{} changes", changes, &[]), &crate::i18n::tn("{} file", "{} files", files, &[])],
                ),
                false,
            ));
        }
        for (step, _) in &tx.steps {
            let line = match step {
                Step::Create { path } => crate::i18n::tf("Created {}", &[&name(path)]),
                Step::Rename { from, to } => crate::i18n::tf("Renamed {0} to {1}", &[&name(from), &name(to)]),
                Step::Delete { path } => crate::i18n::tf("Deleted {}", &[&name(path)]),
                Step::Write { .. } => continue,
            };
            out.push((line, false));
        }
        for left in &tx.leftovers {
            out.push((crate::i18n::tf("Could not remove {}; delete it by hand.", &[&name(left)]), true));
        }
        return out;
    }
    if rolled_back || failed {
        out.push((crate::i18n::t("The refactoring failed, so everything it had done was taken back. Nothing was changed.").to_string(), true));
    }
    if tx.steps.iter().any(|(_, o)| *o == StepOutcome::Conflict) {
        // What the preview shows is not what the files hold any more: trying again cannot work.
        out.push((crate::i18n::t("The files changed since the preview. Discard this page and start the refactoring again.").to_string(), true));
    }
    for (step, outcome) in &tx.steps {
        let line = match outcome {
            StepOutcome::Failed(why) => crate::i18n::tf("Not changed: {0} ({1})", &[&name(step.path()), why]),
            StepOutcome::Conflict => crate::i18n::tf("File changed since preview: {}", &[&name(step.path())]),
            StepOutcome::RollbackFailed(why) => crate::i18n::tf("Could not take back {0}: {1}. Check it before going on.", &[&name(step.path()), why]),
            StepOutcome::Done | StepOutcome::RolledBack | StepOutcome::NotRun => continue,
        };
        out.push((line, true));
    }
    out
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
