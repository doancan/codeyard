//! The accessibility snapshot follows the real workbench: pickers, the find bar, suggestions and the terminal show up
//! in it while they are on screen, and actions from an assistive technology reach them.

use gpui::TestAppContext;

use crate::qa_workbench_tests::{fresh, open, setup};
use crate::workbench::a11y::FocusSnap;

#[gpui::test]
fn about_exposes_its_details_and_copy_action(cx: &mut TestAppContext) {
    use accesskit::{Action, ActionRequest, Role, TreeId};

    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Help: About Codeyard", window, cx));
    cx.run_until_parked();
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert_eq!(snap.focus, FocusSnap::AboutCopy);
    let about = snap.about.as_ref().expect("About is in the accessibility tree");
    assert!(about.active);
    assert!(about.details.iter().any(|(_, value)| value == env!("CARGO_PKG_VERSION")));
    let nodes = snap.to_tree().nodes;
    assert!(nodes.iter().any(|(_, n)| n.role() == Role::Tab && n.label() == Some("About Codeyard") && n.is_selected() == Some(true)));
    let copy = nodes.iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some("Copy Version")).expect("Copy Version button").0;
    let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: copy, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(format!("Codeyard {}", env!("CARGO_PKG_VERSION"))));
}

#[gpui::test]
fn command_palette_is_exposed_with_its_selected_row(cx: &mut TestAppContext) {
    let dir = fresh("a11y-palette");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("cmd-shift-p");
    cx.run_until_parked();
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let picker = snap.picker.expect("the palette is in the tree");
    assert_eq!(snap.focus, FocusSnap::Picker);
    assert_eq!(picker.query, ">");
    assert_eq!(picker.list.selected, Some(0));
    assert!(!picker.list.rows.is_empty());
    cx.simulate_keystrokes("down");
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert_eq!(snap.picker.unwrap().list.selected, Some(1), "moving the selection moves the active descendant");
    cx.simulate_keystrokes("escape");
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert!(snap.picker.is_none(), "a closed picker leaves the tree");
}

