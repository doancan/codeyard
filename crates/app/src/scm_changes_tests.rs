use super::*;

use std::time::{Duration, Instant};

use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};

use crate::git::{Change, Mark};
use crate::scm::{ACCESS_LINES, ScmEvent};
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

#[test]
fn list_keys_move_open_fold_stage_and_discard_without_modifiers() {
    let none = gpui::Modifiers::default();
    let with = |f: fn(&mut gpui::Modifiers)| {
        let mut m = gpui::Modifiers::default();
        f(&mut m);
        m
    };
    assert_eq!(list_key("up", none), Some(ListKey::Step(-1)));
    assert_eq!(list_key("down", none), Some(ListKey::Step(1)));
    assert_eq!(list_key("home", none), Some(ListKey::Step(isize::MIN)));
    assert_eq!(list_key("end", none), Some(ListKey::Step(isize::MAX)));
    assert_eq!(list_key("enter", none), Some(ListKey::Open));
    assert_eq!(list_key("space", none), Some(ListKey::Toggle));
    assert_eq!(list_key("left", none), Some(ListKey::Out));
    assert_eq!(list_key("right", none), Some(ListKey::In));
    assert_eq!(list_key("delete", none), Some(ListKey::Discard));
    assert_eq!(list_key("backspace", with(|m| m.platform = true)), Some(ListKey::Discard));
    // Plain Backspace, shortcuts and selection keys go on to the panel.
    assert_eq!(list_key("backspace", none), None);
    assert_eq!(list_key("down", with(|m| m.shift = true)), None);
    assert_eq!(list_key("enter", with(|m| m.platform = true)), None);
    assert_eq!(list_key("up", with(|m| m.alt = true)), None);
    assert_eq!(list_key("a", none), None);
}

#[test]
fn only_unstaged_non_conflicted_rows_can_be_discarded() {
    assert!(discardable(Group::Changes, &change(0, None, Some(Mark::Modified))));
    assert!(discardable(Group::Changes, &change(1, None, Some(Mark::Untracked))));
    assert!(!discardable(Group::Staged, &change(2, Some(Mark::Added), None)));
    assert!(!discardable(Group::Merge, &change(3, None, Some(Mark::Conflict))));
}

/// A conflict, a file staged and changed again, and a deleted file.
fn mixed() -> Arc<Status> {
    Arc::new(Status {
        root: "/scm-changes-cursor".into(),
        changes: vec![change(0, None, Some(Mark::Conflict)), change(1, Some(Mark::Added), Some(Mark::Modified)), change(2, None, Some(Mark::Deleted))],
        ..Default::default()
    })
}

fn path(i: usize) -> String {
    change(i, None, None).path
}

fn cursor(panel: &Entity<ScmPanel>, cx: &mut VisualTestContext) -> Option<ChangeKey> {
    panel.read_with(cx, |p, _| p.change_cursor_key())
}

fn is_open(panel: &Entity<ScmPanel>, id: &str, cx: &mut VisualTestContext) -> bool {
    panel.read_with(cx, |p, _| p.sections.is_open(id, true))
}

#[gpui::test]
fn the_keyboard_walks_the_change_list_and_folds_its_groups(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    set(&panel, mixed(), cx);
    let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = opened.clone();
    cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, ev: &ScmEvent, _| {
            if let ScmEvent::OpenDiff { path, staged } = ev {
                sink.borrow_mut().push((path.clone(), *staged));
            }
        })
        .detach()
    });
    assert!(panel.update_in(cx, |p, window, cx| p.focus_changes(None, window, cx)), "focus on the first row");
    cx.run_until_parked();
    assert!(panel.update_in(cx, |p, window, _| p.change_focus().is_focused(window)));
    assert_eq!(cursor(&panel, cx), Some(ChangeKey::Group(Group::Merge)));
    cx.simulate_keystrokes("down");
    assert_eq!(cursor(&panel, cx), Some(ChangeKey::File(Group::Merge, path(0))));
    cx.simulate_keystrokes("down down");
    assert_eq!(cursor(&panel, cx), Some(ChangeKey::File(Group::Staged, path(1))), "past the Staged Changes header");
    cx.simulate_keystrokes("enter");
    assert_eq!(*opened.borrow(), [(path(1), true)], "Enter opens the staged side's diff");
    cx.simulate_keystrokes("end");
    assert_eq!(cursor(&panel, cx), Some(ChangeKey::File(Group::Changes, path(2))));
    cx.simulate_keystrokes("down");
    assert_eq!(cursor(&panel, cx), Some(ChangeKey::File(Group::Changes, path(2))), "the last row stays");
    // Left goes out to the group, Left again folds it; Right unfolds it and then goes in.
    cx.simulate_keystrokes("left");
    assert_eq!(cursor(&panel, cx), Some(ChangeKey::Group(Group::Changes)));
    cx.simulate_keystrokes("left");
    assert!(!is_open(&panel, "scm-changes", cx));
    assert_eq!(cursor(&panel, cx), Some(ChangeKey::Group(Group::Changes)), "the cursor stays on the folded group");
    cx.simulate_keystrokes("right");
    assert!(is_open(&panel, "scm-changes", cx));
    cx.simulate_keystrokes("right");
    assert_eq!(cursor(&panel, cx), Some(ChangeKey::File(Group::Changes, path(1))));
    cx.simulate_keystrokes("home");
    assert_eq!(cursor(&panel, cx), Some(ChangeKey::Group(Group::Merge)));
    // Enter and Space on a header fold and unfold it.
    cx.simulate_keystrokes("enter");
    assert!(!is_open(&panel, "scm-merge", cx));
    cx.simulate_keystrokes("space");
    assert!(is_open(&panel, "scm-merge", cx));
}

