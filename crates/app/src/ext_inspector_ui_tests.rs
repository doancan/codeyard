//! #108 end to end with a real exthost and crates/ext-host/testdata/probe.wasm: permission switches that apply without
//! a restart, the denials list with its fix, call times, and the crash card. Skipped when codeyard-exthost is not built
//! (`cargo test --workspace` always builds it).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ext_api::value::Why;
use gpui::{Entity, Focusable, TestAppContext, VisualTestContext};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe.wasm");

pub(crate) fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-inspector-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn manifest(version: &str) -> String {
    format!(
        r#"[extension]
id = "you.probe"
version = "{version}"
name = "Probe"
license = "MIT"
repository = "https://github.com/you/probe"

[code]
entry = "Cargo.toml"

[permissions]
read = ["a.txt"]
net = ["api.example.com"]

[[commands]]
id = "fetch"
title = "Probe: Fetch"

[[commands]]
id = "trap"
title = "Probe: Trap"

[[commands]]
id = "doc"
title = "Probe: Document"
"#
    )
}

pub(crate) fn install(cfg: &Path, version: &str) {
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest(version).into_bytes(), executable: false },
        ext_pkg::Entry { path: "Cargo.toml".into(), data: Vec::new(), executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: PROBE.to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).expect("the probe passes the import check");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "/downloads/probe.cyx".into() }).expect("installs");
}

pub(crate) fn setup<'a>(cx: &'a mut TestAppContext, cfg: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let dir = cfg.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| {
        wb.reload_config_for_test(window, cx);
        window.focus(&wb.focus_handle(cx));
    });
    cx.run_until_parked();
    (wb, cx)
}

fn wait_for(cx: &mut VisualTestContext, wb: &Entity<Workbench>, done: impl Fn(&Workbench) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        cx.run_until_parked();
        if wb.read_with(cx, |wb, _| done(wb)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out; notice: {:?}; answer: {:?}",
        wb.read_with(cx, |wb, _| wb.notice_for_test()),
        wb.read_with(cx, |wb, _| wb.extension_answer_for_test("you.probe"))
    );
}

fn exthost_built() -> bool {
    let built = crate::ext_runtime::exthost_path().is_some();
    if !built {
        eprintln!("skipped: codeyard-exthost is not built (cargo build -p ext-host)");
    }
    built
}

fn answer(wb: &Workbench) -> String {
    wb.extension_answer_for_test("you.probe").unwrap_or_default()
}