#[gpui::test]
fn find_bar_reports_its_fields_status_and_options(cx: &mut TestAppContext) {
    let dir = fresh("a11y-find");
    let file = dir.join("a.txt");
    std::fs::write(&file, "needle\nhay\nneedle\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_input("needle");
    cx.run_until_parked();
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert_eq!(snap.focus, FocusSnap::Find(0));
    let find = snap.find.clone().expect("the find bar is in the tree");
    assert_eq!(find.query, "needle");
    assert_eq!(find.replacement, None);
    assert!(find.status.as_deref().is_some_and(|s| s.contains('2')), "{:?}", find.status);
    assert_eq!(find.controls[0].1, Some(false), "match case starts off");
    // The fields, the status and every control have their painted frames, and a hit test finds each one.
    let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
    let state = tree.state();
    let nodes = snap.to_tree().nodes;
    let query = nodes.iter().find(|(_, n)| n.role() == accesskit::Role::SearchInput && n.label() == Some("Find")).cloned();
    let status = nodes.iter().find(|(_, n)| n.role() == accesskit::Role::Status && n.label() == find.status.as_deref()).cloned();
    let mut framed = vec![query.expect("the query field"), status.expect("the status")];
    framed.extend(find.controls.iter().map(|(label, _)| nodes.iter().find(|(_, n)| n.label() == Some(label.as_str())).cloned().expect("the control")));
    for (id, node) in framed {
        let b = node.bounds().unwrap_or_else(|| panic!("{:?} has a frame", node.label()));
        let at = accesskit::Point::new((b.x0 + b.x1) / 2., (b.y0 + b.y1) / 2.);
        let hit = state.root().node_at_point(at, &|_| accesskit_consumer::FilterResult::Include).unwrap();
        assert_eq!(hit.locate().0, id, "hit test at {:?}", node.label());
    }
    // Pressing the option from the tree turns it on, as a click would.
    let bar = wb.read_with(cx, |wb, cx| wb.active_editor_entity().and_then(|e| e.read(cx).find_bar())).unwrap();
    bar.update(cx, |b, cx| b.a11y_press(0, cx));
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert_eq!(snap.find.unwrap().controls[0].1, Some(true));
}

#[cfg(unix)]
#[gpui::test]
fn terminal_screen_is_exposed_as_text(cx: &mut TestAppContext) {
    let dir = fresh("a11y-terminal");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    cx.simulate_keystrokes("ctrl-`");
    cx.simulate_keystrokes("e c h o space a 1 1 y - $ ( ( 6 * 7 ) ) enter");
    let start = std::time::Instant::now();
    loop {
        cx.run_until_parked();
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        if let Some(term) = snap.terminal.as_ref().filter(|t| t.runs.iter().any(|r| r.text.starts_with("a11y-42"))) {
            assert_eq!(snap.focus, FocusSnap::Terminal);
            assert!(term.cursor.is_some());
            // Every row on screen is painted: its characters sit on the grid, one cell each.
            let run = term.runs.iter().find(|r| r.text.starts_with("a11y-42")).unwrap();
            let frame = run.frame.as_ref().expect("the row's frame");
            let cell = frame.widths[0];
            assert!(cell > 0. && frame.positions.iter().enumerate().take(7).all(|(i, x)| (x - cell * i as f32).abs() < 0.01), "{frame:?}");
            // The caret's bounds, as a magnifier asks for them.
            let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
            let state = tree.state();
            let node = state.focus().expect("the terminal has the focus");
            let caret = node.text_selection().expect("the cursor").bounding_boxes();
            assert!(caret.len() == 1 && caret[0].height() > 0., "{caret:?}");
            break;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(10 * crate::test_sshd::timeout_scale()), "terminal output never reached the tree");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Built-in themes pass WCAG AA on their own; a colour customization that breaks it is reported, a fixed one is not.
#[gpui::test]
fn colour_settings_below_wcag_aa_are_reported(cx: &mut TestAppContext) {
    let cfg = fresh("a11y-contrast");
    let settings = cfg.join("settings.jsonc");
    std::fs::write(&settings, r##"{ "theme": "midnight" }"##).unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_for_test()), None);
    std::fs::write(&settings, r##"{ "theme": "midnight", "workbench.colorCustomizations": { "text": "#2a2a2a" } }"##).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    let (error, text) = wb.read_with(cx, |wb, _| wb.notice_for_test()).expect("a contrast notice");
    assert!(!error);
    assert!(text.contains("WCAG AA") && text.contains("Midnight") && text.contains("text on"), "{text}");
    // Back to readable colours: no new notice.
    wb.update(cx, |wb, _| wb.clear_notice_for_test());
    std::fs::write(&settings, r##"{ "theme": "midnight", "workbench.colorCustomizations": { "accent": "#7aa2f7" } }"##).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_for_test()), None);
}

/// Reduce motion (the setting, or the system on "auto") keeps the cursor from blinking.
#[gpui::test]
fn reduce_motion_stops_the_cursor_blinking(cx: &mut TestAppContext) {
    let dir = fresh("a11y-motion");
    let file = dir.join("a.txt");
    std::fs::write(&file, "abc\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let tick = |cx: &mut gpui::VisualTestContext| {
        cx.executor().advance_clock(std::time::Duration::from_millis(540));
        cx.run_until_parked();
    };
    let type_and_wait = |cx: &mut gpui::VisualTestContext, text: &str| {
        ed.update_in(cx, |e, window, cx| gpui::EntityInputHandler::replace_text_in_range(e, None, text, window, cx));
        assert!(ed.update(cx, |e, cx| e.blink_for_test(true, cx)));
        tick(cx);
        ed.update(cx, |e, cx| e.blink_for_test(true, cx))
    };
    assert!(!type_and_wait(cx, "x"), "blinks by default");
    cx.update(|_, cx| cx.set_global(crate::system_prefs::SystemPrefs { reduce_motion: true, increase_contrast: false }));
    assert!(type_and_wait(cx, "y"), "the system's Reduce motion keeps it on");
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().reduce_motion = Some(false));
    assert!(!type_and_wait(cx, "z"), "the setting \"off\" wins over the system");
}

/// "Auto" picks the high-contrast pair while the system asks for more contrast.
#[gpui::test]
fn auto_theme_follows_increase_contrast(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let palette = |cx: &mut gpui::VisualTestContext| cx.update(|_, cx| cx.global::<crate::ui::ActiveTheme>().0);
    let high: Vec<_> = theme::themes::builtin().into_iter().filter(|t| t.id.starts_with("high-contrast-")).map(|t| t.palette).collect();
    wb.update_in(cx, |wb, window, cx| wb.use_theme("auto", window, cx));
    assert!(!high.contains(&palette(cx)));
    cx.update(|_, cx| cx.set_global(crate::system_prefs::SystemPrefs { reduce_motion: false, increase_contrast: true }));
    wb.update_in(cx, |wb, window, cx| wb.use_theme("auto", window, cx));
    assert!(high.contains(&palette(cx)), "a high-contrast theme is in use");
}
