//! A language server's refactoring as a reviewable plan: the files it would change, each change with its old and new
//! line, and the file operations (create, rename, delete) it asks for. Nothing is written while a plan exists.
//!
//! The plan is made from the authoritative text of each file (an open editor's buffer, otherwise the bytes on disk) and
//! reuses `replace_plan::FilePlan`, so encodings, byte-order marks and line endings behave exactly as in the Search
//! Editor's replace preview. What is new here is trust: every path, version and position comes from a server and is
//! checked before it can be selected.
//!
//! - A path outside the open folders (after `..` and symlinks) is refused, shown with its real location.
//! - Edits for a document the editor holds are refused when the document changed since the request, when the version the
//!   server computed against is not the editor's, or when it has unsaved text the server was never told about.
//! - Positions are UTF-16 code units; one that lands inside a character, a line past the end, or edits that overlap refuse
//!   the file. A server that is wrong about the text is never guessed at.
//! - File operations are rows of their own and start unselected; edits to a file such an operation creates or moves depend
//!   on it. An operation that would overwrite is refused (the protocol's `overwrite` is not supported).

use std::collections::HashMap;

use super::edit::{self, Op, Parsed, TextEdit, TextOp};
use crate::encoding::Encoding;
use crate::replace_plan::{Change, FilePlan, Problem, Source, Tri};

/// A refactoring larger than this is refused (the language server would have to be asked for less).
pub struct Limits {
    pub files: usize,
    pub changes: usize,
    pub bytes: usize,
}
pub const LIMITS: Limits = Limits { files: 1_000, changes: 20_000, bytes: 64 * 1024 * 1024 };

/// What a path is on the host (a symbolic link counts as what it is, not what it points to).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Missing,
    File,
    Dir,
    Other,
}

/// A document an editor holds, as the editor sees it.
#[derive(Clone, Debug)]
pub struct OpenDoc {
    /// The text as the editor shows it (`\n` line ends, no byte-order mark).
    pub text: String,
    pub epoch: u64,
    pub version: u64,
    pub encoding: Encoding,
    pub dirty: bool,
    /// The server that made the edit has been told this document's text (the editor is attached to it).
    pub synced: bool,
    /// The version the editor last sent that server.
    pub server_version: Option<i64>,
    /// Text was typed since the request was sent: the server's positions may point elsewhere.
    pub edited_since_request: bool,
}

/// What the plan needs from the outside world. The app implements it with the host and the open editors; tests with a map.
pub trait Env {
    fn open_doc(&self, path: &str) -> Option<&OpenDoc>;
    fn kind(&self, path: &str) -> Kind;
    /// Bytes on the host and their hash.
    fn read(&self, path: &str) -> Result<(Vec<u8>, u64), String>;
    /// `path` with `..` and symbolic links resolved; a path that does not exist yet through its nearest existing parent.
    fn resolve(&self, path: &str) -> String;
    /// Called once with every path the edit and the folders name, so a remote host can answer `resolve` for all of them in
    /// one request.
    fn prepare(&self, _paths: &[String]) {}
    /// What a folder holds, for the safety checks and the question before a delete. Walks at most `DIR_WALK_LIMIT` entries.
    fn dir_info(&self, _path: &str) -> DirInfo {
        DirInfo::default()
    }
    /// The open documents at or below `path`: (path, has unsaved changes).
    fn open_paths_under(&self, _path: &str) -> Vec<(String, bool)> {
        Vec::new()
    }
}

/// Entries looked at when a folder is to be deleted; a bigger folder is refused (what it holds cannot be shown).
pub const DIR_WALK_LIMIT: usize = 20_000;

/// What deleting a folder would take.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirInfo {
    /// Files below it (not folders).
    pub files: usize,
    pub bytes: u64,
    /// Entries directly in it (files and folders), and the first few names.
    pub top_count: usize,
    pub top: Vec<String>,
    /// A `.git` at any depth.
    pub has_git: bool,
    /// The walk stopped at `DIR_WALK_LIMIT`.
    pub truncated: bool,
}

