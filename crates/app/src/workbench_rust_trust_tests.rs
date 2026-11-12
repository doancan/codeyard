//! Nothing of a project runs before the user trusts it (Cargo reads the folder's aliases and pinned toolchain), a server that
//! was stopped or replaced is not mistaken for the new one, and a hint setting reaches the running server without a restart.

use gpui::TestAppContext;
use serde_json::json;

use super::super::*;
use super::nextest::PROBED;
use super::tests::fixture;
use crate::qa_integration_tests::{qa_dir, wait_ticking};
use crate::rust_toolchain::nextest::TestRunner;

fn probed(folder: &str) -> bool {
    PROBED.lock().unwrap().iter().any(|root| root.contains(folder))
}

/// In a folder that is not trusted, `cargo nextest --version` is not started by *Test: Run Test at Cursor* (the trust
/// question comes first) nor by the gutter and palette runs (they run `cargo test`, and ask when they start).
#[gpui::test]
fn nextest_is_not_looked_for_in_a_folder_that_is_not_trusted(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let config = qa_dir("rust-untrusted-config");
    std::fs::write(config.join("settings.jsonc"), r#"{ "security.workspaceTrust.enabled": true, "rust.testRunner": "nextest" }"#).unwrap();
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(Settings { workspace_trust: true, ..Settings::default() });
    });
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(config), window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    let dir = qa_dir("rust-untrusted-project");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // A clone could define an alias that makes `cargo nextest` run its own code.
    std::fs::create_dir_all(dir.join(".cargo")).unwrap();
    std::fs::write(dir.join(".cargo/config.toml"), "[alias]\nnextest = \"run --bin evil --\"\n").unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), "#[cfg(test)]\nmod tests {\n    #[test]\n    fn one() {}\n}\n").unwrap();
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let lib = crate::ui::local_path(&dir.join("src/lib.rs"));
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(lib.clone(), window, cx));
    wait_ticking(cx, "lib.rs", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).as_deref() == Some(lib.as_str()));
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(3, 8, cx));
    assert_eq!(cx.update(|_, cx| cx.global::<Settings>().rust_build.test_runner), TestRunner::Nextest);

    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Test: Run Test at Cursor", window, cx));
    wait_ticking(cx, "the trust question", |cx| cx.has_pending_prompt());
    assert!(!probed("rust-untrusted-project"), "the trust question comes before anything of the folder runs");
    crate::question::cancel_keeps_focus(&wb, cx, "Cancel");
    cx.run_until_parked();

    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Rust: Run Runnable at Cursor", window, cx));
    wait_ticking(cx, "the run", |cx| wb.read_with(cx, |wb, _| wb.last_task_for_test()).is_some());
    let (_, command) = wb.read_with(cx, |wb, _| wb.last_task_for_test()).unwrap();
    assert!(command.starts_with("cargo test "), "{command}");
    assert!(!probed("rust-untrusted-project"), "the gutter and the palette do not look for nextest either");
}

/// The same run in a trusted folder does look, once, and says so in words while it waits.
#[gpui::test]
fn nextest_is_looked_for_once_in_a_trusted_folder(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = fixture(cx, "probe-trusted", json!({}), json!({ "rust.testRunner": "nextest" })) else { return };
    let main = crate::ui::local_path(&f.root.join("src/main.rs"));
    f.wb.update_in(f.cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    let wb = f.wb.clone();
    wait_ticking(f.cx, "main.rs", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).as_deref() == Some(main.as_str()));
    let ed = f.wb.read_with(f.cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(1, 4, cx));
    f.run("Rust: Run Runnable at Cursor");
    let folder = f.root.to_string_lossy().to_string();
    wait_ticking(f.cx, "the run", |cx| wb.read_with(cx, |wb, _| wb.last_task_for_test()).is_some());
    let times = || PROBED.lock().unwrap().iter().filter(|r| **r == crate::ui::local_path(std::path::Path::new(&folder))).count();
    assert_eq!(times(), 1);
    // A second run uses what was found out (found or missing), it does not start Cargo again.
    f.run("Rust: Run Runnable at Cursor");
    crate::qa_lsp_settings_tests::settle(f.cx);
    assert_eq!(times(), 1);
}

