//! Explorer bulk copy, move and delete on an SSH host, against the throwaway `sshd` (`test_sshd`): every file operation
//! goes through the remote agent, a per-file failure is reported without stopping the rest, cancelling before the
//! confirmation changes nothing, and a delete says it is permanent and names the host.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::qa_integration_tests::wait_secs;
use crate::qa_workbench_tests::{fresh, setup};
use crate::test_sshd::ALIAS;
use crate::tree::TreeEvent;
use crate::workbench::Workbench;

fn s(p: &Path) -> String {
    crate::ui::local_path(p)
}

fn tick(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(300));
}

fn wait(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    wait_secs(cx, 30, what, |cx| {
        tick(cx);
        done(cx)
    });
}

/// A workbench connected to the test server with `dir` open there; the Explorer lists `rows` entries.
fn connected<'a>(dir: &Path, rows: usize, cx: &'a mut TestAppContext) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.connect(ALIAS.into(), window, cx));
    wait(cx, "connected", |cx| wb.read_with(cx, |wb, _| wb.ports_ready()));
    let path = s(dir);
    wb.update_in(cx, |wb, window, cx| wb.open_path(&path, window, cx));
    wait(cx, "the folder over SSH", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == rows);
    assert_eq!(wb.read_with(cx, |wb, _| wb.remote_host_label_for_test()), Some(format!("ssh://{ALIAS}")), "the files go through the SSH host");
    (wb, cx)
}

fn event(wb: &Entity<Workbench>, ev: TreeEvent, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(ev, window, cx));
    cx.run_until_parked();
}

fn idle(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wait(cx, "the file operation to finish", |cx| !wb.read_with(cx, |wb, _| wb.file_operation_active_for_test()));
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> (bool, String) {
    wb.read_with(cx, |wb, _| wb.notice_for_test()).unwrap_or_default()
}

fn write(dir: &Path, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|name| {
            std::fs::write(dir.join(name), format!("{name}\n")).unwrap();
            s(&dir.join(name))
        })
        .collect()
}

fn read(p: PathBuf) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// Copy and cut/paste of a selection over SSH: a name taken in the destination gets a copy name on copy and fails
/// that one entry on move (never overwritten), the rest goes on, and the summary reports both.
#[gpui::test]
fn bulk_copy_and_move_run_on_the_ssh_host_with_per_file_failures(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("bulk-copy-move-ssh") else { return };
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("bulk-ssh-copy-move");
    let paths = write(&dir, &["a.txt", "b.txt", "c.txt"]);
    let copies = dir.join("copies");
    let moved = dir.join("moved");
    for d in [&copies, &moved] {
        std::fs::create_dir(d).unwrap();
        std::fs::write(d.join("b.txt"), "keep\n").unwrap();
    }
    let (wb, cx) = connected(&dir, 5, cx);

    // Copy: one question naming the destination and the host, then all three land; b.txt is not overwritten.
    event(&wb, TreeEvent::CopyMany(paths.clone()), cx);
    event(&wb, TreeEvent::Paste { dir: s(&copies) }, cx);
    let (title, detail) = cx.pending_prompt().expect("one confirmation");
    assert_eq!(title, "Apply a file operation to 3 entries?");
    assert!(detail.contains(&format!("{} on {ALIAS}", s(&copies))), "{detail}");
    cx.simulate_prompt_answer("Copy");
    idle(&wb, cx);
    assert_eq!(read(copies.join("a.txt")), "a.txt\n");
    assert_eq!(read(copies.join("c.txt")), "c.txt\n");
    assert_eq!(read(copies.join("b.txt")), "keep\n", "never overwritten");
    let copied_b = std::fs::read_dir(&copies).unwrap().filter_map(|e| e.ok()).filter(|e| read(e.path()) == "b.txt\n").count();
    assert_eq!(copied_b, 1, "b.txt is copied under a copy name");
    assert_eq!(notice(&wb, cx), (false, "File operation: 3 completed, 0 failed, 0 not processed.".into()));
    assert!(paths.iter().all(|p| Path::new(p).exists()), "a copy keeps the sources");

    // Cancelled before the question is answered: nothing moves and the clipboard keeps every entry.
    event(&wb, TreeEvent::CutMany(paths.clone()), cx);
    event(&wb, TreeEvent::Paste { dir: s(&moved) }, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("File: Cancel File Operation", window, cx));
    cx.simulate_prompt_answer("Move");
    idle(&wb, cx);
    assert!(paths.iter().all(|p| Path::new(p).exists()));
    assert_eq!(read(moved.join("b.txt")), "keep\n");
    assert_eq!(wb.read_with(cx, |wb, _| wb.file_clipboard_paths_for_test()), paths);

    // Move: a.txt and c.txt move, b.txt fails on the name already taken, and stays on the clipboard to retry.
    event(&wb, TreeEvent::Paste { dir: s(&moved) }, cx);
    cx.simulate_prompt_answer("Move");
    idle(&wb, cx);
    assert_eq!(read(moved.join("a.txt")), "a.txt\n");
    assert_eq!(read(moved.join("c.txt")), "c.txt\n");
    assert_eq!(read(moved.join("b.txt")), "keep\n", "never overwritten");
    assert!(!dir.join("a.txt").exists() && !dir.join("c.txt").exists() && dir.join("b.txt").exists());
    let (error, text) = notice(&wb, cx);
    assert!(error && text.starts_with("File operation: 2 completed, 1 failed, 0 not processed."), "{text}");
    assert!(text.contains(&paths[1]), "the failed entry is named: {text}");
    assert_eq!(wb.read_with(cx, |wb, _| wb.file_clipboard_paths_for_test()), [paths[1].clone()]);
    wait(cx, "the Explorer after the move", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == 3);
}

