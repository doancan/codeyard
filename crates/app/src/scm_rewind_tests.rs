//! Undo Last Commit and the graph's resets count what they take off the branch first: a question when commits are
//! already on the upstream, a hard reset that says what exists nowhere else, and an answer that applies only to the
//! `HEAD` it was about. End to end in a repository with a remote.

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::graph::{CommitOp, ResetMode};
use crate::qa_integration_tests::{git_in, repo_with_remote, wait_ticking};
use crate::qa_workbench_tests::{open, setup};
use crate::scm::ScmPanel;
use crate::workbench::Workbench;

fn head(dir: &std::path::Path) -> String {
    git_in(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

fn commit(dir: &std::path::Path, subject: &str) -> String {
    git_in(dir, &["commit", "-q", "--allow-empty", "-m", subject]);
    head(dir)
}

struct Fixture<'a> {
    dir: std::path::PathBuf,
    wb: Entity<Workbench>,
    scm: Entity<ScmPanel>,
    cx: &'a mut VisualTestContext,
}

/// A repository whose `main` tracks `origin/main`, with `pushed` commits pushed and `local` ones not, open in Source Control.
fn started<'a>(name: &str, pushed: &[&str], local: &[&str], cx: &'a mut TestAppContext) -> Fixture<'a> {
    let _en = crate::i18n::LangGuard::set("en");
    let (dir, _bare, _other) = repo_with_remote(name);
    for s in pushed {
        commit(&dir, s);
    }
    if !pushed.is_empty() {
        git_in(&dir, &["push", "-q"]);
    }
    for s in local {
        commit(&dir, s);
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "status", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some()));
    Fixture { dir, wb, scm, cx }
}

impl Fixture<'_> {
    fn undo(&mut self) {
        self.wb.update_in(self.cx, |wb, window, cx| wb.run_command_for_test("Git: Undo Last Commit", window, cx));
        self.cx.run_until_parked();
    }

    fn reset(&mut self, to: &str, mode: ResetMode) {
        let to = to.to_string();
        self.wb.update_in(self.cx, |wb, window, cx| wb.scm_panel().update(cx, |s, cx| s.commit_op(to, CommitOp::Reset(mode), window, cx)));
        self.cx.run_until_parked();
    }

    /// The panel's result once nothing runs.
    fn result(&mut self) -> String {
        let scm = self.scm.clone();
        wait_ticking(self.cx, "the result", |cx| scm.read_with(cx, |s, _| !s.is_busy() && s.result_text().is_some()));
        self.scm.read_with(self.cx, |s, _| s.result_text()).unwrap_or_default()
    }
}

/// AC1: a pushed last commit is undone only after a question; Cancel changes nothing.
#[gpui::test]
fn undoing_a_pushed_commit_asks_first(cx: &mut TestAppContext) {
    let mut f = started("rewind-undo-pushed", &["shared"], &[], cx);
    let before = head(&f.dir);
    f.undo();
    assert!(f.cx.has_pending_prompt(), "the question");
    f.cx.simulate_prompt_answer("Cancel");
    f.cx.run_until_parked();
    assert_eq!(head(&f.dir), before, "Cancel undoes nothing");
    f.undo();
    f.cx.simulate_prompt_answer("Undo Commit");
    let result = f.result();
    assert!(result.starts_with("Undid commit “shared” (the branch was at "), "{result}");
    assert_ne!(head(&f.dir), before);
}

/// AC2: an unpushed commit is undone at once, and the result names the old tip.
#[gpui::test]
fn undoing_a_local_commit_needs_no_question(cx: &mut TestAppContext) {
    let mut f = started("rewind-undo-local", &[], &["mine"], cx);
    let short = git_in(&f.dir, &["rev-parse", "--short", "HEAD"]).trim().to_string();
    f.undo();
    assert!(!f.cx.has_pending_prompt());
    assert_eq!(f.result(), format!("Undid commit “mine” (the branch was at {short})"));
}

