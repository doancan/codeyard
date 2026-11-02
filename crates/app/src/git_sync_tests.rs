//! Sync Changes through the workbench: the button's states, the confirmation, the guard against two syncs at once, pull
//! conflicts, commit variants, post-commit commands and the background fetch. The remote is a real bare repository.

use std::path::{Path, PathBuf};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::qa_integration_tests::{git_in, repo_with_remote, setup_with, wait_ticking};
use crate::qa_workbench_tests::open;
use crate::settings::Settings;
use crate::workbench::Workbench;

/// `settings` over the defaults, with the confirmation off unless a test is about it.
fn quiet() -> Settings {
    Settings { git_confirm_sync: false, git_autofetch: false, ..Settings::default() }
}

struct Fixture {
    dir: PathBuf,
    bare: PathBuf,
    other: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let (dir, bare, other) = repo_with_remote(name);
        let dir = crate::ui::canonical_for_test(dir);
        Fixture { dir, bare, other }
    }

    /// Someone else commits `file` and pushes.
    fn push_elsewhere(&self, file: &str, text: &str) {
        git_in(&self.other, &["pull", "-q", "--rebase"]);
        std::fs::write(self.other.join(file), text).unwrap();
        git_in(&self.other, &["add", "-A"]);
        git_in(&self.other, &["commit", "-q", "-m", &format!("remote {file}")]);
        git_in(&self.other, &["push", "-q"]);
    }

    fn commit_here(&self, file: &str, text: &str) {
        std::fs::write(self.dir.join(file), text).unwrap();
        git_in(&self.dir, &["add", "-A"]);
        git_in(&self.dir, &["commit", "-q", "-m", &format!("local {file}")]);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for p in [&self.dir, &self.bare, &self.other] {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

fn status(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<std::sync::Arc<crate::git::Status>> {
    wb.read_with(cx, |wb, _| wb.git_status())
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default()
}

fn label(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<String> {
    wb.read_with(cx, |wb, _| wb.scm_panel()).read_with(cx, |s, _| s.sync_label_for_test())
}

fn refresh(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
}

fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(command, window, cx));
}

fn opened<'a>(cx: &'a mut TestAppContext, f: &Fixture, settings: Settings) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    let (wb, cx) = setup_with(cx, settings);
    open(&wb, &f.dir, cx);
    wait_ticking(cx, "git status", |cx| status(&wb, cx).is_some());
    (wb, cx)
}

fn head_subject(dir: &Path) -> String {
    git_in(dir, &["log", "-1", "--format=%s"]).trim().to_string()
}

#[gpui::test]
fn the_sync_button_follows_ahead_behind_and_publish(cx: &mut TestAppContext) {
    let f = Fixture::new("sync-button");
    let (wb, cx) = opened(cx, &f, quiet());
    wait_ticking(cx, "the panel reads the remotes", |cx| wb.read_with(cx, |wb, _| wb.scm_panel()).read_with(cx, |s, _| s.has_remote_for_test()));
    assert_eq!(label(&wb, cx), None, "in step with the upstream: no button");

    f.commit_here("a.txt", "a\n");
    refresh(&wb, cx);
    wait_ticking(cx, "ahead 1", |cx| label(&wb, cx).is_some());
    assert_eq!(label(&wb, cx).as_deref(), Some("Sync Changes ↑1"));

    f.push_elsewhere("b.txt", "b\n");
    git_in(&f.dir, &["fetch", "-q"]);
    refresh(&wb, cx);
    wait_ticking(cx, "ahead 1 behind 1", |cx| label(&wb, cx).as_deref() == Some("Sync Changes ↓1 ↑1"));

    // A branch without an upstream offers to publish.
    git_in(&f.dir, &["checkout", "-q", "-b", "feature"]);
    refresh(&wb, cx);
    wait_ticking(cx, "publish", |cx| label(&wb, cx).as_deref() == Some("Publish Branch"));
    run(&wb, cx, "Git: Sync");
    wait_ticking(cx, "published", |cx| notice(&wb, cx) == "Published to origin" && status(&wb, cx).is_some_and(|s| s.upstream.is_some()));
    assert!(!git_in(&f.bare, &["branch", "--list", "feature"]).trim().is_empty(), "the branch is on the remote");
    wait_ticking(cx, "button gone", |cx| label(&wb, cx).is_none());
}