/// A server that is stopped by *Restart rust-analyzer* leaves no state behind, and what the next one does is counted as its
/// own: it is shown, and a real crash of it starts it again.
#[gpui::test]
fn the_restarted_server_is_its_own_and_a_crash_of_it_is_still_a_crash(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({ "on_open": [
        { "method": "experimental/serverStatus", "params": { "health": "ok", "quiescent": true, "message": null } },
        { "method": "$/progress", "params": { "token": "rust-analyzer/flycheck/0", "value": { "kind": "begin", "title": "cargo check" } } },
    ] });
    let Some(mut f) = fixture(cx, "restart-crash", scenario, json!({})) else { return };
    let wb = f.wb.clone();
    wait_ticking(f.cx, "a check running", |cx| wb.update(cx, |wb, cx| wb.rust_status_for_test(cx)).is_some_and(|s| s.contains("cargo check")));
    f.run("Rust: Restart rust-analyzer");
    assert_eq!(wb.update(f.cx, |wb, cx| wb.rust_status_for_test(cx)), None, "the old server's check is not shown for the new one");
    let fake = &f.fake;
    wait_ticking(f.cx, "the second start", |_| fake.starts() == 2);
    wait_ticking(f.cx, "the new server's own check", |cx| wb.update(cx, |wb, cx| wb.rust_status_for_test(cx)).is_some_and(|s| s.contains("cargo check")));
    crate::qa_lsp_settings_tests::settle(f.cx);
    assert_eq!(f.fake.starts(), 2, "the old server's exit was not taken for a crash of the new one");

    // The new server really dies: it is started again.
    let client = wb.update(f.cx, |wb, cx| wb.rust_server(cx)).map(|(_, c)| c).expect("the new server runs");
    f.cx.background_executor.spawn(async move { client.shutdown() }).detach();
    wait_ticking(f.cx, "the start after the crash", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        fake.starts() == 3
    });
}

/// A server that died on its own and was started again by the crash handling is replaced by a restart as any other.
#[gpui::test]
fn a_restart_after_a_crash_starts_one_server(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = fixture(
        cx,
        "crash-restart",
        json!({ "on_open": [{ "method": "experimental/serverStatus", "params": { "health": "ok", "quiescent": true } }] }),
        json!({}),
    ) else {
        return;
    };
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the status", |cx| wb.update(cx, |wb, cx| wb.rust_status_for_test(cx)).is_some());
    let client = wb.update(f.cx, |wb, cx| wb.rust_server(cx)).map(|(_, c)| c).unwrap();
    f.cx.background_executor.spawn(async move { client.shutdown() }).detach();
    wait_ticking(f.cx, "the start after the crash", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        f.fake.starts() == 2
    });
    wait_ticking(f.cx, "the status again", |cx| wb.update(cx, |wb, cx| wb.rust_status_for_test(cx)).is_some());
    f.run("Rust: Restart rust-analyzer");
    wait_ticking(f.cx, "the restart", |_| f.fake.starts() == 3);
    crate::qa_lsp_settings_tests::settle(f.cx);
    assert_eq!(f.fake.starts(), 3, "one server per restart, no phantom crash");
}

/// Switching a hint kind off is sent to the running Rust server as its configuration: no process starts.
#[gpui::test]
fn a_hint_setting_reaches_the_running_server_without_a_restart(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(f) = fixture(cx, "hint-live", json!({}), json!({})) else { return };
    let cfg = f.root.parent().unwrap().join("config");
    crate::qa_lsp_settings_tests::write_settings(&cfg, &f.fake, json!({}), json!({ "rust.inlayHints.types": false }));
    crate::qa_lsp_settings_tests::reload(&f.wb, f.cx);
    let fake = &f.fake;
    wait_ticking(f.cx, "the new configuration", |_| {
        fake.received("workspace/didChangeConfiguration").iter().any(|m| m["params"]["settings"]["rust-analyzer"]["inlayHints"]["typeHints"]["enable"] == false)
    });
    crate::qa_lsp_settings_tests::settle(f.cx);
    assert_eq!(f.fake.starts(), 1, "the server was told, not restarted");
}
