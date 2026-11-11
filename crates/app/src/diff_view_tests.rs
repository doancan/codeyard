//! The diff viewer's folds, overview ruler, whitespace option, syntax colors, header controls and guards, driven
//! through the workbench like a user (a real repository, key strokes, the accessibility tree's presses).

use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};

use crate::diff::DiffView;
use crate::diff::fold::Item;
use crate::diff::guard::Unsupported;
use crate::diff::ruler::Kind;
use crate::diff::whitespace::Whitespace;
use crate::git::DiffRow;
use crate::qa_integration_tests::{changes, git_in, repo, wait_ticking};
use crate::qa_workbench_tests::{open, setup};
use crate::workbench::Workbench;

const LINES: usize = 120;

/// `src/lib.rs` with `LINES` lines, committed, then lines 10 and 100 (1-based) changed in the working tree.
fn long_file<'a>(name: &str, cx: &'a mut TestAppContext) -> (std::path::PathBuf, Entity<Workbench>, &'a mut VisualTestContext) {
    let dir = repo(name);
    let text = |change: &[(usize, &str)]| -> String {
        (1..=LINES).map(|i| format!("{}\n", change.iter().find(|(k, _)| *k == i).map_or(format!("let line_{i} = {i};"), |(_, t)| t.to_string()))).collect()
    };
    std::fs::write(dir.join("src/lib.rs"), text(&[])).unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "long"]);
    std::fs::write(dir.join("src/lib.rs"), text(&[(10, "let changed_ten = 10;"), (100, "let changed_hundred = 100;")])).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| !c.is_empty()));
    (dir, wb, cx)
}

