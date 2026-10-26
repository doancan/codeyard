//! Compare Folders through the workbench: scans of real folders, the copy and delete prompts, the trust gate, cancelling,
//! and one comparison between this computer and an SSH host (the test sshd).

use std::path::{Path, PathBuf};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::compare_folders::CompareView;
use crate::compare_logic::{Direction, Side, Status, Why};
use crate::qa_integration_tests::{notice, setup_with, wait_secs, wait_ticking};
use crate::qa_workbench_tests::fresh;
use crate::settings::Settings;
use crate::workbench::Workbench;

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// Two folders with one of everything: same, edited, one-sided, same size but different content.
fn pair(name: &str) -> (PathBuf, PathBuf) {
    let (l, r) = (fresh(&format!("{name}-left")), fresh(&format!("{name}-right")));
    for d in [&l, &r] {
        write(d, "same.txt", "unchanged\n");
        write(d, "src/lib.rs", "fn main() {}\n");
    }
    write(&l, "edited.txt", "one\ntwo\n");
    write(&r, "edited.txt", "one\ntwo\nthree\n");
    write(&l, "swapped.txt", "aaaa");
    write(&r, "swapped.txt", "bbbb");
    write(&l, "only-left/a.txt", "a");
    write(&r, "only-right.txt", "r");
    (l, r)
}

fn open_pair(wb: &Entity<Workbench>, l: &Path, r: &Path, cx: &mut VisualTestContext) -> Entity<CompareView> {
    let (l, r) = (l.to_string_lossy().into_owned(), r.to_string_lossy().into_owned());
    wb.update_in(cx, |wb, window, cx| wb.open_folder_compare_for_test((true, &l), (true, &r), window, cx));
    let view = wb.read_with(cx, |wb, _| wb.compare_view_for_test()).expect("the compare tab is open");
    wait_secs(cx, 20, "the comparison finishes", |cx| !view.read_with(cx, |v, _| v.scanning()));
    view
}

fn statuses(view: &Entity<CompareView>, cx: &mut VisualTestContext, paths: &[&str]) -> Vec<Option<Status>> {
    view.read_with(cx, |v, _| paths.iter().map(|p| v.status_of(p)).collect())
}

fn with_trust(on: bool) -> Settings {
    Settings { workspace_trust: on, ..Settings::default() }
}

#[gpui::test]
fn two_local_folders_are_classified_through_the_agent(cx: &mut TestAppContext) {
    let (l, r) = pair("cmp-classify");
    write(&l, "node_modules/x/index.js", "skipped by default");
    write(&l, ".gitignore", "*.log\n");
    write(&r, ".gitignore", "*.log\n");
    write(&l, "noise.log", "ignored by .gitignore");
    let (wb, cx) = setup_with(cx, with_trust(false));
    let view = open_pair(&wb, &l, &r, cx);
    assert_eq!(
        statuses(&view, cx, &["same.txt", "edited.txt", "swapped.txt", "only-left", "only-left/a.txt", "only-right.txt", "src"]),
        [
            Some(Status::Identical),
            Some(Status::Changed(Why::Size)),
            Some(Status::Changed(Why::Content)),
            Some(Status::OnlyLeft),
            Some(Status::OnlyLeft),
            Some(Status::OnlyRight),
            Some(Status::Identical),
        ],
        "same size is told apart by hash, never by time"
    );
    assert_eq!(statuses(&view, cx, &["node_modules", "noise.log"]), [None, None], "the ignore setting and .gitignore are honoured");
    let counts = view.read_with(cx, |v, _| v.counts());
    assert_eq!((counts.only_left, counts.only_right, counts.changed, counts.identical), (1, 1, 2, 3));
    // The list hides identical rows and folded folders until asked.
    let shown = view.read_with(cx, |v, _| v.shown_paths());
    assert_eq!(shown, ["edited.txt", "only-left", "only-right.txt", "swapped.txt"], "{shown:?}");
    view.update(cx, |v, cx| v.set_options_for_test(true, false, false, cx));
    let all = view.read_with(cx, |v, _| v.shown_paths());
    assert!(all.contains(&"same.txt".to_string()) && !all.contains(&"only-left/a.txt".to_string()), "{all:?}");
    // Respecting .gitignore is a toggle: off, the log file shows up.
    view.update(cx, |v, cx| {
        v.gitignore = false;
        v.rescan(cx);
    });
    wait_secs(cx, 20, "the second scan", |cx| !view.read_with(cx, |v, _| v.scanning()));
    assert_eq!(statuses(&view, cx, &["noise.log"]), [Some(Status::OnlyLeft)]);
}

