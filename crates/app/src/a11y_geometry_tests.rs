use super::*;
use accesskit::{Point, Rect};
use gpui::{TestAppContext, VisualTestContext, size};

fn rig<'cx>(cx: &'cx mut TestAppContext, name: &str) -> (Entity<Workbench>, &'cx mut VisualTestContext) {
    let (wb, cx) = crate::qa_integration_tests::setup_with(cx, Settings { lsp_enabled: false, ..Default::default() });
    let project = crate::qa_workbench_tests::project(name);
    crate::qa_workbench_tests::open(&wb, &project, cx);
    crate::qa_workbench_tests::open_in(&wb, &project.join("src/main.rs"), cx);
    (wb, cx)
}

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn bounds(snapshot: &Snapshot, id: NodeId) -> Option<Rect> {
    snapshot.to_tree().nodes.into_iter().find(|(node_id, _)| *node_id == id).and_then(|(_, node)| node.bounds())
}

fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(command, window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn accessibility_landmarks_have_painted_bounds(cx: &mut TestAppContext) {
    let (wb, cx) = rig(cx, "a11y-geometry-resize");
    for width in [640., 1280.] {
        cx.simulate_resize(size(px(width), px(800.)));
        cx.run_until_parked();
        let snapshot = snapshot(&wb, cx);
        let root = bounds(&snapshot, ROOT).expect("window bounds");
        let scale = cx.update(|window, _| f64::from(window.scale_factor()));
        assert_eq!(root, Rect::new(0., 0., f64::from(width) * scale, 800. * scale));
        let tree = accesskit_consumer::Tree::new(snapshot.to_tree(), true);
        for (id, role) in [(EXPLORER, Role::Tree), (EDITOR, Role::MultilineTextInput), (STATUS, Role::Status)] {
            let rect = bounds(&snapshot, id).unwrap_or_else(|| panic!("landmark {id:?} has no bounds at width {width}"));
            assert!(rect.width() > 0. && rect.height() > 0., "landmark {id:?}: {rect:?}");
            assert!(rect.x0 >= root.x0 && rect.y0 >= root.y0 && rect.x1 <= root.x1 && rect.y1 <= root.y1, "landmark {id:?}: {rect:?} inside {root:?}");
            let center = Point::new((rect.x0 + rect.x1) / 2., (rect.y0 + rect.y1) / 2.);
            let state = tree.state();
            let hit = state.root().node_at_point(center, &|_| accesskit_consumer::FilterResult::Include).expect("landmark hit");
            assert_eq!(hit.role(), role, "hit testing landmark {id:?} at {center:?}");
        }
    }
}

#[gpui::test]
fn accessibility_landmarks_clear_bounds_when_hidden_and_restore_them(cx: &mut TestAppContext) {
    let (wb, cx) = rig(cx, "a11y-geometry-hidden");
    let shown = snapshot(&wb, cx);
    assert!(bounds(&shown, EXPLORER).is_some());
    assert!(bounds(&shown, STATUS).is_some());
    run(&wb, cx, "View: Toggle Zen Mode");
    let hidden = snapshot(&wb, cx);
    assert!(bounds(&hidden, EXPLORER).is_none(), "hidden explorer retained old coordinates");
    assert!(bounds(&hidden, STATUS).is_none(), "hidden status bar retained old coordinates");
    assert!(bounds(&hidden, EDITOR).is_some());
    run(&wb, cx, "View: Toggle Zen Mode");
    let restored = snapshot(&wb, cx);
    assert!(bounds(&restored, EXPLORER).is_some());
    assert!(bounds(&restored, STATUS).is_some());
}

#[gpui::test]
fn accessibility_landmarks_follow_the_focused_split_editor(cx: &mut TestAppContext) {
    let (wb, cx) = rig(cx, "a11y-geometry-split");
    cx.simulate_resize(size(px(1280.), px(800.)));
    run(&wb, cx, "View: Split Editor");
    let right = bounds(&snapshot(&wb, cx), EDITOR).expect("right editor bounds");
    run(&wb, cx, "View: Focus Left Editor Group");
    let left = bounds(&snapshot(&wb, cx), EDITOR).expect("left editor bounds");
    assert!(left.x1 <= right.x0, "split bounds overlap: left {left:?}, right {right:?}");
    run(&wb, cx, "View: Focus Right Editor Group");
    assert_eq!(bounds(&snapshot(&wb, cx), EDITOR), Some(right));
}
