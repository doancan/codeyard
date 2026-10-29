//! Interface scale (`ui.scale`, View: Zoom In / Zoom Out / Reset Zoom).
//!
//! GPUI applies the scale at one boundary (`vendor/gpui/src/ui_scale.rs`): the window renders with display scale ×
//! interface scale, its viewport is the content size divided by it, and platform input is divided by it. These tests
//! cover the pure transforms, the setting, and the whole workbench through the test platform, which delivers mouse
//! events in platform points exactly like a real window does.

use std::path::{Path, PathBuf};

use gpui::{
    Bounds, Entity, EntityInputHandler, FileDropEvent, Focusable, KeyDownEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseExitEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, Size, TestAppContext, TouchPhase, VisualTestContext,
    WindowHandle, point, px, size,
};
use serde_json::{Value, json};

use crate::qa_workbench_tests::{fresh, globals, open, open_in, setup_with_config};
use crate::settings::{self, Settings};
use crate::workbench::Workbench;
use crate::workbench::settings_ui::parse_setting_input;

const MESSAGE: &str = "Interface scale must be between 75% and 200%.";

// --- Setting: validation and serialization -------------------------------------------------------------------

fn parsed(text: &str) -> (f32, Vec<String>) {
    let (s, warnings) = settings::parse(text);
    (s.ui_scale, warnings)
}

