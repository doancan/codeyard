//! A program that redraws its last frame in place (an expanding and collapsing section) must leave no blank gap behind.

use super::*;
use gpui::TestAppContext;

fn setup(cx: &mut TestAppContext) -> (gpui::Entity<TerminalView>, &mut gpui::VisualTestContext) {
    crate::qa_workbench_tests::globals(cx);
    cx.add_window_view(|_, cx| TerminalView::output_only(Arc::new(host::local()), "test".into(), cx))
}

/// What a line-redrawing program sends to replace a frame of `lines` rows with `next`: back up, erase each line, write.
fn replace_frame(lines: usize, next: &str) -> String {
    let mut out = String::new();
    for i in 0..lines {
        out.push_str("\x1b[2K");
        if i + 1 < lines {
            out.push_str("\x1b[1A");
        }
    }
    out.push('\r');
    out.push_str(next);
    out
}

fn frame(prefix: &str, n: usize) -> String {
    (1..=n).map(|i| format!("{prefix}{i}")).collect::<Vec<_>>().join("\r\n")
}

/// The lines of the screen (trailing blank rows dropped).
fn lines(t: &TerminalView) -> Vec<String> {
    let mut v: Vec<String> = t.screen_text().lines().map(str::to_string).collect();
    while v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

#[gpui::test]
fn collapsing_a_frame_that_fits_leaves_no_gap(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    term.update(cx, |t, _| {
        t.feed(b"header\r\n");
        t.feed(frame("open ", 10).as_bytes());
        t.feed(replace_frame(10, &frame("shut ", 2)).as_bytes());
    });
    let got = term.read_with(cx, |t, _| lines(t));
    assert_eq!(got, ["header", "shut 1", "shut 2"]);
}

#[gpui::test]
fn collapsing_a_frame_taller_than_the_screen_leaves_no_gap(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    let rows = term.read_with(cx, |t, _| t.term.screen_lines());
    term.update(cx, |t, _| {
        t.feed(b"header\r\n");
        t.feed(frame("open ", rows + 15).as_bytes());
        t.feed(replace_frame(rows + 15, &frame("shut ", 2)).as_bytes());
    });
    let got = term.read_with(cx, |t, _| lines(t));
    assert_eq!(got.last().map(String::as_str), Some("shut 2"), "{got:?}");
    assert_eq!(got.iter().rev().nth(1).map(String::as_str), Some("shut 1"), "{got:?}");
}

#[gpui::test]
fn collapsing_while_scrolled_back_keeps_the_view_inside_the_history(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    term.update(cx, |t, _| {
        t.feed("old\r\n".repeat(100).as_bytes());
        t.feed(frame("open ", 30).as_bytes());
        t.term.scroll_display(alacritty_terminal::grid::Scroll::Delta(40));
        t.feed(replace_frame(30, &frame("shut ", 2)).as_bytes());
    });
    let (offset, history) = term.read_with(cx, |t, _| t.a11y_scroll());
    assert!(offset <= history, "offset {offset} history {history}");
}

#[gpui::test]
fn a_collapse_inside_a_synchronized_update_is_applied_at_its_end(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    term.update(cx, |t, _| {
        t.feed(b"header\r\n");
        t.feed(format!("\x1b[?2026h{}\x1b[?2026l", frame("open ", 10)).as_bytes());
        t.feed(format!("\x1b[?2026h{}\x1b[?2026l", replace_frame(10, &frame("shut ", 2))).as_bytes());
    });
    let got = term.read_with(cx, |t, _| lines(t));
    assert_eq!(got, ["header", "shut 1", "shut 2"]);
}

#[gpui::test]
fn a_collapse_split_across_reads_is_applied_once_complete(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    let collapse = format!("\x1b[?2026h{}\x1b[?2026l", replace_frame(10, &frame("shut ", 2)));
    let (a, b) = collapse.as_bytes().split_at(collapse.len() / 2);
    term.update(cx, |t, _| {
        t.feed(b"header\r\n");
        t.feed(frame("open ", 10).as_bytes());
        t.feed(a);
        t.feed(b);
    });
    let got = term.read_with(cx, |t, _| lines(t));
    assert_eq!(got, ["header", "shut 1", "shut 2"]);
}

/// A frame redrawn with the cursor addressed absolutely (`CSI row;col H`) and erased with `CSI J`, as full-screen layouts do.
#[gpui::test]
fn collapsing_with_absolute_moves_and_erase_below_leaves_no_gap(cx: &mut TestAppContext) {
    let (term, cx) = setup(cx);
    term.update(cx, |t, _| {
        t.feed(b"\x1b[2J\x1b[H");
        t.feed(frame("open ", 12).as_bytes());
        t.feed(b"\x1b[3;1H\x1b[J");
        t.feed(b"shut 1\r\nshut 2");
    });
    let got = term.read_with(cx, |t, _| lines(t));
    assert_eq!(got, ["open 1", "open 2", "shut 1", "shut 2"]);
}
