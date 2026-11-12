//! The record of a transaction in progress.

use std::path::Path;
use std::sync::Arc;

use super::*;

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-refactor-journal-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    crate::ui::canonical_for_test(d)
}

fn steps() -> Vec<Step> {
    vec![
        Step::Write { path: "/w/a.rs".into(), bytes: b"new".to_vec(), expect: 7, original: Arc::new(b"old".to_vec()), changes: 1 },
        Step::Rename { from: "/w/b.rs".into(), to: "/w/c.rs".into() },
        Step::Delete { path: "/w/d".into() },
        Step::Create { path: "/w/e.rs".into() },
    ]
}

fn files(d: &Path) -> usize {
    std::fs::read_dir(d).unwrap().count()
}

#[test]
fn a_record_exists_while_the_steps_run_and_is_gone_when_they_end() {
    let d = dir("lifecycle");
    let j = Journal::begin(&d, "local", false, "/w", &steps()).expect("written");
    assert_eq!(files(&d), 1);
    // Our own process is not interrupted.
    assert!(take_interrupted(&d).is_empty());
    assert_eq!(files(&d), 1, "and its record is not taken");
    j.finish(false);
    assert_eq!(files(&d), 0);
    // A transaction whose rollback failed leaves it, for the warning.
    Journal::begin(&d, "local", false, "/w", &steps()).unwrap().finish(true);
    assert_eq!(files(&d), 1);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_record_of_a_process_that_is_gone_is_reported_once_with_its_paths() {
    let d = dir("stale");
    // What a killed Codeyard left: written by another process long ago.
    let body = serde_json::json!({
        "pid": 4_194_000, "started": 1, "host": "dev (ssh)", "remote": true, "root": "/srv/app",
        "steps": [{ "op": "write", "path": "/srv/app/a.rs" }, { "op": "delete", "path": "/srv/app/old" }],
    });
    std::fs::write(d.join("refactor-journal-4194000-0.json"), body.to_string()).unwrap();
    std::fs::write(d.join("unrelated.json"), "{}").unwrap();
    std::fs::write(d.join("refactor-journal-1-0.json"), "not json").unwrap();
    let found = take_interrupted(&d);
    assert_eq!(
        found,
        [Interrupted {
            host: "dev (ssh)".into(),
            remote: true,
            root: "/srv/app".into(),
            steps: vec![("/srv/app/a.rs".into(), "write".into()), ("/srv/app/old".into(), "delete".into())],
        }]
    );
    assert!(take_interrupted(&d).is_empty(), "reported once");
    assert!(d.join("unrelated.json").exists(), "other files are left alone");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_record_that_cannot_be_written_does_not_stop_the_transaction() {
    let d = dir("unwritable");
    // The folder is a file: nothing can be written there.
    let blocker = d.join("file");
    std::fs::write(&blocker, "x").unwrap();
    assert!(Journal::begin(&blocker.join("sub"), "local", false, "/w", &steps()).is_none());
    std::fs::remove_dir_all(&d).ok();
}