/// A file operation, resolved to what it will do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resource {
    Create { path: String },
    Rename { from: String, to: String },
    Delete { path: String, dir: bool },
}

pub struct ResourceItem {
    pub op: Resource,
    /// Why it cannot be selected.
    pub problem: Option<String>,
    /// Nothing to do (the file the server wants created is there already): shown, applied as a no-op.
    pub noop: bool,
    /// For the review: the real location, when it differs from the path the server gave.
    pub resolved: Option<String>,
    /// For a folder to delete: what is in it.
    pub info: Option<DirInfo>,
}

pub struct FileItem {
    pub file: FilePlan,
    /// The index of the file operation that makes this file exist (it is created or moved there).
    pub needs: Option<usize>,
}

pub enum Item {
    File(FileItem),
    Resource(ResourceItem),
}

impl Item {
    /// Whether anything can be selected in this item.
    pub fn usable(&self) -> bool {
        match self {
            Item::File(f) => f.file.problem.is_none() && !f.file.changes.is_empty(),
            Item::Resource(r) => r.problem.is_none(),
        }
    }
}

pub struct RefactorPlan {
    pub host: String,
    pub root: String,
    /// The open folders, resolved: writes are checked against them again when they happen.
    pub roots: Vec<String>,
    pub items: Vec<Item>,
    /// Parts of the server's edit that were not usable at all.
    pub skipped: Vec<String>,
}

impl RefactorPlan {
    pub fn files(&self) -> impl Iterator<Item = &FileItem> {
        self.items.iter().filter_map(|i| if let Item::File(f) = i { Some(f) } else { None })
    }

    pub fn resources(&self) -> impl Iterator<Item = &ResourceItem> {
        self.items.iter().filter_map(|i| if let Item::Resource(r) = i { Some(r) } else { None })
    }

    pub fn change_count(&self) -> usize {
        self.files().map(|f| f.file.changes.len()).sum()
    }

    /// Files that really change (with at least one change), for deciding whether a review is worthwhile.
    pub fn changed_files(&self) -> Vec<&FilePlan> {
        self.files().map(|f| &f.file).filter(|f| !f.changes.is_empty() || f.problem.is_some()).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    TooLarge { files: usize, changes: usize },
}

impl PlanError {
    pub fn message(&self) -> String {
        match self {
            PlanError::TooLarge { files, changes } => crate::i18n::tf(
                "The language server's refactoring is too large to review ({0} files, {1} changes; the limit is {2} files and {3} changes). Nothing was changed.",
                &[files, changes, &LIMITS.files, &LIMITS.changes],
            ),
        }
    }
}

// --- Selection ---

/// Which parts of a plan are selected: a file's changes one by one, a file operation as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen(pub Vec<Vec<bool>>);

/// Why a selection cannot be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    /// A selected file needs an operation that is not selected.
    NeedsOperation(String),
    Nothing,
}

impl Chosen {
    /// Every change of every usable file that does not depend on an operation. File operations start unselected: they
    /// move or remove files, so they are chosen deliberately.
    pub fn initial(plan: &RefactorPlan) -> Self {
        Chosen(
            plan.items
                .iter()
                .map(|item| match item {
                    Item::File(f) => vec![item.usable() && f.needs.is_none(); f.file.changes.len()],
                    Item::Resource(_) => vec![false],
                })
                .collect(),
        )
    }

    pub fn item(&self, i: usize) -> &[bool] {
        self.0.get(i).map_or(&[], Vec::as_slice)
    }

    pub fn state(&self, i: usize) -> Tri {
        let s = self.item(i);
        match s.iter().filter(|x| **x).count() {
            0 => Tri::None,
            n if n == s.len() => Tri::All,
            _ => Tri::Some,
        }
    }

    pub fn toggle_part(&mut self, plan: &RefactorPlan, i: usize, part: usize) {
        if plan.items.get(i).is_some_and(Item::usable)
            && let Some(x) = self.0.get_mut(i).and_then(|v| v.get_mut(part))
        {
            *x = !*x;
        }
    }

