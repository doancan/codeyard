//! Live search counts follow output without taking selection, scrolling or focus away.

use std::sync::Arc;
use std::time::Duration;

use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use gpui::{Bounds, Entity, TestAppContext, VisualTestContext, point, size};

use super::*;

fn setup(cx: &mut TestAppContext) -> (Entity<TerminalView>, &mut VisualTestContext) {
    crate::qa_workbench_tests::globals(cx);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(Arc::new(host::local()), "find".into(), cx));
    // The exact debounce tests use a single count batch. A maximized TestDisplay
    // has more than COUNT_ROWS_PER_TURN rows and deliberately needs another turn.
    cx.simulate_resize(size(px(640.), px(400.)));
    cx.run_until_parked();
    (term, cx)
}

fn query(term: &Entity<TerminalView>, text: &str, cx: &mut VisualTestContext) -> usize {
    term.update_in(cx, |term, window, cx| term.find_for_test(text, window, cx).0)
}

fn settle(cx: &mut VisualTestContext, milliseconds: u64) {
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(milliseconds));
    cx.run_until_parked();
}

#[gpui::test]
fn live_output_updates_the_count_and_clear_removes_old_results(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    assert_eq!(query(&term, "needle", cx), 0);
    term.update(cx, |term, cx| term.append(b"a needle arrived\n", cx));
    settle(cx, 149);
    assert_eq!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().total), 0, "recounts wait for the quiet period");
    settle(cx, 1);
    assert_eq!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().total), 1);
    assert!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().current.is_none()), "new output does not select or jump to a result");
    term.update(cx, |term, cx| term.reset_output(cx));
    settle(cx, 150);
    assert_eq!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().total), 0, "clearing output updates the empty state");
}

#[gpui::test]
fn a_tall_view_yields_after_the_debounce_before_finishing_its_remaining_rows(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    let height = term.read_with(cx, |term, _| term.line_h() * (COUNT_ROWS_PER_TURN + 2) as f32 + px(crate::terminal::PAD_Y * 2.));
    cx.simulate_resize(size(px(640.), height));
    cx.run_until_parked();
    assert_eq!(term.read_with(cx, |term, _| term.term.screen_lines()), COUNT_ROWS_PER_TURN + 2);
    query(&term, "needle", cx);
    term.update(cx, |term, cx| term.append(b"needle\n", cx));
    settle(cx, 150);
    term.read_with(cx, |term, _| {
        let find = term.find.as_ref().unwrap();
        assert_eq!(find.total, 0, "a partial scan does not publish an incomplete count");
        assert!(find.refresh_pending.is_some(), "the remaining rows wait for another UI turn");
    });
    settle(cx, 1);
    term.read_with(cx, |term, _| {
        let find = term.find.as_ref().unwrap();
        assert_eq!(find.total, 1);
        assert!(find.refresh_pending.is_none());
    });
}

#[gpui::test]
fn output_bursts_replace_the_pending_recount(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    query(&term, "needle", cx);
    term.update(cx, |term, cx| term.append(b"needle ", cx));
    settle(cx, 100);
    term.update(cx, |term, cx| term.append(b"needle", cx));
    settle(cx, 100);
    assert_eq!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().total), 0, "the superseded timer cannot publish a result");
    settle(cx, 50);
    assert_eq!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().total), 2);
}

#[gpui::test]
fn refreshing_results_preserves_the_selected_match_scroll_and_find_focus(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    term.update(cx, |term, cx| term.append(b"first needle\nsecond needle\n", cx));
    assert_eq!(query(&term, "needle", cx), 2);
    let before = term.read_with(cx, |term, _| (term.find.as_ref().unwrap().current.clone(), term.term.grid().display_offset()));
    term.update(cx, |term, cx| term.append(b"third needle\n", cx));
    settle(cx, 150);
    let after = term.read_with(cx, |term, _| (term.find.as_ref().unwrap().current.clone(), term.term.grid().display_offset()));
    assert_eq!(after, before, "a recount must not move the selected result or scroll position");
    assert_eq!(term.update_in(cx, |term, window, cx| term.find_state_for_test(window, cx)), (true, 3));
}

#[gpui::test]
fn changing_query_or_closing_find_cancels_pending_output_work(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    query(&term, "needle", cx);
    term.update(cx, |term, cx| term.append(b"needle ", cx));
    settle(cx, 100);
    assert_eq!(query(&term, "other", cx), 0);
    settle(cx, 150);
    assert_eq!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().total), 0, "the previous query cannot overwrite the new count");
    term.update(cx, |term, cx| term.append(b"other", cx));
    settle(cx, 100);
    term.update_in(cx, |term, window, cx| term.close_find(window, cx));
    settle(cx, 200);
    assert!(term.read_with(cx, |term, _| term.find.is_none()), "a closed search stays closed after its old timer");
    assert!(term.update_in(cx, |term, window, _| term.focus_handle.is_focused(window)), "Escape returns focus to the terminal");
}