#[gpui::test]
fn sync_asks_first_when_it_moves_commits_and_skips_the_question_when_off(cx: &mut TestAppContext) {
    // Off by default: Sync pulls and pushes at once. This test turns the question on.
    assert!(!Settings::default().git_confirm_sync);
    let f = Fixture::new("sync-confirm");
    let (wb, cx) = opened(cx, &f, Settings { git_confirm_sync: true, ..Settings::default() });
    f.push_elsewhere("b.txt", "b\n");
    git_in(&f.dir, &["fetch", "-q"]);
    refresh(&wb, cx);
    wait_ticking(cx, "behind 1", |cx| status(&wb, cx).is_some_and(|s| s.behind == 1));

    run(&wb, cx, "Git: Sync");
    wait_ticking(cx, "the question", |cx| cx.has_pending_prompt());
    let (title, detail) = cx.pending_prompt().unwrap();
    assert_eq!(title, "Sync changes?");
    assert!(detail.contains("origin/main"), "{detail}");
    crate::question::cancel_keeps_focus(&wb, cx, "Cancel");
    cx.run_until_parked();
    assert!(!f.dir.join("b.txt").exists(), "cancelled: nothing pulled");

    run(&wb, cx, "Git: Sync");
    wait_ticking(cx, "the question again", |cx| cx.has_pending_prompt());
    cx.simulate_prompt_answer("Sync");
    wait_ticking(cx, "pulled", |cx| notice(&wb, cx) == "Pulled" && f.dir.join("b.txt").exists());

    // Fetch-only syncs and direct Pull / Push never ask; nor does Sync once the setting is off.
    run(&wb, cx, "Git: Fetch");
    wait_ticking(cx, "fetched", |cx| notice(&wb, cx) == "Fetched");
    assert!(!cx.has_pending_prompt());
    wb.update(cx, |_, cx| cx.set_global(quiet()));
    f.push_elsewhere("c.txt", "c\n");
    git_in(&f.dir, &["fetch", "-q"]);
    refresh(&wb, cx);
    wait_ticking(cx, "behind again", |cx| status(&wb, cx).is_some_and(|s| s.behind == 1));
    run(&wb, cx, "Git: Sync");
    wait_ticking(cx, "pulled without asking", |_| f.dir.join("c.txt").exists());
    assert!(!cx.has_pending_prompt());
}

#[gpui::test]
fn two_syncs_never_run_at_once(cx: &mut TestAppContext) {
    let f = Fixture::new("sync-busy");
    let (wb, cx) = opened(cx, &f, quiet());
    wb.update(cx, |wb, cx| {
        wb.sync_for_test(vec![crate::git::SyncOp::Fetch], cx);
        assert!(wb.sync_running_for_test());
        wb.sync_for_test(vec![crate::git::SyncOp::Fetch], cx);
        assert_eq!(wb.notice_text().as_deref(), Some("A sync is already running."));
    });
    wait_ticking(cx, "the first one ends", |cx| notice(&wb, cx) == "Fetched");
    assert!(!wb.read_with(cx, |wb, _| wb.sync_running_for_test()));
}

