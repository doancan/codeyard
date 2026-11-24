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

/// The graph's top edge drags up and down whether the Worktrees section is open or closed: the line is not only drawn, the
/// section follows it.
#[gpui::test]
fn the_graph_edge_moves_the_section_with_worktrees_open_and_closed(cx: &mut TestAppContext) {
    let dir = repo("graph-sash");
    for n in 0..30 {
        std::fs::write(dir.join(format!("f{n}.txt")), "x").unwrap();
        git_in(&dir, &["add", "-A"]);
        git_in(&dir, &["commit", "-q", "-m", &format!("commit {n}")]);
    }
    // A long list of changes: the graph does not fill the panel then, and its top edge can be dragged.
    for n in 0..14 {
        std::fs::write(dir.join(format!("new-{n}.txt")), "x").unwrap();
    }
    let (_wb, scm, cx) = open(cx, &dir);
    wait_ticking(cx, "graph rows", |cx| cx.debug_bounds("scm-graph-body").is_some());
    let drag = |cx: &mut VisualTestContext, dy: f32| {
        let sash = cx.debug_bounds("sash-graph").unwrap().center();
        cx.simulate_mouse_down(sash, gpui::MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(sash + gpui::point(px(0.), px(dy)), Some(gpui::MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(sash + gpui::point(px(0.), px(dy)), gpui::MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    };
    let height = |cx: &mut VisualTestContext| f32::from(cx.debug_bounds("scm-graph-body").unwrap().size.height);
    for (worktrees_open, window_h) in [(true, 900.), (false, 900.), (true, 700.), (false, 700.), (true, 620.), (false, 620.)] {
        cx.simulate_resize(gpui::size(px(1000.), px(window_h)));
        cx.run_until_parked();
        scm.update(cx, |s, cx| {
            s.sections.set_open("worktrees", worktrees_open);
            cx.notify();
        });
        cx.run_until_parked();
        let before = height(cx);
        drag(cx, -60.);
        let grown = height(cx);
        assert!(grown > before + 30., "worktrees open {worktrees_open}, window {window_h}: dragged up 60, the graph went from {before} to {grown}");
        drag(cx, 60.);
        let back = height(cx);
        assert!(back < grown - 30., "worktrees open {worktrees_open}, window {window_h}: dragged down 60, the graph went from {grown} to {back}");
    }
}

/// Under a short list of changes the graph fills the panel, whatever height was dragged earlier, and there is no edge to drag
/// (it would move nothing): no empty band is left under the list.
#[gpui::test]
fn the_graph_fills_the_panel_under_a_short_list(cx: &mut TestAppContext) {
    let dir = repo("graph-fill");
    for n in 0..30 {
        std::fs::write(dir.join(format!("f{n}.txt")), "x").unwrap();
        git_in(&dir, &["add", "-A"]);
        git_in(&dir, &["commit", "-q", "-m", &format!("commit {n}")]);
    }
    std::fs::write(dir.join("README.md"), "changed\n").unwrap();
    let (_wb, scm, cx) = open(cx, &dir);
    cx.simulate_resize(gpui::size(px(1000.), px(900.)));
    scm.update(cx, |s, cx| {
        s.sections.set_height("graph", 120.);
        cx.notify();
    });
    wait_ticking(cx, "graph rows", |cx| cx.debug_bounds("scm-graph-body").is_some());
    let height = f32::from(cx.debug_bounds("scm-graph-body").unwrap().size.height);
    assert!(height > 300., "the graph fills the panel instead of keeping the dragged 120: {height}");
    assert!(cx.debug_bounds("sash-graph").is_none(), "no edge to drag while the graph fills");
}

/// A commit of very many files lists a bounded number of them in the details (the list is drawn whole every frame) and says how
/// many more there are.
#[gpui::test]
fn the_details_of_a_huge_commit_list_a_bounded_number_of_files(cx: &mut TestAppContext) {
    let dir = repo("detail-cap");
    let total = crate::scm::DETAIL_FILE_ROWS + 20;
    for n in 0..total {
        std::fs::write(dir.join(format!("bulk-{n:04}.txt")), "x").unwrap();
    }
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "a very large commit"]);
    let (_wb, scm, cx) = open(cx, &dir);
    cx.simulate_resize(gpui::size(px(1000.), px(900.)));
    wait_ticking(cx, "graph", |cx| scm.read_with(cx, |s, _| s.graph_subjects().iter().any(|x| x == "a very large commit")));
    let ix = scm.read_with(cx, |s, _| s.graph_subjects().iter().position(|x| x == "a very large commit")).unwrap();
    let commit = scm.read_with(cx, |s, _| s.graph_commit(ix)).unwrap();
    scm.update(cx, |s, cx| s.select_commit(commit, cx));
    wait_ticking(cx, "details", |cx| scm.read_with(cx, |s, _| s.detail_for_test().is_some()));
    cx.run_until_parked();
    assert_eq!(scm.read_with(cx, |s, _| s.detail_for_test()).unwrap().files.len(), total, "the data is whole");
    assert!(cx.debug_bounds("scm-detail-more").is_some(), "the rest is one line: more files, double-click to see them");
}

/// A click on a section header opens or closes it and leaves the focus where it was: the header's active-colour fill belongs to the
/// keyboard, and a fill left behind by the mouse read as a stuck hover.
#[gpui::test]
fn a_click_on_a_section_header_does_not_take_the_focus(cx: &mut TestAppContext) {
    let dir = repo("header-focus");
    let (_wb, scm, cx) = open(cx, &dir);
    wait_ticking(cx, "graph header", |cx| cx.debug_bounds("scm-worktree-header").is_some());
    let focused = |cx: &mut VisualTestContext| cx.update(|window, cx| window.focused(cx));
    let before = focused(cx);
    let header = cx.debug_bounds("scm-worktree-header").unwrap();
    for _ in 0..2 {
        cx.simulate_click(header.origin + gpui::point(px(8.), px(8.)), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(focused(cx), before, "the click did not move the focus to the header");
    }
    assert!(scm.read_with(cx, |s, _| s.sections.is_open("worktrees", true)), "two clicks: closed and open again");
}

fn commit_chain(name: &str, n: usize) -> std::path::PathBuf {
    let dir = repo(name);
    for i in 0..n {
        std::fs::write(dir.join(format!("c{i}.txt")), "x").unwrap();
        git_in(&dir, &["add", "-A"]);
        git_in(&dir, &["commit", "-q", "-m", &format!("commit {i}")]);
    }
    dir
}

/// Up, Down, Home and End move the selected commit in the graph, Page keys move by ten, and the keys stop at the ends.
#[gpui::test]
fn the_graph_list_moves_the_selected_commit_with_the_keys(cx: &mut TestAppContext) {
    let dir = commit_chain("graph-keys", 15);
    let (_wb, scm, cx) = open(cx, &dir);
    cx.simulate_resize(gpui::size(px(1000.), px(900.)));
    wait_ticking(cx, "graph", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() >= 16));
    scm.update_in(cx, |s, window, _| window.focus(&s.graph_focus));
    let selected = |cx: &mut VisualTestContext| scm.read_with(cx, |s, _| s.selected_hash());
    let hash_at = |cx: &mut VisualTestContext, ix: usize| scm.read_with(cx, |s, _| s.graph_commit(ix)).map(|c| c.hash);
    cx.simulate_keystrokes("down");
    assert_eq!(selected(cx), hash_at(cx, 0), "the first key lands on the first commit");
    cx.simulate_keystrokes("down down");
    assert_eq!(selected(cx), hash_at(cx, 2));
    cx.simulate_keystrokes("up");
    assert_eq!(selected(cx), hash_at(cx, 1));
    cx.simulate_keystrokes("pagedown");
    assert_eq!(selected(cx), hash_at(cx, 11), "a page is ten commits");
    cx.simulate_keystrokes("end");
    let last = scm.read_with(cx, |s, _| s.graph_subjects().len()) - 1;
    assert_eq!(selected(cx), hash_at(cx, last));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(cx), hash_at(cx, last), "no further than the last commit");
    cx.simulate_keystrokes("home");
    assert_eq!(selected(cx), hash_at(cx, 0));
    cx.simulate_keystrokes("up");
    assert_eq!(selected(cx), hash_at(cx, 0), "no further than the first commit");
}

/// "Go to Current Commit" selects the commit the checked-out branch is at, from wherever the selection was.
#[gpui::test]
fn go_to_current_commit_selects_where_head_is(cx: &mut TestAppContext) {
    let dir = commit_chain("graph-head", 12);
    let (_wb, scm, cx) = open(cx, &dir);
    wait_ticking(cx, "graph", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() >= 13));
    let head = git_in(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    let old = scm.read_with(cx, |s, _| s.graph_commit(10)).unwrap();
    scm.update(cx, |s, cx| s.pick_commit(old.clone(), false, cx));
    assert_eq!(scm.read_with(cx, |s, _| s.selected_hash()), Some(old.hash));
    scm.update(cx, |s, cx| s.go_to_head(cx));
    assert_eq!(scm.read_with(cx, |s, _| s.selected_hash()), Some(head));
}

/// The menu of a commit offers the local branches that point at it, not the checked-out one, remote branches or tags.
#[test]
fn a_commit_menu_offers_its_local_branches() {
    use crate::graph::{GraphCommit, RefKind, RefLabel};
    let label = |name: &str, kind| RefLabel { name: name.into(), kind };
    let commit = GraphCommit {
        refs: vec![label("main", RefKind::Head), label("feature/x", RefKind::Local), label("origin/feature/x", RefKind::Remote), label("v1", RefKind::Tag)],
        ..Default::default()
    };
    assert_eq!(crate::scm::switchable_branches(&commit), vec!["feature/x".to_string()]);
}

/// Right click on a worktree row: Open and Remove only for a checkout that can be opened or removed; Copy Path always.
#[gpui::test]
fn the_worktree_menu_offers_what_the_row_can_do(cx: &mut TestAppContext) {
    let dir = repo("worktree-menu");
    let linked = dir.join("linked");
    git_in(&dir, &["worktree", "add", "-q", "-b", "linked", linked.to_str().unwrap()]);
    let (wb, scm, cx) = open(cx, &dir);
    wait_ticking(cx, "both worktrees", |cx| scm.read_with(cx, |s, _| s.worktrees.entries.len()) == 2);
    cx.simulate_resize(gpui::size(px(1000.), px(900.)));
    cx.run_until_parked();
    let labels = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.menu_labels(cx));
    let right_click = |cx: &mut VisualTestContext, row: &'static str| {
        let at = cx.debug_bounds(row).unwrap().center();
        cx.simulate_mouse_down(at, gpui::MouseButton::Right, Modifiers::none());
        cx.run_until_parked();
    };
    // The row of the open (main) checkout: no Open, no Remove.
    let current = scm.read_with(cx, |s, _| s.worktrees.entries.iter().position(|e| e.branch.as_deref() != Some("linked"))).unwrap();
    right_click(cx, Box::leak(format!("scm-worktree-row-{current}").into_boxed_str()));
    let menu = labels(cx).expect("a menu opened");
    assert!(menu.iter().any(|l| l == "Copy Path"), "{menu:?}");
    assert!(!menu.iter().any(|l| l == "Open Worktree" || l == "Remove Worktree…"), "the open checkout cannot be opened or removed: {menu:?}");
    cx.simulate_keystrokes("escape");
    // The linked checkout: both.
    let other = scm.read_with(cx, |s, _| s.worktrees.entries.iter().position(|e| e.branch.as_deref() == Some("linked"))).unwrap();
    right_click(cx, Box::leak(format!("scm-worktree-row-{other}").into_boxed_str()));
    let menu = labels(cx).expect("a menu opened");
    assert!(menu.iter().any(|l| l == "Open Worktree") && menu.iter().any(|l| l == "Remove Worktree…"), "{menu:?}");
}