#[gpui::test]
fn delete_asks_before_discarding_and_only_where_the_row_can_be_discarded(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    set(&panel, mixed(), cx);
    let on = |key: ChangeKey, panel: &Entity<ScmPanel>, cx: &mut VisualTestContext| {
        assert!(panel.update_in(cx, |p, window, cx| p.focus_changes(Some(&key), window, cx)));
        cx.run_until_parked();
    };
    on(ChangeKey::File(Group::Merge, path(0)), &panel, cx);
    cx.simulate_keystrokes("delete");
    assert!(!cx.has_pending_prompt(), "a conflict is resolved, not discarded");
    on(ChangeKey::File(Group::Staged, path(1)), &panel, cx);
    cx.simulate_keystrokes("delete");
    assert!(!cx.has_pending_prompt(), "a staged row is unstaged first");
    on(ChangeKey::File(Group::Changes, path(2)), &panel, cx);
    cx.simulate_keystrokes("delete");
    assert!(cx.has_pending_prompt(), "the painted button's question");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(!panel.read_with(cx, |p, _| p.is_busy()), "cancelled: nothing runs");
}

#[gpui::test]
fn the_cursor_follows_its_file_through_a_refresh(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    set(&panel, status(40), cx);
    let key = ChangeKey::File(Group::Changes, path(20));
    assert!(panel.update_in(cx, |p, window, cx| p.focus_changes(Some(&key), window, cx)));
    // A row inserted above: the same file.
    let mut more = (*status(40)).clone();
    more.changes.insert(0, Change { path: "a-first.rs".into(), orig: None, staged: None, unstaged: Some(Mark::Untracked) });
    set(&panel, Arc::new(more.clone()), cx);
    assert_eq!(cursor(&panel, cx), Some(key.clone()));
    // The file went: the cursor takes the row at or above its place, never one far away.
    let gone = more.changes.iter().position(|c| c.path == path(20)).unwrap();
    more.changes.remove(gone);
    set(&panel, Arc::new(more), cx);
    let now = cursor(&panel, cx);
    assert!(matches!(&now, Some(ChangeKey::File(Group::Changes, p)) if *p == path(19) || *p == path(21)), "{now:?}");
    // A key that is not in the list does not take the focus there.
    let missing = ChangeKey::File(Group::Changes, "nowhere.rs".into());
    assert!(!panel.update_in(cx, |p, window, cx| p.focus_changes(Some(&missing), window, cx)));
    // Another repository: no cursor.
    set(&panel, Arc::new(Status { root: "/scm-changes-cursor-other".into(), ..(*status(3)).clone() }), cx);
    assert_eq!(cursor(&panel, cx), None);
}