#[test]
fn ui_scale_defaults_to_100_percent_without_warnings() {
    assert_eq!(Settings::default().ui_scale, 1.);
    assert_eq!(parsed("{}"), (1., vec![]));
    assert_eq!(parsed(r#"{ "ui.scale": null }"#), (1., vec![]));
}

#[test]
fn ui_scale_accepts_both_ends_of_the_range_and_values_between_steps() {
    for (text, want) in [("0.75", 0.75), ("1", 1.), ("1.15", 1.15), ("1.5", 1.5), ("2", 2.), ("2.0", 2.)] {
        assert_eq!(parsed(&format!(r#"{{ "ui.scale": {text} }}"#)), (want, vec![]), "ui.scale {text}");
    }
}

#[test]
fn invalid_ui_scale_falls_back_to_100_percent_and_says_why() {
    for bad in [r#""1.5""#, r#""NaN""#, r#""Infinity""#, "-1", "0", "0.7499", "2.0001", "1e308", "-1e308", "true", "[]", "{}", r#""""#] {
        let (scale, warnings) = parsed(&format!(r#"{{ "ui.scale": {bad} }}"#));
        assert_eq!(scale, 1., "ui.scale {bad}");
        assert_eq!(warnings, vec![format!("settings.jsonc: ui.scale: {MESSAGE}")], "ui.scale {bad}");
    }
}

#[test]
fn invalid_ui_scale_is_reported_in_turkish() {
    let _g = crate::i18n::LangGuard::set("tr");
    let (_, warnings) = parsed(r#"{ "ui.scale": 9 }"#);
    assert_eq!(warnings, vec!["settings.jsonc: ui.scale: Arayüz ölçeği %75 ile %200 arasında olmalıdır.".to_string()]);
}

#[test]
fn non_json_numbers_keep_the_previous_settings_instead_of_zooming() {
    // JSON has no NaN or Infinity: such a file is unreadable as a whole, and the last valid settings stay.
    for bad in ["NaN", "Infinity", "-Infinity"] {
        assert!(settings::parse_checked(&format!(r#"{{ "ui.scale": {bad} }}"#)).is_err(), "{bad}");
    }
}

#[test]
fn a_workspace_settings_file_cannot_set_the_interface_scale() {
    assert!(settings::USER_ONLY_KEYS.contains(&"ui.scale"));
    for trusted in [false, true] {
        let project = [(".codeyard/settings.jsonc", r#"{ "ui.scale": 2 }"#, true)];
        let (s, warnings) = settings::parse_layers_trusted(r#"{ "ui.scale": 1.25 }"#, &project, trusted).unwrap();
        assert_eq!(s.ui_scale, 1.25, "the user's value stays (trusted: {trusted})");
        assert_eq!(warnings, vec![".codeyard/settings.jsonc: “ui.scale” can only be set in your own settings.jsonc".to_string()]);
        let (s, _) = settings::parse_layers_trusted("{}", &project, trusted).unwrap();
        assert_eq!(s.ui_scale, 1., "nor can it set one the user has not");
    }
}

#[test]
fn ui_scale_is_written_as_whole_percent_and_reads_back_unchanged() {
    for step in settings::UI_SCALE_STEPS {
        let value = settings::ui_scale_value(*step);
        let text = settings::set_value_in("{\n  // mine\n  \"theme\": \"paper\"\n}\n", "ui.scale", &value);
        assert!(text.contains("// mine") && text.contains("\"theme\": \"paper\""), "{text}");
        assert_eq!(parsed(&text), (*step, vec![]), "{text}");
    }
    assert_eq!(settings::ui_scale_value(1.1), json!(1.1), "no f32 noise such as 1.100000023841858");
    assert_eq!(settings::ui_scale_value(0.75).to_string(), "0.75");
    assert_eq!(settings::ui_scale_value(2.).to_string(), "2.0");
    // A second write replaces the value instead of adding another key.
    let once = settings::set_value_in("{}", "ui.scale", &settings::ui_scale_value(1.1));
    let twice = settings::set_value_in(&once, "ui.scale", &settings::ui_scale_value(1.2));
    assert_eq!(twice.matches("ui.scale").count(), 1, "{twice}");
}

#[test]
fn zoom_steps_cover_the_range_and_stop_at_both_ends() {
    let mut up = vec![0.75];
    while let Some(&last) = up.last() {
        let next = settings::step_ui_scale(last, true);
        if next == last {
            break;
        }
        up.push(next);
    }
    assert_eq!(up, settings::UI_SCALE_STEPS);
    let mut down = vec![2.];
    while let Some(&last) = down.last() {
        let next = settings::step_ui_scale(last, false);
        if next == last {
            break;
        }
        down.push(next);
    }
    down.reverse();
    assert_eq!(down, settings::UI_SCALE_STEPS);
    assert_eq!(settings::step_ui_scale(1.15, true), 1.2, "between steps: the next one up");
    assert_eq!(settings::step_ui_scale(1.15, false), 1.1, "between steps: the next one down");
    assert_eq!(settings::step_ui_scale(1.1000001, true), 1.2, "float noise does not repeat a step");
    for bad in [f32::NAN, f32::INFINITY] {
        assert_eq!(settings::step_ui_scale(bad, true), 1.1, "{bad}");
    }
    assert_eq!(settings::step_ui_scale(9., true), 2.);
    assert_eq!(settings::step_ui_scale(-3., false), 0.75);
}

#[test]
fn the_notice_shows_a_localized_percentage() {
    assert_eq!(crate::workbench::interface_scale_notice(1.1), "Interface scale: 110%");
    assert_eq!(crate::workbench::interface_scale_notice(0.75), "Interface scale: 75%");
    let _g = crate::i18n::LangGuard::set("tr");
    assert_eq!(crate::workbench::interface_scale_notice(1.1), "Arayüz ölçeği: %110");
    assert_eq!(crate::workbench::interface_scale_notice(2.), "Arayüz ölçeği: %200");
}

#[test]
fn the_settings_list_validates_typed_scales() {
    let entry = crate::workbench::settings_ui::CATALOG.iter().find(|e| e.key == "ui.scale").expect("ui.scale is in the settings list");
    assert_eq!(parse_setting_input("ui.scale", entry.kind, "1.5"), Ok(json!(1.5)));
    assert_eq!(parse_setting_input("ui.scale", entry.kind, "2"), Ok(json!(2)));
    assert_eq!(parse_setting_input("ui.scale", entry.kind, ""), Ok(Value::Null), "empty: back to the default");
    for bad in ["NaN", "inf", "-1", "0.5", "2.5", "abc", "150%"] {
        assert_eq!(parse_setting_input("ui.scale", entry.kind, bad), Err(MESSAGE.to_string()), "{bad}");
    }
    let s = Settings { ui_scale: 1.1, ..Settings::default() };
    assert_eq!(crate::workbench::settings_ui::current(&s, "ui.scale"), json!(1.1));
}

// --- GPUI transforms --------------------------------------------------------------------------------------------

fn every_input(at: Point<Pixels>) -> Vec<PlatformInput> {
    let m = Modifiers::none();
    vec![
        PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position: at, modifiers: m, click_count: 1, first_mouse: false }),
        PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: m, click_count: 1 }),
        PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: m }),
        PlatformInput::MouseExited(MouseExitEvent { position: at, pressed_button: None, modifiers: m }),
        PlatformInput::ScrollWheel(ScrollWheelEvent { position: at, delta: ScrollDelta::Pixels(at), modifiers: m, touch_phase: TouchPhase::Moved }),
        PlatformInput::FileDrop(FileDropEvent::Pending { position: at }),
        PlatformInput::FileDrop(FileDropEvent::Submit { position: at }),
    ]
}

/// The positions an event carries (scroll deltas in pixels included).
fn positions(event: &PlatformInput) -> Vec<Point<Pixels>> {
    match event {
        PlatformInput::MouseDown(e) => vec![e.position],
        PlatformInput::MouseUp(e) => vec![e.position],
        PlatformInput::MouseMove(e) => vec![e.position],
        PlatformInput::MouseExited(e) => vec![e.position],
        PlatformInput::ScrollWheel(e) => match e.delta {
            ScrollDelta::Pixels(d) => vec![e.position, d],
            ScrollDelta::Lines(_) => vec![e.position],
        },
        PlatformInput::FileDrop(FileDropEvent::Entered { position, .. } | FileDropEvent::Pending { position } | FileDropEvent::Submit { position }) => {
            vec![*position]
        }
        _ => vec![],
    }
}

#[test]
fn at_100_percent_input_is_bit_for_bit_unchanged() {
    for at in [point(px(0.), px(0.)), point(px(12.345), px(678.9)), point(px(-3.25), px(1e6))] {
        for event in every_input(at) {
            let before = positions(&event);
            let after = positions(&event.to_logical(1.));
            assert_eq!(before.len(), after.len());
            for (b, a) in before.iter().zip(&after) {
                assert_eq!((f32::from(b.x).to_bits(), f32::from(b.y).to_bits()), (f32::from(a.x).to_bits(), f32::from(a.y).to_bits()));
            }
        }
    }
    let b = Bounds::new(point(px(10.5), px(20.25)), size(px(100.), px(17.)));
    assert_eq!(gpui::ui_bounds_to_platform(b, 1.), b);
    assert_eq!(gpui::ui_size_to_logical(b.size, 1.), b.size);
}

#[test]
fn platform_input_is_divided_by_the_interface_scale() {
    for scale in [0.75, 1.5, 2.] {
        for event in every_input(point(px(300.), px(150.))) {
            for p in positions(&event.to_logical(scale)) {
                assert_eq!(p, point(px(300. / scale), px(150. / scale)), "{scale}");
            }
        }
        let lines = PlatformInput::ScrollWheel(ScrollWheelEvent { delta: ScrollDelta::Lines(point(1., -3.)), ..Default::default() });
        let PlatformInput::ScrollWheel(e) = lines.to_logical(scale) else { unreachable!() };
        assert!(matches!(e.delta, ScrollDelta::Lines(d) if d == point(1., -3.)), "line deltas are not geometry");
        let key = PlatformInput::KeyDown(KeyDownEvent { keystroke: Keystroke::parse("cmd-=").unwrap(), is_held: false });
        let PlatformInput::KeyDown(k) = key.to_logical(scale) else { panic!("a key event stays a key event") };
        assert_eq!(k.keystroke, Keystroke::parse("cmd-=").unwrap());
    }
}

#[test]
fn outgoing_geometry_is_multiplied_and_round_trips() {
    for scale in [0.75, 1.1, 1.5, 2.] {
        let b = Bounds::new(point(px(40.), px(80.)), size(px(8.), px(18.)));
        let out = gpui::ui_bounds_to_platform(b, scale);
        assert_eq!(out, Bounds::new(point(px(40. * scale), px(80. * scale)), size(px(8. * scale), px(18. * scale))));
        let back = gpui::ui_point_to_logical(gpui::ui_point_to_platform(b.origin, scale), scale);
        assert!((f32::from(back.x) - 40.).abs() < 1e-4 && (f32::from(back.y) - 80.).abs() < 1e-4, "{back:?}");
        assert_eq!(gpui::ui_size_to_platform(gpui::ui_size_to_logical(size(px(1280.), px(800.)), scale), scale), size(px(1280.), px(800.)));
    }
}

#[test]
fn gpui_rejects_unusable_scales() {
    for (input, want) in
        [(f32::NAN, 1.), (f32::INFINITY, 1.), (f32::NEG_INFINITY, 1.), (0., 1.), (-2., 1.), (0.01, gpui::MIN_UI_SCALE), (100., gpui::MAX_UI_SCALE), (1.5, 1.5)]
    {
        assert_eq!(gpui::normalize_ui_scale(input), want, "{input}");
    }
}

// --- Whole workbench -------------------------------------------------------------------------------------------

fn config(name: &str) -> PathBuf {
    fresh(&format!("ui-scale-config-{name}"))
}

fn settings_text(cfg: &Path) -> String {
    std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap_or_default()
}

fn rig<'a>(cx: &'a mut TestAppContext, name: &str) -> (Entity<Workbench>, &'a mut VisualTestContext, PathBuf) {
    let cfg = config(name);
    let (wb, cx) = setup_with_config(cx, &cfg);
    cx.simulate_resize(size(px(1280.), px(800.)));
    cx.run_until_parked();
    (wb, cx, cfg)
}

fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(command, window, cx));
    cx.run_until_parked();
}

/// (App scale, setting, window scale, window scale factor, viewport) after the frame.
fn state(cx: &mut VisualTestContext) -> (f32, f32, f32, f32, Size<Pixels>) {
    cx.update(|window, cx| (cx.ui_scale(), cx.global::<Settings>().ui_scale, window.ui_scale(), window.scale_factor(), window.viewport_size()))
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default()
}

fn scaled(p: Point<Pixels>, scale: f32) -> Point<Pixels> {
    point(p.x * scale, p.y * scale)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

/// Equal up to GPUI's snapping of layout to device pixels (at most half a device pixel, in logical pixels).
fn same_place(a: Bounds<Pixels>, b: Bounds<Pixels>, scale_factor: f32) -> bool {
    let tolerance = 0.5 / scale_factor + 0.001;
    [(a.origin.x, b.origin.x), (a.origin.y, b.origin.y), (a.size.width, b.size.width), (a.size.height, b.size.height)]
        .into_iter()
        .all(|(x, y)| (f32::from(x) - f32::from(y)).abs() <= 2. * tolerance)
}

/// A workbench window on `cfg`, focused like a window the user just opened.
fn workbench_window(cx: &mut TestAppContext, cfg: &Path) -> WindowHandle<Workbench> {
    let dir = cfg.to_path_buf();
    let handle = cx.add_window(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    handle.update(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx))).unwrap();
    handle
}

#[gpui::test]
fn content_size_requests_are_multiplied_and_the_window_frame_is_not(cx: &mut TestAppContext) {
    let (_wb, cx, _cfg) = rig(cx, "resize");
    cx.update(|_, cx| cx.set_ui_scale(1.5));
    cx.run_until_parked();
    let frame = cx.update(|window, _| window.bounds());
    assert_eq!(frame.size, size(px(1280.), px(800.)), "zooming does not resize the window");
    // A logical content size (a dialog sized to its content) becomes platform points.
    cx.update(|window, _| window.resize(size(px(400.), px(300.))));
    assert_eq!(cx.update(|window, _| window.bounds().size), size(px(600.), px(450.)));
    // Unusable values never reach the windows.
    for bad in [f32::NAN, 0., -1., f32::INFINITY] {
        cx.update(|_, cx| cx.set_ui_scale(bad));
        cx.run_until_parked();
        assert_eq!(cx.update(|window, cx| (cx.ui_scale(), window.ui_scale())), (1., 1.), "{bad}");
    }
}

#[gpui::test]
fn at_100_percent_the_window_geometry_is_unchanged(cx: &mut TestAppContext) {
    let (_wb, cx, cfg) = rig(cx, "ac1");
    assert_eq!(state(cx), (1., 1., 1., 2., size(px(1280.), px(800.))), "display scale only, viewport = content size");
    assert!(!settings_text(&cfg).contains("ui.scale"), "nothing is written at start");
    let b = cx.debug_bounds("act-search").expect("activity bar button");
    cx.simulate_mouse_move(b.center(), None, Modifiers::none());
    assert_eq!(cx.update(|window, _| window.mouse_position()), b.center(), "platform points are logical pixels at 100%");
}

#[gpui::test]
fn zoom_commands_and_keys_change_the_scale_and_reset_returns_to_100(cx: &mut TestAppContext) {
    let (wb, cx, cfg) = rig(cx, "commands");
    run(&wb, cx, "View: Zoom In");
    let (app, setting, win, factor, viewport) = state(cx);
    assert_eq!((app, setting, win), (1.1, 1.1, 1.1));
    assert!(close(factor, 2.2), "display 2.0 × interface 1.1: {factor}");
    assert!(close(f32::from(viewport.width), 1280. / 1.1) && close(f32::from(viewport.height), 800. / 1.1), "{viewport:?}");
    assert_eq!(notice(&wb, cx), "Interface scale: 110%");
    assert!(settings_text(&cfg).contains("\"ui.scale\": 1.1"), "{}", settings_text(&cfg));

    run(&wb, cx, "View: Zoom Out");
    run(&wb, cx, "View: Zoom Out");
    assert_eq!(state(cx).0, 0.9);
    assert_eq!(notice(&wb, cx), "Interface scale: 90%");
    run(&wb, cx, "View: Reset Zoom");
    assert_eq!(state(cx), (1., 1., 1., 2., size(px(1280.), px(800.))), "reset is exactly 100%");
    assert_eq!(notice(&wb, cx), "Interface scale: 100%");

    // Keyboard: ⌘= and ⌘+ zoom in, ⌘- out, ⌘0 resets.
    cx.simulate_keystrokes("cmd-= cmd-=");
    assert_eq!(state(cx).0, 1.2);
    cx.simulate_keystrokes("cmd-+");
    assert_eq!(state(cx).0, 1.3);
    cx.simulate_keystrokes("cmd--");
    assert_eq!(state(cx).0, 1.2);
    cx.simulate_keystrokes("cmd-0");
    assert_eq!(state(cx).0, 1.);
    assert!(settings_text(&cfg).contains("\"ui.scale\": 1"), "{}", settings_text(&cfg));
}

#[gpui::test]
fn zoom_without_a_settings_folder_still_applies_for_the_session(cx: &mut TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    run(&wb, cx, "View: Zoom In");
    assert_eq!(state(cx).0, 1.1);
    assert_eq!(notice(&wb, cx), "Interface scale: 110%");
    run(&wb, cx, "View: Reset Zoom");
    assert_eq!(state(cx).0, 1.);
}

#[gpui::test]
fn rapid_zoom_clamps_at_both_ends_and_writes_only_real_changes(cx: &mut TestAppContext) {
    let (wb, cx, cfg) = rig(cx, "rapid");
    for _ in 0..40 {
        cx.simulate_keystrokes("cmd-=");
    }
    assert_eq!(state(cx).0, 2.);
    assert_eq!(settings_text(&cfg).matches("ui.scale").count(), 1, "{}", settings_text(&cfg));
    assert!(settings_text(&cfg).contains("\"ui.scale\": 2.0"), "{}", settings_text(&cfg));
    // Already at 200%: another Zoom In is not written again.
    let file = cfg.join("settings.jsonc");
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
    run(&wb, cx, "View: Zoom In");
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), old, "no write without a change");
    assert_eq!(notice(&wb, cx), "Interface scale: 200%", "the notice still shows where it is");
    for _ in 0..40 {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Zoom Out", window, cx));
    }
    cx.run_until_parked();
    assert_eq!(state(cx).0, 0.75);
    assert!(settings_text(&cfg).contains("\"ui.scale\": 0.75"), "{}", settings_text(&cfg));
    let (.., factor, viewport) = state(cx);
    assert!(close(factor, 1.5) && close(f32::from(viewport.width), 1280. / 0.75), "{factor} {viewport:?}");
}

#[gpui::test]
fn the_scale_persists_and_comes_back_after_reload_and_restart(cx: &mut TestAppContext) {
    let cfg = config("persist");
    globals(cx);
    let first = workbench_window(cx, &cfg);
    cx.run_until_parked();
    first.update(cx, |wb, window, cx| wb.run_command_for_test("View: Zoom In", window, cx)).unwrap();
    first.update(cx, |wb, window, cx| wb.run_command_for_test("View: Zoom In", window, cx)).unwrap();
    cx.run_until_parked();
    assert!(settings_text(&cfg).contains("\"ui.scale\": 1.2"));

    // A settings reload finds the same value: nothing changes and nothing is written.
    let before = settings_text(&cfg);
    first.update(cx, |wb, window, cx| wb.reload_config_for_test(window, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(settings_text(&cfg), before);
    assert_eq!(cx.update(|cx| cx.ui_scale()), 1.2);

    // "Restart": a fresh application state, then a window on the same settings folder.
    first.update(cx, |_, window, _| window.remove_window()).unwrap();
    cx.update(|cx| {
        cx.set_ui_scale(1.);
        cx.set_global(Settings::default());
    });
    let again = workbench_window(cx, &cfg);
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| (cx.ui_scale(), cx.global::<Settings>().ui_scale)), (1.2, 1.2));
    assert_eq!(again.update(cx, |_, window, _| window.ui_scale()).unwrap(), 1.2, "the first frame already uses the saved scale");

    // Edited by hand to something invalid: back to 100% with the message, the window stays usable.
    std::fs::write(cfg.join("settings.jsonc"), "{ \"ui.scale\": \"huge\" }").unwrap();
    again.update(cx, |wb, window, cx| wb.reload_config_for_test(window, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| (cx.ui_scale(), cx.global::<Settings>().ui_scale)), (1., 1.));
    assert!(again.update(cx, |wb, _, _| wb.notice_text()).unwrap().unwrap_or_default().contains(MESSAGE));
    for bad in ["-2", "0", "2.5", "1e99", "[1.5]"] {
        std::fs::write(cfg.join("settings.jsonc"), "{ \"ui.scale\": 1.5 }").unwrap();
        again.update(cx, |wb, window, cx| wb.reload_config_for_test(window, cx)).unwrap();
        assert_eq!(cx.update(|cx| cx.ui_scale()), 1.5);
        std::fs::write(cfg.join("settings.jsonc"), format!("{{ \"ui.scale\": {bad} }}")).unwrap();
        again.update(cx, |wb, window, cx| wb.reload_config_for_test(window, cx)).unwrap();
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.ui_scale()), 1., "{bad}");
        assert_eq!(again.update(cx, |_, window, _| window.ui_scale()).unwrap(), 1., "{bad}");
    }
    // A half-written file keeps the last valid settings rather than jumping.
    std::fs::write(cfg.join("settings.jsonc"), "{ \"ui.scale\": 1.3 }").unwrap();
    again.update(cx, |wb, window, cx| wb.reload_config_for_test(window, cx)).unwrap();
    std::fs::write(cfg.join("settings.jsonc"), "{ \"ui.scale\": NaN }").unwrap();
    again.update(cx, |wb, window, cx| wb.reload_config_for_test(window, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.ui_scale()), 1.3);
}

#[gpui::test]
fn every_open_window_follows_the_scale_without_an_update_loop(cx: &mut TestAppContext) {
    let cfg = config("windows");
    globals(cx);
    let (a, b, c) = (workbench_window(cx, &cfg), workbench_window(cx, &cfg), workbench_window(cx, &cfg));
    cx.run_until_parked();
    a.update(cx, |wb, window, cx| wb.run_command_for_test("View: Zoom In", window, cx)).unwrap();
    cx.run_until_parked();
    for w in [a, b, c] {
        assert_eq!(w.update(cx, |_, window, _| (window.ui_scale(), window.scale_factor())).unwrap(), (1.1, 2.2));
    }
    // The other windows notice the file changed and re-read it: same value, so no write and no further change.
    let written = settings_text(&cfg);
    for _ in 0..3 {
        for w in [b, c, a] {
            w.update(cx, |wb, window, cx| wb.reload_config_for_test(window, cx)).unwrap();
        }
        cx.run_until_parked();
    }
    assert_eq!(settings_text(&cfg), written);
    // Zooming from another window continues from the shared value.
    c.update(cx, |wb, window, cx| wb.run_command_for_test("View: Zoom In", window, cx)).unwrap();
    cx.run_until_parked();
    for w in [a, b, c] {
        assert_eq!(w.update(cx, |_, window, _| window.ui_scale()).unwrap(), 1.2);
    }
    assert_eq!(settings_text(&cfg).matches("ui.scale").count(), 1);
}

#[gpui::test]
fn a_workspace_file_cannot_force_a_global_zoom(cx: &mut TestAppContext) {
    let (wb, cx, _cfg) = rig(cx, "workspace");
    let project = fresh("ui-scale-workspace-project");
    std::fs::create_dir_all(project.join(".codeyard")).unwrap();
    std::fs::write(project.join(".codeyard/settings.jsonc"), "{ \"ui.scale\": 2, \"editor.tabSize\": 3 }").unwrap();
    open(&wb, &project, cx);
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.global::<Settings>().tab_size), Some(3), "the project layer was read");
    assert_eq!(state(cx).0, 1., "but it cannot zoom the application");
    assert!(notice(&wb, cx).contains("“ui.scale” can only be set in your own settings.jsonc"), "{}", notice(&wb, cx));
}

