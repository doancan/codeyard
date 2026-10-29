use super::*;

#[test]
fn explorer_batch_deduplicates_descendants_and_rejects_roots_and_traversal() {
    let paths = ["/work/a/x", "/work/a", "/work/a", "/work/ab", "/work", "/", "/work/../victim", "relative", "/work/b"];
    let selected = top_level(paths.map(str::to_string).to_vec(), &["/work".into()]);
    assert_eq!(selected, ["/work/a", "/work/ab", "/work/b"]);
}

#[test]
fn explorer_batch_copy_resolves_name_collisions_without_overwriting() {
    let dir = crate::qa_workbench_tests::fresh("batch-copy-conflicts");
    let source = dir.join("data.txt");
    std::fs::write(&source, "source").unwrap();
    std::fs::write(dir.join("data copy.txt"), "existing").unwrap();
    let host = host::local();
    let source = crate::ui::local_path(&source);
    let progress = Progress { total: 1, ..Default::default() };
    let result = run(&host, &[source], &Operation::Copy(crate::ui::local_path(&dir)), &progress, &[]);
    assert!(result.failed.is_empty(), "{:?}", result.failed);
    assert_eq!(result.completed.len(), 1);
    assert_eq!(std::fs::read_to_string(dir.join("data copy 2.txt")).unwrap(), "source");
    assert_eq!(std::fs::read_to_string(dir.join("data copy.txt")).unwrap(), "existing");
}

#[test]
fn explorer_batch_move_continues_after_conflicts_and_keeps_unprocessed_paths_on_cancel() {
    let dir = crate::qa_workbench_tests::fresh("batch-move-conflicts");
    let dest = dir.join("dest");
    std::fs::create_dir(&dest).unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(dir.join(name), name).unwrap();
    }
    std::fs::write(dest.join("b"), "keep").unwrap();
    let paths: Vec<_> = ["a", "b", "c"].map(|n| crate::ui::local_path(&dir.join(n))).to_vec();
    let progress = Progress { total: 3, ..Default::default() };
    let host = host::local();
    let result = run(&host, &paths, &Operation::Move(crate::ui::local_path(&dest)), &progress, &[]);
    assert_eq!(result.completed.len(), 2);
    assert_eq!(result.failed.len(), 1);
    assert_eq!(std::fs::read_to_string(dest.join("b")).unwrap(), "keep");
    assert!(dir.join("b").exists());
    let progress = Progress { total: 1, ..Default::default() };
    progress.cancel.store(true, Ordering::Release);
    let result = run(&host, &paths[1..2], &Operation::Delete { trash: false }, &progress, &[]);
    assert!(result.completed.is_empty() && result.failed.is_empty());
    assert_eq!(result.remaining, paths[1..2]);
    assert!(dir.join("b").exists());
}
