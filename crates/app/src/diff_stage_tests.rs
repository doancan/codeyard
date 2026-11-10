//! Staging and unstaging picked lines from Source Control's diff, end to end in a real repository.

use std::path::Path;

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::diff::DiffView;
use crate::git::DiffRow;
use crate::qa_integration_tests::{changes, git_in, repo, wait_ticking};
use crate::qa_workbench_tests::{open, setup};
use crate::workbench::Workbench;

/// `src/lib.rs` of `repo` with lines 3 and 9 changed.
fn two_changes(dir: &Path) {
    let text: String = (1..=10).map(|i| if i == 3 || i == 9 { format!("// changed {i}\n") } else { format!("// satır {i}\n") }).collect();
    std::fs::write(dir.join("src/lib.rs"), text).unwrap();
}

fn index(dir: &Path) -> String {
    git_in(dir, &["show", ":src/lib.rs"])
}

/// Open the file's diff (`staged`: HEAD ↔ Staged) and wait for it.
fn open_diff(wb: &Entity<Workbench>, staged: bool, cx: &mut VisualTestContext) -> Entity<DiffView> {
    wb.update_in(cx, |wb, window, cx| wb.open_diff("src/lib.rs".into(), staged, window, cx));
    wait_ticking(cx, "the diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| wb.diff_view()).unwrap()
}

/// The side-by-side row that shows new line `line` (0-based).
fn row_of(view: &Entity<DiffView>, line: usize, cx: &mut VisualTestContext) -> usize {
    view.read_with(cx, |v, _| v.rows.iter().position(|r| matches!(r, DiffRow::Changed(_, Some(b)) if *b == line)).unwrap())
}

fn outcome(view: &Entity<DiffView>, cx: &mut VisualTestContext) -> Option<(String, bool)> {
    view.read_with(cx, |v, _| v.outcome.clone().map(|(m, failed)| (m.to_string(), failed)))
}

fn started<'a>(name: &str, cx: &'a mut TestAppContext) -> (std::path::PathBuf, Entity<Workbench>, &'a mut VisualTestContext) {
    let dir = repo(name);
    two_changes(&dir);
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| !c.is_empty()));
    (dir, wb, cx)
}

#[gpui::test]
fn picked_lines_are_staged_with_enter_and_the_diff_reloads(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (dir, wb, cx) = started("diffstage-pick", cx);
    let view = open_diff(&wb, false, cx);
    assert!(view.read_with(cx, |v, _| v.can_stage() && !v.unstages()));
    assert_eq!(view.read_with(cx, |v, _| v.blocks.len()), 2);
    // Pick line 3's row, then Enter.
    let row = row_of(&view, 2, cx);
    view.update(cx, |v, cx| v.click_row(row, false, cx));
    assert_eq!(view.read_with(cx, |v, _| v.picked.iter().copied().collect::<Vec<_>>()), vec![row]);
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "the index", |_| index(&dir).contains("changed 3"));
    assert!(!index(&dir).contains("changed 9"), "only the picked line");
    wait_ticking(cx, "the reload", |cx| view.read_with(cx, |v, _| v.blocks.len() == 1 && !v.busy));
    assert_eq!(outcome(&view, cx), Some(("Staged 1 line".into(), false)));
    assert!(view.read_with(cx, |v, _| v.picked.is_empty()), "picks go with the reload");
    // Source Control shows the file both staged and changed.
    wait_ticking(cx, "status", |cx| {
        changes(&wb, cx).is_some_and(|c| c.iter().any(|(p, staged, unstaged)| p == "src/lib.rs" && staged.is_some() && unstaged.is_some()))
    });
    // Enter with nothing picked stages the change under F7: the last one.
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "the rest", |_| index(&dir).contains("changed 9"));
    wait_ticking(cx, "empty", |cx| view.read_with(cx, |v, _| v.blocks.is_empty() && !v.busy));
    assert_eq!(outcome(&view, cx), Some(("Staged 1 line".into(), false)));
}

