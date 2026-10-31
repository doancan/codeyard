//! Painted frames of the picker, suggestions, the Extensions list, the Settings page's controls, the bottom panel's
//! tabs, the Search Editor and the editor area's Search Editor and terminal tabs: each node has the bounds it was
//! drawn at (inside its list or page), AccessKit's hit test at its centre finds it, and what is not drawn has none.

use accesskit::{Action, ActionRequest, Node, NodeId, Point, Rect, Role, TreeId};
use accesskit_consumer::{FilterResult, Tree};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_workbench_tests::{fresh, open, project, setup, setup_with_config};

fn nodes(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> (Snapshot, Vec<(NodeId, Node)>) {
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let nodes = snap.to_tree().nodes;
    (snap, nodes)
}

fn find<'a>(nodes: &'a [(NodeId, Node)], role: Role, label: &str) -> &'a (NodeId, Node) {
    nodes.iter().find(|(_, n)| n.role() == role && n.label() == Some(label)).unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
}

fn center(r: Rect) -> Point {
    Point::new((r.x0 + r.x1) / 2., (r.y0 + r.y1) / 2.)
}

fn inside(inner: Rect, outer: Rect) -> bool {
    inner.x0 >= outer.x0 - 0.5 && inner.y0 >= outer.y0 - 0.5 && inner.x1 <= outer.x1 + 0.5 && inner.y1 <= outer.y1 + 0.5
}

/// Each of `ids` has a frame inside `outer` (when given) and is what AccessKit's hit test finds at its centre.
fn hit(snap: &Snapshot, ids: &[NodeId], outer: Option<Rect>) {
    let update = snap.to_tree();
    let tree = Tree::new(update.clone(), true);
    for id in ids {
        let node = &update.nodes.iter().find(|(i, _)| i == id).unwrap_or_else(|| panic!("{id:?} is in the tree")).1;
        let b = node.bounds().unwrap_or_else(|| panic!("{:?} {:?} has a frame", node.role(), node.label()));
        assert!(b.width() > 0. && b.height() > 0., "{b:?}");
        if let Some(outer) = outer {
            assert!(inside(b, outer), "{:?} {b:?} inside {outer:?}", node.label());
        }
        let found = tree.state().root().node_at_point(center(b), &|_| FilterResult::Include).map(|n| n.locate().0);
        assert_eq!(found, Some(*id), "the hit test at the centre of {:?} {:?}", node.role(), node.label());
    }
}

/// The command palette: the dialog, its query box and its list are where they are drawn; each row on screen too,
/// inside the list, and a row the list does not draw (beyond its height) has no frame.
#[gpui::test]
fn picker_rows_have_their_painted_frames(cx: &mut TestAppContext) {
    let dir = fresh("a11y-frames-picker");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("cmd-shift-p");
    cx.run_until_parked();
    let (snap, tree) = nodes(&wb, cx);
    let picker = snap.picker.as_ref().expect("the palette");
    let dialog = picker.bounds.expect("the picker's frame");
    let list = picker.list.bounds.expect("the list's frame");
    assert!(inside(picker.input.expect("the query box's frame"), dialog) && inside(list, dialog));
    let drawn: Vec<NodeId> = (0..picker.list.rows.len()).filter(|i| picker.list.frames[*i].is_some()).map(|i| NodeId(PICK_OPTION + i as u64)).collect();
    assert!(drawn.len() >= 5, "{} rows drawn", drawn.len());
    assert_eq!(drawn[0], NodeId(PICK_OPTION), "the first row is drawn");
    assert!(picker.list.rows.len() > drawn.len(), "rows below the list's height are not drawn: {} rows", picker.list.rows.len());
    hit(&snap, &drawn, Some(list));
    hit(&snap, &[PICKER_INPUT], Some(dialog));
    assert!(tree.iter().any(|(id, n)| *id == PICKER && n.bounds() == Some(dialog)));
}

