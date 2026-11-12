//! The explorer's filter and file nesting, driven through a real tree over a real folder.

use super::*;

/// Removes a test's folder when the test's thread ends.
struct Cleanup(std::path::PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

thread_local! {
    static FOLDERS: std::cell::RefCell<Vec<Cleanup>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn project(tag: &str, files: &[&str]) -> String {
    let dir = std::env::temp_dir().join(format!("codeyard-panel-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    FOLDERS.with(|f| f.borrow_mut().push(Cleanup(dir.clone())));
    for f in files {
        let path = dir.join(f);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x\n").unwrap();
    }
    crate::ui::local_path(&crate::ui::canonical_for_test(&dir))
}

fn open_tree<'a>(cx: &'a mut gpui::TestAppContext, root: &str) -> (gpui::Entity<FileTree>, &'a mut gpui::VisualTestContext) {
    crate::qa_workbench_tests::globals(cx);
    let root = root.to_string();
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = FileTree::new(Arc::new(host::local()), cx);
        tree.set_options(ViewOptions::default(), cx);
        tree.set_root(Arc::new(host::local()), Some(root), cx);
        tree
    });
    wait(cx, "the root to load", |cx| tree.read_with(cx, |t, _| !t.rows.is_empty()));
    (tree, cx)
}

fn wait(cx: &mut gpui::VisualTestContext, what: &str, mut done: impl FnMut(&mut gpui::VisualTestContext) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "timed out after 30 s waiting for {what}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn names(tree: &gpui::Entity<FileTree>, cx: &gpui::VisualTestContext) -> Vec<String> {
    tree.read_with(cx, |t, _| t.rows.iter().map(|r| format!("{}{}", "  ".repeat(r.depth), r.name)).collect())
}

fn press(tree: &gpui::Entity<FileTree>, cx: &mut gpui::VisualTestContext, keys: &str) {
    tree.update_in(cx, |t, w, _| w.focus(&t.focus_handle));
    cx.simulate_keystrokes(keys);
}

const NESTING_FILES: &[&str] = &["Cargo.toml", "Cargo.lock", "README.md", "src/a.ts", "src/a.test.ts", "src/a.d.ts", "src/b.ts"];

#[gpui::test]
fn related_files_are_listed_under_their_parent_and_open_with_the_arrow_keys(cx: &mut gpui::TestAppContext) {
    let root = project("nest", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    assert_eq!(names(&tree, cx), ["src", "Cargo.toml", "README.md"]);
    tree.update(cx, |t, cx| t.reveal(&format!("{root}/src"), cx));
    press(&tree, cx, "right");
    wait(cx, "src to load", |cx| names(&tree, cx).len() > 3);
    assert_eq!(names(&tree, cx), ["src", "  a.ts", "  b.ts", "Cargo.toml", "README.md"]);

    // Right opens the nested files of the selected parent; a second Right steps into the first of them.
    tree.update(cx, |t, cx| t.reveal(&format!("{root}/src/a.ts"), cx));
    assert_eq!(tree.read_with(cx, |t, _| t.selected_path()), Some(format!("{root}/src/a.ts")));
    press(&tree, cx, "right");
    assert_eq!(names(&tree, cx), ["src", "  a.ts", "    a.d.ts", "    a.test.ts", "  b.ts", "Cargo.toml", "README.md"]);
    press(&tree, cx, "right");
    assert_eq!(tree.read_with(cx, |t, _| t.selected_path()), Some(format!("{root}/src/a.d.ts")));
    // Left from a nested file goes to its parent, and Left again closes the parent.
    press(&tree, cx, "left");
    assert_eq!(tree.read_with(cx, |t, _| t.selected_path()), Some(format!("{root}/src/a.ts")));
    assert_eq!(tree.read_with(cx, |t, _| t.open_nests()), [format!("{root}/src/a.ts")]);
    press(&tree, cx, "left");
    assert_eq!(names(&tree, cx), ["src", "  a.ts", "  b.ts", "Cargo.toml", "README.md"]);
    assert!(tree.read_with(cx, |t, _| t.open_nests().is_empty()));
}

#[gpui::test]
fn closing_a_parent_moves_the_selection_from_its_nested_file_to_the_parent(cx: &mut gpui::TestAppContext) {
    let root = project("nest-close", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    tree.update(cx, |t, cx| t.toggle_nest(&format!("{root}/Cargo.toml"), cx));
    assert_eq!(names(&tree, cx), ["src", "Cargo.toml", "  Cargo.lock", "README.md"]);
    tree.update(cx, |t, cx| {
        let ix = t.rows.iter().position(|r| r.name == "Cargo.lock").unwrap();
        t.select(ix, cx);
        t.toggle_nest(&format!("{root}/Cargo.toml"), cx);
    });
    assert_eq!(tree.read_with(cx, |t, _| t.selected_paths()), [format!("{root}/Cargo.toml")]);
}

#[gpui::test]
fn revealing_a_nested_file_opens_its_parent(cx: &mut gpui::TestAppContext) {
    let root = project("nest-reveal", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    tree.update(cx, |t, cx| t.reveal(&format!("{root}/Cargo.lock"), cx));
    assert_eq!(names(&tree, cx), ["src", "Cargo.toml", "  Cargo.lock", "README.md"]);
    assert_eq!(tree.read_with(cx, |t, _| t.selected_path()), Some(format!("{root}/Cargo.lock")));
}

#[gpui::test]
fn nesting_off_lists_every_file_and_back_on_groups_them_again(cx: &mut gpui::TestAppContext) {
    let root = project("nest-off", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    let off = ViewOptions { nesting: nesting::Nesting::default().with_enabled(false), ..ViewOptions::default() };
    tree.update(cx, |t, cx| t.set_options(off, cx));
    assert_eq!(names(&tree, cx), ["src", "Cargo.lock", "Cargo.toml", "README.md"]);
    tree.update(cx, |t, cx| t.set_options(ViewOptions::default(), cx));
    assert_eq!(names(&tree, cx), ["src", "Cargo.toml", "README.md"]);
}

#[gpui::test]
fn selecting_all_does_not_take_hidden_nested_files_along(cx: &mut gpui::TestAppContext) {
    let root = project("nest-bulk", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    tree.update_in(cx, |t, w, cx| t.select_all_entries(&SelectAllEntries, w, cx));
    assert_eq!(tree.read_with(cx, |t, _| t.selected_paths()), [format!("{root}/src"), format!("{root}/Cargo.toml"), format!("{root}/README.md")]);
}

#[gpui::test]
fn dragging_or_deleting_a_parent_names_only_the_parent(cx: &mut gpui::TestAppContext) {
    let root = project("nest-drag", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = events.clone();
    let _sub = cx.update(|_, cx| {
        cx.subscribe(&tree, move |_, ev: &TreeEvent, _| {
            if let TreeEvent::Delete(path, _) = ev {
                log.borrow_mut().push(path.clone());
            }
        })
    });
    tree.update(cx, |t, cx| t.reveal(&format!("{root}/Cargo.toml"), cx));
    press(&tree, cx, "secondary-backspace");
    assert_eq!(*events.borrow(), [format!("{root}/Cargo.toml")]);
}

#[gpui::test]
fn the_filter_leaves_matching_names_with_the_folders_that_lead_to_them(cx: &mut gpui::TestAppContext) {
    let root = project("filter", &["src/deep/Needle.rs", "src/other.rs", "docs/İstanbul.md", "docs/readme.md", "Cargo.toml"]);
    let (tree, cx) = open_tree(cx, &root);
    assert_eq!(names(&tree, cx), ["docs", "src", "Cargo.toml"], "nothing is opened yet");
    tree.update(cx, |t, cx| t.set_filter_text("needle", cx));
    wait(cx, "the file list", |cx| names(&tree, cx).contains(&"    Needle.rs".to_string()));
    assert_eq!(names(&tree, cx), ["src", "  deep", "    Needle.rs"], "found in a folder that was never opened");
    assert_eq!(tree.read_with(cx, |t, _| t.filter_status().hits), 1);

    tree.update(cx, |t, cx| t.set_filter_text("ISTANBUL", cx));
    assert_eq!(names(&tree, cx), ["docs", "  İstanbul.md"], "the dotted capital I matches a plain one");

    tree.update(cx, |t, cx| t.set_filter_text("zzz", cx));
    assert!(names(&tree, cx).is_empty());
    assert_eq!(tree.read_with(cx, |t, _| t.filter_status().hits), 0);

    tree.update(cx, |t, cx| t.set_filter_text("", cx));
    assert_eq!(names(&tree, cx), ["docs", "src", "Cargo.toml"], "the tree is back as it was");
}

#[gpui::test]
fn filtered_rows_can_be_selected_together_and_escape_reveals_the_selection(cx: &mut gpui::TestAppContext) {
    let root = project("filter-select", &["a/one.rs", "b/two.rs", "b/three.txt"]);
    let (tree, cx) = open_tree(cx, &root);
    tree.update_in(cx, |t, w, cx| t.toggle_filter(w, cx));
    assert!(tree.read_with(cx, |t, _| t.filter_open()));
    tree.update(cx, |t, cx| t.set_filter_text(".rs", cx));
    wait(cx, "the file list", |cx| names(&tree, cx).len() == 4);
    assert_eq!(names(&tree, cx), ["a", "  one.rs", "b", "  two.rs"]);
    tree.update(cx, |t, cx| {
        t.select(1, cx);
        t.select_with(3, true, false, cx);
    });
    assert_eq!(tree.read_with(cx, |t, _| t.selected_paths()), [format!("{root}/a/one.rs"), format!("{root}/b"), format!("{root}/b/two.rs")]);

    // A folder is not opened or closed while filtering; Escape in the tree closes the filter and shows where the selection is.
    tree.update_in(cx, |t, w, cx| {
        t.select(3, cx);
        w.focus(&t.focus_handle);
    });
    cx.simulate_keystrokes("escape");
    wait(cx, "the whole tree", |cx| names(&tree, cx).contains(&"  three.txt".to_string()));
    assert!(!tree.read_with(cx, |t, _| t.filter_open()));
    assert_eq!(tree.read_with(cx, |t, _| t.selected_path()), Some(format!("{root}/b/two.rs")));
}

#[gpui::test]
fn a_filter_keeps_git_marks_and_finds_nested_files_by_name(cx: &mut gpui::TestAppContext) {
    let root = project("filter-nest", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    let mut marks = HashMap::new();
    marks.insert(format!("{root}/Cargo.lock"), crate::git::Mark::Modified);
    tree.update(cx, |t, cx| t.set_git(Arc::new(marks), cx));
    tree.update(cx, |t, cx| t.set_filter_text("cargo", cx));
    wait(cx, "the file list", |cx| names(&tree, cx).len() == 2);
    assert_eq!(names(&tree, cx), ["Cargo.lock", "Cargo.toml"], "nesting is off while filtering");
    assert!(tree.read_with(cx, |t, _| t.git.contains_key(&format!("{root}/Cargo.lock"))));
}

#[gpui::test]
fn fifty_thousand_entries_stay_responsive(cx: &mut gpui::TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (tree, cx) = cx.add_window_view(|_, cx| {
        let mut tree = FileTree::new(Arc::new(host::local()), cx);
        tree.root = Some("/fixture".into());
        let entries = (0..50_000)
            .map(|i| proto::DirEntry {
                name: if i % 5 == 0 {
                    format!("mod{i}.ts")
                } else if i % 5 == 1 {
                    format!("mod{}.d.ts", i - 1)
                } else {
                    format!("file-{i:05}.txt")
                },
                kind: proto::Kind::File,
                target_kind: None,
            })
            .collect();
        tree.children.insert("/fixture".into(), Load::Loaded(entries));
        tree
    });
    let started = std::time::Instant::now();
    tree.update(cx, |t, _| t.rebuild());
    let rebuild = started.elapsed();
    let rows = tree.read_with(cx, |t, _| t.rows.len());
    eprintln!("rebuild of 50,000 entries with nesting: {rebuild:?} ({rows} rows)");
    assert!(rows < 50_000, "files were nested");
    assert!(rebuild < std::time::Duration::from_secs(30), "{rebuild:?}");
    tree.update(cx, |t, cx| t.set_options(ViewOptions { nesting: nesting::Nesting::off(), ..ViewOptions::default() }, cx));
    let started = std::time::Instant::now();
    tree.update(cx, |t, _| t.rebuild());
    eprintln!("rebuild of 50,000 entries without nesting: {:?}", started.elapsed());
    tree.update(cx, |t, cx| t.set_options(ViewOptions::default(), cx));

    let started = std::time::Instant::now();
    tree.update(cx, |t, cx| t.set_filter_text("file-4999", cx));
    wait(cx, "the filtered rows", |cx| tree.read_with(cx, |t, _| !t.filter_status().searching));
    eprintln!("first filter over 50,000 entries: {:?}", started.elapsed());
    let started = std::time::Instant::now();
    for q in ["file-4999", "file-49", "mod1", "d.ts", "x"] {
        tree.update(cx, |t, cx| t.set_filter_text(q, cx));
    }
    let per_key = started.elapsed() / 5;
    eprintln!("each further keystroke: {per_key:?}");
    assert!(per_key < std::time::Duration::from_secs(5), "{per_key:?}");
}

#[gpui::test]
fn deleting_a_parent_lists_its_nested_files_on_their_own(cx: &mut gpui::TestAppContext) {
    let root = project("nest-delete", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    tree.update(cx, |t, cx| t.toggle_nest(&format!("{root}/Cargo.toml"), cx));
    std::fs::remove_file(format!("{root}/Cargo.toml")).unwrap();
    tree.update(cx, |t, cx| t.reload(&root, cx));
    wait(cx, "the new listing", |cx| names(&tree, cx) == ["src", "Cargo.lock", "README.md"]);
    assert!(tree.read_with(cx, |t, _| t.open_nests().is_empty()), "the deleted parent is not remembered as open");
}

#[gpui::test]
fn a_renamed_parent_is_not_remembered_as_open_and_a_restored_stale_one_is_dropped(cx: &mut gpui::TestAppContext) {
    let root = project("nest-prune", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    tree.update(cx, |t, cx| t.toggle_nest(&format!("{root}/Cargo.toml"), cx));
    std::fs::rename(format!("{root}/Cargo.toml"), format!("{root}/Cargo2.toml")).unwrap();
    tree.update(cx, |t, cx| t.reload(&root, cx));
    wait(cx, "the new listing", |cx| names(&tree, cx).contains(&"Cargo2.toml".to_string()));
    assert!(tree.read_with(cx, |t, _| t.open_nests().is_empty()));
    // A saved list from the last session can name files that have gone since.
    tree.update(cx, |t, cx| t.restore_nests(vec![format!("{root}/Cargo.toml"), format!("{root}/src/gone.ts")], cx));
    assert!(tree.read_with(cx, |t, _| t.open_nests().iter().all(|p| !p.ends_with("Cargo.toml"))), "a file in a loaded folder that is not there");
}

#[gpui::test]
fn switching_folders_while_the_filter_is_working_shows_only_the_new_folders_files(cx: &mut gpui::TestAppContext) {
    let first = project("switch-a", &["alpha/needle-a.rs", "other.txt"]);
    let second = project("switch-b", &["beta/needle-b.rs", "other.txt"]);
    let (tree, cx) = open_tree(cx, &first);
    // The filter starts reading folder A's files on a worker ...
    tree.update(cx, |t, cx| t.set_filter_text("needle", cx));
    // ... and the tree moves to folder B before the worker answers.
    tree.update(cx, |t, cx| t.set_root(Arc::new(host::local()), Some(second.clone()), cx));
    // At no moment may a row of folder A show.
    wait(cx, "folder B's matches", |cx| {
        assert!(!names(&tree, cx).iter().any(|n| n.contains("needle-a") || n == "alpha"), "{:?}", names(&tree, cx));
        names(&tree, cx).contains(&"  needle-b.rs".to_string())
    });
    for _ in 0..20 {
        cx.run_until_parked();
        assert!(!names(&tree, cx).iter().any(|n| n.contains("needle-a")));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(names(&tree, cx), ["beta", "  needle-b.rs"], "nothing of folder A");
    assert_eq!(tree.read_with(cx, |t, _| t.filter_status().hits), 1);
}

#[gpui::test]
fn a_reload_while_the_filter_is_working_does_not_bring_back_the_old_file_list(cx: &mut gpui::TestAppContext) {
    let root = project("reload-mid-build", &["a/needle-1.rs"]);
    let (tree, cx) = open_tree(cx, &root);
    tree.update(cx, |t, cx| t.set_filter_text("needle", cx));
    wait(cx, "the first list", |cx| names(&tree, cx).contains(&"  needle-1.rs".to_string()));
    std::fs::write(format!("{root}/a/needle-2.rs"), "x").unwrap();
    tree.update(cx, |t, cx| {
        // The first reload starts a new read of the file list; a second one arrives while it is still running.
        t.reload_all(cx);
        t.reload_all(cx);
    });
    wait(cx, "the new file", |cx| names(&tree, cx).contains(&"  needle-2.rs".to_string()));
    assert_eq!(tree.read_with(cx, |t, _| t.filter_status().hits), 2);
}

#[gpui::test]
fn renaming_a_file_into_a_pattern_nests_it_after_the_reload(cx: &mut gpui::TestAppContext) {
    let root = project("nest-rename", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    std::fs::rename(format!("{root}/Cargo.lock"), format!("{root}/lock.txt")).unwrap();
    tree.update(cx, |t, cx| t.reload(&root, cx));
    wait(cx, "the new listing", |cx| names(&tree, cx) == ["src", "Cargo.toml", "lock.txt", "README.md"]);
}

#[gpui::test]
fn the_open_parents_are_remembered_and_come_back(cx: &mut gpui::TestAppContext) {
    let root = project("nest-restore", NESTING_FILES);
    let (tree, cx) = open_tree(cx, &root);
    tree.update(cx, |t, cx| t.toggle_nest(&format!("{root}/Cargo.toml"), cx));
    let saved = tree.read_with(cx, |t, _| t.open_nests());
    assert_eq!(saved, [format!("{root}/Cargo.toml")]);
    let (again, cx) = open_tree(cx, &root);
    assert_eq!(names(&again, cx), ["src", "Cargo.toml", "README.md"], "a new tree starts closed");
    again.update(cx, |t, cx| t.restore_nests(saved, cx));
    assert_eq!(names(&again, cx), ["src", "Cargo.toml", "  Cargo.lock", "README.md"]);
}

#[gpui::test]
fn typing_right_after_the_shortcut_goes_into_the_box(cx: &mut gpui::TestAppContext) {
    let root = project("filter-type", &["README.md", "src/main.rs"]);
    let (tree, cx) = open_tree(cx, &root);
    press(&tree, cx, "secondary-f");
    cx.simulate_input("rs");
    assert!(tree.read_with(cx, |t, _| t.filter_open()));
    assert_eq!(tree.read_with(cx, |t, cx| t.filter_text(cx)), Some("rs".to_string()));
}
