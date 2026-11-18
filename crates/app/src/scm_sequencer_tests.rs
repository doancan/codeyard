//! A cherry-pick or revert that stopped on a conflict can be continued or aborted from Source Control (#308), through the palette
//! commands the banner's links dispatch. Before, the banner named it and offered no way out.

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::qa_integration_tests::{git_in, repo, wait_ticking};
use crate::qa_workbench_tests::{open, setup};
use crate::workbench::Workbench;

/// A repository in the middle of a cherry-pick of a commit that conflicts with `f.txt`.
fn waiting<'a>(name: &str, cx: &'a mut TestAppContext) -> (std::path::PathBuf, Entity<Workbench>, &'a mut VisualTestContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = repo(name);
    let base = git_in(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    git_in(&dir, &["checkout", "-q", "-b", "other", &base]);
    std::fs::write(dir.join("f.txt"), "theirs\n").unwrap();
    git_in(&dir, &["add", "f.txt"]);
    git_in(&dir, &["commit", "-q", "-m", "theirs"]);
    let theirs = git_in(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    git_in(&dir, &["checkout", "-q", "-"]);
    std::fs::write(dir.join("f.txt"), "ours\n").unwrap();
    git_in(&dir, &["add", "f.txt"]);
    git_in(&dir, &["commit", "-q", "-m", "ours"]);
    let stopped = crate::test_git::git().args(["cherry-pick", &theirs]).current_dir(&dir).output().unwrap();
    assert!(!stopped.status.success());
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "the operation is read", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|s| s.op == crate::repos::OpState::CherryPicking)));
    (dir, wb, cx)
}

fn command(wb: &Entity<Workbench>, cx: &mut VisualTestContext, name: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(name, window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn abort_asks_and_then_puts_the_files_back(cx: &mut TestAppContext) {
    let (dir, wb, cx) = waiting("seq-ui-abort", cx);
    command(&wb, cx, "Git: Abort Cherry-pick or Revert");
    assert!(cx.has_pending_prompt(), "aborting discards what was resolved: it asks");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(dir.join(".git/CHERRY_PICK_HEAD").exists(), "Cancel leaves it waiting");
    command(&wb, cx, "Git: Abort Cherry-pick or Revert");
    cx.simulate_prompt_answer("Abort");
    wait_ticking(cx, "the abort", |_| !dir.join(".git/CHERRY_PICK_HEAD").exists());
    assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "ours\n");
}

#[gpui::test]
fn continue_says_what_is_missing_and_then_finishes_once_resolved(cx: &mut TestAppContext) {
    let (dir, wb, cx) = waiting("seq-ui-continue", cx);
    command(&wb, cx, "Git: Continue Cherry-pick or Revert");
    wait_ticking(cx, "the answer", |cx| wb.read_with(cx, |wb, _| wb.notice_for_test().is_some_and(|(error, _)| error)));
    let (_, said) = wb.read_with(cx, |wb, _| wb.notice_for_test()).unwrap();
    assert!(said.contains("conflicts"), "{said}");
    assert!(dir.join(".git/CHERRY_PICK_HEAD").exists());
    std::fs::write(dir.join("f.txt"), "resolved\n").unwrap();
    git_in(&dir, &["add", "f.txt"]);
    command(&wb, cx, "Git: Continue Cherry-pick or Revert");
    wait_ticking(cx, "the commit", |_| !dir.join(".git/CHERRY_PICK_HEAD").exists());
    assert_eq!(git_in(&dir, &["log", "-1", "--format=%s"]).trim(), "theirs");
}

#[gpui::test]
fn without_an_operation_the_commands_say_so(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = repo("seq-ui-idle");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some()));
    command(&wb, cx, "Git: Abort Cherry-pick or Revert");
    assert!(!cx.has_pending_prompt(), "nothing to abort, nothing to ask");
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_for_test()), Some((false, "No cherry-pick or revert is in progress.".into())));
}