#[gpui::test]
fn a_pull_that_conflicts_points_at_the_way_out(cx: &mut TestAppContext) {
    let f = Fixture::new("sync-conflict");
    let (wb, cx) = opened(cx, &f, quiet());
    f.commit_here("README.md", "mine\n");
    f.push_elsewhere("README.md", "theirs\n");
    run(&wb, cx, "Git: Pull");
    wait_ticking(cx, "merge conflict", |cx| notice(&wb, cx).contains("Pull has conflicts"));
    assert!(git_in(&f.dir, &["status", "--porcelain"]).contains("UU README.md"));
    git_in(&f.dir, &["merge", "--abort"]);

    run(&wb, cx, "Git: Pull (Rebase)");
    wait_ticking(cx, "rebase conflict", |cx| notice(&wb, cx).contains("Rebase stopped on conflicts"));
    git_in(&f.dir, &["rebase", "--abort"]);

    // A dirty tree that the pull would overwrite is refused in plain words.
    git_in(&f.dir, &["reset", "-q", "--hard", "origin/main"]);
    f.push_elsewhere("README.md", "theirs again\n");
    std::fs::write(f.dir.join("README.md"), "unsaved\n").unwrap();
    run(&wb, cx, "Git: Pull");
    wait_ticking(cx, "refused", |cx| notice(&wb, cx).contains("Pull failed: Commit or stash your changes first"));
}

#[gpui::test]
fn pull_follows_the_rebase_setting(cx: &mut TestAppContext) {
    let f = Fixture::new("sync-rebase-setting");
    let (wb, cx) = opened(cx, &f, Settings { git_rebase_when_sync: true, ..quiet() });
    f.commit_here("local.txt", "l\n");
    f.push_elsewhere("remote.txt", "r\n");
    run(&wb, cx, "Git: Pull");
    wait_ticking(cx, "pulled", |cx| notice(&wb, cx) == "Pulled" && f.dir.join("remote.txt").exists());
    assert_eq!(git_in(&f.dir, &["rev-list", "--merges", "--count", "HEAD"]).trim(), "0", "rebased, no merge commit");
}

fn commit_with_message(
    wb: &Entity<Workbench>,
    cx: &mut VisualTestContext,
    message: &str,
    f: impl FnOnce(&mut crate::scm::ScmPanel, &mut gpui::Context<crate::scm::ScmPanel>),
) {
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    scm.update(cx, |s, cx| {
        s.set_message(message, cx);
        f(s, cx);
    });
}

#[gpui::test]
fn commit_variants_add_the_trailer_skip_hooks_and_follow_smart_commit(cx: &mut TestAppContext) {
    let f = Fixture::new("sync-commit");
    let (wb, cx) = opened(cx, &f, quiet());
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "status in the panel", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some()));

    std::fs::write(f.dir.join("a.txt"), "a\n").unwrap();
    git_in(&f.dir, &["add", "a.txt"]);
    refresh(&wb, cx);
    wait_ticking(cx, "staged", |cx| status(&wb, cx).is_some_and(|s| s.staged().count() == 1));
    commit_with_message(&wb, cx, "signed", |s, cx| s.commit_signed_off(cx));
    wait_ticking(cx, "committed", |cx| head_subject(&f.dir) == "signed" && !scm.read_with(cx, |s, _| s.is_busy()));
    assert!(git_in(&f.dir, &["log", "-1", "--format=%B"]).contains("Signed-off-by: Test <t@example.com>"));

    // A refusing hook: a plain commit fails, "No Verify" gets through.
    #[cfg(unix)]
    {
        let hook = f.dir.join(".git/hooks/pre-commit");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, "#!/bin/sh\necho no >&2\nexit 1\n").unwrap();
        crate::test_files::make_executable(&hook);
        std::fs::write(f.dir.join("b.txt"), "b\n").unwrap();
        git_in(&f.dir, &["add", "b.txt"]);
        refresh(&wb, cx);
        wait_ticking(cx, "staged b", |cx| status(&wb, cx).is_some_and(|s| s.staged().count() == 1));
        commit_with_message(&wb, cx, "hooked", |s, cx| s.commit(cx));
        wait_ticking(cx, "refused", |cx| scm.read_with(cx, |s, _| s.result_text()).is_some_and(|r| r.contains("no")) && !scm.read_with(cx, |s, _| s.is_busy()));
        assert_eq!(head_subject(&f.dir), "signed");
        commit_with_message(&wb, cx, "hooked", |s, cx| s.commit_no_verify(cx));
        wait_ticking(cx, "skipped the hook", |_| head_subject(&f.dir) == "hooked");
    }

    #[cfg(unix)]
    std::fs::remove_file(f.dir.join(".git/hooks/pre-commit")).unwrap();

    // Smart commit off: nothing staged means nothing is committed, and the button says so.
    wb.update(cx, |_, cx| cx.set_global(Settings { git_smart_commit: false, ..quiet() }));
    std::fs::write(f.dir.join("README.md"), "changed\n").unwrap();
    refresh(&wb, cx);
    wait_ticking(cx, "a change", |cx| status(&wb, cx).is_some_and(|s| !s.changes.is_empty()));
    let before = head_subject(&f.dir);
    commit_with_message(&wb, cx, "should not happen", |s, cx| s.commit(cx));
    assert!(scm.read_with(cx, |s, _| s.result_text()).unwrap().starts_with("Nothing is staged"));
    assert_eq!(head_subject(&f.dir), before);
    // With it on, the same press commits the tracked change.
    wb.update(cx, |_, cx| cx.set_global(quiet()));
    commit_with_message(&wb, cx, "smart", |s, cx| s.commit(cx));
    wait_ticking(cx, "smart commit", |_| head_subject(&f.dir) == "smart");
}

