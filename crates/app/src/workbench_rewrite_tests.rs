//! Squash and reorder from the graph, end to end in a window (#270): the result of each, the refusals as they reach the user,
//! and that the old selection goes away because its hashes do.

use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::qa_integration_tests::{git_in, repo, repo_with_remote, wait_ticking};
use crate::qa_workbench_tests::{open, setup};

struct Fixture<'a> {
    dir: std::path::PathBuf,
    wb: Entity<Workbench>,
    cx: &'a mut VisualTestContext,
}

fn started<'a>(dir: std::path::PathBuf, files: &[(&str, &str)], cx: &'a mut TestAppContext) -> Fixture<'a> {
    let _en = crate::i18n::LangGuard::set("en");
    for (name, subject) in files {
        std::fs::write(dir.join(name), name).unwrap();
        git_in(&dir, &["add", name]);
        git_in(&dir, &["commit", "-q", "-m", subject]);
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let tip = git_in(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "the graph", |cx| scm.read_with(cx, |s, _| s.commit_by_hash(&tip).is_some()));
    Fixture { dir, wb, cx }
}

impl Fixture<'_> {
    fn hash(&self, rev: &str) -> String {
        git_in(&self.dir, &["rev-parse", rev]).trim().to_string()
    }

    fn subjects(&self) -> Vec<String> {
        git_in(&self.dir, &["log", "--format=%s"]).lines().map(str::to_string).collect()
    }

    /// What the menu does first: remember the repository it was opened on.
    fn open_menu_repo(&mut self) {
        self.wb.update(self.cx, |wb, cx| wb.vcs.menu_repo = wb.scm.read(cx).history_target());
    }

    fn result(&mut self) -> String {
        let scm = self.wb.read_with(self.cx, |wb, _| wb.scm_panel());
        wait_ticking(self.cx, "the result", |cx| scm.read_with(cx, |s, _| !s.is_busy() && s.result_text().is_some()));
        scm.read_with(self.cx, |s, _| s.result_text()).unwrap_or_default()
    }

    fn pick_run(&mut self, from: &str, to: &str) {
        let scm = self.wb.read_with(self.cx, |wb, _| wb.scm_panel());
        let (a, b) = (scm.read_with(self.cx, |s, _| s.commit_by_hash(from)).unwrap(), scm.read_with(self.cx, |s, _| s.commit_by_hash(to)).unwrap());
        scm.update(self.cx, |s, cx| {
            s.pick_commit(a, false, cx);
            s.pick_commit(b, true, cx)
        });
    }
}

#[gpui::test]
fn squashing_the_picked_run_makes_one_commit_with_the_subject_and_the_other_messages(cx: &mut TestAppContext) {
    let mut f = started(repo("rewrite-ui-squash"), &[("a.txt", "wip a"), ("b.txt", "wip b"), ("c.txt", "wip c")], cx);
    let (a, c) = (f.hash("HEAD~2"), f.hash("HEAD"));
    let tree = git_in(&f.dir, &["rev-parse", "HEAD^{tree}"]);
    f.pick_run(&a, &c);
    f.open_menu_repo();
    let (run, tip, repo) = f.wb.read_with(f.cx, |wb, cx| (wb.scm.read(cx).range_run(), wb.scm.read(cx).head_hash().unwrap(), wb.vcs.menu_repo.clone()));
    assert_eq!(run.len(), 3);
    f.wb.update(f.cx, |wb, cx| wb.finish_squash(run, tip, repo, "Add a, b and c\n\nwip b\n\nwip c".into(), cx));
    let result = f.result();
    assert!(result.starts_with("Squashed 3 commits into "), "{result}");
    assert_eq!(f.subjects(), vec!["Add a, b and c", "ilk"]);
    assert_eq!(git_in(&f.dir, &["log", "-1", "--format=%b"]).trim(), "wip b\n\nwip c");
    assert_eq!(git_in(&f.dir, &["rev-parse", "HEAD^{tree}"]), tree);
    let scm = f.wb.read_with(f.cx, |wb, _| wb.scm_panel());
    assert!(scm.read_with(f.cx, |s, _| s.selected_hash().is_none() && s.range_run().is_empty()), "the old hashes are gone from the selection");
}

#[gpui::test]
fn moving_a_commit_up_and_down_swaps_it_with_its_neighbour(cx: &mut TestAppContext) {
    let mut f = started(repo("rewrite-ui-move"), &[("a.txt", "first"), ("b.txt", "second")], cx);
    let first = f.hash("HEAD~1");
    f.open_menu_repo();
    f.wb.update(f.cx, |wb, cx| wb.move_commit(first, false, cx));
    assert!(f.result().starts_with("Moved "));
    assert_eq!(f.subjects(), vec!["first", "second", "ilk"]);
}

