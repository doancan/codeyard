use super::*;

#[test]
fn worktree_porcelain_preserves_paths_and_checkout_states() {
    let entries = parse_worktree_entries(
        "worktree /repo\0bare\0\0worktree /with spaces/çalışma\n\"tree\"\0HEAD abc123\0branch refs/heads/feature/ui\0locked on another disk\0\0worktree /gone\0HEAD def456\0detached\0prunable gitdir file points to non-existent location\0\0",
    );
    assert_eq!(entries.len(), 3);
    assert!(entries[0].bare);
    assert_eq!(entries[1].path, "/with spaces/çalışma\n\"tree\"");
    assert_eq!(entries[1].branch.as_deref(), Some("feature/ui"));
    assert_eq!(entries[1].locked.as_deref(), Some("on another disk"));
    assert_eq!(entries[2].branch, None);
    assert_eq!(entries[2].head.as_deref(), Some("def456"));
    assert!(entries[2].prunable.is_some());
    assert!(parse_worktree_entries("").is_empty());
}

#[test]
fn worktree_inventory_reads_real_git_locked_detached_and_missing_checkouts() {
    let Some((dir, run)) = crate::git::repo("worktree-inventory") else { return };
    // Keeping the linked directories in the fixture makes cleanup independent of the outcome.
    let linked = dir.join("linked space ü");
    let detached = dir.join("detached");
    run(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()]).unwrap();
    run(&["worktree", "add", "--detach", detached.to_str().unwrap()]).unwrap();
    run(&["worktree", "lock", "--reason", "keep this checkout", linked.to_str().unwrap()]).unwrap();
    let host = host::local();
    let entries = worktree_entries(&host, dir.to_str().unwrap()).unwrap();
    assert_eq!(entries.len(), 3);
    let entry = entries.iter().find(|entry| entry.branch.as_deref() == Some("linked")).unwrap();
    assert!(entry.path.ends_with("linked space ü"));
    assert_eq!(entry.locked.as_deref(), Some("keep this checkout"));
    let entry = entries.iter().find(|entry| entry.path.ends_with("/detached")).unwrap();
    assert_eq!(entry.branch, None);
    assert_eq!(entry.head.as_ref().map(String::len), Some(40));
    std::fs::remove_dir_all(&detached).unwrap();
    let entries = worktree_entries(&host, dir.to_str().unwrap()).unwrap();
    assert!(entries.iter().find(|entry| entry.path.ends_with("/detached")).unwrap().prunable.is_some());
    std::fs::remove_dir_all(&dir).unwrap();
}
