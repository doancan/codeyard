use super::*;

#[gpui::test]
fn opening_a_file_from_explorer_restores_a_maximized_panel(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let root = crate::qa_workbench_tests::project("maximized-explorer");
    crate::qa_workbench_tests::open(&wb, &root, cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.chrome.panel_open = true;
        wb.chrome.panel_max = true;
        wb.chrome.panel_h = 240.;
        wb.on_tree_event(&TreeEvent::OpenFile { path: ui::local_path(&root.join("src/main.rs")), preview: true }, window, cx);
    });
    crate::qa_integration_tests::wait_secs(cx, 5, "Explorer file to open", |cx| wb.read_with(cx, |wb, _| wb.active_editor().is_some()));
    wb.read_with(cx, |wb, _| {
        assert!(!wb.chrome.panel_max, "opening a file must make the editor visible");
        assert!(wb.chrome.panel_open, "keep the terminal available at its previous size");
        assert_eq!(wb.chrome.panel_h, 240.);
    });
}

#[gpui::test]
fn quick_open_of_an_existing_tab_restores_a_maximized_panel(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let root = crate::qa_workbench_tests::project("maximized-quick-open");
    crate::qa_workbench_tests::open(&wb, &root, cx);
    crate::qa_workbench_tests::open_in(&wb, &root.join("src/main.rs"), cx);
    wb.update(cx, |wb, _| {
        wb.chrome.panel_open = true;
        wb.chrome.panel_max = true;
    });
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-p" } else { "ctrl-p" });
    cx.simulate_input("main.rs");
    crate::qa_integration_tests::wait_secs(cx, 5, "quick-open file list", |cx| {
        wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).is_some_and(|labels| labels.iter().any(|l| l.contains("main.rs"))))
    });
    cx.simulate_keystrokes("enter");
    assert!(wb.read_with(cx, |wb, _| !wb.chrome.panel_max && wb.chrome.panel_open));
}

/// Zoom In / Out / Reset change `ui.scale` in the settings file and the whole window with it; the editor and terminal font
/// sizes stay as they are, so the window applies the scale to them exactly once.
#[gpui::test]
fn zoom_scales_the_whole_window_and_is_saved(cx: &mut gpui::TestAppContext) {
    let cfg = crate::qa_workbench_tests::fresh("cfg-ui-scale");
    std::fs::write(cfg.join("settings.jsonc"), "{\n  // kept\n  \"editor.fontSize\": 15\n}\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    let (viewport, fonts) = cx.update(|window, cx| {
        let s = cx.global::<Settings>();
        (window.viewport_size(), (s.editor_font_size, s.editor_line_height, s.terminal_font_size))
    });
    assert_eq!(cx.update(|window, _| window.interface_scale()), 1., "the default changes nothing");
    for _ in 0..3 {
        cx.dispatch_action(ZoomIn);
    }
    cx.update(|window, cx| {
        let s = cx.global::<Settings>();
        assert_eq!(s.ui_scale, 1.5);
        assert_eq!(window.interface_scale(), 1.5);
        assert_eq!((s.editor_font_size, s.editor_line_height, s.terminal_font_size), fonts, "font preferences are not changed");
        let expected = viewport.map(|v| v * (1. / 1.5));
        assert!((window.viewport_size().width - expected.width).abs() < px(0.01), "layout sees the zoomed size");
    });
    let text = std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap();
    assert!(text.contains("\"ui.scale\": 1.5") && text.contains("// kept") && text.contains("\"editor.fontSize\": 15"), "{text}");
    assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(m)) if m.contains("150%"))));
    for _ in 0..20 {
        cx.dispatch_action(ZoomIn);
    }
    assert_eq!(cx.update(|window, _| window.interface_scale()), 2., "Zoom In stops at 200%");
    cx.dispatch_action(ZoomReset);
    cx.update(|window, cx| {
        assert_eq!(cx.global::<Settings>().ui_scale, 1.);
        assert_eq!((window.interface_scale(), window.viewport_size()), (1., viewport));
    });
}

/// A window opened with `ui.scale` in the settings file starts zoomed; a value out of range keeps 100% and says why.
#[gpui::test]
fn saved_interface_scale_applies_when_the_window_opens(cx: &mut gpui::TestAppContext) {
    let cfg = crate::qa_workbench_tests::fresh("cfg-ui-scale-open");
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "ui.scale": 1.25 }"#).unwrap();
    let (_wb, vcx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    assert_eq!(vcx.update(|window, _| window.interface_scale()), 1.25);

    let (s, warnings) = crate::settings::parse(r#"{ "ui.scale": 5 }"#);
    assert_eq!(s.ui_scale, 1.);
    assert!(warnings.iter().any(|w| w.contains("between 75% and 200%")), "{warnings:?}");
    let (s, warnings) = crate::settings::parse(r#"{ "ui.scale": "big" }"#);
    assert_eq!(s.ui_scale, 1.);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

/// A folder's settings cannot zoom every window of the app.
#[test]
fn a_project_cannot_set_the_interface_scale() {
    let (s, warnings) = crate::settings::parse_layers("{}", &[(".codeyard/settings.json", r#"{ "ui.scale": 2 }"#, true)]).unwrap();
    assert_eq!(s.ui_scale, 1.);
    assert!(warnings.iter().any(|w| w.contains("ui.scale")), "{warnings:?}");
}