#[gpui::test]
fn post_commit_command_and_commit_and_push_publish_the_commit(cx: &mut TestAppContext) {
    let f = Fixture::new("sync-postcommit");
    let (wb, cx) = opened(cx, &f, Settings { git_post_commit: "push".into(), ..quiet() });
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    std::fs::write(f.dir.join("a.txt"), "a\n").unwrap();
    git_in(&f.dir, &["add", "a.txt"]);
    refresh(&wb, cx);
    wait_ticking(cx, "staged", |cx| status(&wb, cx).is_some_and(|s| s.staged().count() == 1));
    commit_with_message(&wb, cx, "via setting", |s, cx| s.commit(cx));
    wait_ticking(cx, "pushed", |_| head_subject(&f.bare) == "via setting");

    // The setting off: a plain commit stays local, "Commit & Sync" still goes out (and pulls first).
    wb.update(cx, |_, cx| cx.set_global(quiet()));
    f.push_elsewhere("r.txt", "r\n");
    std::fs::write(f.dir.join("b.txt"), "b\n").unwrap();
    git_in(&f.dir, &["add", "b.txt"]);
    refresh(&wb, cx);
    wait_ticking(cx, "staged b", |cx| status(&wb, cx).is_some_and(|s| s.staged().count() == 1));
    commit_with_message(&wb, cx, "plain", |s, cx| s.commit(cx));
    wait_ticking(cx, "committed", |cx| head_subject(&f.dir) == "plain" && !scm.read_with(cx, |s, _| s.is_busy()));
    assert_eq!(head_subject(&f.bare), "remote r.txt", "not pushed");
    std::fs::write(f.dir.join("c.txt"), "c\n").unwrap();
    git_in(&f.dir, &["add", "c.txt"]);
    refresh(&wb, cx);
    wait_ticking(cx, "staged c", |cx| status(&wb, cx).is_some_and(|s| s.staged().count() == 1));
    commit_with_message(&wb, cx, "and sync", |s, cx| s.commit_and(crate::git::PostCommit::Sync, cx));
    wait_ticking(cx, "synced", |_| git_in(&f.bare, &["log", "--format=%s"]).contains("and sync") && f.dir.join("r.txt").exists());
}

fn tick(cx: &mut VisualTestContext, seconds: u64) {
    for _ in 0..seconds.div_ceil(5) {
        cx.executor().advance_clock(std::time::Duration::from_secs(5));
        cx.run_until_parked();
    }
}

fn autofetched(f: &Fixture) -> bool {
    git_in(&f.dir, &["rev-list", "--count", "HEAD..origin/main"]).trim() == "1"
}

