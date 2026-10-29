use super::*;
use gpui::TestAppContext;

fn configure_profiles(wb: &mut Workbench, config: &std::path::Path, window: &mut Window, cx: &mut Context<Workbench>) {
    let os = crate::terminal_profiles::os_key(&wb.remote.host.info().os);
    let mut settings = serde_json::Map::new();
    settings.insert(format!("terminal.profiles.{os}"), serde_json::json!({ "Fixture": { "path": crate::ui::local_path(&config.join("not-a-shell.exe")) } }));
    std::fs::write(config.join("settings.jsonc"), serde_json::Value::Object(settings).to_string()).unwrap();
    wb.reload_config(window, cx);
}

#[gpui::test]
fn a_pending_terminal_launch_dropdown_cannot_open_in_another_context(cx: &mut TestAppContext) {
    check_launch_dropdown(cx, true);
}

#[gpui::test]
fn a_visible_terminal_launch_dropdown_cannot_start_in_another_context(cx: &mut TestAppContext) {
    check_launch_dropdown(cx, false);
}

fn check_launch_dropdown(cx: &mut TestAppContext, pending: bool) {
    for change_host in [false, true] {
        let config = crate::qa_workbench_tests::fresh(&format!("launch-dropdown-config-{pending}-{change_host}"));
        let other = crate::qa_workbench_tests::fresh(&format!("launch-dropdown-root-{pending}-{change_host}"));
        let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &config);
        wb.update_in(cx, |wb, window, cx| {
            configure_profiles(wb, &config, window, cx);
            wb.open_terminal_launch_menu(gpui::point(px(0.), px(0.)), window, cx);
        });
        if !pending {
            cx.run_until_parked();
            assert!(wb.read_with(cx, |wb, cx| wb.menu_labels(cx).is_some_and(|labels| labels.first().is_some_and(|label| label == "Fixture"))));
        }
        wb.update_in(cx, |wb, _, cx| {
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.replace_root(crate::ui::local_path(&other), cx);
            }
            if !pending {
                wb.select_menu_for_test("Fixture", cx);
            }
        });
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.menu.is_none()), "a stale launch menu must not remain in the new context");
        assert!(wb.read_with(cx, |wb, _| wb.terminals.is_empty()), "a stale dropdown profile must not start a terminal in the new context");
    }
}

#[gpui::test]
fn current_terminal_profile_menus_keep_accepting_the_selected_profile(cx: &mut TestAppContext) {
    for menu in 0..3 {
        let config = crate::qa_workbench_tests::fresh(&format!("current-profile-menu-{menu}"));
        let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &config);
        wb.update_in(cx, |wb, window, cx| {
            configure_profiles(wb, &config, window, cx);
            match menu {
                0 => wb.new_terminal_with_profile(&NewTerminalWithProfile, window, cx),
                1 => wb.select_default_terminal_profile(&SelectDefaultTerminalProfile, window, cx),
                _ => wb.open_terminal_launch_menu(gpui::point(px(0.), px(0.)), window, cx),
            }
        });
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| {
            if menu == 2 {
                wb.select_menu_for_test("Fixture", cx);
            } else {
                wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx);
            }
        });
        if menu == 1 {
            assert_eq!(cx.read(|cx| cx.global::<Settings>().terminal_default_profile.clone()).as_deref(), Some("Fixture"));
            assert!(std::fs::read_to_string(config.join("settings.jsonc")).unwrap().contains("terminal.defaultProfile"));
        } else {
            assert_eq!(wb.read_with(cx, |wb, _| wb.terminals.len()), 1, "the fixture launch is accepted in its original context");
        }
    }
}

#[gpui::test]
fn a_pending_terminal_profile_list_cannot_open_in_another_context(cx: &mut TestAppContext) {
    for change_host in [false, true] {
        let config = crate::qa_workbench_tests::fresh(&format!("pending-profile-config-{change_host}"));
        let other = crate::qa_workbench_tests::fresh(&format!("pending-profile-root-{change_host}"));
        let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &config);
        wb.update_in(cx, |wb, window, cx| {
            configure_profiles(wb, &config, window, cx);
            wb.new_terminal_with_profile(&NewTerminalWithProfile, window, cx);
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.replace_root(crate::ui::local_path(&other), cx);
            }
            wb.notice = Some(Notice::Info("new context".into()));
        });
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()), "an old shell listing must not open a menu in the new context");
        assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")));
    }
}

