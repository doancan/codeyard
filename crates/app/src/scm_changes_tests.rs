use super::*;

use std::time::{Duration, Instant};

use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};

use crate::git::{Change, Mark};
use crate::ui::layout;

fn change(i: usize, staged: Option<Mark>, unstaged: Option<Mark>) -> Change {
    Change { path: format!("gen/dir-{}/file-{i:05}.rs", i % 50), orig: None, staged, unstaged }
}

fn status(n: usize) -> Arc<Status> {
    let changes = (0..n).map(|i| change(i, (i % 4 == 0).then_some(Mark::Modified), Some(Mark::Modified))).collect();
    Arc::new(Status { root: "/scm-changes-bench".into(), branch: Some("main".into()), changes, ..Default::default() })
}

fn panel(cx: &mut TestAppContext) -> (Entity<ScmPanel>, &mut VisualTestContext) {
    crate::qa_workbench_tests::globals(cx);
    cx.add_window_view(|_, cx| ScmPanel::new(Arc::new(host::local()), cx))
}

fn set(panel: &Entity<ScmPanel>, st: Arc<Status>, cx: &mut VisualTestContext) {
    // The panel's own host: another one counts as another repository.
    panel.update(cx, |p, cx| p.set_status(p.host.clone(), Some(st), cx));
    cx.run_until_parked();
}

/// Median time to draw the panel once with `n` changes.
fn frame(n: usize, cx: &mut TestAppContext) -> Duration {
    let (panel, cx) = panel(cx);
    set(&panel, status(n), cx);
    let mut times: Vec<Duration> = (0..5)
        .map(|_| {
            let start = Instant::now();
            panel.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            start.elapsed()
        })
        .collect();
    times.sort();
    times[2]
}

#[test]
fn lines_group_conflicts_staged_and_changes_and_skip_closed_groups() {
    let st = Status {
        changes: vec![change(0, None, Some(Mark::Conflict)), change(1, Some(Mark::Added), Some(Mark::Modified)), change(2, None, Some(Mark::Deleted))],
        ..Default::default()
    };
    use ChangeLine::{Empty, File};
    use Group::{Changes, Merge, Staged};
    let g = ChangeLine::Group;
    assert_eq!(change_lines(&st, |_| true), [g(Merge, 1), File(Merge, 0), g(Staged, 1), File(Staged, 1), g(Changes, 2), File(Changes, 1), File(Changes, 2)]);
    assert_eq!(change_lines(&st, |id| id != "scm-changes").last(), Some(&g(Changes, 2)));
    assert_eq!(change_lines(&Status::default(), |_| true), [g(Changes, 0), Empty]);
}

/// Before the list was virtualized a debug build took ~240 ms (1k) and ~2.3 s (10k) per frame; now the cost no
/// longer grows with the number of changes. The bound is loose for slow CI machines.
#[gpui::test]
fn ten_thousand_changes_draw_within_the_frame_budget(cx: &mut TestAppContext) {
    let t = frame(10_000, cx);
    eprintln!("scm frame with 10k changes: {t:?}");
    assert!(t < Duration::from_millis(150), "{t:?}");
}

#[gpui::test]
fn one_thousand_changes_draw_within_the_frame_budget(cx: &mut TestAppContext) {
    let t = frame(1_000, cx);
    eprintln!("scm frame with 1k changes: {t:?}");
    assert!(t < Duration::from_millis(150), "{t:?}");
}

#[gpui::test]
fn refresh_keeps_the_scroll_position_and_focus(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    set(&panel, status(2_000), cx);
    assert!(cx.debug_bounds("scm-list").is_some());
    panel.update(cx, |p, cx| {
        p.scroll_changes_to(800);
        cx.notify();
    });
    cx.run_until_parked();
    let top = panel.read_with(cx, |p, _| p.change_scroll_top());
    assert!(top >= 790. * layout::ROW_H, "scrolled to {top}");
    panel.update_in(cx, |p, window, cx| p.focus_message(window, cx));
    // A refresh with one change fewer: same position, focus stays in the message box.
    let mut st = (*status(2_000)).clone();
    st.changes.remove(1_999);
    set(&panel, Arc::new(st), cx);
    assert_eq!(panel.read_with(cx, |p, _| p.change_scroll_top()), top);
    assert!(panel.update_in(cx, |p, window, cx| p.message.focus_handle(cx).is_focused(window)));
    // Another repository starts at the top.
    let other = Status { root: "/scm-changes-other".into(), ..(*status(2_000)).clone() };
    set(&panel, Arc::new(other), cx);
    assert_eq!(panel.read_with(cx, |p, _| p.change_scroll_top()), 0.);
}
