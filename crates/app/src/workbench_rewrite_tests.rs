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

#[test]
fn the_subject_is_the_first_line_and_the_body_is_the_rest() {
    assert_eq!(split_message("wip a\n\nwip b\n\nlonger body"), ("wip a".to_string(), "wip b\n\nlonger body".to_string()));
    assert_eq!(split_message("only one"), ("only one".to_string(), String::new()));
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
    f.wb.update(f.cx, |wb, cx| wb.finish_squash(run, tip, "wip b\n\nwip c".into(), repo, "Add a, b and c".into(), cx));
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
    f.wb.update(f.cx, |wb, cx| wb.finish_squash(run, tip, String::new(), repo, "x".into(), cx));
    assert_eq!(f.result(), "Only commits that are not pushed can be squashed");
    assert_eq!(f.hash("HEAD"), before);
}

#[gpui::test]
fn an_empty_subject_is_refused_before_anything_runs(cx: &mut TestAppContext) {
    let mut f = started(repo("rewrite-ui-empty"), &[("a.txt", "a"), ("b.txt", "b")], cx);
    let before = f.hash("HEAD");
    f.open_menu_repo();
    let repo = f.wb.read_with(f.cx, |wb, _| wb.vcs.menu_repo.clone());
    let run = vec![f.hash("HEAD~1"), before.clone()];
    f.wb.update(f.cx, |wb, cx| wb.finish_squash(run, before.clone(), String::new(), repo, "   ".into(), cx));
    f.cx.run_until_parked();
    assert_eq!(f.hash("HEAD"), before);
    let notice = f.wb.read_with(f.cx, |wb, _| wb.notice_for_test());
    assert!(notice.is_some_and(|(error, text)| error && text.contains("cannot be empty")));
}