#[gpui::test]
fn the_accessible_range_is_bounded_and_follows_the_view_and_the_cursor(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    set(&panel, status(10_000), cx);
    let view = |panel: &Entity<ScmPanel>, cx: &mut VisualTestContext| panel.update_in(cx, |p, window, cx| p.a11y_view(window, cx));
    let v = view(&panel, cx);
    // Staged Changes (2,500 files) and Changes (10,000): 12,502 lines, one range of them.
    assert_eq!((v.lines_first, v.lines_shown, v.lines_total), (0, ACCESS_LINES, 12_502));
    assert_eq!(v.groups.len(), 1);
    assert!(v.groups[0].header_shown && v.groups[0].total == 2_500 && v.groups[0].files.len() == ACCESS_LINES - 1);
    // Scrolled far down: the range is around the rows on screen; places stay relative to the whole group.
    panel.update(cx, |p, cx| {
        p.scroll_changes_to(9_000);
        cx.notify();
    });
    cx.run_until_parked();
    let v = view(&panel, cx);
    assert!(v.lines_first <= 9_000 && 9_000 < v.lines_first + v.lines_shown, "range from {}", v.lines_first);
    assert_eq!(v.lines_shown, ACCESS_LINES);
    let changes = v.groups.iter().find(|g| g.id == "scm-changes").unwrap();
    assert!(!changes.header_shown, "its header is far above");
    let f = &changes.files[0];
    assert_eq!(f.position, f.line - 2_501, "the place in Changes, not in the range");
    // The cursor on a row on screen centres the range on it.
    let key = ChangeKey::File(Group::Changes, changes.files[300].change.path.clone());
    assert!(panel.update_in(cx, |p, window, cx| p.focus_changes(Some(&key), window, cx)));
    cx.run_until_parked();
    let v = view(&panel, cx);
    let at = v.cursor.expect("a cursor");
    assert!(v.list_focused);
    assert_eq!(at - v.lines_first, ACCESS_LINES / 2, "centred");
    // The accessibility scroll action pages the list; the cursor stays.
    let before = panel.read_with(cx, |p, _| p.change_scroll_top());
    panel.update(cx, |p, cx| p.a11y_scroll_changes(-3, cx));
    cx.run_until_parked();
    assert!(panel.read_with(cx, |p, _| p.change_scroll_top()) < before);
    assert_eq!(view(&panel, cx).cursor, Some(at));
}

#[gpui::test]
fn tab_goes_from_the_message_box_to_the_change_list(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    set(&panel, mixed(), cx);
    panel.update_in(cx, |p, window, cx| p.focus_message(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("tab");
    let state = panel.update_in(cx, |p, window, cx| {
        (
            p.change_focus.is_focused(window),
            p.message.focus_handle(cx).is_focused(window),
            p.query.focus_handle(cx).is_focused(window),
            p.focus_handle.is_focused(window),
        )
    });
    assert_eq!(state, (true, false, false, false));
}

#[gpui::test]
fn the_menu_key_asks_for_the_cursor_files_menu_under_its_row(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    set(&panel, mixed(), cx);
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = asked.clone();
    cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, ev: &ScmEvent, _| {
            if let ScmEvent::FileMenu(path, staged, at) = ev {
                sink.borrow_mut().push((path.clone(), *staged, *at));
            }
        })
        .detach()
    });
    let on = |key: ChangeKey, panel: &Entity<ScmPanel>, cx: &mut VisualTestContext| {
        assert!(panel.update_in(cx, |p, window, cx| p.focus_changes(Some(&key), window, cx)));
        cx.run_until_parked();
    };
    // A group header has no menu.
    on(ChangeKey::Group(Group::Changes), &panel, cx);
    cx.simulate_keystrokes("shift-f10");
    assert!(asked.borrow().is_empty());
    // Shift+F10 on the last row: its own menu, unstaged side, at the bottom left of the painted row.
    on(ChangeKey::File(Group::Changes, path(2)), &panel, cx);
    cx.simulate_keystrokes("shift-f10");
    let (top, left) = panel.read_with(cx, |p, _| {
        let bounds = p.change_scroll.0.borrow().base_handle.bounds();
        (bounds.top(), bounds.left())
    });
    let lines = panel.read_with(cx, |p, _| p.current_lines(&mixed()));
    let row = lines.iter().position(|l| *l == ChangeLine::File(Group::Changes, 2)).expect("the row");
    assert_eq!(*asked.borrow(), [(path(2), false, gpui::point(left, top + gpui::px(layout::ROW_H * (row + 1) as f32)))]);
    // The Menu key on the staged row of the same file asks for the staged side's menu.
    on(ChangeKey::File(Group::Staged, path(1)), &panel, cx);
    cx.simulate_keystrokes("menu");
    assert_eq!(asked.borrow().last().map(|(p, s, _)| (p.clone(), *s)), Some((path(1), true)));
    // The staged row went away: the key acts on the row the cursor is on now, never on the old file.
    asked.borrow_mut().clear();
    let changes = vec![change(0, None, Some(Mark::Conflict)), change(5, None, Some(Mark::Modified))];
    set(&panel, Arc::new(Status { root: "/scm-changes-cursor".into(), changes, ..Default::default() }), cx);
    assert_eq!(cursor(&panel, cx), Some(ChangeKey::File(Group::Changes, path(5))));
    cx.simulate_keystrokes("shift-f10");
    assert_eq!(asked.borrow().iter().map(|(p, s, _)| (p.clone(), *s)).collect::<Vec<_>>(), [(path(5), false)]);
}
