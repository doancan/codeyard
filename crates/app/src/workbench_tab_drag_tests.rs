//! A tab dragged out of the window: hit testing of other windows, a window of its own for the same document, and a
//! move to another Codeyard window.

use super::*;
use crate::qa_workbench_tests::{fresh, open, open_in, setup};
use gpui::{Modifiers, VisualTestContext, point, size};

#[test]
fn the_front_most_window_under_the_pointer_takes_the_drop() {
    let b = |x: f32, y: f32, w: f32, h: f32| Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
    let windows = [(1, b(100., 100., 400., 300.)), (2, b(0., 0., 1000., 800.)), (3, b(2000., 0., 800., 600.))];
    assert_eq!(window_at(point(px(150.), px(150.)), &windows), Some(1), "in front of the bigger one behind it");
    assert_eq!(window_at(point(px(50.), px(50.)), &windows), Some(2));
    assert_eq!(window_at(point(px(2100.), px(10.)), &windows), Some(3), "on another display");
    assert_eq!(window_at(point(px(1500.), px(10.)), &windows), None, "between windows: a window of its own");
    assert_eq!(window_at(point(px(-5.), px(-5.)), &windows), None);
}

/// Dragging a tab and releasing it outside the window: an editor window of its own showing the same document, and the
/// tab leaves the group.
#[gpui::test]
fn a_tab_released_outside_the_window_becomes_a_window_of_its_own(cx: &mut gpui::TestAppContext) {
    let dir = fresh("tabdrag-out");
    std::fs::write(dir.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(dir.join("b.txt"), "beta\n").unwrap();
    let (wb, vcx) = setup(cx);
    vcx.simulate_resize(size(px(1000.), px(700.)));
    open(&wb, &dir, vcx);
    open_in(&wb, &dir.join("a.txt"), vcx);
    open_in(&wb, &dir.join("b.txt"), vcx);
    let b = wb.read_with(vcx, |wb, _| wb.tabs.editors[1].clone());
    let windows = vcx.update(|_, cx| cx.windows().len());
    let start = vcx.debug_bounds("tab-0-1").expect("the second tab is drawn").center();
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(start + point(px(30.), px(10.)), Some(MouseButton::Left), Modifiers::none());
    let outside = point(px(1200.), px(300.));
    vcx.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::none());
    vcx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| cx.windows().len()), windows + 1, "a new window");
    wb.read_with(vcx, |wb, cx| {
        assert_eq!(wb.chrome.editor_windows.len(), 1);
        assert_eq!(wb.chrome.editor_windows[0].editor, b, "the same document, not a copy");
        let names: Vec<String> = wb.tabs.editors.iter().map(|e| e.read(cx).name.to_string()).collect();
        assert_eq!(names, ["a.txt"], "its tab left the group");
        assert!(wb.chrome.tab_drag.is_none());
    });
}

/// A drag released inside the window changes nothing here (the drop targets handle it), and one that never started
/// leaves nothing behind.
#[gpui::test]
fn a_tab_released_inside_the_window_stays(cx: &mut gpui::TestAppContext) {
    let dir = fresh("tabdrag-in");
    std::fs::write(dir.join("a.txt"), "alpha\n").unwrap();
    let (wb, vcx) = setup(cx);
    vcx.simulate_resize(size(px(1000.), px(700.)));
    open(&wb, &dir, vcx);
    open_in(&wb, &dir.join("a.txt"), vcx);
    let start = vcx.debug_bounds("tab-0-0").expect("the tab is drawn").center();
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(start + point(px(30.), px(10.)), Some(MouseButton::Left), Modifiers::none());
    vcx.simulate_mouse_move(point(px(500.), px(400.)), Some(MouseButton::Left), Modifiers::none());
    vcx.simulate_mouse_up(point(px(500.), px(400.)), MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(wb.chrome.editor_windows.is_empty() && wb.tabs.editors.len() == 1 && wb.chrome.tab_drag.is_none());
    });
}

/// Dropped on another Codeyard window: a clean file opens there and its tab here closes; a file with unsaved changes
/// stays and says why.
#[gpui::test]
fn a_tab_dropped_on_another_window_opens_there(cx: &mut gpui::TestAppContext) {
    let dir = fresh("tabdrag-other");
    std::fs::write(dir.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(dir.join("b.txt"), "beta\n").unwrap();
    let (wb, vcx) = setup(cx);
    open(&wb, &dir, vcx);
    open_in(&wb, &dir.join("a.txt"), vcx);
    open_in(&wb, &dir.join("b.txt"), vcx);
    let (other, ocx) = vcx.add_window_view(|window, cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx));
    let mut ocx = VisualTestContext::from_window(ocx.window_handle(), ocx);
    open(&other, &dir, &mut ocx);
    let target = ocx.window_handle().downcast::<Workbench>().unwrap();
    let (a, b, from) = wb.read_with(vcx, |wb, _| (wb.tabs.editors[0].clone(), wb.tabs.editors[1].clone(), wb.tabs.current_key()));
    b.update(vcx, |e, cx| e.insert_at_for_test(0, "x", cx));
    wb.update_in(vcx, |wb, window, cx| wb.move_tab_to_window(b.clone(), from, target, window, cx));
    vcx.run_until_parked();
    assert_eq!(wb.read_with(vcx, |wb, _| wb.tabs.editors.len()), 2, "a tab with unsaved changes stays");
    assert!(wb.read_with(vcx, |wb, _| wb.notice.is_some()));
    wb.update_in(vcx, |wb, window, cx| wb.move_tab_to_window(a.clone(), from, target, window, cx));
    vcx.run_until_parked();
    let ocx = VisualTestContext::from_window(target.into(), vcx);
    ocx.run_until_parked();
    assert_eq!(wb.read_with(vcx, |wb, _| wb.tabs.editors.len()), 1, "its tab here closed");
    let there: Vec<String> = other.read_with(&ocx, |wb, cx| wb.tabs.editors.iter().map(|e| e.read(cx).name.to_string()).collect());
    assert_eq!(there, ["a.txt"], "it opened in the other window");
}
