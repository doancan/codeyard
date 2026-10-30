//! Apply and undo on real files through the local agent, with external writes and failures injected; every assertion
//! compares exact bytes.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

use super::*;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-replace-apply-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    crate::ui::canonical_for_test(d)
}

fn host() -> Arc<dyn Host> {
    Arc::new(host::local())
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn job(path: &Path, before: &[u8], after: &[u8]) -> Job {
    Job { path: s(path), bytes: after.to_vec(), expect: proto::content_hash(before), original: Arc::new(before.to_vec()), changes: 1 }
}

/// Wraps a store: write number `fail_at` (0-based) fails as configured; everything else goes through.
struct Faulty {
    inner: Arc<dyn Host>,
    fail_at: usize,
    mode: Fault,
    writes: AtomicUsize,
}

#[derive(Clone, Copy)]
enum Fault {
    /// The agent refuses; nothing is written.
    Refused,
    /// The connection drops before the write; the read-back fails too.
    Disconnected,
    /// The write lands but the reply is lost.
    LostReply,
}

impl Store for Faulty {
    fn read(&self, path: &str) -> Result<(Vec<u8>, u64), HostError> {
        if matches!(self.mode, Fault::Disconnected) && self.writes.load(Ordering::SeqCst) > self.fail_at {
            return Err(HostError::Disconnected("connection lost".into()));
        }
        self.inner.read(path)
    }
    fn write(&self, path: &str, data: Vec<u8>, expect: Option<u64>) -> Result<u64, HostError> {
        let n = self.writes.fetch_add(1, Ordering::SeqCst);
        if n != self.fail_at {
            return self.inner.write(path, data, expect);
        }
        match self.mode {
            Fault::Refused => Err(HostError::Op(proto::Error::PermissionDenied(path.into()))),
            Fault::Disconnected => Err(HostError::Disconnected("connection lost".into())),
            Fault::LostReply => {
                self.inner.write(path, data, expect)?;
                Err(HostError::Uncertain("no reply".into()))
            }
        }
    }
}

#[test]
fn apply_and_undo_write_exact_bytes() {
    let d = dir("roundtrip");
    let (a, b) = (d.join("a.txt"), d.join("b.txt"));
    let (a0, b0) = (b"one\r\ntwo\r\n".to_vec(), b"\xEF\xBB\xBFx\n".to_vec());
    std::fs::write(&a, &a0).unwrap();
    std::fs::write(&b, &b0).unwrap();
    let h = host();
    let stop = AtomicBool::new(false);
    let (out, undo) = write_all(&h, vec![job(&a, &a0, b"ONE\r\ntwo\r\n"), job(&b, &b0, b"\xEF\xBB\xBFy\n")], &stop);
    assert!(out.iter().all(|(_, o)| o.changed()), "{out:?}");
    assert_eq!(std::fs::read(&a).unwrap(), b"ONE\r\ntwo\r\n");
    assert_eq!(std::fs::read(&b).unwrap(), b"\xEF\xBB\xBFy\n");
    let back = undo_all(&h, &undo, &stop);
    assert!(back.iter().all(|(_, o)| o.changed()), "{back:?}");
    assert_eq!(std::fs::read(&a).unwrap(), a0);
    assert_eq!(std::fs::read(&b).unwrap(), b0);
}

#[test]
fn an_external_write_after_the_preview_is_a_conflict_and_is_kept() {
    let d = dir("conflict");
    let a = d.join("a.txt");
    std::fs::write(&a, b"old\n").unwrap();
    // Someone else saves between preview and apply.
    std::fs::write(&a, b"theirs\n").unwrap();
    let (out, undo) = write_all(&host(), vec![job(&a, b"old\n", b"new\n")], &AtomicBool::new(false));
    assert_eq!(out[0].1, Outcome::Conflict);
    assert!(undo.is_empty());
    assert_eq!(std::fs::read(&a).unwrap(), b"theirs\n", "never overwritten");
    let report = Report { undo: false, files: out };
    assert!(report.lines(ui_name).iter().any(|(l, bad)| *bad && l == "File changed since preview: a.txt"));
}

fn ui_name(p: &str) -> String {
    crate::ui::file_name(p).to_string()
}

#[test]
fn an_external_write_after_apply_is_an_undo_conflict_and_is_kept() {
    let d = dir("undo-conflict");
    let (a, b) = (d.join("a.txt"), d.join("b.txt"));
    std::fs::write(&a, b"a0\n").unwrap();
    std::fs::write(&b, b"b0\n").unwrap();
    let h = host();
    let stop = AtomicBool::new(false);
    let (_, undo) = write_all(&h, vec![job(&a, b"a0\n", b"a1\n"), job(&b, b"b0\n", b"b1\n")], &stop);
    std::fs::write(&a, b"edited later\n").unwrap();
    let back = undo_all(&h, &undo, &stop);
    assert_eq!(back[0].1, Outcome::Conflict);
    assert!(back[1].1.changed());
    assert_eq!(std::fs::read(&a).unwrap(), b"edited later\n", "the later edit survives");
    assert_eq!(std::fs::read(&b).unwrap(), b"b0\n");
    let report = Report { undo: true, files: back };
    let lines = report.lines(ui_name);
    assert!(lines.iter().any(|(l, bad)| *bad && l.contains("Undo conflict: a.txt")), "{lines:?}");
    assert!(lines.iter().any(|(l, bad)| !*bad && l.contains("b.txt")), "{lines:?}");
}

#[test]
fn a_refused_write_mid_batch_is_reported_and_the_rest_is_exact() {
    let d = dir("refused");
    let files: Vec<PathBuf> = (0..3).map(|i| d.join(format!("f{i}.txt"))).collect();
    for (i, f) in files.iter().enumerate() {
        std::fs::write(f, format!("v0 {i}\n")).unwrap();
    }
    let store = Faulty { inner: host(), fail_at: 1, mode: Fault::Refused, writes: AtomicUsize::new(0) };
    let jobs = files.iter().enumerate().map(|(i, f)| job(f, format!("v0 {i}\n").as_bytes(), format!("v1 {i}\n").as_bytes())).collect::<Vec<_>>();
    let (out, undo) = write_all(&store, jobs, &AtomicBool::new(false));
    assert!(out[0].1.changed() && out[2].1.changed());
    assert!(matches!(out[1].1, Outcome::Failed(_)), "{:?}", out[1].1);
    assert_eq!(undo.len(), 2);
    assert_eq!(std::fs::read(&files[0]).unwrap(), b"v1 0\n");
    assert_eq!(std::fs::read(&files[1]).unwrap(), b"v0 1\n", "confirmed unchanged");
    assert_eq!(std::fs::read(&files[2]).unwrap(), b"v1 2\n");
}

#[test]
fn a_lost_connection_is_uncertain_not_a_success_or_a_failure() {
    let d = dir("disconnect");
    let a = d.join("a.txt");
    std::fs::write(&a, b"x\n").unwrap();
    let store = Faulty { inner: host(), fail_at: 0, mode: Fault::Disconnected, writes: AtomicUsize::new(0) };
    let (out, undo) = write_all(&store, vec![job(&a, b"x\n", b"y\n")], &AtomicBool::new(false));
    assert!(matches!(out[0].1, Outcome::Uncertain(_)), "{:?}", out[0].1);
    assert!(undo.is_empty());
    assert_eq!(std::fs::read(&a).unwrap(), b"x\n");
    let lines = Report { undo: false, files: out }.lines(ui_name);
    assert!(lines[0].0.starts_with("Unknown outcome, check before retrying: a.txt"), "{lines:?}");
}

#[test]
fn a_write_that_landed_without_a_reply_is_confirmed_and_undoable() {
    let d = dir("lost-reply");
    let a = d.join("a.txt");
    std::fs::write(&a, b"x\n").unwrap();
    let store = Faulty { inner: host(), fail_at: 0, mode: Fault::LostReply, writes: AtomicUsize::new(0) };
    let (out, undo) = write_all(&store, vec![job(&a, b"x\n", b"y\n")], &AtomicBool::new(false));
    assert_eq!(out[0].1, Outcome::Changed(1), "read back: it landed");
    assert_eq!(undo.len(), 1);
    assert_eq!(undo_all(&host(), &undo, &AtomicBool::new(false))[0].1, Outcome::Changed(0));
    assert_eq!(std::fs::read(&a).unwrap(), b"x\n");
}

#[test]
fn cancel_stops_before_the_next_file() {
    let d = dir("cancel");
    let (a, b) = (d.join("a.txt"), d.join("b.txt"));
    std::fs::write(&a, b"a\n").unwrap();
    std::fs::write(&b, b"b\n").unwrap();
    let stop = AtomicBool::new(true);
    let (out, undo) = write_all(&host(), vec![job(&a, b"a\n", b"A\n"), job(&b, b"b\n", b"B\n")], &stop);
    assert!(out.iter().all(|(_, o)| *o == Outcome::NotAttempted));
    assert!(undo.is_empty());
    assert_eq!(std::fs::read(&a).unwrap(), b"a\n");
    let lines = Report { undo: false, files: out }.lines(ui_name);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].0.starts_with("Not changed (cancelled): a.txt"));
}