#[gpui::test]
fn editor_and_terminal_font_preferences_are_kept_and_scaled_once(cx: &mut TestAppContext) {
    let (wb, cx, cfg) = rig(cx, "fonts");
    std::fs::write(cfg.join("settings.jsonc"), "{ \"editor.fontSize\": 15, \"editor.lineHeight\": 22, \"terminal.fontSize\": 13 }").unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    let project = crate::qa_workbench_tests::project("ui-scale-fonts");
    open(&wb, &project, cx);
    open_in(&wb, &project.join("src/lib.rs"), cx);
    cx.run_until_parked();
    let fonts = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let s = cx.global::<Settings>();
            (s.editor_font_size, s.editor_line_height, s.terminal_font_size)
        })
    };
    // Chrome, tree, tabs, editor text and status bar: their logical layout in a window of the same logical size.
    let selectors = ["act-explorer", "act-search", "act-manage", "editor-group-0"];
    let layout = |cx: &mut VisualTestContext| selectors.map(|s| cx.debug_bounds(s).unwrap_or_else(|| panic!("{s}")));
    let base = (fonts(cx), layout(cx));
    for (steps, scale) in [(5, 1.5), (5, 2.)] {
        for _ in 0..steps {
            run(&wb, cx, "View: Zoom In");
        }
        // A window `scale` times larger in points has the same logical size as the 100% window.
        cx.simulate_resize(size(px(1280. * scale), px(800. * scale)));
        cx.run_until_parked();
        let (_, _, _, factor, viewport) = state(cx);
        assert!(close(factor, 2. * scale), "{factor}");
        assert_eq!(viewport, size(px(1280.), px(800.)));
        // The preferences are what the user set and the logical layout is the 100% layout, so the window's single
        // scale is the only multiplication: 15 pt editor text is drawn at 15 × scale points, never 15 × scale².
        assert_eq!(fonts(cx), base.0, "at {scale}");
        for (now, before) in layout(cx).into_iter().zip(base.1) {
            assert!(same_place(now, before, factor), "no component scales itself again at {scale}: {now:?} vs {before:?}");
        }
        assert!(settings_text(&cfg).contains("\"editor.fontSize\": 15"));
    }
    run(&wb, cx, "View: Reset Zoom");
    assert_eq!(fonts(cx), base.0);
}