#[gpui::test]
fn background_fetch_waits_for_its_period_and_for_a_front_trusted_idle_window(cx: &mut TestAppContext) {
    let f = Fixture::new("sync-autofetch");
    let on = Settings { git_autofetch: true, git_autofetch_period: 60, ..quiet() };
    let (wb, cx) = opened(cx, &f, on.clone());
    f.push_elsewhere("r.txt", "r\n");
    // Not yet due.
    tick(cx, 30);
    assert!(!autofetched(&f), "before the period");
    tick(cx, 40);
    wait_ticking(cx, "the fetch", |cx| autofetched(&f) && !wb.read_with(cx, |wb, _| wb.sync_running_for_test()));
    wait_ticking(cx, "status shows it", |cx| status(&wb, cx).is_some_and(|s| s.behind == 1));
    assert_eq!(notice(&wb, cx), "", "a background fetch is silent");

    // The gates, one at a time.
    let attempt = |wb: &Entity<Workbench>, cx: &mut VisualTestContext, active: bool| wb.update(cx, |wb, cx| wb.autofetch_for_test(active, cx));
    assert!(!attempt(&wb, cx, false), "the window is not in front");
    wb.update(cx, |_, cx| cx.set_global(quiet()));
    assert!(!attempt(&wb, cx, true), "the setting is off");
    wb.update(cx, |_, cx| cx.set_global(Settings { workspace_trust: true, ..on.clone() }));
    assert!(!attempt(&wb, cx, true), "the folder is not trusted");
    wb.update(cx, |_, cx| cx.set_global(on.clone()));
    wb.update(cx, |wb, cx| wb.sync_for_test(vec![crate::git::SyncOp::Fetch], cx));
    assert!(!attempt(&wb, cx, true), "a sync holds the guard");
    wait_ticking(cx, "the sync ends", |cx| !wb.read_with(cx, |wb, _| wb.sync_running_for_test()));
    assert!(attempt(&wb, cx, true), "everything allows it");
    wait_ticking(cx, "ends", |cx| !wb.read_with(cx, |wb, _| wb.sync_running_for_test()));
}

#[gpui::test]
fn a_failing_background_fetch_only_sets_the_tooltip(cx: &mut TestAppContext) {
    let f = Fixture::new("sync-autofetch-error");
    let (wb, cx) = opened(cx, &f, Settings { git_autofetch: true, ..quiet() });
    git_in(&f.dir, &["remote", "set-url", "origin", f.dir.join("nowhere.git").to_str().unwrap()]);
    assert!(wb.update(cx, |wb, cx| wb.autofetch_for_test(true, cx)));
    wait_ticking(cx, "the failure", |cx| wb.read_with(cx, |wb, _| wb.autofetch_error_for_test()).is_some());
    assert_eq!(notice(&wb, cx), "", "no notice, no toast");
    let tip = wb.read_with(cx, |wb, _| wb.sync_tooltip_for_test());
    assert!(tip.contains("The last background fetch failed"), "{tip}");
    // A good fetch clears it.
    git_in(&f.dir, &["remote", "set-url", "origin", f.bare.to_str().unwrap()]);
    assert!(wb.update(cx, |wb, cx| wb.autofetch_for_test(true, cx)));
    wait_ticking(cx, "cleared", |cx| {
        wb.read_with(cx, |wb, _| wb.autofetch_error_for_test()).is_none() && !wb.read_with(cx, |wb, _| wb.sync_running_for_test())
    });
}