/// Delete over SSH is permanent: the question names the count and the host and says there is no Trash, the button is
/// "Delete Permanently", Cancel deletes nothing, and a file that cannot be removed fails alone.
#[gpui::test]
fn bulk_delete_on_the_ssh_host_is_permanent_and_says_so(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("bulk-delete-ssh") else { return };
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("bulk-ssh-delete");
    let locked = dir.join("locked");
    std::fs::create_dir(&locked).unwrap();
    let mut paths = write(&dir, &["x.txt", "y.txt"]);
    paths.extend(write(&locked, &["z.txt"]));
    // A folder without write permission: its file cannot be removed (root ignores the mode, so only the count differs).
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    // SAFETY: geteuid has no preconditions.
    let enforced = unsafe { libc::geteuid() } != 0;
    let (wb, cx) = connected(&dir, 3, cx);

    event(&wb, TreeEvent::DeleteMany(paths.clone()), cx);
    let (title, detail) = cx.pending_prompt().expect("one confirmation");
    assert_eq!(title, format!("Permanently delete 3 items on {ALIAS}?"));
    assert!(detail.starts_with(&format!("{ALIAS} has no Trash: the items are deleted at once and can't be restored.")), "{detail}");
    assert!(paths.iter().all(|p| detail.contains(p.as_str())), "the entries are listed: {detail}");
    cx.simulate_prompt_answer("Cancel");
    idle(&wb, cx);
    assert!(paths.iter().all(|p| Path::new(p).exists()), "Cancel deletes nothing");

    event(&wb, TreeEvent::DeleteMany(paths.clone()), cx);
    cx.simulate_prompt_answer("Delete Permanently");
    idle(&wb, cx);
    assert!(!dir.join("x.txt").exists() && !dir.join("y.txt").exists());
    let (error, text) = notice(&wb, cx);
    if enforced {
        assert!(locked.join("z.txt").exists(), "a file that cannot be removed stays");
        assert!(error && text.starts_with("File operation: 2 completed, 1 failed, 0 not processed."), "{text}");
        assert!(text.contains(&paths[2]), "the failed entry is named: {text}");
    } else {
        assert_eq!((error, text.as_str()), (false, "File operation: 3 completed, 0 failed, 0 not processed."));
    }
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(dir.join("locked").exists(), "only the selected entries are deleted");

    // A single folder says the same: permanent, on the host, with the folder's contents.
    event(&wb, TreeEvent::Delete(s(&locked), true), cx);
    let (title, detail) = cx.pending_prompt().expect("the confirmation");
    assert_eq!(title, format!("Permanently delete “locked” on {ALIAS}?"));
    assert!(detail.contains(&format!("{ALIAS} has no Trash: it is deleted at once and can't be restored.")), "{detail}");
    cx.simulate_prompt_answer("Delete Permanently");
    wait(cx, "the folder deleted over SSH", |_| !locked.exists());
}