/// A history cut at the limit says so and offers Load more: one press brings in the next page, and the bar goes when the whole
/// history is there. A search that finds nothing says it looked only in the commits that are loaded.
#[gpui::test]
fn a_history_cut_at_the_limit_offers_to_load_more(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = commit_chain("graph-more", 25);
    let (wb, scm, cx) = open(cx, &dir);
    cx.simulate_resize(gpui::size(px(1000.), px(900.)));
    cx.update(|_, cx| {
        let mut settings = cx.global::<Settings>().clone();
        settings.graph_limit = 20;
        cx.set_global(settings);
    });
    scm.update(cx, |s, cx| s.load_graph(cx));
    wait_ticking(cx, "the first page", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() == 20));
    assert!(cx.debug_bounds("scm-graph-more").is_some(), "20 of 26 commits: there is more to load");
    assert!(scm.read_with(cx, |s, cx| s.graph_truncated(cx)));
    scm.update(cx, |s, cx| s.set_query_for_test("no such commit anywhere", cx));
    cx.run_until_parked();
    let why = scm.read_with(cx, |s, cx| s.graph_items(cx).err().map(|(why, _)| why.to_string())).expect("nothing matches");
    assert_eq!(why, "No commit among the latest 20 matches.", "it says it looked only in what is loaded");
    scm.update(cx, |s, cx| s.set_query_for_test("", cx));
    scm.update(cx, |s, cx| s.load_more_commits(cx));
    wait_ticking(cx, "everything", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() == 26));
    assert!(!scm.read_with(cx, |s, cx| s.graph_truncated(cx)), "the whole history is loaded: nothing more to offer");
    let _ = wb;
}