    /// All selected → none, otherwise all. Choosing a file operation chooses the edits that depend on it, and dropping it
    /// drops them.
    pub fn toggle_item(&mut self, plan: &RefactorPlan, i: usize) {
        if !plan.items.get(i).is_some_and(Item::usable) {
            return;
        }
        let on = self.state(i) != Tri::All;
        if let Some(v) = self.0.get_mut(i) {
            v.iter_mut().for_each(|x| *x = on);
        }
        if matches!(plan.items[i], Item::Resource(_)) {
            for (j, item) in plan.items.iter().enumerate() {
                if matches!(item, Item::File(f) if f.needs == Some(i)) && item.usable() {
                    self.0[j].iter_mut().for_each(|x| *x = on);
                }
            }
        }
    }

    /// (selected changes and operations, items with at least one).
    pub fn counts(&self) -> (usize, usize) {
        let parts = self.0.iter().map(|v| v.iter().filter(|x| **x).count()).sum();
        let items = self.0.iter().filter(|v| v.iter().any(|x| *x)).count();
        (parts, items)
    }

    /// Selected file operations of one kind, for the summary.
    pub fn selected_resources<'a>(&self, plan: &'a RefactorPlan) -> Vec<&'a ResourceItem> {
        plan.items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                Item::Resource(r) if self.item(i).first() == Some(&true) => Some(r),
                _ => None,
            })
            .collect()
    }

    pub fn blocker(&self, plan: &RefactorPlan) -> Option<Blocker> {
        if self.counts().0 == 0 {
            return Some(Blocker::Nothing);
        }
        for (i, item) in plan.items.iter().enumerate() {
            if let Item::File(f) = item
                && let Some(op) = f.needs
                && self.item(i).iter().any(|x| *x)
                && self.item(op).first() != Some(&true)
            {
                return Some(Blocker::NeedsOperation(f.file.path.clone()));
            }
        }
        None
    }
}

// --- Building ---

/// The components of a path as the host sees them: `\` and `/` alike, `.` and `..` resolved.
fn parts(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in path.replace('\\', "/").split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(p.to_string()),
        }
    }
    out
}

/// Names compare as the file system does: case is folded on Windows.
fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) { a.to_lowercase() == b.to_lowercase() } else { a == b }
}

fn starts_with(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && path.iter().zip(prefix).all(|(a, b)| same_name(a, b))
}

/// The path written the same way every time (`/` separators, no `.` or `..`).
pub fn clean(path: &str) -> String {
    let joined = parts(path).join("/");
    if path.replace('\\', "/").starts_with('/') { format!("/{joined}") } else { joined }
}

/// Is `path` (already resolved) the folder `root` or inside it?
pub fn inside(path: &str, root: &str) -> bool {
    starts_with(&parts(path), &parts(root))
}

fn same_path(a: &str, b: &str) -> bool {
    let (a, b) = (parts(a), parts(b));
    a.len() == b.len() && starts_with(&a, &b)
}

/// Where a path stands after the operations before it in the same edit.
enum Virtual {
    /// A file the edit creates; its text starts empty.
    Created(usize),
    /// A file or folder the edit moves here; its text is the old one's. `origin` is where the bytes are on the host.
    Moved {
        origin: String,
        op: usize,
    },
    Gone,
}

struct Overlay {
    entries: Vec<(Vec<String>, Virtual)>,
    /// Every path an operation has set, in order: what edits to a path were meant for depends on how many came before.
    log: Vec<Vec<String>>,
}

impl Overlay {
    /// The entry for `path` or the moved/removed folder above it, with the part of the path below that folder.
    fn lookup(&self, path: &str) -> Option<(&Virtual, String)> {
        let key = parts(path);
        self.entries.iter().rev().find_map(|(at, v)| {
            if !starts_with(&key, at) {
                return None;
            }
            // A created file has nothing below it.
            if key.len() > at.len() && matches!(v, Virtual::Created(_)) {
                return None;
            }
            Some((v, key[at.len()..].join("/")))
        })
    }