#[gpui::test]
fn a_click_at_the_scaled_position_hits_the_target_at_150_percent(cx: &mut TestAppContext) {
    let (wb, cx, _cfg) = rig(cx, "hit");
    for _ in 0..5 {
        run(&wb, cx, "View: Zoom In");
    }
    assert_eq!(state(cx).2, 1.5);
    for (selector, panel) in [("act-search", "search"), ("act-scm", "scm"), ("act-explorer", "explorer")] {
        let b = cx.debug_bounds(selector).unwrap();
        // The platform reports points: the painted target sits at its logical position × 1.5.
        cx.simulate_click(scaled(b.center(), 1.5), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some(panel), "{selector} at {b:?}");
        cx.simulate_mouse_move(scaled(b.center(), 1.5), None, Modifiers::none());
        let at = cx.update(|window, _| window.mouse_position());
        assert!(close(f32::from(at.x), f32::from(b.center().x)) && close(f32::from(at.y), f32::from(b.center().y)), "{at:?} vs {b:?}");
    }
    // Scroll wheel positions are converted too: a scroll over the scaled target does not panic and keeps the panel.
    let b = cx.debug_bounds("act-explorer").unwrap();
    cx.simulate_event(ScrollWheelEvent { position: scaled(b.center(), 1.5), delta: ScrollDelta::Pixels(point(px(0.), px(30.))), ..Default::default() });
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some("explorer"));
}

