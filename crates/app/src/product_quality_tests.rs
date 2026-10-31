//! Regressions from the 2026-10-28 native product walkthrough.

use accesskit::{Action, ActionData, ActionRequest, TreeId};
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::qa_workbench_tests::{fresh, open, setup};
use crate::workbench::{Workbench, a11y::Snapshot};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn act(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, label: &str, action: Action, data: Option<ActionData>) {
    let id = snap.to_tree().nodes.into_iter().find(|(_, n)| n.label() == Some(label)).map(|(id, _)| id).unwrap_or_else(|| panic!("missing node: {label}"));
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn product_explorer_right_arrow_stays_on_an_empty_expanded_folder(cx: &mut TestAppContext) {
    let dir = fresh("product-empty-folder");
    std::fs::create_dir(dir.join("empty")).unwrap();
    std::fs::create_dir(dir.join("next")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    crate::qa_integration_tests::wait_ticking(cx, "the tree", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == 2);
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("home right");
    cx.run_until_parked();
    cx.simulate_keystrokes("right");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_selected(cx)), Some(crate::ui::local_path(&dir.join("empty"))));
}

/// The accessibility tree follows Right Arrow: into the first child of a folder that has one, and staying on the
/// expanded empty folder (its row, expanded, keeps the focus) instead of jumping to the next sibling.
#[gpui::test]
fn product_explorer_right_arrow_focus_in_the_accessibility_tree(cx: &mut TestAppContext) {
    let dir = fresh("product-right-arrow-ax");
    std::fs::create_dir(dir.join("empty")).unwrap();
    std::fs::create_dir(dir.join("full")).unwrap();
    std::fs::write(dir.join("full/a.txt"), "a\n").unwrap();
    std::fs::write(dir.join("full/b.txt"), "b\n").unwrap();
    std::fs::write(dir.join("z.txt"), "z\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    crate::qa_integration_tests::wait_ticking(cx, "the tree", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == 3);
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    let focused = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| {
        let snap = snapshot(wb, cx);
        let tree = snap.to_tree();
        let node = tree.nodes.iter().find(|(id, _)| *id == tree.focus).map(|(_, n)| n.clone()).expect("a focused node");
        (node.label().map(str::to_string), node.is_expanded())
    };
    // The full folder: Right opens it (the focus stays on it), Right again goes to its first child.
    cx.simulate_keystrokes("home down right");
    crate::qa_integration_tests::wait_ticking(cx, "the open folder", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == 5);
    assert_eq!(focused(&wb, cx), (Some("full".into()), Some(true)));
    cx.simulate_keystrokes("right");
    assert_eq!(focused(&wb, cx), (Some("a.txt".into()), None));
    // The empty folder: open, and Right again keeps the focus on it.
    cx.simulate_keystrokes("home right");
    cx.run_until_parked();
    cx.simulate_keystrokes("right");
    assert_eq!(focused(&wb, cx), (Some("empty".into()), Some(true)));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_selected(cx)), Some(crate::ui::local_path(&dir.join("empty"))));
}

#[gpui::test]
fn product_hidden_explorer_is_not_exposed_to_assistive_technology(cx: &mut TestAppContext) {
    let dir = fresh("product-hidden-explorer");
    std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some("scm"));
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert!(snap.explorers.is_empty(), "Source Control is visible; the hidden file tree must not be reachable");
}

#[gpui::test]
fn product_source_control_exposes_its_empty_state(cx: &mut TestAppContext) {
    let dir = fresh("product-scm-empty");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
    assert!(tree.state().root().children().any(|n| n.label().as_deref() == Some("Source Control")), "the visible Git panel is missing");
}