    /// How many operations so far created, moved or removed `path` or a folder above it: edits to it before and after one
    /// are for different files.
    fn generation(&self, path: &str) -> usize {
        let key = parts(path);
        self.log.iter().filter(|set| starts_with(&key, set)).count()
    }

    fn set(&mut self, path: &str, v: Virtual) {
        let key = parts(path);
        self.log.push(key.clone());
        self.entries.retain(|(at, _)| !(at.len() == key.len() && starts_with(at, &key)));
        self.entries.push((key, v));
    }
}

struct Builder<'a> {
    env: &'a dyn Env,
    roots: Vec<String>,
    overlay: Overlay,
}

impl Builder<'_> {
    fn kind(&self, path: &str) -> Kind {
        match self.overlay.lookup(path) {
            Some((Virtual::Created(_), _)) => Kind::File,
            Some((Virtual::Gone, _)) => Kind::Missing,
            Some((Virtual::Moved { origin, .. }, rest)) => {
                let from = if rest.is_empty() { origin.clone() } else { format!("{origin}/{rest}") };
                self.kind(&from)
            }
            None => self.env.kind(path),
        }
    }

    /// `Err` with the reason when the path is not inside an open folder after resolving links.
    fn allowed(&self, path: &str) -> Result<String, String> {
        let real = self.env.resolve(path);
        if self.roots.iter().any(|r| inside(&real, r)) {
            return Ok(real);
        }
        Err(crate::i18n::tf("Outside the open folders: {}", &[&real]))
    }
}

/// Make the plan for a server's edit. `roots` are the open folders as given; they are resolved here like every path.
pub fn build(parsed: Parsed, env: &dyn Env, roots: &[String], host: String, root: String, fallback: Encoding) -> Result<RefactorPlan, PlanError> {
    let paths: std::collections::HashSet<String> = parsed
        .ops
        .iter()
        .map(|op| match op {
            Op::Text(t) => clean(&t.path),
            Op::Create { path, .. } | Op::Delete { path, .. } => clean(path),
            Op::Rename { from, .. } => clean(from),
        })
        .collect();
    let changes = parsed.edit_count();
    let bytes: usize = parsed.ops.iter().map(|op| if let Op::Text(t) = op { t.edits.iter().map(|e| e.new_text.len()).sum() } else { 0 }).sum();
    if paths.len() > LIMITS.files || changes > LIMITS.changes || bytes > LIMITS.bytes {
        return Err(PlanError::TooLarge { files: paths.len(), changes });
    }
    let named: Vec<String> = parsed
        .ops
        .iter()
        .flat_map(|op| match op {
            Op::Text(t) => vec![t.path.clone()],
            Op::Create { path, .. } | Op::Delete { path, .. } => vec![path.clone()],
            Op::Rename { from, to, .. } => vec![from.clone(), to.clone()],
        })
        .chain(roots.iter().cloned())
        .collect();
    env.prepare(&named);
    let mut b = Builder { env, roots: roots.iter().map(|r| env.resolve(r)).collect(), overlay: Overlay { entries: Vec::new(), log: Vec::new() } };
    // First pass: operations in the server's order, with text edits of the same file gathered into one entry.
    let mut slots: Vec<Slot> = Vec::new();
    let mut at: HashMap<(String, usize), usize> = HashMap::new();
    for op in parsed.ops {
        match op {
            Op::Text(t) => {
                let key = clean(&t.path);
                let key = (if cfg!(windows) { key.to_lowercase() } else { key }, b.overlay.generation(&t.path));
                let needs = match b.overlay.lookup(&t.path) {
                    Some((Virtual::Created(i), _)) => Some(*i),
                    Some((Virtual::Moved { op, .. }, _)) => Some(*op),
                    _ => None,
                };
                match at.get(&key) {
                    Some(&i) => {
                        if let Slot::Text { op: have, .. } = &mut slots[i] {
                            merge(have, t);
                        }
                    }
                    None => {
                        at.insert(key, slots.len());
                        slots.push(Slot::Text { op: t, needs });
                    }
                }
            }
            resource => {
                let item = resource_item(&mut b, resource, slots.len());
                slots.push(Slot::Resource(item));
            }
        }
    }
    // Second pass: read each file and turn its edits into changes.
    let mut items = Vec::with_capacity(slots.len());
    for slot in slots {
        items.push(match slot {
            Slot::Resource(r) => Item::Resource(r),
            Slot::Text { op, needs } => Item::File(FileItem { file: file_plan(&b, op, needs, fallback), needs }),
        });
    }
    Ok(RefactorPlan { host, root, roots: b.roots.clone(), items, skipped: parsed.skipped })
}