fn open_diff(wb: &Entity<Workbench>, staged: bool, cx: &mut VisualTestContext) -> Entity<DiffView> {
    wb.update_in(cx, |wb, window, cx| wb.open_diff("src/lib.rs".into(), staged, window, cx));
    wait_ticking(cx, "the diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| wb.diff_view()).unwrap()
}

fn folds(view: &Entity<DiffView>, cx: &mut VisualTestContext) -> Vec<(usize, usize)> {
    view.read_with(cx, |v, _| v.items.iter().filter_map(|i| if let Item::Fold { start, len } = i { Some((*start, *len)) } else { None }).collect())
}

fn controls(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> (crate::workbench::a11y::Snapshot, Vec<(accesskit::NodeId, accesskit::Node)>) {
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let nodes = snap.to_tree().nodes;
    (snap, nodes)
}

fn press(wb: &Entity<Workbench>, label: &str, cx: &mut VisualTestContext) {
    let (snap, nodes) = controls(wb, cx);
    let (id, _) =
        nodes.iter().find(|(_, n)| n.role() == accesskit::Role::Button && n.label() == Some(label)).cloned().unwrap_or_else(|| panic!("no button {label:?}"));
    let req = accesskit::ActionRequest { action: accesskit::Action::Click, target_tree: accesskit::TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
    cx.run_until_parked();
}

fn row_of(view: &Entity<DiffView>, line: usize, cx: &mut VisualTestContext) -> usize {
    view.read_with(cx, |v, _| v.rows.iter().position(|r| matches!(r, DiffRow::Changed(_, Some(b)) if *b == line)).unwrap())
}

#[gpui::test]
fn unchanged_regions_fold_to_three_lines_of_context_and_a_click_opens_one(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (_dir, wb, cx) = long_file("diffv2-fold", cx);
    let view = open_diff(&wb, false, cx);
    // Changes at rows 9 and 99: [0, 6) above, [13, 96) between, [103, 120) below.
    assert_eq!(folds(&view, cx), vec![(0, 6), (13, 83), (103, 17)]);
    assert_eq!(view.read_with(cx, |v, _| (v.items.len(), v.rows.len())), (3 + (120 - 6 - 83 - 17), 120));
    // The fold row is a button in the tree, named by what it hides.
    press(&wb, "83 unchanged lines", cx);
    assert_eq!(folds(&view, cx), vec![(0, 6), (103, 17)]);
    assert_eq!(view.read_with(cx, |v, _| v.items.len()), 120 - 6 + 1 - 17 + 1);
}

#[gpui::test]
fn the_header_toggle_and_its_key_fold_and_unfold_everything(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (_dir, wb, cx) = long_file("diffv2-collapse", cx);
    let view = open_diff(&wb, false, cx);
    assert!(view.read_with(cx, |v, _| v.collapse && v.items.len() < v.rows.len()));
    cx.simulate_keystrokes("cmd-k cmd-o");
    assert!(view.read_with(cx, |v, _| !v.collapse && v.items.len() == v.rows.len()));
    assert!(folds(&view, cx).is_empty());
    // The same through the tree, and the button says whether it is on.
    let (_, nodes) = controls(&wb, cx);
    let toggle = nodes.iter().find(|(_, n)| n.label() == Some("Collapse unchanged lines")).expect("the collapse button");
    assert_eq!(toggle.1.toggled(), Some(accesskit::Toggled::False));
    press(&wb, "Collapse unchanged lines", cx);
    assert!(view.read_with(cx, |v, _| v.collapse && v.items.len() < v.rows.len()));
    // Opening one fold, then collapsing again, closes it.
    view.update(cx, |v, cx| v.expand_fold(0, cx));
    assert_eq!(folds(&view, cx).len(), 2);
    cx.simulate_keystrokes("cmd-k cmd-o cmd-k cmd-o");
    assert_eq!(folds(&view, cx).len(), 3);
}

#[gpui::test]
fn folding_leaves_navigation_and_staging_on_the_right_rows(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (dir, wb, cx) = long_file("diffv2-stage", cx);
    let view = open_diff(&wb, false, cx);
    // The first change shows with its context, the fold above it comes into view.
    assert_eq!(view.read_with(cx, |v, _| v.change_scroll_target(0)), Some(0));
    // F7 moves to the second change: three context rows and the fold above them.
    let (row, at) = view.read_with(cx, |v, _| (v.hunks[1], v.list_index(v.hunks[1])));
    let target = view.read_with(cx, |v, _| v.change_scroll_target(1)).unwrap();
    assert_eq!(target, at - 4);
    assert!(view.read_with(cx, |v, _| matches!(v.items[target], Item::Fold { .. })));
    cx.simulate_keystrokes("f7");
    assert_eq!(view.read_with(cx, |v, _| v.current), 1);
    // Pick the second change's row (a row index of the whole diff, not of the folded list) and stage it.
    let picked = row_of(&view, 99, cx);
    assert_eq!(picked, row);
    view.update(cx, |v, cx| v.click_row(picked, false, cx));
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "the index", |_| git_in(&dir, &["show", ":src/lib.rs"]).contains("changed_hundred"));
    let staged = git_in(&dir, &["show", ":src/lib.rs"]);
    assert!(!staged.contains("changed_ten"), "only the picked change");
    wait_ticking(cx, "the reload", |cx| view.read_with(cx, |v, _| v.blocks.len() == 1 && !v.busy));
    // The reloaded diff folds again around the one change left.
    assert_eq!(view.read_with(cx, |v, _| v.items.iter().filter(|i| matches!(i, Item::Fold { .. })).count()), 2);
}

#[gpui::test]
fn the_ruler_marks_each_change_in_proportion_and_follows_the_view(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (_dir, wb, cx) = long_file("diffv2-ruler", cx);
    let view = open_diff(&wb, false, cx);
    let total = view.read_with(cx, |v, _| v.items.len());
    let spans = view.read_with(cx, |v, _| v.ruler_spans.clone());
    assert_eq!(spans.len(), 2);
    assert!(spans.iter().all(|s| s.kind == Kind::Modified && s.items.len() == 1));
    // Item 0 is the first fold, then three context rows, then the first change.
    assert_eq!(spans[0].items.start, 1 + 3);
    assert!(spans[1].items.start > spans[0].items.end && spans[1].items.end <= total);
    // Inline: the same changes as a removed and an added line each.
    cx.simulate_keystrokes("cmd-k cmd-i");
    let inline = view.read_with(cx, |v, _| v.ruler_spans.iter().map(|s| (s.kind, s.items.len())).collect::<Vec<_>>());
    assert_eq!(inline, vec![(Kind::Removed, 1), (Kind::Added, 1), (Kind::Removed, 1), (Kind::Added, 1)]);
    // Without folds the ruler is over the whole file.
    cx.simulate_keystrokes("cmd-k cmd-o");
    assert_eq!(view.read_with(cx, |v, _| (v.items.len(), v.ruler_spans[0].items.start)), (122, 9));
    // The accessibility tree has the lane as a slider, and a press moves the list.
    let (_, nodes) = controls(&wb, cx);
    let lane = nodes.iter().find(|(_, n)| n.role() == accesskit::Role::Slider && n.label() == Some("Change overview")).expect("the ruler");
    assert_eq!(lane.1.description(), Some("2 changes"));
}

#[gpui::test]
fn a_click_on_the_ruler_scrolls_there_and_f7_is_outlined(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (_dir, wb, cx) = long_file("diffv2-ruler-click", cx);
    let view = open_diff(&wb, false, cx);
    // The whole file, so the list is taller than the window.
    cx.simulate_keystrokes("cmd-k cmd-o");
    cx.run_until_parked();
    view.update(cx, |v, cx| v.scroll_to_fraction(0.5, cx));
    cx.run_until_parked();
    let halfway = view.read_with(cx, |v, _| f32::from(v.scroll.0.borrow().base_handle.offset().y));
    assert!(halfway < 0.);
    // The list was laid out, so the thumb has a viewport to show.
    let (viewport, content) =
        view.read_with(cx, |v, _| v.scroll.0.borrow().last_item_size.map(|s| (f32::from(s.item.height), f32::from(s.contents.height)))).unwrap();
    assert!(viewport > 0. && content > viewport, "{viewport} {content}");
    assert!((halfway - crate::diff::ruler::offset_for(0.5, viewport, content)).abs() < 1., "centered on the middle: {halfway}");
    view.update(cx, |v, cx| v.scroll_to_fraction(1.0, cx));
    let end = view.read_with(cx, |v, _| f32::from(v.scroll.0.borrow().base_handle.offset().y));
    assert!(end < halfway, "further down: {end} < {halfway}");
    view.update(cx, |v, cx| v.scroll_to_fraction(0.0, cx));
    assert_eq!(view.read_with(cx, |v, _| f32::from(v.scroll.0.borrow().base_handle.offset().y)), 0.);
    // The change F7 is on is the one the ruler outlines.
    assert_eq!(view.read_with(cx, |v, _| v.current), 0);
    cx.simulate_keystrokes("f7");
    assert_eq!(view.read_with(cx, |v, _| v.current), 1);
    // The tree's slider scrolls too.
    let (snap, nodes) = controls(&wb, cx);
    let (id, _) = nodes.iter().find(|(_, n)| n.label() == Some("Change overview")).cloned().unwrap();
    let req = accesskit::ActionRequest {
        action: accesskit::Action::SetValue,
        target_tree: accesskit::TreeId::ROOT,
        target_node: id,
        data: Some(accesskit::ActionData::NumericValue(50.)),
    };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
    assert!(view.read_with(cx, |v, _| f32::from(v.scroll.0.borrow().base_handle.offset().y)) < 0.);
}

#[gpui::test]
fn ignoring_whitespace_recomputes_the_diff_and_turns_staging_off(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = repo("diffv2-ws");
    let base: String = (1..=10).map(|i| format!("fn f{i}() {{}}\n")).collect();
    std::fs::write(dir.join("src/lib.rs"), &base).unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "ws"]);
    // Line 3: trailing blanks; line 5: indentation; line 8: a real change.
    let edited = base.replace("fn f3() {}\n", "fn f3() {}   \n").replace("fn f5() {}\n", "    fn f5() {}\n").replace("fn f8() {}\n", "fn g8() {}\n");
    std::fs::write(dir.join("src/lib.rs"), edited).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| !c.is_empty()));
    let view = open_diff(&wb, false, cx);
    assert_eq!(view.read_with(cx, |v, _| (v.blocks.len(), v.can_stage())), (3, true));
    cx.simulate_keystrokes("cmd-k cmd-h");
    assert_eq!(view.read_with(cx, |v, _| (v.ws, v.blocks.len(), v.can_stage())), (Whitespace::Trailing, 2, false));
    cx.simulate_keystrokes("cmd-k cmd-h");
    assert_eq!(view.read_with(cx, |v, _| (v.ws, v.blocks.len())), (Whitespace::All, 1));
    assert_eq!(view.read_with(cx, |v, _| v.summary()), (1, 1, 1), "stats follow the recomputed diff");
    // The button is the same as the key and cycles back to the exact diff.
    press(&wb, "Ignore whitespace", cx);
    assert_eq!(view.read_with(cx, |v, _| (v.ws, v.blocks.len(), v.can_stage())), (Whitespace::Exact, 3, true));
}