/// Word suggestions: the list and each suggestion it draws have their frames, inside the window, under the caret.
#[gpui::test]
fn suggestions_have_their_painted_frames(cx: &mut TestAppContext) {
    let dir = fresh("a11y-frames-suggest");
    std::fs::write(dir.join("words.txt"), "merhaba dünya\nmerdiven ve masa\n\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("words.txt"), cx);
    cx.simulate_keystrokes("cmd-down");
    cx.simulate_input("me");
    cx.run_until_parked();
    let (snap, _) = nodes(&wb, cx);
    let list = snap.completion.as_ref().expect("suggestions are open");
    assert!(list.rows.len() >= 2, "{:?}", list.rows);
    let bounds = list.bounds.expect("the list's frame");
    let ids: Vec<NodeId> = (0..list.rows.len()).map(|i| NodeId(COMPLETION_OPTION + i as u64)).collect();
    hit(&snap, &ids, Some(bounds));
    let editor = Tree::new(snap.to_tree(), true).state().root().children().find(|n| n.role() == Role::MultilineTextInput).and_then(|n| n.data().bounds());
    let caret_line = snap.editor.as_ref().and_then(|e| e.runs.iter().rev().find_map(|r| r.frame.as_ref())).map(|f| f.bounds).expect("the caret's line");
    assert!(bounds.y0 >= caret_line.y1 - 0.5 || bounds.y1 <= caret_line.y0 + 0.5, "the list is above or below the caret's line");
    assert!(editor.is_some());
}

/// The installed extensions: the tabs, the search field, the list and each row have their frames and are found at
/// their centres.
#[gpui::test]
fn extension_rows_have_their_painted_frames(cx: &mut TestAppContext) {
    let cfg = crate::ext_view_tests::config_dir("a11y-frames");
    crate::ext_view_tests::install(&cfg, &crate::ext_view_tests::theme_files("you.night", "1.0.0"));
    crate::ext_view_tests::install(&cfg, &crate::ext_view_tests::theme_files("acme.dawn", "2.0.0"));
    let (wb, cx) = crate::ext_view_tests::setup(cx, &cfg);
    let (snap, _) = nodes(&wb, cx);
    let view = snap.exts.as_ref().expect("the Extensions view");
    assert_eq!(view.list.rows.len(), 2);
    hit(&snap, &[NodeId(EXT_ROW), NodeId(EXT_ROW + 1), EXT_TAB_INSTALLED, EXT_TAB_MARKET, EXT_SEARCH], None);
    let list = snap.to_tree().nodes.into_iter().find(|(id, _)| *id == EXT_LIST).and_then(|(_, n)| n.bounds()).expect("the list's frame");
    hit(&snap, &[NodeId(EXT_ROW), NodeId(EXT_ROW + 1)], Some(list));
    let (a, b) = (view.list.frames[0].unwrap(), view.list.frames[1].unwrap());
    assert!(b.y0 >= a.y1 - 0.5, "rows are drawn one below the other: {a:?} {b:?}");
}

/// The Settings page: its search field, categories, the options of a choice, a changed setting's Reset button and the
/// page's buttons each have their own frame; the options and Reset lie inside their setting's row.
#[gpui::test]
fn settings_controls_have_their_painted_frames(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let cfg = fresh("a11y-frames-settings-cfg");
    let dir = project("a11y-frames-settings");
    let (wb, cx) = setup_with_config(cx, &cfg);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Open Settings", window, cx));
    cx.run_until_parked();
    let (snap, tree) = nodes(&wb, cx);
    let (switch, _) = *find(&tree, Role::CheckBox, "Editor › Minimap: Enabled");
    let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: switch, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
    cx.run_until_parked();
    let (snap, tree) = nodes(&wb, cx);
    let page = tree.iter().find(|(_, n)| n.role() == Role::Group && n.label() == Some("Settings")).unwrap().0;
    let (wrap, row) = find(&tree, Role::RadioGroup, "Editor: Word Wrap");
    let row_bounds = row.bounds().expect("the row's frame");
    let options = row.children().to_vec();
    assert!(options.len() >= 2, "{wrap:?}");
    hit(&snap, &options, Some(row_bounds));
    let reset = tree.iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some("Reset")).expect("Reset for the changed setting");
    let minimap_row = find(&tree, Role::CheckBox, "Editor › Minimap: Enabled").1.bounds().expect("its row's frame");
    hit(&snap, &[reset.0], Some(minimap_row));
    let fixed: Vec<NodeId> = ["Search settings"]
        .iter()
        .map(|l| find(&tree, Role::SearchInput, l).0)
        .chain(["Commonly Used", "Editor", "Modified"].iter().map(|l| find(&tree, Role::Tab, l).0))
        .chain(["Open settings.jsonc", "Keyboard Shortcuts", "File icon theme…"].iter().map(|l| find(&tree, Role::Button, l).0))
        .collect();
    hit(&snap, &fixed, None);
    assert!(tree.iter().any(|(id, _)| *id == page));
}

/// The bottom panel's tabs: each has its frame, side by side, and is found at its centre.
#[gpui::test]
fn panel_tabs_have_their_painted_frames(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-frames-panel");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, cx| {
        wb.chrome.panel_open = true;
        wb.chrome.bottom = Bottom::Problems;
        cx.notify();
    });
    cx.run_until_parked();
    let (snap, tree) = nodes(&wb, cx);
    let tabs: Vec<NodeId> = ["Problems", "Output", "Ports", "Terminal"].iter().map(|l| find(&tree, Role::Tab, l).0).collect();
    hit(&snap, &tabs, None);
    let xs: Vec<f64> = tabs.iter().map(|id| tree.iter().find(|(i, _)| i == id).unwrap().1.bounds().unwrap().x0).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "in screen order: {xs:?}");
}

/// The Search Editor in front: the page, the query, the status, the list and each drawn row have their frames, and its
/// tab in the editor area too.
#[gpui::test]
fn the_search_editor_has_its_painted_frames(cx: &mut TestAppContext) {
    let dir = fresh("a11y-frames-search-editor");
    std::fs::write(dir.join("a.txt"), "needle one\nhay\nneedle two\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let spec = crate::search_doc::SearchSpec { pattern: "needle".into(), case_sensitive: true, context: 1, ..Default::default() };
    let ed = wb.update_in(cx, |wb, window, cx| wb.open_search_editor_for_test(spec, None, window, cx)).expect("a folder is open");
    crate::qa_integration_tests::wait_ticking(cx, "the search", |cx| {
        ed.read_with(cx, |e, _| !e.is_running() && e.doc.state != crate::search_doc::RunState::Running)
    });
    let (snap, tree) = nodes(&wb, cx);
    let view = &snap.search_editor.as_ref().expect("in front").view;
    let page = view.bounds.page.expect("the page's frame");
    let list = view.bounds.list.expect("the list's frame");
    assert!(view.bounds.rows.iter().filter(|r| r.is_some()).count() >= 2, "{:?}", view.bounds.rows);
    let rows: Vec<NodeId> =
        view.bounds.rows.iter().enumerate().filter(|(_, r)| r.is_some()).map(|(i, _)| NodeId(search_access::SEARCH_ED_ROW + i as u64)).collect();
    hit(&snap, &rows, Some(list));
    hit(&snap, &[search_access::SEARCH_ED_QUERY], Some(page));
    let status = tree.iter().find(|(id, _)| *id == search_access::SEARCH_ED_STATUS).unwrap().1.bounds().expect("the status's frame");
    assert!(inside(status, page));
    hit(&snap, &[NodeId(SEARCH_ED_TAB)], None);
}
