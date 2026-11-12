//! Applying a refactoring on real folders through the local agent: the whole path from a server's edit to the bytes on disk,
//! with failures injected at every step. Every assertion compares exact bytes or whole folder snapshots.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use serde_json::{Value, json};

use super::*;
use crate::lsp::refactor::edit::parse;
use crate::lsp::refactor::env::HostEnv;
use crate::lsp::refactor::plan::{self, Chosen};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-refactor-apply-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    crate::ui::canonical_for_test(d)
}

fn host() -> Arc<dyn Host> {
    Arc::new(host::local())
}

fn s(p: &Path) -> String {
    crate::ui::local_path(p)
}

/// Every file and folder below `root` with its bytes (`None` for a folder), so a rollback can be compared whole.
fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(base: &Path, at: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        let mut entries: Vec<_> = std::fs::read_dir(at).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for p in entries {
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                out.insert(rel, None);
                walk(base, &p, out);
            } else {
                out.insert(rel, Some(std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Wraps the host: an operation `fail` answers with an error does not happen.
struct Faulty {
    inner: Arc<dyn Host>,
    fail: Box<dyn Fn(&str, &str) -> Option<HostError> + Send + Sync>,
    /// Called after every operation with its number (for stopping in the middle).
    after: Box<dyn Fn(usize) + Send + Sync>,
    ops: AtomicUsize,
    log: Mutex<Vec<String>>,
}

impl Faulty {
    fn new(fail: impl Fn(&str, &str) -> Option<HostError> + Send + Sync + 'static) -> Self {
        Faulty { inner: host(), fail: Box::new(fail), after: Box::new(|_| {}), ops: AtomicUsize::new(0), log: Mutex::new(Vec::new()) }
    }

    fn step(&self, op: &str, path: &str) -> Result<(), HostError> {
        self.log.lock().unwrap().push(format!("{op} {path}"));
        let n = self.ops.fetch_add(1, Ordering::SeqCst);
        let out = match (self.fail)(op, path) {
            Some(e) => Err(e),
            None => Ok(()),
        };
        (self.after)(n);
        out
    }
}

impl Store for Faulty {
    fn read(&self, path: &str) -> Result<(Vec<u8>, u64), HostError> {
        self.inner.read(path)
    }
    fn write(&self, path: &str, data: Vec<u8>, expect: Option<u64>) -> Result<u64, HostError> {
        self.step("write", path)?;
        self.inner.write(path, data, expect)
    }
}

impl Fs for Faulty {
    fn create(&self, path: &str) -> Result<(), HostError> {
        self.step("create", path)?;
        self.inner.create_file(path)
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), HostError> {
        self.step("rename", &format!("{from} -> {to}"))?;
        self.inner.rename(from, to)
    }
    fn delete(&self, path: &str) -> Result<(), HostError> {
        self.step("delete", path)?;
        self.inner.delete(path)
    }
    fn is_dir(&self, path: &str) -> bool {
        self.inner.is_dir(path)
    }
    fn dir_is_empty(&self, path: &str) -> bool {
        self.inner.dir_is_empty(path)
    }
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

fn write_step(path: &Path, before: &str, after: &str) -> Step {
    Step::Write {
        path: s(path),
        bytes: after.as_bytes().to_vec(),
        expect: proto::content_hash(before.as_bytes()),
        original: Arc::new(before.as_bytes().to_vec()),
        changes: 1,
    }
}

/// A folder with two files and a folder, and a script of steps that touch all four kinds of step.
fn fixture(name: &str) -> (PathBuf, Vec<Step>) {
    let d = dir(name);
    std::fs::write(d.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(d.join("şablon 😀.txt"), "beta\n").unwrap();
    std::fs::create_dir(d.join("eski")).unwrap();
    std::fs::write(d.join("eski").join("x.txt"), "x\n").unwrap();
    let steps = vec![
        write_step(&d.join("a.txt"), "alpha\n", "ALPHA\n"),
        Step::Create { path: s(&d.join("yeni").join("n.txt")) },
        Step::Rename { from: s(&d.join("şablon 😀.txt")), to: s(&d.join("taşındı.txt")) },
        Step::Delete { path: s(&d.join("eski")) },
    ];
    (d, steps)
}

#[test]
fn every_kind_of_step_lands_and_the_deleted_folder_is_really_gone() {
    let (d, steps) = fixture("ok");
    let tx = run(&host(), steps, &no_cancel());
    assert!(tx.ok(), "{:?}", tx.steps);
    assert_eq!(
        snapshot(&d),
        BTreeMap::from([
            ("a.txt".to_string(), Some(b"ALPHA\n".to_vec())),
            ("taşındı.txt".to_string(), Some(b"beta\n".to_vec())),
            ("yeni".to_string(), None),
            ("yeni/n.txt".to_string(), Some(Vec::new())),
        ])
    );
    assert!(tx.leftovers.is_empty());
    assert_eq!(tx.undo.len(), 1, "only the text write can be undone");
    assert!(tx.has_resource_steps());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_failure_at_any_step_leaves_the_folder_exactly_as_it_was() {
    for failing in 0..4 {
        let (d, steps) = fixture(&format!("fail{failing}"));
        let before = snapshot(&d);
        let counter = AtomicUsize::new(0);
        // The nth forward operation fails (rollback operations come after and are not failed).
        let fs = Faulty::new(move |op, path| {
            let _ = op;
            (counter.fetch_add(1, Ordering::SeqCst) == failing).then(|| HostError::Op(proto::Error::PermissionDenied(path.into())))
        });
        let tx = run(&fs, steps, &no_cancel());
        assert!(!tx.ok());
        assert_eq!(snapshot(&d), before, "step {failing} failed: {:?}", tx.steps);
        assert!(tx.undo.is_empty());
        assert!(tx.steps.iter().take(failing).all(|(_, o)| *o == StepOutcome::RolledBack), "{:?}", tx.steps);
        assert!(matches!(tx.steps[failing].1, StepOutcome::Failed(_)));
        assert!(tx.steps.iter().skip(failing + 1).all(|(_, o)| *o == StepOutcome::NotRun));
        std::fs::remove_dir_all(&d).ok();
    }
}

#[test]
fn a_file_changed_after_the_preview_is_a_conflict_that_takes_everything_back() {
    let (d, mut steps) = fixture("conflict");
    // The preview read "OLD"; somebody saved other text since.
    steps.insert(2, write_step(&d.join("a.txt"), "OLD\n", "NEW\n"));
    let before = snapshot(&d);
    let tx = run(&host(), steps, &no_cancel());
    assert_eq!(snapshot(&d), before);
    assert!(tx.steps.iter().any(|(_, o)| *o == StepOutcome::Conflict), "{:?}", tx.steps);
    let lines = report(&tx, &[], |p| p.rsplit('/').next().unwrap().to_string());
    assert!(lines[0].0.contains("taken back"), "{lines:?}");
    assert!(lines.iter().any(|(l, bad)| *bad && l.contains("File changed since preview: a.txt")), "{lines:?}");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_rename_never_overwrites_a_name_that_is_taken_and_the_earlier_steps_are_taken_back() {
    let (d, mut steps) = fixture("taken");
    std::fs::write(d.join("hedef.txt"), "mine\n").unwrap();
    steps.push(Step::Rename { from: s(&d.join("a.txt")), to: s(&d.join("hedef.txt")) });
    let before = snapshot(&d);
    let tx = run(&host(), steps, &no_cancel());
    assert_eq!(snapshot(&d), before);
    assert_eq!(std::fs::read_to_string(d.join("hedef.txt")).unwrap(), "mine\n");
    assert!(matches!(&tx.steps[4].1, StepOutcome::Failed(why) if why.contains("already exists")), "{:?}", tx.steps[4]);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_create_that_finds_the_file_there_fails_instead_of_truncating_it() {
    let d = dir("create-exists");
    std::fs::write(d.join("a.txt"), "keep\n").unwrap();
    let tx = run(&host(), vec![Step::Create { path: s(&d.join("a.txt")) }], &no_cancel());
    assert!(!tx.ok());
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "keep\n");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_step_that_cannot_be_taken_back_is_named_with_where_it_stands() {
    let (d, mut steps) = fixture("stuck");
    steps.push(Step::Create { path: s(&d.join("a.txt")) });
    // Renaming back to the old name is refused, as if the name were taken by then.
    let stuck = s(&d.join("şablon 😀.txt"));
    let fs = Faulty::new(move |op, path| {
        (op == "rename" && path.ends_with(&format!("-> {stuck}"))).then(|| HostError::Op(proto::Error::AlreadyExists(stuck.clone())))
    });
    let tx = run(&fs, steps, &no_cancel());
    assert!(tx.steps.iter().any(|(_, o)| matches!(o, StepOutcome::RollbackFailed(_))));
    // Everything else was taken back: the file stands where the rename left it.
    assert!(d.join("taşındı.txt").exists());
    assert!(d.join("eski").join("x.txt").exists());
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "alpha\n");
    let lines = report(&tx, &[], |p| p.rsplit('/').next().unwrap().to_string());
    assert!(lines.iter().any(|(l, _)| l.contains("Could not take back") && l.contains("şablon 😀.txt")), "{lines:?}");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn stopping_in_the_middle_takes_back_what_was_done() {
    let (d, steps) = fixture("stop");
    let before = snapshot(&d);
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let mut fs = Faulty::new(|_, _| None);
    fs.after = Box::new(move |n| {
        if n == 1 {
            flag.store(true, Ordering::SeqCst);
        }
    });
    let tx = run(&fs, steps, &cancel);
    assert_eq!(snapshot(&d), before);
    assert!(matches!(&tx.steps[2].1, StepOutcome::Failed(why) if why.contains("Stopped")), "{:?}", tx.steps);
    // Stopped before anything: nothing was started.
    let (d2, steps) = fixture("stop-first");
    let tx = run(&host(), steps, &AtomicBool::new(true));
    assert!(tx.steps.iter().skip(1).all(|(_, o)| *o == StepOutcome::NotRun));
    assert_eq!(snapshot(&d2).len(), 4);
    std::fs::remove_dir_all(&d).ok();
    std::fs::remove_dir_all(&d2).ok();
}

#[test]
fn a_write_whose_reply_was_lost_but_landed_counts_as_done() {
    let d = dir("lost");
    std::fs::write(d.join("a.txt"), "alpha\n").unwrap();
    struct LostReply(Arc<dyn Host>);
    impl Store for LostReply {
        fn read(&self, path: &str) -> Result<(Vec<u8>, u64), HostError> {
            self.0.read(path)
        }
        fn write(&self, path: &str, data: Vec<u8>, expect: Option<u64>) -> Result<u64, HostError> {
            self.0.write(path, data, expect)?;
            Err(HostError::Uncertain("reply lost".into()))
        }
    }
    impl Fs for LostReply {
        fn create(&self, path: &str) -> Result<(), HostError> {
            self.0.create_file(path)
        }
        fn rename(&self, from: &str, to: &str) -> Result<(), HostError> {
            self.0.rename(from, to)
        }
        fn delete(&self, path: &str) -> Result<(), HostError> {
            self.0.delete(path)
        }
        fn is_dir(&self, path: &str) -> bool {
            self.0.is_dir(path)
        }
        fn dir_is_empty(&self, path: &str) -> bool {
            self.0.dir_is_empty(path)
        }
    }
    let tx = run(&LostReply(host()), vec![write_step(&d.join("a.txt"), "alpha\n", "ALPHA\n")], &no_cancel());
    assert!(tx.ok(), "{:?}", tx.steps);
    assert_eq!(tx.undo[0].hash_after, proto::content_hash(b"ALPHA\n"));
    // And a later failure still takes that write back.
    let steps = vec![write_step(&d.join("a.txt"), "ALPHA\n", "Alpha!\n"), Step::Rename { from: s(&d.join("nope")), to: s(&d.join("nope2")) }];
    let tx = run(&host(), steps, &no_cancel());
    assert!(!tx.ok());
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "ALPHA\n");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_write_that_never_landed_is_not_blamed_on_the_rollback() {
    let d = dir("unsure");
    std::fs::write(d.join("a.txt"), "alpha\n").unwrap();
    struct Dropped(Arc<dyn Host>);
    impl Store for Dropped {
        fn read(&self, path: &str) -> Result<(Vec<u8>, u64), HostError> {
            self.0.read(path)
        }
        fn write(&self, _: &str, _: Vec<u8>, _: Option<u64>) -> Result<u64, HostError> {
            Err(HostError::Disconnected("gone".into()))
        }
    }
    impl Fs for Dropped {
        fn create(&self, path: &str) -> Result<(), HostError> {
            self.0.create_file(path)
        }
        fn rename(&self, from: &str, to: &str) -> Result<(), HostError> {
            self.0.rename(from, to)
        }
        fn delete(&self, path: &str) -> Result<(), HostError> {
            self.0.delete(path)
        }
        fn is_dir(&self, path: &str) -> bool {
            self.0.is_dir(path)
        }
        fn dir_is_empty(&self, path: &str) -> bool {
            self.0.dir_is_empty(path)
        }
    }
    let steps = vec![Step::Create { path: s(&d.join("n.txt")) }, write_step(&d.join("a.txt"), "alpha\n", "ALPHA\n")];
    let tx = run(&Dropped(host()), steps, &no_cancel());
    assert!(!tx.ok());
    assert!(!d.join("n.txt").exists());
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "alpha\n");
    assert!(tx.steps.iter().all(|(_, o)| !matches!(o, StepOutcome::RollbackFailed(_))), "{:?}", tx.steps);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_leftover_tombstone_is_reported_not_hidden() {
    let d = dir("left");
    std::fs::write(d.join("a.txt"), "a\n").unwrap();
    // Removing the hidden name fails: the delete is done (the file is no longer at its path) but the name is reported.
    let fs = Faulty::new(|op, path| (op == "delete" && path.contains(".codeyard-removed")).then(|| HostError::Op(proto::Error::PermissionDenied(path.into()))));
    let tx = run(&fs, vec![Step::Delete { path: s(&d.join("a.txt")) }], &no_cancel());
    assert!(tx.ok());
    assert_eq!(tx.leftovers.len(), 1);
    assert!(!d.join("a.txt").exists());
    let lines = report(&tx, &[], |p| p.rsplit('/').next().unwrap().to_string());
    assert!(lines.iter().any(|(l, bad)| *bad && l.starts_with("Could not remove .codeyard-removed")), "{lines:?}");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn undo_puts_the_text_back_only_for_files_nobody_changed_since() {
    let d = dir("undo");
    std::fs::write(d.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(d.join("b.txt"), "beta\n").unwrap();
    let tx = run(&host(), vec![write_step(&d.join("a.txt"), "alpha\n", "ALPHA\n"), write_step(&d.join("b.txt"), "beta\n", "BETA\n")], &no_cancel());
    std::fs::write(d.join("b.txt"), "BETA edited\n").unwrap();
    let outcomes = undo(&host(), &tx.undo, &no_cancel());
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "alpha\n");
    assert_eq!(std::fs::read_to_string(d.join("b.txt")).unwrap(), "BETA edited\n");
    assert!(outcomes[0].1.changed());
    assert_eq!(outcomes[1].1, Outcome::Conflict);
    std::fs::remove_dir_all(&d).ok();
}

// --- From a server's edit to the bytes on disk ---

fn edit_in(d: &Path) -> Value {
    let uri = |name: &str| format!("file://{}", s(&d.join(name)));
    let at = |l: u32, c0: u32, c1: u32, new: &str| json!({ "range": { "start": { "line": l, "character": c0 }, "end": { "line": l, "character": c1 } }, "newText": new });
    json!({ "documentChanges": [
        { "textDocument": { "uri": uri("a.rs"), "version": null }, "edits": [at(0, 7, 14, "topla")] },
        { "textDocument": { "uri": uri("b.rs"), "version": null }, "edits": [at(0, 21, 28, "topla"), at(1, 4, 11, "topla")] },
        { "kind": "rename", "oldUri": uri("c.rs"), "newUri": uri("ç.rs") },
        { "kind": "create", "uri": uri("sub/new.rs") },
        { "textDocument": { "uri": uri("sub/new.rs"), "version": null }, "edits": [at(0, 0, 0, "// yeni 😀\n")] },
    ] })
}

fn project(name: &str) -> PathBuf {
    let d = dir(name);
    std::fs::write(d.join("a.rs"), "pub fn hesapla() {}\n").unwrap();
    std::fs::write(d.join("b.rs"), "use crate::a::{x, y, hesapla};\n    hesapla();\r\n").unwrap();
    std::fs::write(d.join("c.rs"), "// c\n").unwrap();
    d
}

#[test]
fn a_server_edit_is_planned_selected_and_written_end_to_end() {
    let d = project("e2e");
    let env = HostEnv::new(host(), Default::default());
    let p = plan::build(parse(&edit_in(&d)), &env, &[s(&d)], "local".into(), s(&d), crate::encoding::Encoding::Utf8).unwrap();
    let mut chosen = Chosen::initial(&p);
    // The file operations are chosen on purpose: the rename, the create and the edit that needs the create.
    for (i, item) in p.items.iter().enumerate() {
        if matches!(item, plan::Item::Resource(_)) {
            chosen.toggle_item(&p, i);
        }
    }
    assert_eq!(chosen.blocker(&p), None);
    let prepared = prepare(&p, &chosen);
    assert!(prepared.skipped.is_empty() && prepared.buffers.is_empty());
    assert_eq!(prepared.steps.len(), 5);
    let tx = run(&host(), prepared.steps, &no_cancel());
    assert!(tx.ok(), "{:?}", tx.steps);
    assert_eq!(std::fs::read_to_string(d.join("a.rs")).unwrap(), "pub fn topla() {}\n");
    assert_eq!(std::fs::read(d.join("b.rs")).unwrap(), b"use crate::a::{x, y, topla};\n    topla();\r\n".to_vec());
    assert!(!d.join("c.rs").exists() && d.join("ç.rs").exists());
    assert_eq!(std::fs::read_to_string(d.join("sub").join("new.rs")).unwrap(), "// yeni 😀\n");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn deselecting_one_change_writes_only_the_others() {
    let d = project("partial");
    let env = HostEnv::new(host(), Default::default());
    let p = plan::build(parse(&edit_in(&d)), &env, &[s(&d)], "local".into(), s(&d), crate::encoding::Encoding::Utf8).unwrap();
    let mut chosen = Chosen::initial(&p);
    // b.rs is the second item; its second change (the call) stays as it was.
    chosen.toggle_part(&p, 1, 1);
    // a.rs is left out altogether.
    chosen.toggle_item(&p, 0);
    let prepared = prepare(&p, &chosen);
    let tx = run(&host(), prepared.steps, &no_cancel());
    assert!(tx.ok());
    assert_eq!(std::fs::read_to_string(d.join("a.rs")).unwrap(), "pub fn hesapla() {}\n");
    assert_eq!(std::fs::read(d.join("b.rs")).unwrap(), b"use crate::a::{x, y, topla};\n    hesapla();\r\n".to_vec());
    assert!(d.join("c.rs").exists(), "an operation that was not chosen does not happen");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_file_changed_on_disk_after_the_plan_was_made_is_refused_and_nothing_else_changes() {
    let d = project("stale");
    let env = HostEnv::new(host(), Default::default());
    let p = plan::build(parse(&edit_in(&d)), &env, &[s(&d)], "local".into(), s(&d), crate::encoding::Encoding::Utf8).unwrap();
    std::fs::write(d.join("b.rs"), "// somebody was here\n").unwrap();
    let before = snapshot(&d);
    let tx = run(&host(), prepare(&p, &Chosen::initial(&p)).steps, &no_cancel());
    assert!(!tx.ok());
    assert!(tx.steps.iter().any(|(_, o)| *o == StepOutcome::Conflict));
    assert_eq!(snapshot(&d), before, "a.rs was written first and was taken back");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn open_documents_become_buffer_jobs_with_the_state_they_were_planned_from() {
    let d = project("buffers");
    let text = "pub fn hesapla() {}\n".to_string();
    let open = HashMap::from([(
        s(&d.join("a.rs")),
        plan::OpenDoc {
            text: text.clone(),
            epoch: 7,
            version: 3,
            encoding: crate::encoding::Encoding::Utf8,
            dirty: false,
            synced: true,
            server_version: None,
            edited_since_request: false,
        },
    )]);
    let env = HostEnv::new(host(), open);
    let p = plan::build(parse(&edit_in(&d)), &env, &[s(&d)], "local".into(), s(&d), crate::encoding::Encoding::Utf8).unwrap();
    let prepared = prepare(&p, &Chosen::initial(&p));
    assert_eq!(prepared.buffers.len(), 1);
    let job = &prepared.buffers[0];
    assert_eq!((job.epoch, job.version, job.hash, job.changes), (7, 3, proto::content_hash(text.as_bytes()), 1));
    // Character ranges in the buffer, not bytes: "pub fn " is 7 characters.
    assert_eq!(job.edits, [(7..14, "topla".to_string())]);
    assert!(prepared.steps.iter().all(|s| s.path() != job.path), "an open file is not written on the host");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn report_lines_say_what_changed_in_words() {
    let (d, steps) = fixture("report");
    let tx = run(&host(), steps, &no_cancel());
    let lines = report(&tx, &[("b.rs".to_string(), 2)], |p| p.rsplit('/').next().unwrap().to_string());
    let texts: Vec<_> = lines.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(texts[0], "Changed 3 changes in 2 files");
    assert!(texts.contains(&"Created n.txt") && texts.contains(&"Deleted eski"));
    assert!(texts.contains(&"Renamed şablon 😀.txt to taşındı.txt"), "{texts:?}");
    assert!(lines.iter().all(|(_, bad)| !bad));
    std::fs::remove_dir_all(&d).ok();
}