#[gpui::test]
fn dragging_a_divider_moves_it_by_the_logical_distance_at_150_percent_even_when_zoomed_mid_drag(cx: &mut TestAppContext) {
    let (wb, cx, _cfg) = rig(cx, "drag");
    let file = fresh("ui-scale-drag").join("notes.txt");
    std::fs::write(&file, "hello\n").unwrap();
    open(&wb, &file, cx);
    run(&wb, cx, "View: Split Editor");
    for _ in 0..5 {
        run(&wb, cx, "View: Zoom In");
    }
    const SASHES: [&str; 4] = ["group-sash-0", "group-sash-1", "group-sash-2", "group-sash-3"];
    let sash_id = SASHES.into_iter().find(|id| cx.debug_bounds(id).is_some()).expect("divider between the groups");
    let sash = cx.debug_bounds(sash_id).unwrap();
    let left = cx.debug_bounds("editor-group-0").unwrap().size.width;
    let s = 1.5;
    cx.simulate_mouse_down(scaled(sash.center(), s), MouseButton::Left, Modifiers::none());
    let target = point(sash.center().x + px(60.), sash.center().y);
    cx.simulate_mouse_move(scaled(target, s), Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(scaled(target, s), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    let grown = cx.debug_bounds("editor-group-0").unwrap().size.width - left;
    assert!((f32::from(grown) - 60.).abs() < 3., "moved {grown:?} logical px for a 60 px drag");

    // Zoom while the divider is held: the drag continues in the new coordinate space without panicking.
    let sash = cx.debug_bounds(sash_id).unwrap();
    cx.simulate_mouse_down(scaled(sash.center(), s), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(scaled(point(sash.center().x - px(20.), sash.center().y), s), Some(MouseButton::Left), Modifiers::none());
    cx.simulate_keystrokes("cmd-=");
    assert_eq!(state(cx).2, 1.6);
    let end = point(sash.center().x - px(40.), sash.center().y);
    cx.simulate_mouse_move(scaled(end, 1.6), Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(scaled(end, 1.6), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    let viewport = state(cx).4;
    let (a, b) = (cx.debug_bounds("editor-group-0").unwrap(), cx.debug_bounds("editor-group-1").unwrap());
    assert!(a.size.width > px(50.) && b.size.width > px(50.), "{a:?} {b:?}");
    assert!(b.right() <= viewport.width + px(1.), "{b:?} inside {viewport:?}");
}

#[gpui::test]
fn zoom_keeps_an_open_picker_and_its_keyboard_focus(cx: &mut TestAppContext) {
    let (wb, cx, _cfg) = rig(cx, "picker");
    run(&wb, cx, "Preferences: Settings");
    assert!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some(), "settings list open");
    cx.simulate_keystrokes("cmd-=");
    assert_eq!(state(cx).0, 1.1);
    assert!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some(), "the list stays open");
    cx.simulate_input("scale");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_query(cx)).as_deref(), Some("scale"), "typing still goes to the list");
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert!(labels.iter().any(|l| l == "ui.scale"), "{labels:?}");
}

#[gpui::test]
fn zoom_keeps_an_open_menu_inside_the_window(cx: &mut TestAppContext) {
    let (wb, cx, _cfg) = rig(cx, "menu");
    wb.update_in(cx, |wb, window, cx| wb.open_manage_menu_for_test(window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.menu_position_for_test()).is_some());
    for _ in 0..10 {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Zoom In", window, cx));
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.menu_position_for_test()).is_some(), "the menu stays open at {}", state(cx).0);
    }
    assert_eq!(state(cx).0, 2.);
}

