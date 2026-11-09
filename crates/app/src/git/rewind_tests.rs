use super::*;
use crate::git::repo;

fn commit(dir: &std::path::Path, run: &impl Fn(&[&str]) -> Option<String>, name: &str) -> String {
    std::fs::write(dir.join(name), name).unwrap();
    run(&["add", name]).unwrap();
    run(&["commit", "-q", "-m", name]).unwrap();
    run(&["rev-parse", "HEAD"]).unwrap().trim().to_string()
}

#[test]
fn counts_pushed_and_only_local_commits() {
    let Some((dir, run)) = repo("rewind-count") else { return };
    let host = host::local();
    let root = dir.to_string_lossy().into_owned();
    let bare = dir.with_extension("origin.git");
    let _ = std::fs::remove_dir_all(&bare);
    crate::test_git::git().args(["init", "-q", "--bare"]).arg(&bare).output().unwrap();
    run(&["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();
    let base = run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
    commit(&dir, &run, "pushed.txt");
    run(&["push", "-q", "-u", "origin", "main"]).unwrap();
    commit(&dir, &run, "tagged.txt");
    run(&["tag", "keep"]).unwrap();
    commit(&dir, &run, "local.txt");
    let head = run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
    // Reset to the first commit: three leave, one is pushed, one is tagged, one exists only here.
    let l = leaving(&host, &root, &base).unwrap();
    assert_eq!(l, Leaving { head: head.clone(), count: 3, upstream: Some("origin/main".into()), pushed: 1, only_here: 1 });
    // Undo: the last commit only, not pushed, only here.
    let u = undoing(&host, &root).unwrap();
    assert_eq!((u.count, u.pushed, u.only_here), (1, 0, 1));
    // Another local branch keeps it: no longer only here.
    run(&["branch", "side"]).unwrap();
    assert_eq!(undoing(&host, &root).unwrap().only_here, 0);
    assert_eq!(head_is(&host, &root, &head), Ok(()));
    commit(&dir, &run, "later.txt");
    assert_eq!(head_is(&host, &root, &head), Err(moved_message()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&bare);
}

#[test]
fn a_root_commit_and_a_detached_head_have_no_upstream() {
    let Some((dir, run)) = repo("rewind-root") else { return };
    let host = host::local();
    let root = dir.to_string_lossy().into_owned();
    let u = undoing(&host, &root).unwrap();
    assert_eq!((u.count, u.upstream.clone(), u.pushed, u.only_here), (1, None, 0, 1));
    run(&["checkout", "-q", "--detach"]).unwrap();
    let u = undoing(&host, &root).unwrap();
    assert_eq!((u.count, u.only_here), (1, 0), "main still has it");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undoing_a_merge_takes_the_merged_commits_off_the_branch() {
    let Some((dir, run)) = repo("rewind-merge") else { return };
    let host = host::local();
    let root = dir.to_string_lossy().into_owned();
    run(&["checkout", "-q", "-b", "topic"]).unwrap();
    commit(&dir, &run, "t1.txt");
    commit(&dir, &run, "t2.txt");
    run(&["checkout", "-q", "main"]).unwrap();
    commit(&dir, &run, "m.txt");
    run(&["merge", "-q", "--no-ff", "topic", "-m", "merge"]).unwrap();
    run(&["branch", "-D", "topic"]).unwrap();
    let u = undoing(&host, &root).unwrap();
    assert_eq!((u.count, u.only_here), (3, 3), "the merge and the two commits it brought in");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_repository_has_nothing_to_undo() {
    let dir = std::env::temp_dir().join(format!("codeyard-rewind-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    if crate::test_git::git().args(["init", "-q"]).current_dir(&dir).output().is_err() {
        return;
    }
    let err = undoing(&host::local(), &dir.to_string_lossy()).unwrap_err();
    assert!(err.contains("no commit"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