/// AC2: turning `net` off on the extension page makes the next call of the running extension return `revoked-by-user`;
/// the denial is listed with the line to turn back on, and the switch is saved for this extension and publisher.
#[gpui::test]
fn a_permission_turned_off_is_denied_from_the_next_call(cx: &mut TestAppContext) {
    if !exthost_built() {
        return;
    }
    let cfg = config_dir("switch");
    install(&cfg, "1.0.0");
    let (wb, cx) = setup(cx, &cfg);
    // Declared and on: the call reaches the app (net is not wired up in this version, so it fails there).
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Fetch", window, cx));
    wait_for(cx, &wb, |wb| answer(wb).starts_with("failed"));
    assert!(wb.read_with(cx, |wb, _| wb.extension_running_for_test("you.probe")));
    assert!(wb.read_with(cx, |wb, _| wb.denials_for_test("you.probe")).is_empty());

    wb.update(cx, |wb, cx| wb.set_permission_for_test("you.probe", "net", "api.example.com", false, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Fetch", window, cx));
    wait_for(cx, &wb, |wb| answer(wb) == "denied net Why::RevokedByUser");
    let denials = wb.read_with(cx, |wb, _| wb.denials_for_test("you.probe"));
    assert_eq!(denials, [("net/fetch".to_string(), "net".to_string(), Why::RevokedByUser, Some(r#"net = ["api.example.com"]"#.to_string()))]);
    let revoked = crate::ext::store(&cfg).revoked_permissions(&ext_manifest::ExtId::parse("you.probe").unwrap(), &ext_core::Source::File { path: "x".into() });
    assert_eq!(revoked.net, ["api.example.com"]);

    // Each call has its times: the request's guest time, the host calls' I/O and overhead.
    let calls = wb.read_with(cx, |wb, _| wb.extension_calls_for_test("you.probe"));
    assert!(calls.iter().any(|c| c.name == "command:fetch" && c.guest.is_some()), "{calls:?}");
    assert!(calls.iter().any(|c| c.name == "net/fetch" && c.guest.is_none()), "{calls:?}");

    // On again: the same running exthost may call it.
    wb.update(cx, |wb, cx| wb.set_permission_for_test("you.probe", "net", "api.example.com", true, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Fetch", window, cx));
    wait_for(cx, &wb, |wb| answer(wb).starts_with("failed"));
    assert_eq!(wb.read_with(cx, |wb, _| wb.denials_for_test("you.probe")).len(), 1);
}

/// The crash card: three traps turn the extension off; the report holds the version and the trap but no file contents
/// and no paths outside the extension (AC3); **Reinstall Previous Version** goes back and lets it run again.
#[gpui::test]
fn three_crashes_show_the_crash_card(cx: &mut TestAppContext) {
    if !exthost_built() {
        return;
    }
    let cfg = config_dir("crash");
    install(&cfg, "1.0.0");
    install(&cfg, "1.0.1");
    let (wb, cx) = setup(cx, &cfg);
    // The extension reads an open document first: its text must not end up in the report.
    let file = cfg.join("notes.rs");
    std::fs::write(&file, "fn secret_main() {}\n").unwrap();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&file.to_string_lossy(), window, cx));
    cx.run_until_parked();
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Document", window, cx));
    wait_for(cx, &wb, |wb| answer(wb).starts_with("document"));
    for _ in 0..3 {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Trap", window, cx));
        wait_for(cx, &wb, |wb| !wb.extension_running_for_test("you.probe"));
    }
    wait_for(cx, &wb, |wb| wb.extension_disabled_for_test("you.probe"));

    wb.update(cx, |wb, cx| wb.open_extension_page_for_test("you.probe", cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("ext-crash-card").is_some(), "the crash card is on the page");
    for button in ["container-button-Reinstall Previous Version", "container-button-Copy Report", "container-button-Report…"] {
        assert!(cx.debug_bounds(button).is_some(), "{button}");
    }
    assert!(cx.debug_bounds("ext-switch-0").is_some() && cx.debug_bounds("ext-switch-1").is_some(), "a switch per declared entry");

    let report = wb.update(cx, |wb, _| wb.crash_report_for_test("you.probe")).unwrap();
    assert!(report.starts_with("you.probe 1.0.1 crashed"), "{report}");
    assert!(report.contains("trap: ") && report.contains("unreachable"), "{report}");
    assert!(!report.contains("secret_main"), "no file contents: {report}");
    let home = std::env::var("HOME").unwrap_or_default();
    for leak in [cfg.to_string_lossy().to_string(), std::env::temp_dir().to_string_lossy().to_string(), home] {
        assert!(leak.is_empty() || !report.contains(leak.trim_end_matches('/')), "{leak} in {report}");
    }

    // Back to 1.0.0, which may run again.
    wb.update_in(cx, |wb, window, cx| wb.reinstall_previous_for_test("you.probe", window, cx));
    cx.run_until_parked();
    assert!(!wb.read_with(cx, |wb, _| wb.extension_disabled_for_test("you.probe")));
    assert!(cx.debug_bounds("ext-crash-card").is_none());
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Document", window, cx));
    cx.run_until_parked();
    assert!(!wb.read_with(cx, |wb, _| wb.notice_for_test().is_some_and(|(_, t)| t.contains("turned off until"))));
    let version = crate::ext::current().active.iter().find(|e| e.id() == "you.probe").map(|e| e.manifest.version.to_string());
    assert_eq!(version.as_deref(), Some("1.0.0"));
}