#[gpui::test]
fn line_ending_and_whitespace_toggles_reclassify_changed_files(cx: &mut TestAppContext) {
    let (l, r) = (fresh("cmp-eol-left"), fresh("cmp-eol-right"));
    write(&l, "crlf.txt", "a\r\nb\r\n");
    write(&r, "crlf.txt", "a\nb\n");
    write(&l, "spaces.txt", "x  =  1\n");
    write(&r, "spaces.txt", "x = 1\n");
    write(&l, "real.txt", "one\n");
    write(&r, "real.txt", "two\n");
    let (wb, cx) = setup_with(cx, with_trust(false));
    let view = open_pair(&wb, &l, &r, cx);
    let all = ["crlf.txt", "spaces.txt", "real.txt"];
    assert!(statuses(&view, cx, &all).iter().all(|s| matches!(s, Some(Status::Changed(_)))), "an EOL difference counts as changed by default");
    view.update(cx, |v, cx| v.set_options_for_test(false, false, true, cx));
    wait_secs(cx, 20, "line endings refined", |cx| !view.read_with(cx, |v, _| v.scanning()));
    assert_eq!(statuses(&view, cx, &all), [Some(Status::Equivalent), Some(Status::Changed(Why::Size)), Some(Status::Changed(Why::Content))]);
    view.update(cx, |v, cx| v.set_options_for_test(false, true, true, cx));
    wait_secs(cx, 20, "whitespace refined", |cx| !view.read_with(cx, |v, _| v.scanning()));
    assert_eq!(statuses(&view, cx, &all), [Some(Status::Equivalent), Some(Status::Equivalent), Some(Status::Changed(Why::Content))]);
    view.update(cx, |v, cx| v.set_options_for_test(false, false, false, cx));
    assert!(matches!(statuses(&view, cx, &all)[0], Some(Status::Changed(_))), "turning the toggles off restores the byte comparison");
}

#[gpui::test]
fn the_same_folder_on_both_sides_says_so(cx: &mut TestAppContext) {
    let (l, _) = pair("cmp-same");
    let (wb, cx) = setup_with(cx, with_trust(false));
    let view = open_pair(&wb, &l, &l, cx);
    assert!(view.read_with(cx, |v, _| v.is_same_folder() && v.rows().is_empty()));
}

#[gpui::test]
fn a_symlink_is_compared_by_its_target_and_not_followed(cx: &mut TestAppContext) {
    let (l, r) = (fresh("cmp-link-left"), fresh("cmp-link-right"));
    let outside = fresh("cmp-link-outside");
    write(&outside, "secret.txt", "not part of the comparison");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, l.join("out")).unwrap();
        std::os::unix::fs::symlink(&outside, r.join("out")).unwrap();
        std::os::unix::fs::symlink("a", l.join("moved")).unwrap();
        std::os::unix::fs::symlink("b", r.join("moved")).unwrap();
    }
    let (wb, cx) = setup_with(cx, with_trust(false));
    let view = open_pair(&wb, &l, &r, cx);
    #[cfg(unix)]
    {
        assert_eq!(statuses(&view, cx, &["out", "moved", "out/secret.txt"]), [Some(Status::Identical), Some(Status::Changed(Why::LinkTarget)), None]);
    }
}