enum Slot {
    Text { op: TextOp, needs: Option<usize> },
    Resource(ResourceItem),
}

/// Two entries for one document: the edits are one list (their versions must agree).
fn merge(have: &mut TextOp, more: TextOp) {
    if have.version != more.version && more.version.is_some() {
        match have.version {
            None => have.version = more.version,
            Some(_) => have.malformed = Some(crate::i18n::t("The server gave two different document versions for this file.").into()),
        }
    }
    if have.malformed.is_none() {
        have.malformed = more.malformed;
    }
    have.edits.extend(more.edits);
}

fn resource_item(b: &mut Builder<'_>, op: Op, index: usize) -> ResourceItem {
    fn refuse(op: Resource, why: impl Into<String>) -> ResourceItem {
        ResourceItem { op, problem: Some(why.into()), noop: false, resolved: None, info: None }
    }
    match op {
        Op::Create { path, overwrite: _, ignore_if_exists } => {
            let item = Resource::Create { path: path.clone() };
            let real = match b.allowed(&path) {
                Ok(real) => real,
                Err(why) => return refuse(item, why),
            };
            let resolved = (!same_path(&real, &path)).then_some(real);
            if b.kind(&path) != Kind::Missing {
                if ignore_if_exists {
                    return ResourceItem { op: item, problem: None, noop: true, resolved, info: None };
                }
                return refuse(item, crate::i18n::t("Already exists; Codeyard never overwrites a file in a refactoring."));
            }
            b.overlay.set(&path, Virtual::Created(index));
            ResourceItem { op: item, problem: None, noop: false, resolved, info: None }
        }
        Op::Rename { from, to, overwrite: _, ignore_if_exists } => {
            let item = Resource::Rename { from: from.clone(), to: to.clone() };
            for p in [&from, &to] {
                if let Err(why) = b.allowed(p) {
                    return refuse(item, why);
                }
            }
            if b.kind(&from) == Kind::Missing {
                return refuse(item, crate::i18n::t("The file to rename is not there."));
            }
            if same_path(&from, &to) {
                return ResourceItem { op: item, problem: None, noop: true, resolved: None, info: None };
            }
            if inside(&to, &from) {
                return refuse(item, crate::i18n::t("A folder cannot be moved into itself."));
            }
            if b.kind(&to) != Kind::Missing {
                if ignore_if_exists {
                    return ResourceItem { op: item, problem: None, noop: true, resolved: None, info: None };
                }
                return refuse(item, crate::i18n::t("The new name is taken; Codeyard never overwrites a file in a refactoring."));
            }
            if b.env.open_doc(&from).is_some_and(|d| d.dirty) {
                return refuse(item, crate::i18n::t("Has unsaved changes: save or revert it first."));
            }
            let origin = match b.overlay.lookup(&from) {
                Some((Virtual::Moved { origin, .. }, rest)) if rest.is_empty() => origin.clone(),
                _ => from.clone(),
            };
            b.overlay.set(&from, Virtual::Gone);
            b.overlay.set(&to, Virtual::Moved { origin, op: index });
            ResourceItem { op: item, problem: None, noop: false, resolved: None, info: None }
        }
        Op::Delete { path, recursive, ignore_if_not_exists } => {
            let kind = b.kind(&path);
            let item = Resource::Delete { path: path.clone(), dir: kind == Kind::Dir };
            let real = match b.allowed(&path) {
                Ok(real) => real,
                Err(why) => return refuse(item, why),
            };
            if kind == Kind::Missing {
                if ignore_if_not_exists {
                    return ResourceItem { op: item, problem: None, noop: true, resolved: None, info: None };
                }
                return refuse(item, crate::i18n::t("The file to delete is not there."));
            }
            // A folder of the workspace, or one that holds it, is never deleted by a language server.
            if b.roots.iter().any(|r| inside(r, &real)) {
                return refuse(item, crate::i18n::t("An open folder is never deleted by a refactoring."));
            }
            if parts(&path).last().is_some_and(|n| same_name(n, ".git")) {
                return refuse(item, crate::i18n::t("A .git folder is never deleted by a refactoring."));
            }
            let mut info = None;
            if kind == Kind::Dir && b.overlay.lookup(&path).is_none() {
                let found = b.env.dir_info(&path);
                if found.has_git {
                    return refuse(item, crate::i18n::t("It holds a Git repository (.git), which a refactoring never deletes."));
                }
                if found.truncated {
                    return refuse(item, crate::i18n::t("It holds too many files to check what would be deleted."));
                }
                if !recursive && found.top_count > 0 {
                    return refuse(item, crate::i18n::t("The folder is not empty and the server did not ask to delete what is in it."));
                }
                if let Some((inner, _)) = b.env.open_paths_under(&path).into_iter().find(|(_, dirty)| *dirty) {
                    return refuse(item, crate::i18n::tf("{} has unsaved changes: save or revert it first.", &[&inner]));
                }
                info = Some(found);
            }
            if b.env.open_doc(&path).is_some_and(|d| d.dirty) {
                return refuse(item, crate::i18n::t("Has unsaved changes: save or revert it first."));
            }
            let resolved = (!same_path(&real, &path)).then_some(real);
            b.overlay.set(&path, Virtual::Gone);
            ResourceItem { op: item, problem: None, noop: false, resolved, info }
        }
        Op::Text(_) => unreachable!("text edits are handled by the caller"),
    }
}

