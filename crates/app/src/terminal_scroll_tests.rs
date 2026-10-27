use super::*;
use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, point};

/// Small trackpad events must add up, rather than each being rounded away.
#[gpui::test]
fn terminal_wheel_preserves_small_trackpad_movements(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(Arc::new(host::local()), "test".into(), cx));
    term.update(cx, |t, cx| {
        t.feed("line\r\n".repeat(200).as_bytes());
        cx.notify();
    });
    cx.run_until_parked();
    let line_h = term.read_with(cx, |t, _| t.line_h());
    for _ in 0..8 {
        term.update_in(cx, |t, window, cx| {
            t.on_scroll(
                &ScrollWheelEvent {
                    position: t.cell_center(5, 5),
                    delta: ScrollDelta::Pixels(point(px(0.), line_h / 8.)),
                    modifiers: Modifiers::none(),
                    touch_phase: TouchPhase::Moved,
                },
                window,
                cx,
            );
        });
    }
    assert!(term.read_with(cx, |t, _| t.term.grid().display_offset()) >= 1, "a line of trackpad motion must scroll the terminal");
}