/// In a narrow sidebar the Graph header keeps Reload and the "…" menu in reach: the quick buttons before them are what gets cut.
#[gpui::test]
fn the_graph_menu_button_stays_inside_a_narrow_sidebar(cx: &mut TestAppContext) {
    let dir = commit_chain("graph-narrow", 4);
    git_in(&dir, &["remote", "add", "origin", "https://example.invalid/x.git"]);
    let (wb, scm, cx) = open(cx, &dir);
    cx.simulate_resize(gpui::size(px(1000.), px(900.)));
    wait_ticking(cx, "graph", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() >= 4));
    for width in [200., 240., 300.] {
        wb.update(cx, |wb, _| wb.set_sidebar_width_for_test(width));
        cx.run_until_parked();
        cx.simulate_resize(gpui::size(px(1000. + width), px(900.)));
        cx.run_until_parked();
        let header = cx.debug_bounds("scm-graph-header").unwrap();
        let more = cx.debug_bounds("scm-graph-more").unwrap();
        assert!(more.right() <= header.right() + px(0.5), "sidebar {width}: the menu button {more:?} runs out of the header {header:?}");
        assert!(more.left() >= header.left(), "sidebar {width}: {more:?} {header:?}");
    }
}

/// A linked checkout with uncommitted changes is marked in the list, and the mark goes when the changes do. The open checkout is
/// marked from the changes Source Control already knows.
#[gpui::test]
fn a_worktree_with_uncommitted_changes_is_marked(cx: &mut TestAppContext) {
    let dir = repo("worktree-dirty");
    let linked = dir.join("linked");
    git_in(&dir, &["worktree", "add", "-q", "-b", "linked", linked.to_str().unwrap()]);
    let (_wb, scm, cx) = open(cx, &dir);
    wait_ticking(cx, "both worktrees", |cx| scm.read_with(cx, |s, _| s.worktrees.entries.len()) == 2);
    let marked = |cx: &mut VisualTestContext| scm.read_with(cx, |s, _| s.worktrees.dirty_paths_for_test());
    let linked_path = scm.read_with(cx, |s, _| s.worktrees.entries.iter().find(|e| e.branch.as_deref() == Some("linked")).unwrap().path.clone());
    // Nothing changed anywhere yet.
    scm.update(cx, |s, cx| s.refresh_worktree_dirty(true, cx));
    wait_ticking(cx, "the first check", |cx| scm.read_with(cx, |s, _| s.worktrees.dirty_checked_for_test()));
    assert!(marked(cx).is_empty(), "{:?}", marked(cx));
    // A tracked file of the linked checkout changes.
    std::fs::write(linked.join("README.md"), "edited in the linked checkout\n").unwrap();
    scm.update(cx, |s, cx| s.refresh_worktree_dirty(true, cx));
    wait_ticking(cx, "the linked checkout is marked", |cx| marked(cx).contains(&linked_path));
    // And is clean again.
    git_in(&linked, &["checkout", "--", "README.md"]);
    scm.update(cx, |s, cx| s.refresh_worktree_dirty(true, cx));
    wait_ticking(cx, "the mark goes", |cx| marked(cx).is_empty());
}