#[gpui::test]
fn old_terminal_profile_rows_cannot_launch_or_change_settings_in_another_context(cx: &mut TestAppContext) {
    check_old_profile_rows(cx, false);
}

#[gpui::test]
fn old_default_terminal_profile_rows_cannot_change_settings_in_another_context(cx: &mut TestAppContext) {
    check_old_profile_rows(cx, true);
}

fn check_old_profile_rows(cx: &mut TestAppContext, default_profile: bool) {
    for change_host in [false, true] {
        let config = crate::qa_workbench_tests::fresh(&format!("old-profile-config-{default_profile}-{change_host}"));
        let other = crate::qa_workbench_tests::fresh(&format!("old-profile-root-{default_profile}-{change_host}"));
        let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &config);
        wb.update_in(cx, |wb, window, cx| {
            configure_profiles(wb, &config, window, cx);
            if default_profile {
                wb.select_default_terminal_profile(&SelectDefaultTerminalProfile, window, cx);
            } else {
                wb.new_terminal_with_profile(&NewTerminalWithProfile, window, cx);
            }
        });
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).is_some_and(|labels| labels.first().is_some_and(|label| label == "Fixture"))));
        let settings_before = std::fs::read_to_string(config.join("settings.jsonc")).unwrap();
        wb.update_in(cx, |wb, window, cx| {
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.replace_root(crate::ui::local_path(&other), cx);
            }
            wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx);
            assert!(wb.terminals.is_empty(), "an old shell/profile must not start on the new host or workspace");
        });
        assert_eq!(std::fs::read_to_string(config.join("settings.jsonc")).unwrap(), settings_before, "a stale default-profile row cannot modify settings");
    }
}

#[gpui::test]
fn terminal_rename_keeps_its_original_target_after_list_changes(cx: &mut TestAppContext) {
    check_terminal_target(cx, false);
}

#[gpui::test]
fn terminal_color_menu_keeps_its_original_target_after_list_changes(cx: &mut TestAppContext) {
    check_terminal_target(cx, true);
}

fn check_terminal_target(cx: &mut TestAppContext, color: bool) {
    for change in 0..4 {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let (first, second) = wb.update(cx, |wb, cx| {
            let first = cx.new(|cx| TerminalView::output_only(wb.remote.host.clone(), "first".into(), cx));
            let second = cx.new(|cx| TerminalView::output_only(wb.remote.host.clone(), "second".into(), cx));
            wb.terminals = vec![first.clone(), second.clone()];
            wb.active_terminal = 0;
            (first, second)
        });
        wb.update_in(cx, |wb, window, cx| {
            if color {
                wb.change_terminal_color(&ChangeTerminalColor, window, cx);
            } else {
                wb.rename_terminal(&RenameTerminal, window, cx);
                wb.set_overlay_query_for_test("renamed".into(), cx);
            }
            match change {
                0 => wb.active_terminal = 1,
                1 => wb.terminals.swap(0, 1),
                2 => {
                    wb.terminals.remove(0);
                }
                _ => {}
            }
            wb.on_overlay_event(&PickerEvent::Confirm(if color { Pick::Item { source: 0, index: 0 } } else { Pick::Input }), window, cx);
        });
        if color {
            assert_eq!(first.read_with(cx, |t, _| t.color.clone()).as_deref(), Some(if change == 2 { "info" } else { "accent" }));
            assert_eq!(second.read_with(cx, |t, _| t.color.clone()).as_deref(), Some("info"), "the menu must never recolor a different terminal");
        } else {
            assert_eq!(first.read_with(cx, |t, _| t.title.to_string()), if change == 2 { "first" } else { "renamed" });
            assert_eq!(second.read_with(cx, |t, _| t.title.to_string()), "second", "the form must never rename a different terminal");
        }
        assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()));
        assert_eq!(wb.read_with(cx, |wb, _| wb.active_terminal), usize::from(change == 0), "confirming a property must not change focus");
    }
}