#[gpui::test]
fn ime_composition_survives_zoom_and_candidates_follow_the_caret(cx: &mut TestAppContext) {
    let (wb, cx, _cfg) = rig(cx, "ime");
    let project = crate::qa_workbench_tests::project("ui-scale-ime");
    open(&wb, &project, cx);
    open_in(&wb, &project.join("src/main.rs"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update_in(cx, |e, window, cx| e.replace_and_mark_text_in_range(None, "´", None, window, cx));
    cx.run_until_parked();
    let at_100 = cx.platform_bounds_for_range(0..1).expect("the editor answers where the composition is");
    let index_100 = cx.platform_character_index_for_point(at_100.center());
    for _ in 0..5 {
        run(&wb, cx, "View: Zoom In");
    }
    assert_eq!(state(cx).2, 1.5);
    assert_eq!(ed.update_in(cx, |e, window, cx| e.marked_text_range(window, cx)), Some(0..1), "composition kept");
    let at_150 = cx.platform_bounds_for_range(0..1).unwrap();
    // Platform points: the logical caret × 1.5, up to the snapping of the caret to device pixels.
    for (a, b) in [(at_150.origin.x, at_100.origin.x), (at_150.origin.y, at_100.origin.y), (at_150.size.height, at_100.size.height)] {
        assert!((f32::from(a) - f32::from(b) * 1.5).abs() <= 0.5, "IME bounds {at_150:?} = {at_100:?} × 1.5");
    }
    assert_eq!(cx.platform_character_index_for_point(at_150.center()), index_100, "a platform point maps to the same character");
    ed.update_in(cx, |e, window, cx| e.replace_text_in_range(None, "é", window, cx));
    cx.run_until_parked();
    assert!(ed.read_with(cx, |e, _| e.text()).starts_with("é"), "{}", ed.read_with(cx, |e, _| e.text()));
}

#[gpui::test]
fn moving_between_displays_keeps_the_logical_scale_and_never_compounds(cx: &mut TestAppContext) {
    let (wb, cx, _cfg) = rig(cx, "dpi");
    for _ in 0..5 {
        run(&wb, cx, "View: Zoom In");
    }
    let b = cx.debug_bounds("act-search").unwrap();
    for (display, factor) in [(1., 1.5), (2., 3.), (1.25, 1.875), (2., 3.), (2., 3.)] {
        cx.simulate_display_scale(display);
        cx.run_until_parked();
        let (app, setting, win, f, viewport) = state(cx);
        assert_eq!((app, setting, win), (1.5, 1.5, 1.5), "display {display}");
        assert!(close(f, factor), "display {display}: {f} (want {factor})");
        assert!(close(f32::from(viewport.width), 1280. / 1.5), "{viewport:?}");
        let now = cx.debug_bounds("act-search").unwrap();
        assert!(same_place(now, b, f), "layout does not jump on display {display}: {now:?} vs {b:?}");
    }
}

#[gpui::test]
fn a_480_point_window_at_200_percent_keeps_its_primary_actions_reachable(cx: &mut TestAppContext) {
    narrow_window_at_200_percent(cx, "en", "Interface scale: 200%");
}

#[gpui::test]
fn a_480_point_window_at_200_percent_in_turkish_keeps_its_primary_actions_reachable(cx: &mut TestAppContext) {
    let _g = crate::i18n::LangGuard::set("tr");
    narrow_window_at_200_percent(cx, "tr", "Arayüz ölçeği: %200");
}

fn narrow_window_at_200_percent(cx: &mut TestAppContext, language: &str, notice_text: &str) {
    let (wb, cx, cfg) = rig(cx, &format!("narrow-{language}"));
    std::fs::write(cfg.join("settings.jsonc"), format!("{{ \"ui.language\": \"{language}\" }}")).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    let project = crate::qa_workbench_tests::project(&format!("ui-scale-narrow-{language}"));
    open(&wb, &project, cx);
    open_in(&wb, &project.join("src/main.rs"), cx);
    for _ in 0..10 {
        run(&wb, cx, "View: Zoom In");
    }
    assert_eq!(state(cx).2, 2.);
    assert_eq!(notice(&wb, cx), notice_text);
    for ((w, h), (target, panel)) in
        [((480., 320.), ("act-search", "search")), ((480., 800.), ("act-scm", "scm")), ((1280., 800.), ("act-explorer", "explorer"))]
    {
        cx.simulate_resize(size(px(w), px(h)));
        cx.run_until_parked();
        let viewport = state(cx).4;
        assert_eq!(viewport, size(px(w / 2.), px(h / 2.)));
        let inside = Bounds::new(point(px(0.), px(0.)), viewport);
        let views = cx.debug_bounds("activity-views").expect("activity views");
        let manage = cx.debug_bounds("act-manage").unwrap_or_else(|| panic!("manage button rendered at {w}×{h}"));
        assert!(inside.contains(&manage.center()), "manage {manage:?} inside {viewport:?}");
        assert!(views.bottom() <= manage.top() + px(0.5), "the views {views:?} do not run under the manage button {manage:?}");
        assert!(views.size.height >= px(47.), "at least one view button is fully visible: {views:?}");
        // A view that does not fit is scrolled into reach with the wheel (in platform points, like a trackpad).
        for _ in 0..12 {
            let b = cx.debug_bounds(target).unwrap();
            if views.contains(&b.center()) {
                break;
            }
            let dy = if b.center().y < views.top() { px(40.) } else { px(-40.) };
            let wheel = ScrollWheelEvent { position: scaled(views.center(), 2.), delta: ScrollDelta::Pixels(point(px(0.), dy)), ..Default::default() };
            cx.simulate_event(wheel);
            cx.run_until_parked();
        }
        let b = cx.debug_bounds(target).unwrap();
        assert!(views.contains(&b.center()) && inside.contains(&b.center()), "{target} {b:?} scrolled into {views:?}");
        // The click reaches it.
        cx.simulate_click(scaled(b.center(), 2.), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some(panel), "{w}×{h}: {target} {b:?} in {viewport:?}");
    }
}
