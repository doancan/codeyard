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