#[gpui::test]
fn copying_asks_first_and_writes_only_what_the_scan_saw(cx: &mut TestAppContext) {
    let (l, r) = pair("cmp-copy");
    let (wb, cx) = setup_with(cx, with_trust(false));
    let view = open_pair(&wb, &l, &r, cx);
    view.update(cx, |v, cx| v.request_for_test("only-left", Direction::LeftToRight, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "a copy is confirmed first");
    let (title, detail) = cx.pending_prompt().unwrap();
    assert!(title.contains("only-left") && detail.contains("This computer"), "{title} / {detail}");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(!r.join("only-left").exists(), "declining copies nothing");

    // The destination changes after the scan: the copy of the edited file is refused, the new file still goes.
    view.update(cx, |v, cx| v.request_for_test("edited.txt", Direction::LeftToRight, cx));
    cx.run_until_parked();
    write(&r, "edited.txt", "edited by someone else\n");
    cx.simulate_prompt_answer("Copy");
    wait_ticking(cx, "the copy ended", |cx| notice(&wb, cx).contains("left alone"));
    assert_eq!(std::fs::read_to_string(r.join("edited.txt")).unwrap(), "edited by someone else\n");
    let message = notice(&wb, cx);
    assert!(message.contains("edited.txt") && message.contains("changed after the scan"), "{message}");

    // A fresh scan sees the new state; now the folder copy goes through.
    wait_secs(cx, 20, "rescan after the copy", |cx| !view.read_with(cx, |v, _| v.scanning() || v.is_busy()));
    view.update(cx, |v, cx| v.request_for_test("only-left", Direction::LeftToRight, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Copy");
    wait_ticking(cx, "folder copied", |_| r.join("only-left/a.txt").exists());
    assert_eq!(std::fs::read_to_string(r.join("only-left/a.txt")).unwrap(), "a");
    wait_secs(cx, 20, "rescan", |cx| !view.read_with(cx, |v, _| v.scanning() || v.is_busy()));
    assert_eq!(statuses(&view, cx, &["only-left/a.txt"]), [Some(Status::Identical)]);
}

#[gpui::test]
fn writes_are_refused_in_a_folder_that_is_not_trusted(cx: &mut TestAppContext) {
    let (l, r) = pair("cmp-trust");
    let (wb, cx) = setup_with(cx, with_trust(true));
    let view = open_pair(&wb, &l, &r, cx);
    view.update(cx, |v, cx| v.request_for_test("only-left", Direction::LeftToRight, cx));
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt(), "no prompt: the refusal comes first");
    assert!(notice(&wb, cx).contains("not a trusted folder"), "{}", notice(&wb, cx));
    assert!(!r.join("only-left").exists());
}

#[gpui::test]
fn delete_needs_a_writable_side_and_a_confirmation_that_names_the_host(cx: &mut TestAppContext) {
    let (l, r) = pair("cmp-delete");
    let (wb, cx) = setup_with(cx, with_trust(false));
    let view = open_pair(&wb, &l, &r, cx);
    wb.update_in(cx, |wb, window, cx| wb.delete_request_for_test(&view, "only-right.txt", Side::Right, window, cx));
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt() && r.join("only-right.txt").exists(), "a side that is not marked writable is never deleted from");
    view.update(cx, |v, _| v.writable[1] = true);
    wb.update_in(cx, |wb, window, cx| wb.delete_request_for_test(&view, "only-right.txt", Side::Right, window, cx));
    wait_secs(cx, 20, "the confirmation", |cx| cx.has_pending_prompt());
    let (title, detail) = cx.pending_prompt().unwrap();
    assert!(title.contains("only-right.txt") && title.contains("This computer") && detail.contains("deleted for good"), "{title} / {detail}");
    // The file changes while the question is open: the delete is refused.
    write(&r, "only-right.txt", "changed while the prompt was open");
    cx.simulate_prompt_answer("Delete");
    wait_ticking(cx, "delete refused", |cx| notice(&wb, cx).starts_with("Not deleted"));
    assert!(r.join("only-right.txt").exists());
}

#[gpui::test]
fn a_changed_text_file_opens_in_the_diff_view(cx: &mut TestAppContext) {
    let (l, r) = pair("cmp-diff");
    let (wb, cx) = setup_with(cx, with_trust(false));
    let view = open_pair(&wb, &l, &r, cx);
    view.update(cx, |v, cx| v.open_for_test("edited.txt", cx));
    wait_ticking(cx, "diff open", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    let diff = wb.read_with(cx, |wb, _| wb.diff_view()).unwrap();
    assert_eq!(diff.read_with(cx, |d, _| d.summary()), (1, 0, 1), "one line added on the right");
}

#[gpui::test]
fn stopping_a_scan_keeps_it_from_producing_more(cx: &mut TestAppContext) {
    let (l, r) = (fresh("cmp-stop-left"), fresh("cmp-stop-right"));
    for i in 0..300 {
        write(&l, &format!("d{}/f{i}.txt", i % 10), "x");
    }
    let (wb, cx) = setup_with(cx, with_trust(false));
    let (ls, rs) = (l.to_string_lossy().into_owned(), r.to_string_lossy().into_owned());
    wb.update_in(cx, |wb, window, cx| wb.open_folder_compare_for_test((true, &ls), (true, &rs), window, cx));
    let view = wb.read_with(cx, |wb, _| wb.compare_view_for_test()).unwrap();
    view.update(cx, |v, _| v.stop());
    assert!(!view.read_with(cx, |v, _| v.scanning()));
    let before = view.read_with(cx, |v, _| v.rows().len());
    std::thread::sleep(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.rows().len()), before, "nothing arrives after Stop");
    assert!(!view.read_with(cx, |v, _| v.scanning()));
}

/// This computer against an SSH host (the test sshd serves this machine, so the "remote" folder is a local temp folder
/// reached through the real agent): the scan, the diff-by-hash and a copy upload.
#[gpui::test]
fn a_local_folder_is_compared_with_one_on_the_ssh_host(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("cmp-ssh") else { return };
    let (local, remote) = pair("cmp-ssh");
    let (wb, cx) = setup_with(cx, with_trust(false));
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    wait_ticking(cx, "connected", |cx| wb.read_with(cx, |wb, _| wb.ports_ready()));
    let (ls, rs) = (local.to_string_lossy().into_owned(), remote.to_string_lossy().into_owned());
    wb.update_in(cx, |wb, window, cx| wb.open_folder_compare_for_test((true, &ls), (false, &rs), window, cx));
    let view = wb.read_with(cx, |wb, _| wb.compare_view_for_test()).unwrap();
    wait_secs(cx, 30, "the comparison finishes", |cx| !view.read_with(cx, |v, _| v.scanning()));
    assert!(!view.read_with(cx, |v, _| v.is_partial()));
    assert_eq!(
        statuses(&view, cx, &["same.txt", "edited.txt", "swapped.txt", "only-left", "only-right.txt"]),
        [Some(Status::Identical), Some(Status::Changed(Why::Size)), Some(Status::Changed(Why::Content)), Some(Status::OnlyLeft), Some(Status::OnlyRight)]
    );
    let (left, right) = view.read_with(cx, |v, _| (v.end(Side::Left).label.clone(), v.end(Side::Right).label.clone()));
    assert_eq!(left, "This computer");
    assert!(right.contains("ssh") || right.contains(crate::test_sshd::ALIAS), "{right}");
    // Copy a new file from this computer to the host.
    view.update(cx, |v, cx| v.request_for_test("only-left", Direction::LeftToRight, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Copy");
    wait_ticking(cx, "uploaded", |_| remote.join("only-left/a.txt").exists());
    assert_eq!(std::fs::read_to_string(remote.join("only-left/a.txt")).unwrap(), "a");
}