#[gpui::test]
fn a_pushed_commit_in_the_run_is_refused_with_its_reason_and_nothing_changes(cx: &mut TestAppContext) {
    let (dir, _bare, _other) = repo_with_remote("rewrite-ui-pushed");
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    git_in(&dir, &["add", "a.txt"]);
    git_in(&dir, &["commit", "-q", "-m", "pushed"]);
    git_in(&dir, &["push", "-q"]);
    let mut f = started(dir, &[("b.txt", "local")], cx);
    let before = f.hash("HEAD");
    let (a, b) = (f.hash("HEAD~1"), f.hash("HEAD"));
    f.pick_run(&a, &b);
    f.open_menu_repo();
    let (run, tip, repo) = f.wb.read_with(f.cx, |wb, cx| (wb.scm.read(cx).range_run(), wb.scm.read(cx).head_hash().unwrap(), wb.vcs.menu_repo.clone()));
    f.wb.update(f.cx, |wb, cx| wb.finish_squash(run, tip, repo, "x".into(), cx));
    assert_eq!(f.result(), "Only commits that are not pushed can be squashed");
    assert_eq!(f.hash("HEAD"), before);
}

#[gpui::test]
fn an_empty_message_is_refused_before_anything_runs(cx: &mut TestAppContext) {
    let mut f = started(repo("rewrite-ui-empty"), &[("a.txt", "a"), ("b.txt", "b")], cx);
    let before = f.hash("HEAD");
    f.open_menu_repo();
    let repo = f.wb.read_with(f.cx, |wb, _| wb.vcs.menu_repo.clone());
    let run = vec![f.hash("HEAD~1"), before.clone()];
    f.wb.update(f.cx, |wb, cx| wb.finish_squash(run, before.clone(), repo, "   ".into(), cx));
    f.cx.run_until_parked();
    assert_eq!(f.hash("HEAD"), before);
    let notice = f.wb.read_with(f.cx, |wb, _| wb.notice_for_test());
    assert!(notice.is_some_and(|(error, text)| error && text.contains("cannot be empty")));
}

/// The same squash on a repository opened over SSH, against the throwaway `sshd`: every git command of the rewrite, the temporary
/// worktree included, runs on the host through the agent.
#[gpui::test]
fn squashing_works_over_ssh_and_leaves_no_temporary_worktree(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("rewrite-ssh") else { return };
    let _en = crate::i18n::LangGuard::set("en");
    let dir = repo("rewrite-ui-ssh");
    for (name, subject) in [("a.txt", "wip a"), ("b.txt", "wip b")] {
        std::fs::write(dir.join(name), name).unwrap();
        git_in(&dir, &["add", name]);
        git_in(&dir, &["commit", "-q", "-m", subject]);
    }
    let (wb, cx) = setup(cx);
    let wait = |cx: &mut VisualTestContext, what: &str, mut done: Box<dyn FnMut(&mut VisualTestContext) -> bool>| {
        crate::qa_integration_tests::wait_secs(cx, 30, what, |cx| {
            cx.executor().advance_clock(std::time::Duration::from_millis(300));
            done(cx)
        })
    };
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    let probe = wb.clone();
    wait(cx, "connected", Box::new(move |cx| probe.read_with(cx, |wb, _| wb.ports_ready())));
    wb.update_in(cx, |wb, window, cx| wb.open_path(&crate::ui::local_path(&dir), window, cx));
    let tip = git_in(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    let (probe, tip_hash) = (scm.clone(), tip.clone());
    wait(cx, "the graph over SSH", Box::new(move |cx| probe.read_with(cx, |s, _| s.commit_by_hash(&tip_hash).is_some())));
    assert_eq!(wb.read_with(cx, |wb, _| wb.remote_host_label_for_test()), Some(format!("ssh://{}", crate::test_sshd::ALIAS)));
    let (a, b) = (git_in(&dir, &["rev-parse", "HEAD~1"]).trim().to_string(), tip.clone());
    scm.update(cx, |s, cx| {
        s.pick_commit(s.commit_by_hash(&a).unwrap(), false, cx);
        s.pick_commit(s.commit_by_hash(&b).unwrap(), true, cx)
    });
    wb.update(cx, |wb, cx| wb.vcs.menu_repo = wb.scm.read(cx).history_target());
    // Through the card, as the user does: the messages arrive over SSH, the edited text is the new message.
    wb.update_in(cx, |wb, window, cx| wb.squash_commits(window, cx));
    let probe = wb.clone();
    wait(cx, "the card", Box::new(move |cx| probe.read_with(cx, |wb, _| wb.message_dialog_field_for_test().is_some())));
    let field = wb.read_with(cx, |wb, _| wb.message_dialog_field_for_test()).unwrap();
    assert_eq!(field.read_with(cx, |t, _| t.text().to_string()), "wip a\n\nwip b");
    field.update(cx, |t, cx| t.set_text("Both\n\nTwo paragraphs\n\nover SSH", cx));
    cx.simulate_keystrokes("cmd-enter");
    let dir_for_probe = dir.clone();
    wait(cx, "the squash", Box::new(move |_| git_in(&dir_for_probe, &["log", "--format=%s"]).lines().next() == Some("Both")));
    assert_eq!(git_in(&dir, &["log", "--format=%s"]).lines().collect::<Vec<_>>(), vec!["Both", "ilk"]);
    assert_eq!(git_in(&dir, &["log", "-1", "--format=%B"]).trim_end(), "Both\n\nTwo paragraphs\n\nover SSH");
    assert_eq!(git_in(&dir, &["worktree", "list"]).lines().count(), 1, "the temporary worktree is gone");
}

/// What the user does: right-click menu → *Squash N Commits…* opens the card with the commits' messages in order; the user edits
/// the whole text (several paragraphs), ⌘↵ squashes, and the message of the new commit is that text, nothing more or less (#324).
impl Fixture<'_> {
    fn open_card(&mut self, from: &str, to: &str) -> Entity<crate::input::TextField> {
        self.pick_run(from, to);
        self.open_menu_repo();
        self.wb.update_in(self.cx, |wb, window, cx| wb.squash_commits(window, cx));
        let wb = self.wb.clone();
        wait_ticking(self.cx, "the card", move |cx| wb.read_with(cx, |wb, _| wb.message_dialog_field_for_test().is_some()));
        self.wb.read_with(self.cx, |wb, _| wb.message_dialog_field_for_test()).unwrap()
    }
}