/// A pull request's remote comes from the repository's own config: one named like an option must not run as one.
#[cfg(unix)]
#[gpui::test]
fn checking_out_a_pull_request_never_runs_a_command_named_by_the_repository(cx: &mut TestAppContext) {
    let f = Fixture::new("sync-pr-evil");
    let (wb, cx) = opened(cx, &f, quiet());
    let marker = std::env::temp_dir().join(format!("codeyard-pr-marker-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let evil = format!("--upload-pack=touch${{IFS}}{}", marker.display());
    git_in(&f.dir, &["config", &format!("remote.{evil}.url"), "https://github.com/someone/repo.git"]);
    wb.update(cx, |wb, cx| wb.checkout_pull_for_test(evil, 7, cx));
    wait_ticking(cx, "the refusal", |cx| notice(&wb, cx).contains("not a usable"));
    assert!(!marker.exists(), "the command in the remote's name ran");
}

/// Right click on a changed file in Source Control: Copy Path, Copy Relative Path and Add to .gitignore, the last one writing the file's path.
#[gpui::test]
fn a_changed_file_has_a_context_menu_in_source_control(cx: &mut TestAppContext) {
    let f = Fixture::new("scm-row-menu");
    std::fs::write(f.dir.join("scratch.log"), "noise\n").unwrap();
    let (wb, cx) = opened(cx, &f, quiet());
    refresh(&wb, cx);
    wait_ticking(cx, "the change shows up", |cx| status(&wb, cx).is_some_and(|s| s.changes.iter().any(|c| c.path == "scratch.log")));
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    let at = gpui::point(gpui::px(60.), gpui::px(60.));
    scm.update(cx, |_, cx| cx.emit(crate::scm::ScmEvent::FileMenu("scratch.log".into(), false, at)));
    cx.run_until_parked();
    let labels = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).expect("the menu opened");
    assert!(labels.iter().any(|l| l == "Stage"), "file actions must be available without hover: {labels:?}");
    assert!(labels.iter().any(|l| l == "Discard changes"));
    assert!(!labels.iter().any(|l| l == "Unstage"));
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Add to .gitignore", cx));
    cx.run_until_parked();
    let ignore = std::fs::read_to_string(f.dir.join(".gitignore")).unwrap_or_default();
    assert!(ignore.lines().any(|l| l == "/scratch.log"), "{ignore:?}");
}

#[gpui::test]
fn file_menu_stages_and_unstages_the_selected_file(cx: &mut TestAppContext) {
    let f = Fixture::new("scm-menu-staging");
    std::fs::write(f.dir.join("README.md"), "changed\n").unwrap();
    let (wb, cx) = opened(cx, &f, quiet());
    refresh(&wb, cx);
    wait_ticking(cx, "modified file", |cx| status(&wb, cx).is_some_and(|s| s.changes.iter().any(|c| c.path == "README.md")));
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    let at = gpui::point(gpui::px(60.), gpui::px(60.));
    scm.update(cx, |_, cx| cx.emit(crate::scm::ScmEvent::FileMenu("README.md".into(), false, at)));
    cx.run_until_parked();
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Stage", cx));
    wait_ticking(cx, "staged through the file menu", |cx| {
        status(&wb, cx).is_some_and(|s| s.changes.iter().any(|c| c.path == "README.md" && c.staged.is_some() && c.unstaged.is_none()))
    });
    assert_eq!(git_in(&f.dir, &["diff", "--cached", "--name-only"]).trim(), "README.md");
    std::fs::write(f.dir.join("README.md"), "changed again\n").unwrap();
    refresh(&wb, cx);
    wait_ticking(cx, "mixed staged and unstaged file", |cx| {
        status(&wb, cx).is_some_and(|s| s.changes.iter().any(|c| c.path == "README.md" && c.staged.is_some() && c.unstaged.is_some()))
    });
    scm.update(cx, |_, cx| cx.emit(crate::scm::ScmEvent::FileMenu("README.md".into(), true, at)));
    cx.run_until_parked();
    let labels = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).unwrap();
    assert!(labels.iter().any(|l| l == "Unstage"));
    assert!(!labels.iter().any(|l| l == "Discard changes" || l == "Stage"), "staged rows must keep their own actions: {labels:?}");
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Unstage", cx));
    wait_ticking(cx, "unstaged through the file menu", |cx| {
        status(&wb, cx).is_some_and(|s| s.changes.iter().any(|c| c.path == "README.md" && c.staged.is_none() && c.unstaged.is_some()))
    });
    assert!(git_in(&f.dir, &["diff", "--cached", "--name-only"]).trim().is_empty());
    assert_eq!(std::fs::read_to_string(f.dir.join("README.md")).unwrap(), "changed again\n");
}
