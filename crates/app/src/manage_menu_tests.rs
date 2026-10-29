//! Menu of the gear button at the bottom left (Manage): grouping, position, platform look and the actions it runs.

use gpui::{Corner, TestAppContext, point, px};

use crate::i18n::LangGuard;
use crate::menu::Flavor;
use crate::qa_workbench_tests::setup;
use crate::workbench::Workbench;

const LABELS: [&str; 6] = ["Command Palette…", "Settings", "Keyboard Shortcuts", "Color Theme…", "File Icon Theme…", "About Codeyard"];

/// Like VS Code: commands first, then preferences, About last; groups are separated by a divider.
#[gpui::test]
fn the_manage_menu_groups_commands_preferences_and_about(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.open_manage_menu_for_test(window, cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).expect("menu opened"), LABELS);
    assert_eq!(
        wb.read_with(cx, |wb, cx| wb.menu_separators_for_test(cx)).unwrap(),
        [false, true, false, false, false, true],
        "separators sit before Settings and before About"
    );
}

/// The menu opens to the right of the icon and its bottom edge is aligned with the icon (not at the mouse position): it grows upward.
#[gpui::test]
fn the_manage_menu_opens_beside_the_gear_and_grows_upward(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let height = wb.update_in(cx, |wb, window, cx| {
        wb.open_manage_menu_for_test(window, cx);
        window.viewport_size().height
    });
    assert_eq!(wb.read_with(cx, |wb, cx| wb.menu_anchor_for_test(cx)), Some(Corner::BottomLeft));
    let at = wb.read_with(cx, |wb, _| wb.menu_position_for_test()).unwrap();
    assert_eq!(at, point(px(theme::layout::ACTIVITYBAR_W + 2.), height - px(theme::layout::STATUSBAR_H)), "right of the activity bar, above the status bar");
}

#[test]
fn the_origin_follows_the_status_bar() {
    let h = px(800.);
    let with = Workbench::manage_menu_origin_for_test(h, true);
    let without = Workbench::manage_menu_origin_for_test(h, false);
    assert_eq!(with.x, without.x);
    assert_eq!(without.y, h, "no status bar: the bar ends at the window's bottom edge");
    assert_eq!(with.y, h - px(theme::layout::STATUSBAR_H));
    assert!(with.x > px(theme::layout::ACTIVITYBAR_W), "the menu clears the activity bar");
}

#[gpui::test]
fn selecting_about_runs_the_command_and_closes_the_menu(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.open_manage_menu_for_test(window, cx));
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("About Codeyard", cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).is_none(), "the menu closes");
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("about", true)));
    cx.simulate_keystrokes("escape");
    assert!(wb.read_with(cx, |wb, _| wb.page_for_test()).is_none());
}

#[gpui::test]
fn selecting_command_palette_opens_it(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.open_manage_menu_for_test(window, cx));
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Command Palette…", cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some(), "the palette is open");
}

#[gpui::test]
fn the_manage_menu_follows_the_language(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let _g = LangGuard::set("tr");
    wb.update_in(cx, |wb, window, cx| wb.open_manage_menu_for_test(window, cx));
    let tr = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).unwrap();
    assert_eq!(tr.len(), LABELS.len());
    assert!(tr.iter().all(|l| !LABELS.contains(&l.as_str())), "every entry is translated: {tr:?}");
    assert_eq!(tr[1], "Ayarlar");
}

/// Each platform has its own menu metrics and the current platform's are chosen (the metrics themselves are tested in `menu.rs`).
#[test]
fn the_menu_uses_the_running_platforms_metrics() {
    let m = Flavor::current().metrics();
    assert_eq!(m.accent_highlight, cfg!(target_os = "macos"));
    assert!(m.row_h >= 24.);
}