#[gpui::test]
fn the_card_holds_every_message_in_order_and_the_edited_text_becomes_the_commit_message(cx: &mut TestAppContext) {
    let mut f = started(repo("rewrite-ui-card"), &[("a.txt", "wip a"), ("b.txt", "wip b"), ("c.txt", "wip c")], cx);
    let (a, c) = (f.hash("HEAD~2"), f.hash("HEAD"));
    let field = f.open_card(&a, &c);
    assert_eq!(field.read_with(f.cx, |t, _| t.text().to_string()), "wip a\n\nwip b\n\nwip c", "every message, oldest first");
    // The user rewrites all of it: a new subject, two paragraphs, a line with Unicode, and a blank line between them.
    field.update(f.cx, |t, cx| t.set_text("Add the three files\n\nFirst paragraph.\nSecond line of it.\n\nİkinci paragraf: çalışıyor ✓", cx));
    f.cx.simulate_keystrokes("cmd-enter");
    let result = f.result();
    assert!(result.starts_with("Squashed 3 commits into "), "{result}");
    assert_eq!(f.subjects(), vec!["Add the three files", "ilk"]);
    assert_eq!(
        git_in(&f.dir, &["log", "-1", "--format=%B"]).trim_end(),
        "Add the three files\n\nFirst paragraph.\nSecond line of it.\n\nİkinci paragraf: çalışıyor ✓"
    );
    assert!(f.wb.read_with(f.cx, |wb, _| wb.message_dialog_field_for_test().is_none()), "the card is closed");
}

#[gpui::test]
fn return_is_a_line_break_in_the_card_and_does_not_squash(cx: &mut TestAppContext) {
    let mut f = started(repo("rewrite-ui-return"), &[("a.txt", "wip a"), ("b.txt", "wip b")], cx);
    let before = f.hash("HEAD");
    let (a, b) = (f.hash("HEAD~1"), before.clone());
    let field = f.open_card(&a, &b);
    field.update(f.cx, |t, cx| t.set_text("one", cx));
    f.cx.simulate_keystrokes("end enter");
    f.cx.simulate_input("two");
    f.cx.run_until_parked();
    assert_eq!(f.wb.read_with(f.cx, |wb, cx| wb.message_dialog_text_for_test(cx)), Some("one\ntwo".to_string()));
    assert_eq!(f.hash("HEAD"), before, "Return squashed nothing");
}

#[gpui::test]
fn escape_closes_the_card_and_changes_nothing(cx: &mut TestAppContext) {
    let mut f = started(repo("rewrite-ui-escape"), &[("a.txt", "wip a"), ("b.txt", "wip b")], cx);
    let before = f.hash("HEAD");
    let (a, b) = (f.hash("HEAD~1"), before.clone());
    let _card = f.open_card(&a, &b);
    f.cx.simulate_keystrokes("escape");
    f.cx.run_until_parked();
    assert!(f.wb.read_with(f.cx, |wb, _| wb.message_dialog_field_for_test().is_none()));
    assert_eq!(f.hash("HEAD"), before);
    assert_eq!(f.subjects(), vec!["wip b", "wip a", "ilk"]);
}

#[gpui::test]
fn an_empty_message_keeps_the_card_open_with_what_was_typed(cx: &mut TestAppContext) {
    let mut f = started(repo("rewrite-ui-card-empty"), &[("a.txt", "wip a"), ("b.txt", "wip b")], cx);
    let before = f.hash("HEAD");
    let (a, b) = (f.hash("HEAD~1"), before.clone());
    let field = f.open_card(&a, &b);
    field.update(f.cx, |t, cx| t.set_text("  \n ", cx));
    f.cx.simulate_keystrokes("cmd-enter");
    f.cx.run_until_parked();
    assert!(f.wb.read_with(f.cx, |wb, _| wb.message_dialog_field_for_test().is_some()), "still open");
    assert_eq!(f.hash("HEAD"), before);
    let notice = f.wb.read_with(f.cx, |wb, _| wb.notice_for_test());
    assert!(notice.is_some_and(|(error, text)| error && text.contains("cannot be empty")));
}
