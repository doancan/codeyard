use super::*;

#[gpui::test]
fn opening_a_file_from_explorer_restores_a_maximized_panel(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let root = crate::qa_workbench_tests::project("maximized-explorer");
    crate::qa_workbench_tests::open(&wb, &root, cx);
    wb.update_in(cx, |wb, window, cx| {
        cx.global_mut::<Settings>().panel_open_from_maximized = "restore".into();
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

#[gpui::test]
fn opening_a_diff_or_a_page_restores_a_maximized_panel(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let root = crate::qa_workbench_tests::project("maximized-diff-page");
    crate::qa_workbench_tests::open(&wb, &root, cx);
    let maximize = |wb: &mut Workbench| {
        wb.chrome.panel_open = true;
        wb.chrome.panel_max = true;
    };
    wb.update_in(cx, |wb, window, cx| {
        maximize(wb);
        wb.open_compare("a ↔ b".into(), "Compare", "one\n", "two\n", window, cx);
        assert!(!wb.chrome.panel_max, "a diff must not open behind a maximized panel");
        assert!(wb.chrome.panel_open, "the panel stays available");
        maximize(wb);
        wb.open_settings_page(&OpenSettingsPage, window, cx);
        assert!(!wb.chrome.panel_max, "Settings must not open behind a maximized panel");
    });
}

#[gpui::test]
fn dragging_a_terminal_row_reorders_the_list_and_keeps_the_active_terminal(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let root = crate::qa_workbench_tests::project("terminal-reorder");
    crate::qa_workbench_tests::open(&wb, &root, cx);
    wb.update_in(cx, |wb, window, cx| {
        for _ in 0..3 {
            wb.new_terminal(&NewTerminal, window, cx);
        }
    });
    cx.run_until_parked();
    wb.update(cx, |wb, cx| {
        let [a, b, c] = wb.terminals_for_test().try_into().expect("three terminals");
        assert_eq!(wb.active_terminal().as_ref(), Some(&c), "the newest terminal is active");
        wb.move_terminal(&c, 0, cx);
        assert_eq!(wb.terminals_for_test(), [c.clone(), a.clone(), b.clone()]);
        assert_eq!(wb.active_terminal().as_ref(), Some(&c), "the active terminal follows its row");
        wb.move_terminal(&a, 99, cx);
        assert_eq!(wb.terminals_for_test(), [c, a, b], "an out-of-range drop changes nothing");
    });
}

/// `workbench.panel.openFromMaximized`: what gives way when a page opens behind a maximized terminal.
#[gpui::test]
fn the_setting_chooses_what_a_maximized_terminal_does_when_something_opens(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let root = crate::qa_workbench_tests::project("maximized-behaviour");
    crate::qa_workbench_tests::open(&wb, &root, cx);
    wb.update_in(cx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    cx.run_until_parked();
    let area = gpui::Bounds::new(gpui::point(px(0.), px(0.)), gpui::size(px(1000.), px(800.)));
    let open_settings = |wb: &mut Workbench, behaviour: &str, window: &mut Window, cx: &mut Context<Workbench>| {
        cx.global_mut::<Settings>().panel_open_from_maximized = behaviour.into();
        wb.chrome.panel_open = true;
        wb.chrome.panel_max = true;
        wb.chrome.panel_h = 500.;
        wb.chrome.panel_area = Some(area);
        wb.open_settings_page(&OpenSettingsPage, window, cx);
    };

    wb.update_in(cx, |wb, window, cx| {
        open_settings(wb, "half", window, cx);
        assert!(!wb.chrome.panel_max && wb.chrome.panel_open);
        assert_eq!(wb.chrome.panel_h, 400., "half of the 800 px the editor and panel share");
        open_settings(wb, "restore", window, cx);
        assert!(!wb.chrome.panel_max);
        assert_eq!(wb.chrome.panel_h, 500., "the size it had");
        open_settings(wb, "tab", window, cx);
        assert!(!wb.chrome.panel_max);
        assert_eq!(wb.editor_terminals_for_test().len(), 1, "the terminal is a tab of the editor area now");
        assert!(wb.tabs.page_active && matches!(wb.tabs.page, Some(Page::Settings(_))), "and Settings, not the terminal, is in front");
    });
}

#[gpui::test]
fn each_tab_offers_its_own_buttons_at_the_end_of_the_strip(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let ids = |cx: &mut gpui::VisualTestContext, tab: Bottom| wb.read_with(cx, |wb, cx| wb.panel_actions(tab, cx).iter().map(|a| a.id).collect::<Vec<_>>());
    assert!(ids(cx, Bottom::Ports).is_empty(), "without an SSH connection there is nothing to forward, so no buttons");
    assert_eq!(ids(cx, Bottom::Output), ["output-copy", "output-clear"]);
    assert_eq!(ids(cx, Bottom::Terminal), ["terminal-clear"]);
    assert!(ids(cx, Bottom::Problems).is_empty(), "Problems has its own filter bar in the page");
}

#[gpui::test]
fn output_can_be_copied_and_cleared_from_the_strip(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update(cx, |wb, _| {
        wb.log_output("first line".into());
        wb.log_output("second line".into());
    });
    wb.update(cx, |wb, cx| wb.copy_output(cx));
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("first line\nsecond line"));
    wb.update(cx, |wb, cx| wb.clear_output(cx));
    assert!(wb.read_with(cx, |wb, _| wb.output_lines()).is_empty());
}

#[gpui::test]
fn buttons_with_nothing_to_act_on_are_off_and_come_on_with_something_to_do(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let on = |cx: &mut gpui::VisualTestContext, tab: Bottom| {
        wb.read_with(cx, |wb, cx| wb.panel_actions(tab, cx).iter().map(|a| (a.id, a.enabled)).collect::<Vec<_>>())
    };
    assert_eq!(on(cx, Bottom::Output), [("output-copy", false), ("output-clear", false)], "an empty Output has nothing to copy or clear");
    wb.update(cx, |wb, _| wb.log_output("a line".into()));
    assert_eq!(on(cx, Bottom::Output), [("output-copy", true), ("output-clear", true)]);
    wb.update(cx, |wb, cx| wb.clear_output(cx));
    assert!(on(cx, Bottom::Output).iter().all(|(_, enabled)| !enabled), "and off again once it is cleared");
    wb.update(cx, |wb, _| wb.terminals.clear());
    assert_eq!(on(cx, Bottom::Terminal), [("terminal-clear", false)], "no terminal, nothing to clear");
}