/// AC4: a soft reset across pushed commits asks; one across local commits only does not.
#[gpui::test]
fn a_soft_reset_asks_only_across_pushed_commits(cx: &mut TestAppContext) {
    let mut f = started("rewind-soft", &["shared"], &["mine"], cx);
    let base = git_in(&f.dir, &["rev-parse", "HEAD~2"]).trim().to_string();
    let middle = git_in(&f.dir, &["rev-parse", "HEAD~1"]).trim().to_string();
    // To the pushed commit: only "mine" leaves, it is local.
    f.reset(&middle, ResetMode::Soft);
    assert!(!f.cx.has_pending_prompt());
    assert!(f.result().starts_with("Reset to "), "done at once");
    assert_eq!(head(&f.dir), middle);
    // Past the pushed commit: asked.
    f.reset(&base, ResetMode::Mixed);
    assert!(f.cx.has_pending_prompt());
    f.cx.simulate_prompt_answer("Reset");
    let result = f.result();
    assert!(result.contains("; the branch was at "), "{result}");
    assert_eq!(head(&f.dir), base);
}

/// AC3: a hard reset always asks; confirmed, it resets.
#[gpui::test]
fn a_hard_reset_asks_and_names_the_old_tip(cx: &mut TestAppContext) {
    let mut f = started("rewind-hard", &[], &["one", "two"], cx);
    let base = git_in(&f.dir, &["rev-parse", "HEAD~2"]).trim().to_string();
    f.reset(&base, ResetMode::Hard);
    assert!(f.cx.has_pending_prompt());
    f.cx.simulate_prompt_answer("Reset");
    assert!(f.result().contains("(hard); the branch was at "));
    assert_eq!(head(&f.dir), base);
}

/// AC5: the branch moves while the question is open: the answer changes nothing.
#[gpui::test]
fn an_answer_applies_only_to_the_head_it_was_about(cx: &mut TestAppContext) {
    let mut f = started("rewind-moved", &["shared"], &[], cx);
    f.undo();
    assert!(f.cx.has_pending_prompt());
    let moved = commit(&f.dir, "from a terminal");
    f.cx.simulate_prompt_answer("Undo Commit");
    assert_eq!(f.result(), crate::git::moved_message());
    assert_eq!(head(&f.dir), moved, "nothing undone");
}

/// Epic #267, exit criteria 2 and 3 against a real SSH server (the throwaway `sshd`): what a pushed undo or a hard reset takes off the
/// branch is counted on the host, and both ask before they run.
#[gpui::test]
fn criteria_2_and_3_hold_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("rewind-ssh") else { return };
    let _en = crate::i18n::LangGuard::set("en");
    let (dir, _bare, _other) = repo_with_remote("rewind-ssh");
    commit(&dir, "shared");
    git_in(&dir, &["push", "-q"]);
    commit(&dir, "mine");
    let base = git_in(&dir, &["rev-parse", "HEAD~2"]).trim().to_string();
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 30, "connected", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    wb.update_in(cx, |wb, window, cx| wb.open_path(&crate::ui::local_path(&dir), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 30, "status over SSH", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        wb.read_with(cx, |wb, _| wb.git_status().is_some())
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.remote_host_label_for_test()), Some(format!("ssh://{}", crate::test_sshd::ALIAS)));
    // Counted on the host: two commits leave, one of them is on the upstream, and both are on no other branch.
    let host = wb.read_with(cx, |wb, _| wb.remote_host_for_test());
    let leaving = crate::git::leaving(host.as_ref(), &crate::ui::local_path(&dir), &base).unwrap();
    assert_eq!((leaving.count, leaving.pushed), (2, 1));
    assert_eq!(leaving.upstream.as_deref(), Some("origin/main"));
    // Criterion 3: a hard reset asks, and Cancel changes nothing.
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    let before = head(&dir);
    wb.update_in(cx, |wb, window, cx| wb.scm_panel().update(cx, |s, cx| s.commit_op(base.clone(), CommitOp::Reset(ResetMode::Hard), window, cx)));
    crate::qa_integration_tests::wait_secs(cx, 30, "the question", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        cx.has_pending_prompt()
    });
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(head(&dir), before, "Cancel resets nothing");
    // Criterion 2: undoing a commit that is on the upstream asks before it does anything, and the answer decides.
    git_in(&dir, &["push", "-q"]);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Undo Last Commit", window, cx));
    crate::qa_integration_tests::wait_secs(cx, 30, "the question", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        cx.has_pending_prompt()
    });
    assert_eq!(head(&dir), before, "nothing is undone before the answer");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(head(&dir), before, "Cancel undoes nothing");
    let _ = scm;
}