#[gpui::test]
fn find_controls_fit_inside_a_narrow_terminal_at_two_hundred_percent(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    cx.update(|_, cx| cx.set_ui_scale(2.));
    cx.simulate_resize(size(px(480.), px(600.)));
    query(&term, "needle", cx);
    cx.run_until_parked();
    let viewport = cx.update(|window, _| window.viewport_size());
    assert_eq!(viewport.width, px(240.));
    let bounds = Bounds::new(point(px(0.), px(0.)), viewport);
    let bar = cx.debug_bounds("term-find-bar").expect("find bar");
    assert!(bar.left() >= bounds.left() && bar.right() <= bounds.right(), "find bar {bar:?} fits {bounds:?}");
    for selector in ["term-find-prev", "term-find-next", "term-find-close"] {
        let button = cx.debug_bounds(selector).expect("find control");
        assert!(bounds.contains(&button.center()), "{selector} remains reachable inside {bounds:?}");
    }
}

fn parser_term(cols: usize, rows: usize, data: &[u8]) -> Term<crate::terminal::Listener> {
    let mut term = Term::new(Config::default(), &crate::terminal::TermSize { cols, rows }, crate::terminal::Listener::default());
    Processor::<StdSyncHandler>::default().advance(&mut term, data);
    term
}

#[test]
fn a_batched_recount_does_not_lose_or_double_count_matches_across_wrapped_rows() {
    let data = format!("{}    needle\r\nneedle", "\r\n".repeat(COUNT_ROWS_PER_TURN - 1));
    let term = parser_term(8, COUNT_ROWS_PER_TURN + 6, data.as_bytes());
    let mut scan =
        CountScan { regex: compile("needle").unwrap(), next: GridPoint::new(Line(0), Column(0)), bottom: term.bottommost_line(), overlap: 2, total: 0 };
    assert!(!scan.advance(&term));
    assert_eq!(scan.total, 1, "the match spanning the batch boundary belongs to the first batch");
    assert!(scan.advance(&term));
    assert_eq!(scan.total, 2);
}

#[test]
fn batch_boundaries_keep_the_non_overlapping_search_position() {
    let term = parser_term(8, 130, "a".repeat(1001).as_bytes());
    let mut scan = CountScan { regex: compile("aaa").unwrap(), next: GridPoint::new(Line(0), Column(0)), bottom: term.bottommost_line(), overlap: 1, total: 0 };
    while !scan.advance(&term) {}
    assert_eq!(scan.total, 333, "a match crossing a row batch must not be counted twice");
}

#[gpui::test]
fn case_insensitive_selected_results_survive_unrelated_output(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    term.update(cx, |term, cx| term.append(b"NEEDLE\n", cx));
    assert_eq!(query(&term, "needle", cx), 1);
    let before = term.read_with(cx, |term, _| term.find.as_ref().unwrap().current.clone());
    assert!(before.is_some());
    term.update(cx, |term, cx| term.append(b"unrelated output\n", cx));
    settle(cx, 150);
    assert_eq!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().current.clone()), before);
}

#[gpui::test]
fn resizing_during_the_debounce_restarts_with_the_current_grid(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    query(&term, "needle", cx);
    term.update(cx, |term, cx| term.append(b"needle\nneedle\nneedle\n", cx));
    settle(cx, 100);
    let generation = term.read_with(cx, |term, _| term.find.as_ref().unwrap().generation);
    cx.simulate_resize(size(px(240.), px(200.)));
    cx.run_until_parked();
    assert!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().generation) > generation);
    settle(cx, 150);
    assert_eq!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().total), 3);
}

#[gpui::test]
fn resizing_between_count_batches_cancels_the_old_grid_scan(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    term.update(cx, |term, cx| term.append("unrelated\n".repeat(200).as_bytes(), cx));
    query(&term, "needle", cx);
    term.update(cx, |term, cx| term.append(b"needle\n", cx));
    settle(cx, 150);
    assert!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().refresh_pending.is_some()), "long recount yields after its first batch");
    let generation = term.read_with(cx, |term, _| term.find.as_ref().unwrap().generation);
    cx.simulate_resize(size(px(240.), px(200.)));
    cx.run_until_parked();
    assert!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().generation) > generation);
    settle(cx, 150);
    for _ in 0..8 {
        settle(cx, 1);
    }
    assert_eq!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().total), 1);
    assert!(term.read_with(cx, |term, _| term.find.as_ref().unwrap().refresh_pending.is_none()));
}

#[test]
fn every_visible_match_is_available_for_highlighting_beyond_the_first_five_hundred() {
    let term = parser_term(80, 30, "a".repeat(80 * 30).as_bytes());
    let found = visible_matches(&term, &mut compile("a").unwrap(), 0, 30);
    assert_eq!(found.len(), 80 * 30);
    assert!(found.last().unwrap().contains(&GridPoint::new(Line(29), Column(79))));
}
