//! Frames of the buttons inside side-view rows, and of rows drawn in a side view's own window.

use accesskit::{Node, NodeId, Point, Rect, Role};
use accesskit_consumer::{FilterResult, Tree};
use gpui::{Entity, VisualTestContext};

use super::*;
use crate::qa_workbench_tests::setup;

fn remote(cx: &mut gpui::TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| {
        wb.remote.ssh_hosts =
            vec![host::ssh::SshHost { alias: "fixture".into(), hostname: Some("example.invalid".into()), user: Some("me".into()), source: None }];
        wb.chrome.panel = Some(Panel::Remote);
        cx.notify();
    });
    cx.run_until_parked();
    (wb, cx)
}

fn labelled<'a>(nodes: &'a [(NodeId, Node)], label: &str) -> &'a Node {
    &nodes.iter().find(|(_, n)| n.label() == Some(label)).unwrap_or_else(|| panic!("{label} is in the tree")).1
}

fn center(r: Rect) -> Point {
    Point::new((r.x0 + r.x1) / 2., (r.y0 + r.y1) / 2.)
}

/// A host row's buttons (shown while the pointer is over the row) have the frames they are laid out at, inside the
/// row, in the order they are drawn; a hit test finds each one, and the row's middle is the row.
#[gpui::test]
fn row_buttons_have_the_frames_they_are_laid_out_at(cx: &mut gpui::TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (wb, cx) = remote(cx);
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let update = snap.to_tree();
    let row = labelled(&update.nodes, "fixture").bounds().expect("the host row's frame");
    let mut previous = row.x0;
    for label in ["Connect in Current Window", "Connect in New Window", "More Actions…"] {
        let b = labelled(&update.nodes, label).bounds().unwrap_or_else(|| panic!("{label} has a frame"));
        assert!(b.x0 >= previous - 0.01 && b.x1 <= row.x1 + 0.01 && b.y0 >= row.y0 - 0.01 && b.y1 <= row.y1 + 0.01, "{label} {b:?} in {row:?}");
        previous = b.x1;
        let tree = Tree::new(update.clone(), true);
        let hit = tree.state().root().node_at_point(center(b), &|_| FilterResult::Include).and_then(|n| n.label());
        assert_eq!(hit.as_deref(), Some(label));
    }
    let tree = Tree::new(update.clone(), true);
    let at = Point::new(row.x0 + 4., (row.y0 + row.y1) / 2.);
    let hit = tree.state().root().node_at_point(at, &|_| FilterResult::Include).unwrap();
    assert_eq!((hit.role(), hit.label().as_deref()), (Role::TreeItem, Some("fixture")));
}

/// The view moved into a window of its own: its rows have frames in that window's coordinates (not the main
/// window's), and the main window's tree, which still lists the shared part, gives them none.
#[gpui::test]
fn rows_in_a_view_window_have_that_windows_frames(cx: &mut gpui::TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (wb, vcx) = remote(cx);
    wb.update_in(vcx, |wb, window, cx| wb.view_into_new_window(window, cx));
    vcx.run_until_parked();
    let handle = wb.read_with(vcx, |wb, _| wb.chrome.view_windows.iter().find(|w| w.view.panel == Panel::Remote).map(|w| w.window)).expect("a window");
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.run_until_parked();
    let root = handle.update(&mut win, |_, _, cx| cx.entity()).unwrap();
    let snap = win.update(|window, cx| (windows_access::VIEW_WINDOW.snapshot)(root.clone().into(), window, cx)).unwrap();
    let nodes = snap.to_tree().nodes;
    let row = labelled(&nodes, "fixture").bounds().expect("the row has its frame in the view's window");
    let (id, scale) = win.update(|window, _| (window.window_handle().window_id(), window.scale_factor()));
    let ix = wb
        .read_with(&win, |wb, cx| {
            wb.remote_rows(cx).iter().position(|r| matches!(r, crate::workbench::remote_glue::tree::RemoteRow::Host { alias, .. } if alias == "fixture"))
        })
        .expect("the host row");
    let painted = wb.read_with(&win, |wb, _| wb.chrome.a11y_bounds.row(id, RowFrame::Remote, ix)).expect("painted in that window");
    assert_eq!(Some(row), crate::painted::physical_scaled(painted, scale));
    let window_bounds = Tree::new(snap.to_tree(), true).state().root().data().bounds().unwrap();
    assert!(row.x1 <= window_bounds.x1 + 0.01 && row.y1 <= window_bounds.y1 + 0.01, "{row:?} inside {window_bounds:?}");
    let main = vcx.update(|window, cx| (windows_access::WORKBENCH.snapshot)(wb.clone().into(), window, cx)).unwrap();
    let main_nodes = main.to_tree().nodes;
    if let Some((_, n)) = main_nodes.iter().find(|(_, n)| n.label() == Some("fixture")) {
        assert!(n.bounds().is_none(), "the main window does not draw the row");
    }
}