#[gpui::test]
fn product_rust_commands_explain_disabled_language_features(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("product-rust-disabled");
    let file = dir.join("main.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().lsp_enabled = false);
    open(&wb, &file, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Rust: Expand Macro Recursively", window, cx));
    let (_, message) = wb.read_with(cx, |wb, _| wb.notice_for_test()).expect("an actionable notice");
    assert!(message.contains("lsp.enabled"), "a Rust file is already open: {message}");
}

#[gpui::test]
fn product_explorer_stale_accessibility_action_follows_the_file_not_its_old_index(cx: &mut TestAppContext) {
    let dir = fresh("product-stale-explorer");
    std::fs::create_dir(dir.join("a-folder")).unwrap();
    std::fs::write(dir.join("a-folder/child.txt"), "child\n").unwrap();
    std::fs::write(dir.join("z.txt"), "requested\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    crate::qa_integration_tests::wait_ticking(cx, "the closed tree", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == 2);
    let before = snapshot(&wb, cx);
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("home right");
    crate::qa_integration_tests::wait_ticking(cx, "the expanded tree", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == 3);
    act(&wb, cx, &before, "z.txt", Action::Click, None);
    crate::qa_integration_tests::wait_ticking(cx, "the requested file", |cx| {
        wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some_and(|p| p.ends_with("/z.txt"))
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap().read_with(cx, |e, _| e.text()), "requested\n");
}

#[gpui::test]
fn product_source_control_accessibility_stages_unstages_reports_errors_and_commits(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(dir) = crate::tests::git_repo("product-accessible-scm") else { return };
    let git = |args: &[&str]| crate::qa_integration_tests::git_in(&dir, args);
    std::fs::write(dir.join("tracked.txt"), "before\n").unwrap();
    git(&["add", "tracked.txt"]);
    git(&["commit", "-qm", "Add tracked fixture"]);
    std::fs::write(dir.join("tracked.txt"), "after\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    crate::qa_integration_tests::wait_ticking(cx, "the unstaged change", |cx| {
        snapshot(&wb, cx).scm.is_some_and(|s| s.view.groups.iter().flat_map(|g| &g.files).any(|f| f.change.path == "tracked.txt" && !f.staged))
    });
    let snap = snapshot(&wb, cx);
    assert_eq!(snap.focus, crate::workbench::a11y::FocusSnap::ScmMessage);
    act(&wb, cx, &snap, "Stage tracked.txt", Action::Click, None);
    crate::qa_integration_tests::wait_ticking(cx, "staging", |cx| {
        snapshot(&wb, cx).scm.is_some_and(|s| !s.view.busy && s.view.groups.iter().flat_map(|g| &g.files).any(|f| f.change.path == "tracked.txt" && f.staged))
    });
    assert_eq!(git(&["diff", "--cached", "--name-only"]).trim(), "tracked.txt");
    let staged = snapshot(&wb, cx);
    act(&wb, cx, &staged, "Unstage tracked.txt", Action::Click, None);
    crate::qa_integration_tests::wait_ticking(cx, "unstaging", |cx| {
        snapshot(&wb, cx).scm.is_some_and(|s| !s.view.busy && s.view.groups.iter().flat_map(|g| &g.files).any(|f| f.change.path == "tracked.txt" && !f.staged))
    });
    assert!(git(&["diff", "--cached", "--name-only"]).trim().is_empty());

    // Replaying the old Unstage after a refresh must not mutate another row.
    act(&wb, cx, &staged, "Unstage tracked.txt", Action::Click, None);
    assert!(git(&["diff", "--cached", "--name-only"]).trim().is_empty());
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, "Stage tracked.txt", Action::Click, None);
    crate::qa_integration_tests::wait_ticking(cx, "restaging", |cx| snapshot(&wb, cx).scm.is_some_and(|s| !s.view.busy && s.view.commit_label == "Commit (1)"));
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, "Commit (1)", Action::Click, None);
    let error = snapshot(&wb, cx).to_tree();
    assert!(error.nodes.iter().any(|(_, n)| n.role() == accesskit::Role::Alert && n.label() == Some("Type a commit message first")));
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, "Message (↵ to commit)", Action::SetValue, Some(ActionData::Value("Accessible commit".into())));
    let snap = snapshot(&wb, cx);
    assert_eq!(snap.scm.as_ref().unwrap().view.message, "Accessible commit");
    act(&wb, cx, &snap, "Commit (1)", Action::Click, None);
    crate::qa_integration_tests::wait_ticking(cx, "the commit", |cx| {
        snapshot(&wb, cx).scm.is_some_and(|s| !s.view.busy && s.view.message.is_empty() && s.view.groups.iter().all(|g| g.total == 0))
    });
    assert_eq!(git(&["log", "-1", "--format=%s"]).trim(), "Accessible commit");
    assert!(git(&["status", "--porcelain"]).is_empty());
}

#[gpui::test]
fn product_rust_open_manifest_without_a_server_chooses_the_member_crate(cx: &mut TestAppContext) {
    let dir = fresh("product-rust-member-manifest");
    std::fs::create_dir_all(dir.join("member/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"member\"]\n").unwrap();
    std::fs::write(dir.join("member/Cargo.toml"), "[package]\nname = \"member\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(dir.join("member/src/lib.rs"), "pub fn f() {}\n").unwrap();
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().lsp_enabled = false);
    open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("member/src/lib.rs"), cx);
    crate::qa_integration_tests::wait_ticking(cx, "the Rust file", |cx| {
        wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some_and(|p| p.ends_with("/member/src/lib.rs"))
    });
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Rust: Open Cargo.toml", window, cx));
    crate::qa_integration_tests::wait_ticking(cx, "the member manifest", |cx| {
        wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some_and(|p| p.ends_with("/member/Cargo.toml"))
    });
}

#[gpui::test]
fn product_rust_manifest_lookup_does_not_escape_the_workspace(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let base = fresh("product-rust-manifest-boundary");
    let dir = base.join("workspace");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(base.join("Cargo.toml"), "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().lsp_enabled = false);
    open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("src/lib.rs"), cx);
    crate::qa_integration_tests::wait_ticking(cx, "the Rust file", |cx| {
        wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some_and(|p| p.ends_with("/src/lib.rs"))
    });
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Rust: Open Cargo.toml", window, cx));
    crate::qa_integration_tests::wait_ticking(cx, "the missing manifest notice", |cx| {
        wb.read_with(cx, |wb, _| wb.notice_for_test()).is_some_and(|(_, text)| text == "No Cargo.toml found for this file")
    });
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("/src/lib.rs"));
}
