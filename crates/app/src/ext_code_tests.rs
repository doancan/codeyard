//! Code extensions in a window, end to end: a package with a real component (crates/ext-host/testdata/probe.wasm)
//! is installed, its commands join the palette, running one starts codeyard-exthost, and what the extension asks
//! for reaches the window. Skipped when codeyard-exthost is not built (`cargo test --workspace` always builds it).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

pub(crate) const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe.wasm");

pub(crate) fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extcode-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) const MANIFEST: &str = r#"[extension]
id = "you.probe"
version = "1.0.0"
name = "Probe"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
read = ["a.txt"]
net = ["api.example.com"]
auth = ["github", "acme"]
secrets = true
why = "Tests every part of the host."

[[commands]]
id = "notice"
title = "Probe: Notice"

[[commands]]
id = "trap"
title = "Probe: Trap"

[[commands]]
id = "nope"
title = "Probe: Unknown"

[[commands]]
id = "pick"
title = "Probe: Pick"

[[commands]]
id = "ask"
title = "Probe: Ask"

[[commands]]
id = "doc"
title = "Probe: Document"

[[oauth]]
id = "acme"
name = "Acme"
authorize-url = "https://acme.example.com/oauth/authorize"
token-url = "https://acme.example.com/oauth/token"
client-id = "cy-123"
scopes = ["read"]
hosts = ["api.acme.example.com"]
account-url = "https://api.acme.example.com/me"
"#;

fn install(cfg: &Path) {
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: MANIFEST.as_bytes().to_vec(), executable: false },
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
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    (wb, cx)
}

/// Let the exthost (a real process) answer: run the window's tasks until `done` or 20 s (times
/// `CODEYARD_TEST_TIMEOUT_SCALE` on slow runners).
pub(crate) fn wait_for(cx: &mut VisualTestContext, wb: &Entity<Workbench>, done: impl Fn(&Workbench) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20 * crate::test_sshd::timeout_scale());
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

pub(crate) fn notice_has(wb: &Workbench, text: &str) -> bool {
    wb.notice_for_test().is_some_and(|(_, t)| t.contains(text))
}

#[gpui::test]
fn a_code_extension_command_runs_in_its_exthost(cx: &mut TestAppContext) {
    if !crate::ext_runtime::code_runs_for_test() {
        return;
    }
    let cfg = config_dir("run");
    install(&cfg);
    let (wb, cx) = setup(cx, &cfg);
    let labels = wb.read_with(cx, |wb, _| wb.command_labels_for_test());
    assert!(labels.iter().any(|l| l == "Probe: Notice"), "{labels:?}");
    // The command asks the window for a notice; the extension's id comes first, so it cannot pose as Codeyard.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Notice", window, cx));
    wait_for(cx, &wb, |wb| notice_has(wb, "you.probe: Pick"));
    // An unknown command comes back as an error with the extension's name.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Unknown", window, cx));
    wait_for(cx, &wb, |wb| notice_has(wb, "you.probe: invalid: unknown command nope"));
}

#[gpui::test]
fn three_crashes_turn_an_extension_off(cx: &mut TestAppContext) {
    if !crate::ext_runtime::code_runs_for_test() {
        return;
    }
    let cfg = config_dir("crash");
    install(&cfg);
    let (wb, cx) = setup(cx, &cfg);
    for _ in 0..3 {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Trap", window, cx));
        // Each trap ends the process; the next command starts a new one.
        wait_for(cx, &wb, |wb| !wb.extension_running_for_test("you.probe"));
    }
    wait_for(cx, &wb, |wb| notice_has(wb, "crashed three times in a minute"));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Notice", window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| notice_has(wb, "turned off until Codeyard restarts")));
}

fn answer_is(wb: &Workbench, want: &str) -> bool {
    wb.extension_answer_for_test("you.probe").as_deref() == Some(want)
}

#[gpui::test]
fn picks_and_inputs_answer_through_the_window(cx: &mut TestAppContext) {
    if !crate::ext_runtime::code_runs_for_test() {
        return;
    }
    let cfg = config_dir("pick");
    install(&cfg);
    let (wb, cx) = setup(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Pick", window, cx));
    wait_for(cx, &wb, |wb| wb.has_overlay_for_test());
    cx.simulate_keystrokes("down enter");
    wait_for(cx, &wb, |wb| answer_is(wb, "chosen [1]"));
    // Escape answers "nothing chosen".
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Pick", window, cx));
    wait_for(cx, &wb, |wb| wb.has_overlay_for_test());
    cx.simulate_keystrokes("escape");
    wait_for(cx, &wb, |wb| answer_is(wb, "chosen []"));
    // An input box starts with the extension's value; Enter sends what was typed.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Ask", window, cx));
    wait_for(cx, &wb, |wb| wb.has_overlay_for_test());
    wb.update_in(cx, |wb, _, cx| wb.set_overlay_query_for_test("Ada".into(), cx));
    cx.simulate_keystrokes("enter");
    wait_for(cx, &wb, |wb| answer_is(wb, "text Some(\"Ada\")"));
}

#[gpui::test]
fn a_command_reads_the_active_document(cx: &mut TestAppContext) {
    if !crate::ext_runtime::code_runs_for_test() {
        return;
    }
    let cfg = config_dir("doc");
    install(&cfg);
    let (wb, cx) = setup(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Document", window, cx));
    wait_for(cx, &wb, |wb| answer_is(wb, "no document"));
    // A command the user ran may read the active document even without `read` on it (activeTab).
    let file = cfg.join("notes.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&file.to_string_lossy(), window, cx));
    cx.run_until_parked();
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Document", window, cx));
    wait_for(cx, &wb, |wb| answer_is(wb, "document rust 13"));
}

#[gpui::test]
fn a_workspace_with_a_matching_file_starts_the_extension(cx: &mut TestAppContext) {
    if !crate::ext_runtime::code_runs_for_test() {
        return;
    }
    let cfg = config_dir("contains");
    let manifest = MANIFEST.replace("[permissions]", "[activation]\nworkspace-contains = [\"vercel.json\"]\n\n[permissions]");
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.into_bytes(), executable: false },
        ext_pkg::Entry { path: "Cargo.toml".into(), data: Vec::new(), executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: PROBE.to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).unwrap();
    crate::ext::store(&cfg).install(&bytes, ext_core::Source::File { path: "/downloads/probe.cyx".into() }).unwrap();
    let (wb, cx) = setup(cx, &cfg);
    assert!(!wb.read_with(cx, |wb, _| wb.extension_running_for_test("you.probe")), "nothing starts without the file");
    let folder = cfg.join("site");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("vercel.json"), "{}").unwrap();
    wb.update_in(cx, |wb, window, cx| {
        wb.set_root(folder.to_string_lossy().into_owned(), cx);
        wb.reload_config_for_test(window, cx);
    });
    wait_for(cx, &wb, |wb| wb.extension_running_for_test("you.probe"));
}