/// The plan for one file: its source text, then the server's edits turned into changes.
fn file_plan(b: &Builder<'_>, op: TextOp, needs: Option<usize>, fallback: Encoding) -> FilePlan {
    let path = op.path.clone();
    let refused = |why: String| {
        let mut f = FilePlan::unreadable(path.clone(), String::new());
        f.problem = Some(Problem::Refused(why));
        f
    };
    if let Some(why) = &op.malformed {
        return refused(why.clone());
    }
    let open = b.env.open_doc(&path);
    // Disk writes stay inside the open folders; an open document is only edited in the editor and saved by the user.
    if open.is_none()
        && let Err(why) = b.allowed(&path)
    {
        return refused(why);
    }
    let source = match (needs.and(b.overlay.lookup(&path)), open) {
        (Some((Virtual::Created(_), _)), _) => Source::Disk { bytes: Vec::new(), hash: proto::content_hash(&[]) },
        (Some((Virtual::Moved { origin, .. }, rest)), _) => {
            let from = if rest.is_empty() { origin.clone() } else { format!("{origin}/{rest}") };
            match b.env.read(&from) {
                Ok((bytes, hash)) => Source::Disk { bytes, hash },
                Err(e) => return FilePlan::unreadable(path, e),
            }
        }
        (_, Some(doc)) => {
            if let Some(why) = stale(doc, op.version) {
                return refused(why);
            }
            Source::Buffer { text: doc.text.clone(), epoch: doc.epoch, version: doc.version, encoding: doc.encoding }
        }
        _ => match b.env.read(&path) {
            Ok((bytes, hash)) => Source::Disk { bytes, hash },
            Err(e) => return FilePlan::unreadable(path, e),
        },
    };
    let edits = op.edits;
    FilePlan::from_edits(path, source, fallback, move |text, eol| changes_of(text, eol, edits))
}