#[cfg(unix)]
#[test]
fn a_read_only_folder_fails_that_file_only() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir("readonly");
    let locked = d.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    let (a, b) = (d.join("a.txt"), locked.join("b.txt"));
    std::fs::write(&a, b"a\n").unwrap();
    std::fs::write(&b, b"b\n").unwrap();
    std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (out, _) = write_all(&host(), vec![job(&a, b"a\n", b"A\n"), job(&b, b"b\n", b"B\n")], &AtomicBool::new(false));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(out[0].1.changed());
    assert!(matches!(out[1].1, Outcome::Failed(_)), "{:?}", out[1].1);
    assert_eq!(std::fs::read(&a).unwrap(), b"A\n");
    assert_eq!(std::fs::read(&b).unwrap(), b"b\n");
}

#[test]
fn a_deleted_file_is_a_conflict_never_a_recreation() {
    let d = dir("deleted");
    let (a, b) = (d.join("a.txt"), d.join("b.txt"));
    std::fs::write(&a, b"a\n").unwrap();
    std::fs::write(&b, b"b\n").unwrap();
    std::fs::remove_file(&a).unwrap();
    let h = host();
    let stop = AtomicBool::new(false);
    let (out, undo) = write_all(&h, vec![job(&a, b"a\n", b"A\n"), job(&b, b"b\n", b"B\n")], &stop);
    assert_eq!(out[0].1, Outcome::Conflict);
    assert!(!a.exists(), "a file deleted after the preview is not brought back");
    // Deleted after the replacement: undo does not recreate it either.
    std::fs::remove_file(&b).unwrap();
    assert_eq!(undo_all(&h, &undo, &stop)[0].1, Outcome::Conflict);
    assert!(!b.exists());
}
