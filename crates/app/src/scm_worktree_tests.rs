use super::*;

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

use crate::qa_integration_tests::{git_in, repo, setup_with, wait_ticking};
use crate::settings::Settings;
use crate::workbench::Workbench;

fn open<'a>(cx: &'a mut TestAppContext, dir: &std::path::Path) -> (Entity<Workbench>, Entity<ScmPanel>, &'a mut VisualTestContext) {
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, hot_exit: false, ..Default::default() });
    wb.update_in(cx, |wb, window, cx| {
        wb.open_path(dir.to_str().unwrap(), window, cx);
        wb.run_command_for_test("View: Source Control", window, cx);
    });
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "worktree inventory", |cx| scm.read_with(cx, |s, _| !s.worktrees.entries.is_empty()));
    (wb, scm, cx)
}

#[gpui::test]
fn worktree_panel_refreshes_external_changes_and_opens_the_selected_checkout(cx: &mut TestAppContext) {
    let dir = repo("worktree-panel-open");
    let linked = dir.join("linked checkout");
    let (wb, scm, cx) = open(cx, &dir);
    assert_eq!(scm.read_with(cx, |s, _| s.worktrees.entries.len()), 1);
    git_in(&dir, &["worktree", "add", "-q", "-b", "feature/ui", linked.to_str().unwrap()]);
    let refresh = cx.debug_bounds("scm-worktree-refresh").unwrap().center();
    cx.simulate_click(refresh, Modifiers::none());
    wait_ticking(cx, "external worktree", |cx| scm.read_with(cx, |s, _| s.worktrees.entries.len()) == 2);
    let path = scm.read_with(cx, |s, _| s.worktrees.entries.iter().find(|e| e.branch.as_deref() == Some("feature/ui")).unwrap().path.clone());
    let row = cx.debug_bounds("scm-worktree-row-1").unwrap();
    let body = cx.debug_bounds("scm-worktrees").unwrap();
    assert!(row.left() >= body.left() && row.right() <= body.right());
    cx.simulate_click(row.center(), Modifiers::none());
    wait_ticking(cx, "selected checkout", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|s| s.root == path)));
    wait_ticking(cx, "inventory in linked checkout", |cx| scm.read_with(cx, |s, _| s.worktrees.entries.len()) == 2);
    assert_eq!(scm.read_with(cx, |s, _| s.status.as_ref().unwrap().branch.clone()), Some("feature/ui".into()));
    // A detached checkout can disappear without HEAD or any ref changing.
    let detached = dir.join("detached");
    git_in(&dir, &["worktree", "add", "-q", "--detach", detached.to_str().unwrap()]);
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "detached worktree without ref changes", |cx| scm.read_with(cx, |s, _| s.worktrees.entries.len()) == 3);
    git_in(&dir, &["worktree", "remove", detached.to_str().unwrap()]);
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "removed worktree without ref changes", |cx| scm.read_with(cx, |s, _| s.worktrees.entries.len()) == 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn worktree_panel_switch_preserves_unsaved_files_and_collapsing_is_persisted(cx: &mut TestAppContext) {
    let dir = repo("worktree-panel-dirty");
    let linked = dir.join("linked");
    git_in(&dir, &["worktree", "add", "-q", "-b", "linked", linked.to_str().unwrap()]);
    let (wb, scm, cx) = open(cx, &dir);
    wb.update_in(cx, |wb, window, cx| wb.open_path(dir.join("README.md").to_str().unwrap(), window, cx));
    wait_ticking(cx, "editor loaded", |cx| wb.read_with(cx, |wb, _| wb.active_editor_entity().is_some()));
    cx.simulate_input("unsaved worktree edit");
    let original = wb.read_with(cx, |wb, _| wb.git_status().unwrap().root.clone());
    let path = scm.read_with(cx, |s, _| s.worktrees.entries.iter().find(|e| e.branch.as_deref() == Some("linked")).unwrap().path.clone());
    scm.update(cx, |s, cx| s.open_worktree(&path, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.git_status().unwrap().root.clone()), original);
    assert!(wb.read_with(cx, |wb, cx| wb.active_editor_entity().unwrap().read(cx).is_dirty()));
    let header = cx.debug_bounds("scm-worktree-header").unwrap();
    cx.simulate_click(header.origin + gpui::point(px(8.), px(8.)), Modifiers::none());
    cx.run_until_parked();
    assert!(scm.read_with(cx, |s, _| s.sections.encode()).contains("worktrees:0:"));
    scm.update(cx, |s, cx| s.refresh_worktrees(cx));
    assert!(scm.read_with(cx, |s, _| s.worktrees.task.is_none()), "collapsed inventory does not poll");
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn worktree_panel_clears_inventory_when_repository_changes_and_recovers_from_errors(cx: &mut TestAppContext) {
    let dir = repo("worktree-panel-context");
    let (wb, scm, cx) = open(cx, &dir);
    // Simulate a failed inventory read using an existing host and an invalid repository root.
    let host = scm.read_with(cx, |s, _| s.host.clone());
    let missing = dir.join("missing").to_string_lossy().into_owned();
    scm.update(cx, |s, cx| {
        s.set_status(host.clone(), Some(Arc::new(git::Status { root: missing, ..Default::default() })), cx);
        s.refresh_worktrees(cx);
    });
    wait_ticking(cx, "worktree error", |cx| scm.read_with(cx, |s, _| s.worktrees.error.is_some()));
    assert!(scm.read_with(cx, |s, _| s.worktrees.entries.is_empty()));
    assert!(cx.debug_bounds("scm-worktree-error").is_some());
    let status = wb.read_with(cx, |wb, _| wb.git_status());
    scm.update(cx, |s, cx| s.set_status(host.clone(), status, cx));
    wait_ticking(cx, "worktree retry", |cx| scm.read_with(cx, |s, _| s.worktrees.entries.len()) == 1);
    scm.update(cx, |s, cx| {
        s.refresh_worktrees(cx);
        assert!(s.worktrees.task.is_some());
        s.set_status(host, None, cx);
    });
    assert!(scm.read_with(cx, |s, _| s.worktrees.entries.is_empty() && s.worktrees.task.is_none()));
    cx.run_until_parked();
    assert!(scm.read_with(cx, |s, _| s.status.is_none() && s.worktrees.entries.is_empty()), "late results cannot restore the previous inventory");
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn worktree_panel_supports_keyboard_navigation(cx: &mut TestAppContext) {
    let dir = repo("worktree-panel-keyboard");
    let linked = dir.join("linked");
    git_in(&dir, &["worktree", "add", "-q", "-b", "linked", linked.to_str().unwrap()]);
    let (wb, _, cx) = open(cx, &dir);
    let current = cx.debug_bounds("scm-worktree-row-0").unwrap();
    cx.simulate_click(current.center(), Modifiers::none());
    cx.simulate_keystrokes("tab enter");
    cx.simulate_event(gpui::KeyUpEvent { keystroke: gpui::Keystroke::parse("enter").unwrap() });
    wait_ticking(cx, "keyboard opens linked worktree", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|s| s.branch.as_deref() == Some("linked"))));
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn worktree_panel_long_rows_fit_light_dark_and_turkish_layouts(cx: &mut TestAppContext) {
    let dir = repo("worktree-panel-layout");
    let (_, scm, cx) = open(cx, &dir);
    scm.update(cx, |s, cx| {
        s.worktrees.entries.push(Worktree {
            path: format!("/workspace/{}", "very long checkout path/".repeat(12)),
            branch: Some("feature/very-long-branch-".repeat(12)),
            locked: Some("kept on another disk".into()),
            ..Default::default()
        });
        cx.notify();
    });
    for language in ["en", "tr"] {
        let _language = crate::i18n::LangGuard::set(language);
        for theme in [theme::LIGHT, theme::DARK] {
            cx.update(|window, cx| {
                cx.set_global(crate::ui::ActiveTheme(theme));
                window.refresh();
            });
            for width in [480., 1280.] {
                cx.simulate_resize(gpui::size(px(width), px(800.)));
                cx.run_until_parked();
                let header = cx.debug_bounds("scm-worktree-header").unwrap();
                let refresh = cx.debug_bounds("scm-worktree-refresh").unwrap();
                let row = cx.debug_bounds("scm-worktree-row-1").unwrap();
                assert!(refresh.right() <= header.right(), "refresh clipped ({language}, {width})");
                assert!(row.left() >= header.left() && row.right() <= header.right(), "row overflow ({language}, {width}): {row:?}");
                assert!(row.size.height >= px(layout::ROW_H * 2.), "two-line row clipped");
            }
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn worktree_panel_opens_checkouts_on_the_existing_ssh_host(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("worktree-panel-ssh") else { return };
    let dir = repo("worktree-panel-ssh");
    let linked = dir.join("linked remote checkout");
    git_in(&dir, &["worktree", "add", "-q", "-b", "remote-linked", linked.to_str().unwrap()]);
    let (wb, cx) = crate::qa_integration_tests::ssh_workbench(cx, &dir);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Source Control", window, cx));
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "remote worktree list", |cx| scm.read_with(cx, |s, _| s.worktrees.entries.len()) == 2);
    let source = scm.read_with(cx, |s, _| s.host.clone());
    assert!(source.is_remote());
    let row = cx.debug_bounds("scm-worktree-row-1").unwrap();
    cx.simulate_click(row.center(), Modifiers::none());
    wait_ticking(cx, "remote worktree opens", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|s| s.branch.as_deref() == Some("remote-linked"))));
    assert!(scm.read_with(cx, |s, _| Arc::ptr_eq(&s.host, &source)), "opening a worktree must retain the SSH host");
    let _ = std::fs::remove_dir_all(dir);
}