#[gpui::test]
fn a_difference_in_whitespace_alone_says_so(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = repo("diffv2-ws-only");
    std::fs::write(dir.join("src/lib.rs"), (1..=10).map(|i| format!("// satır {i}  \n")).collect::<String>()).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| !c.is_empty()));
    let view = open_diff(&wb, false, cx);
    cx.simulate_keystrokes("cmd-k cmd-h");
    assert!(view.read_with(cx, |v, _| v.blocks.is_empty() && v.ws.ignores()));
    let (_, nodes) = controls(&wb, cx);
    assert!(nodes.iter().any(|(_, n)| n.label() == Some("The two sides differ only in whitespace")));
}

#[gpui::test]
fn binary_huge_and_non_utf8_texts_get_a_message_not_garbage(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (_dir, wb, cx) = long_file("diffv2-guard", cx);
    let view = open_diff(&wb, false, cx);
    view.update(cx, |v, cx| {
        v.set_texts("PNG\0\u{1}\u{2}", "PNG\0\u{3}");
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| (v.unsupported, v.rows.len(), v.can_stage())), (Some(Unsupported::Binary), 0, false));
    let (_, nodes) = controls(&wb, cx);
    assert!(nodes.iter().any(|(_, n)| n.label() == Some("This file is binary, so there is no line diff to show")));
    // The list is not even drawn: no rows, no ruler.
    assert!(!nodes.iter().any(|(_, n)| n.label() == Some("Change overview")));
    let big = "x\n".repeat(crate::diff::guard::MAX_BYTES / 2 + 1);
    view.update(cx, |v, cx| {
        v.set_texts("x\n", &big);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(matches!(view.read_with(cx, |v, _| v.unsupported), Some(Unsupported::TooLarge(_))));
    let (_, nodes) = controls(&wb, cx);
    assert!(nodes.iter().any(|(_, n)| n.label().is_some_and(|l| l.starts_with("This file is too large to compare here"))));
    // Not UTF-8: the diff still shows, with a warning.
    let text = String::from_utf8_lossy(b"caf\xe9\n").into_owned();
    view.update(cx, |v, cx| {
        v.set_texts("cafe\n", &text);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| (v.unsupported, v.invalid_bytes, v.rows.len())), (None, true, 1));
    let (_, nodes) = controls(&wb, cx);
    assert!(nodes.iter().any(|(_, n)| n.label().is_some_and(|l| l.starts_with("Some bytes are not valid UTF-8"))));
}

#[gpui::test]
fn known_languages_are_colored_on_both_sides_and_unknown_ones_are_not(cx: &mut TestAppContext) {
    let (_dir, wb, cx) = long_file("diffv2-syntax", cx);
    let view = open_diff(&wb, false, cx);
    let (left, right) = view.read_with(cx, |v, _| v.colored_lines());
    assert!(left > 100 && right > 100, "keywords color most lines of both sides: {left}, {right}");
    let unknown = wb.update_in(cx, |_, _, cx| {
        let view = cx.new(|cx| DiffView::new("notes.zzz".into(), false, "let a = 1;\n", "let a = 2;\n", cx));
        view.read(cx).colored_lines()
    });
    assert_eq!(unknown, (0, 0));
}

#[gpui::test]
fn the_header_counts_changes_and_its_buttons_move_between_them(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (_dir, wb, cx) = long_file("diffv2-header", cx);
    let view = open_diff(&wb, false, cx);
    let (_, nodes) = controls(&wb, cx);
    let label = |wanted: &str| nodes.iter().any(|(_, n)| n.label() == Some(wanted));
    for button in ["Previous change", "Next change", "Copy path", "Collapse unchanged lines", "Ignore whitespace", "Toggle inline view"] {
        assert!(label(button), "{button}");
    }
    assert!(label("Change 1 of 2"), "the counter is read out");
    press(&wb, "Next change", cx);
    assert_eq!(view.read_with(cx, |v, _| v.current), 1);
    assert!(controls(&wb, cx).1.iter().any(|(_, n)| n.label() == Some("Change 2 of 2")));
    press(&wb, "Next change", cx);
    assert_eq!(view.read_with(cx, |v, _| v.current), 0, "wraps around");
    press(&wb, "Previous change", cx);
    assert_eq!(view.read_with(cx, |v, _| v.current), 1);
    // Both sides are named from the label.
    assert_eq!(view.read_with(cx, |v, _| v.titles.clone().map(|(l, r)| (l.to_string(), r.to_string()))), Some(("Index".into(), "Working Tree".into())));
    // Copy path puts the file's path on the clipboard.
    press(&wb, "Copy path", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some("src/lib.rs".to_string()));
    assert!(view.read_with(cx, |v, _| v.copied));
    cx.executor().advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    assert!(!view.read_with(cx, |v, _| v.copied));
}

#[gpui::test]
fn with_no_changes_nothing_folds_and_f7_stays_put(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (_dir, wb, cx) = long_file("diffv2-same", cx);
    let view = open_diff(&wb, false, cx);
    view.update(cx, |v, cx| {
        v.set_texts("a\nb\n", "a\nb\n");
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| (v.hunks.len(), v.items.len())), (0, 2), "nothing folds when there is no change");
    let before = view.read_with(cx, |v, _| v.current);
    cx.simulate_keystrokes("f7");
    assert_eq!(view.read_with(cx, |v, _| v.current), before);
}

#[test]
fn inline_sources_walks_the_blocks_once() {
    // 60k rows, every third one changed: a lookup of the block per row would take minutes.
    let rows: Vec<DiffRow> = (0..60_000).map(|i| if i % 3 == 0 { DiffRow::Changed(Some(i), Some(i)) } else { DiffRow::Same(i, i) }).collect();
    let blocks = crate::git::blocks(&rows);
    assert_eq!(blocks.len(), 20_000);
    let started = std::time::Instant::now();
    let sources = super::inline_sources(&rows, &blocks);
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    // Every changed row shows twice (removed, then added), every unchanged row once.
    assert_eq!(sources.len(), 60_000 + 20_000);
    assert_eq!(sources[..4], [0, 0, 1, 2]);
}

#[gpui::test]
fn text_that_is_not_utf8_shows_but_cannot_be_staged(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = repo("diffv2-latin1");
    std::fs::write(dir.join("latin.txt"), b"caf\xe9\nx\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "latin"]);
    std::fs::write(dir.join("latin.txt"), b"caf\xe9\ny\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| c.iter().any(|(p, ..)| p == "latin.txt")));
    wb.update_in(cx, |wb, window, cx| wb.open_diff("latin.txt".into(), false, window, cx));
    wait_ticking(cx, "the diff", |cx| wb.read_with(cx, |wb, cx| wb.diff_view().is_some_and(|v| v.read(cx).path == "latin.txt")));
    cx.run_until_parked();
    let view = wb.read_with(cx, |wb, _| wb.diff_view()).unwrap();
    assert_eq!(view.read_with(cx, |v, _| (v.invalid_bytes, v.can_stage(), v.blocks.len())), (true, false, 1));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(git_in(&dir, &["diff", "--cached", "--name-only"]).is_empty(), "nothing staged");
    // The header says why there is no staging.
    let (_, nodes) = controls(&wb, cx);
    assert!(nodes.iter().any(|(_, n)| n.label().is_some_and(|l| l.starts_with("Some bytes are not valid UTF-8"))));
}

#[gpui::test]
fn folds_the_reader_opened_stay_open_after_a_stage_reloads_the_diff(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (_dir, wb, cx) = long_file("diffv2-keepopen", cx);
    let view = open_diff(&wb, false, cx);
    view.update(cx, |v, cx| v.expand_fold(0, cx));
    assert_eq!(folds(&view, cx), vec![(13, 83), (103, 17)]);
    let row = row_of(&view, 99, cx);
    view.update(cx, |v, cx| v.click_row(row, false, cx));
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "the reload", |cx| view.read_with(cx, |v, _| v.blocks.len() == 1 && !v.busy));
    // One change is left; the fold above it is still open.
    let left = folds(&view, cx);
    assert!(left.iter().all(|(start, _)| *start != 0), "{left:?}");
    assert!(view.read_with(cx, |v, _| v.expanded.contains(&0)));
}

#[gpui::test]
fn changing_the_whitespace_option_keeps_the_syntax_colors(cx: &mut TestAppContext) {
    let (_dir, wb, cx) = long_file("diffv2-keepcolors", cx);
    let view = open_diff(&wb, false, cx);
    // A marker in place of the colors survives: the toggle lays the rows out again but does not parse again.
    view.update(cx, |v, _| v.syn_left = vec![vec![(1, None)]]);
    cx.simulate_keystrokes("cmd-k cmd-h");
    assert_eq!(view.read_with(cx, |v, _| (v.ws, v.syn_left.clone())), (Whitespace::Trailing, vec![vec![(1, None)]]));
}

#[gpui::test]
fn the_ruler_slider_runs_from_the_top_to_the_end_and_nudges_from_either(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (_dir, wb, cx) = long_file("diffv2-slider", cx);
    let view = open_diff(&wb, false, cx);
    cx.simulate_keystrokes("cmd-k cmd-o");
    cx.run_until_parked();
    let progress = |view: &Entity<DiffView>, cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            let s = v.scroll.0.borrow();
            let (viewport, content) = s.last_item_size.map(|z| (f32::from(z.item.height), f32::from(z.contents.height))).unwrap();
            crate::diff::ruler::progress(f32::from(s.base_handle.offset().y), viewport, content)
        })
    };
    view.update(cx, |v, cx| v.scroll_to_progress(1.0, cx));
    cx.run_until_parked();
    assert_eq!(progress(&view, cx), 1.0);
    let value = |nodes: &[(accesskit::NodeId, accesskit::Node)]| {
        nodes.iter().find(|(_, n)| n.label() == Some("Change overview")).map(|(id, n)| (*id, n.numeric_value().unwrap()))
    };
    let (snap, nodes) = controls(&wb, cx);
    let (id, at_end) = value(&nodes).unwrap();
    assert_eq!(at_end, 100., "the slider says the end");
    // Decrement moves back by one step (10%) of the way, not to some other scale.
    let req = accesskit::ActionRequest { action: accesskit::Action::Decrement, target_tree: accesskit::TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
    cx.run_until_parked();
    assert!((progress(&view, cx) - 0.9).abs() < 0.01, "{}", progress(&view, cx));
    assert!((value(&controls(&wb, cx).1).unwrap().1 - 90.).abs() < 1.);
    // And from the top, Increment goes down by a step.
    view.update(cx, |v, cx| v.scroll_to_progress(0.0, cx));
    cx.run_until_parked();
    let (snap, nodes) = controls(&wb, cx);
    let (id, _) = value(&nodes).unwrap();
    let req = accesskit::ActionRequest { action: accesskit::Action::Increment, target_tree: accesskit::TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
    cx.run_until_parked();
    assert!((progress(&view, cx) - 0.1).abs() < 0.01);
}