/// Why an open document cannot take the server's edits, if it cannot.
fn stale(doc: &OpenDoc, version: Option<i64>) -> Option<String> {
    if doc.edited_since_request {
        return Some(crate::i18n::t("The file was edited while the language server was working, so its answer no longer fits. Try again.").into());
    }
    if let (Some(sent), Some(have)) = (version, doc.server_version)
        && sent != have
    {
        return Some(crate::i18n::tf("The server's edit is for version {0} of this file, the editor has version {1}.", &[&sent, &have]));
    }
    if doc.dirty && !doc.synced {
        return Some(crate::i18n::t("Has unsaved changes the language server has not seen: save or revert it first.").into());
    }
    None
}

// --- Positions ---

/// (start, end of the content without the line break) of every line. A line ends at `\n`, `\r\n` or `\r`.
fn lines(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push((start, i));
                start = i + 1;
            }
            b'\r' => {
                out.push((start, i));
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push((start, bytes.len()));
    out
}

/// Byte offset of a (line, UTF-16 column). A column past the end of the line is the end of the line, and the line after the
/// last is the end of the text (both are what the protocol says); anything else that does not fit is an error.
fn offset(text: &str, lines: &[(usize, usize)], (line, col): edit::Position) -> Result<usize, String> {
    let line = line as usize;
    if line > lines.len() {
        return Err(crate::i18n::tf("Line {0} does not exist (the file has {1} lines).", &[&(line + 1), &lines.len()]));
    }
    let Some(&(start, end)) = lines.get(line) else { return Ok(text.len()) };
    let mut units = 0u32;
    for (i, ch) in text[start..end].char_indices() {
        if units == col {
            return Ok(start + i);
        }
        units += ch.len_utf16() as u32;
        if units > col {
            return Err(crate::i18n::tf("Line {0}, column {1} is inside a character.", &[&(line + 1), &(col + 1)]));
        }
    }
    Ok(end)
}

/// The changes for a file's edits: byte ranges in `text`, sorted, without overlaps.
fn changes_of(text: &str, eol: &str, edits: Vec<TextEdit>) -> Result<Vec<Change>, String> {
    let lines = lines(text);
    let mut out = Vec::with_capacity(edits.len());
    for e in &edits {
        let (start, end) = (offset(text, &lines, e.start)?, offset(text, &lines, e.end)?);
        if end < start {
            return Err(crate::i18n::t("An edit ends before it starts.").into());
        }
        out.push(Change { line: e.start.0 + 1, start, end, old: text[start..end].to_string(), new: crate::replace_plan::unify_breaks(&e.new_text, eol) });
    }
    // Inserts at one place keep the server's order and come before a replacement that starts there.
    out.sort_by_key(|c| (c.start, c.end));
    if out.windows(2).any(|w| w[1].start < w[0].end) {
        return Err(crate::i18n::t("Edits overlap, so the server's answer cannot be trusted. Nothing in this file is changed.").into());
    }
    Ok(out)
}

/// Decide whether an edit gets a review. It always does when it moves, creates or removes files or when part of it is refused
/// (nothing is dropped silently, whatever the setting), and when asked (`always`). Under `auto` it also does when the edit
/// reaches more than one file or a file that is not open in an editor (there is no editor undo for it). A change inside one
/// open file is applied at once, as one undo step in that editor.
pub fn needs_review(plan: &RefactorPlan, mode: ReviewMode) -> bool {
    let files = plan.changed_files();
    if mode == ReviewMode::Always || plan.resources().next().is_some() || !plan.skipped.is_empty() || files.iter().any(|f| f.problem.is_some()) {
        return true;
    }
    mode == ReviewMode::Auto && (files.len() > 1 || files.iter().any(|f| !matches!(f.version, crate::replace_plan::Version::Buffer { .. })))
}

/// `editor.refactorPreview`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewMode {
    #[default]
    Auto,
    Always,
    Never,
}

impl ReviewMode {
    pub const CHOICES: [&'static str; 3] = ["auto", "always", "never"];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