#[gpui::test]
fn shift_click_space_and_escape_pick_and_clear(cx: &mut TestAppContext) {
    let (_dir, wb, cx) = started("diffstage-keys", cx);
    let view = open_diff(&wb, false, cx);
    let (first, last) = (row_of(&view, 2, cx), row_of(&view, 8, cx));
    view.update(cx, |v, cx| v.click_row(first, false, cx));
    view.update(cx, |v, cx| v.click_row(last, true, cx));
    // Shift picks every changed row in between, never an unchanged one.
    assert_eq!(view.read_with(cx, |v, _| v.picked.iter().copied().collect::<Vec<_>>()), vec![first, last]);
    // A click on a picked row unpicks it; an unchanged row cannot be picked.
    view.update(cx, |v, cx| v.click_row(last, false, cx));
    view.update(cx, |v, cx| v.click_row(0, false, cx));
    assert_eq!(view.read_with(cx, |v, _| v.picked.len()), 1);
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |v, _| v.picked.is_empty()));
    // Space picks the change under F7, and unpicks it when it is all picked.
    cx.simulate_keystrokes("space");
    assert_eq!(view.read_with(cx, |v, _| v.picked.len()), 1);
    cx.simulate_keystrokes("space");
    assert!(view.read_with(cx, |v, _| v.picked.is_empty()));
}

#[gpui::test]
fn picked_lines_are_unstaged_from_the_staged_diff(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (dir, wb, cx) = started("diffstage-unstage", cx);
    git_in(&dir, &["add", "src/lib.rs"]);
    let view = open_diff(&wb, true, cx);
    assert!(view.read_with(cx, |v, _| v.unstages()));
    let row = row_of(&view, 8, cx);
    view.update(cx, |v, cx| v.click_row(row, false, cx));
    view.update(cx, |v, cx| v.apply_selection(cx));
    wait_ticking(cx, "the index", |_| !index(&dir).contains("changed 9"));
    assert!(index(&dir).contains("changed 3"), "the other line stays staged");
    assert!(std::fs::read_to_string(dir.join("src/lib.rs")).unwrap().contains("changed 9"), "the file keeps the line");
    wait_ticking(cx, "the reload", |cx| view.read_with(cx, |v, _| v.blocks.len() == 1 && !v.busy));
    assert_eq!(outcome(&view, cx), Some(("Unstaged 1 line".into(), false)));
}

#[gpui::test]
fn a_change_s_button_stages_it_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (dir, wb, cx) = started("diffstage-button", cx);
    let _view = open_diff(&wb, false, cx);
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let tree = snap.to_tree();
    let (id, _) = tree
        .nodes
        .iter()
        .find(|(_, n)| n.role() == accesskit::Role::Button && n.label() == Some("Stage Change, change 2 of 2"))
        .cloned()
        .expect("the second change's button");
    // Each changed line is a check box too.
    assert!(tree.nodes.iter().any(|(_, n)| n.role() == accesskit::Role::CheckBox && n.label().is_some_and(|l| l.contains("// changed 3"))));
    let req = accesskit::ActionRequest { action: accesskit::Action::Click, target_tree: accesskit::TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
    wait_ticking(cx, "the index", |_| index(&dir).contains("changed 9"));
    assert!(!index(&dir).contains("changed 3"));
}

#[gpui::test]
fn a_diff_older_than_the_file_writes_nothing_and_reloads(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (dir, wb, cx) = started("diffstage-stale", cx);
    let view = open_diff(&wb, false, cx);
    let row = row_of(&view, 2, cx);
    // The file changes after the diff was shown.
    std::fs::write(dir.join("src/lib.rs"), "// rewritten\n").unwrap();
    view.update(cx, |v, cx| v.click_row(row, false, cx));
    view.update(cx, |v, cx| v.apply_selection(cx));
    wait_ticking(cx, "the answer", |cx| outcome(&view, cx).is_some());
    assert_eq!(outcome(&view, cx), Some((crate::git::stale_diff_message(), true)));
    assert!(!index(&dir).contains("changed"), "nothing staged");
    assert!(view.read_with(cx, |v, _| v.new_text == "// rewritten\n"), "the diff shows the file as it is now");
}

#[gpui::test]
fn a_staged_rename_and_a_comparison_do_not_stage(cx: &mut TestAppContext) {
    let (dir, wb, cx) = started("diffstage-rename", cx);
    git_in(&dir, &["checkout", "-q", "--", "src/lib.rs"]);
    git_in(&dir, &["mv", "src/lib.rs", "src/moved.rs"]);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| c.iter().any(|(p, ..)| p == "src/moved.rs")));
    wb.update_in(cx, |wb, window, cx| wb.open_diff("src/moved.rs".into(), true, window, cx));
    wait_ticking(cx, "the diff", |cx| wb.read_with(cx, |wb, cx| wb.diff_view().is_some_and(|v| v.read(cx).path == "src/moved.rs")));
    let view = wb.read_with(cx, |wb, _| wb.diff_view()).unwrap();
    assert!(!view.read_with(cx, |v, _| v.can_stage()), "a rename is unstaged as a whole file");
}
